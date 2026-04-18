#!/usr/bin/env python3
"""
Stress Testing Suite

Verifies system handles large-scale deployment scenarios:
1. 100k memories - scalability verification
2. Continuous write load - sustained operation
3. Burst writes - spike handling
4. Concurrent operations - multi-threaded safety

Run:
    pytest benchmarks/test_stress.py -v
"""

import os
import sys
import tempfile
import time
import random
import threading
from concurrent.futures import ThreadPoolExecutor, as_completed

import pytest

# Add parent to path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

from engram import Memory
from engram.config import MemoryConfig


class Test100kMemories:
    """Test system behavior with 100,000 memories."""
    
    def test_bulk_insert_and_recall(self):
        """Insert 100k memories and measure recall performance."""
        with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as f:
            db_path = f.name
        
        try:
            config = MemoryConfig.default()
            mem = Memory(db_path, config=config)
            
            target_count = 100000
            batch_size = 10000
            
            print(f"\n  Inserting {target_count:,} memories...")
            
            insert_start = time.time()
            
            for batch in range(target_count // batch_size):
                batch_start = time.time()
                
                for i in range(batch_size):
                    content = f"Memory {batch * batch_size + i}: {random.randint(1, 1000000)}"
                    importance = random.uniform(0.1, 0.6)
                    mem_type = random.choice(["factual", "episodic", "relational"])
                    mem.add(content, type=mem_type, importance=importance)
                
                batch_time = time.time() - batch_start
                total_so_far = (batch + 1) * batch_size
                print(f"    {total_so_far:,} memories inserted "
                      f"({batch_time:.2f}s for batch, "
                      f"{batch_size/batch_time:.0f} inserts/sec)")
            
            insert_time = time.time() - insert_start
            insert_rate = target_count / insert_time
            
            print(f"\n  Total insert time: {insert_time:.2f}s ({insert_rate:.0f} inserts/sec)")
            
            # Measure recall latency
            print(f"\n  Measuring recall latency with {target_count:,} memories...")
            
            queries = [
                "memory",
                "event",
                str(random.randint(1, 1000000)),
                str(random.randint(1, target_count)),
            ]
            
            latencies = []
            for query in queries:
                recall_start = time.time()
                results = mem.recall(query, limit=10)
                latency_ms = (time.time() - recall_start) * 1000
                latencies.append(latency_ms)
                print(f"    Query '{query}': {latency_ms:.2f}ms ({len(results)} results)")
            
            avg_latency = sum(latencies) / len(latencies)
            max_latency = max(latencies)
            
            print(f"\n  Average recall latency: {avg_latency:.2f}ms")
            print(f"  Max recall latency: {max_latency:.2f}ms")
            
            # Recall should stay under 1 second even with 100k memories
            assert max_latency < 1000, f"Recall too slow: {max_latency:.2f}ms"
            
            # Verify database file size
            db_size_mb = os.path.getsize(db_path) / (1024 * 1024)
            print(f"  Database size: {db_size_mb:.2f} MB")
            
            print(f"\n✓ 100k memories test passed")
        
        finally:
            if os.path.exists(db_path):
                os.unlink(db_path)
    
    @pytest.mark.slow
    def test_consolidation_at_scale(self):
        """Test consolidation performance with large memory count."""
        with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as f:
            db_path = f.name
        
        try:
            config = MemoryConfig.default()
            mem = Memory(db_path, config=config)
            
            # Insert 50k memories (faster than full 100k for testing)
            target_count = 50000
            print(f"\n  Inserting {target_count:,} memories for consolidation test...")
            
            for i in range(target_count):
                if i % 10000 == 0:
                    print(f"    {i:,}...")
                
                content = f"Memory {i}: {random.randint(1, 1000000)}"
                importance = random.uniform(0.1, 0.8)
                mem.add(content, type="episodic", importance=importance)
            
            # Run consolidation
            print(f"\n  Running consolidation on {target_count:,} memories...")
            consolidate_start = time.time()
            mem.consolidate()
            consolidate_time = time.time() - consolidate_start
            
            print(f"  Consolidation time: {consolidate_time:.2f}s")
            
            # Consolidation should complete in reasonable time (< 60s)
            assert consolidate_time < 60, f"Consolidation too slow: {consolidate_time:.2f}s"
            
            print(f"\n✓ Large-scale consolidation test passed")
        
        finally:
            if os.path.exists(db_path):
                os.unlink(db_path)


class TestContinuousLoad:
    """Test sustained write operations."""
    
    def test_continuous_writes(self):
        """Simulate continuous writes (10/sec for 60 seconds)."""
        with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as f:
            db_path = f.name
        
        try:
            config = MemoryConfig.default()
            mem = Memory(db_path, config=config)
            
            duration_seconds = 60
            target_rate = 10  # writes per second
            total_writes = duration_seconds * target_rate
            
            print(f"\n  Continuous write test: {target_rate} writes/sec for {duration_seconds}s "
                  f"({total_writes} total)...")
            
            start_time = time.time()
            write_count = 0
            errors = 0
            latencies = []
            
            while time.time() - start_time < duration_seconds:
                write_start = time.time()
                
                try:
                    content = f"Continuous write {write_count}: {time.time()}"
                    importance = random.uniform(0.1, 0.6)
                    mem.add(content, type="episodic", importance=importance)
                    write_count += 1
                    
                    write_latency = (time.time() - write_start) * 1000
                    latencies.append(write_latency)
                    
                except Exception as e:
                    errors += 1
                    print(f"    Error: {e}")
                
                # Sleep to maintain target rate
                elapsed = time.time() - write_start
                sleep_time = (1.0 / target_rate) - elapsed
                if sleep_time > 0:
                    time.sleep(sleep_time)
                
                # Progress update
                if write_count % 100 == 0:
                    elapsed_total = time.time() - start_time
                    current_rate = write_count / elapsed_total
                    print(f"    {write_count} writes, {current_rate:.1f}/sec, "
                          f"{errors} errors")
            
            total_time = time.time() - start_time
            actual_rate = write_count / total_time
            avg_latency = sum(latencies) / len(latencies) if latencies else 0
            
            print(f"\n  Results:")
            print(f"    Total writes: {write_count}")
            print(f"    Total time: {total_time:.2f}s")
            print(f"    Actual rate: {actual_rate:.2f}/sec")
            print(f"    Errors: {errors}")
            print(f"    Avg write latency: {avg_latency:.2f}ms")
            
            # Should have minimal errors
            error_rate = errors / write_count if write_count > 0 else 1
            assert error_rate < 0.01, f"Too many errors: {errors}/{write_count}"
            
            print(f"\n✓ Continuous write test passed")
        
        finally:
            if os.path.exists(db_path):
                os.unlink(db_path)


class TestBurstWrites:
    """Test burst traffic handling."""
    
    def test_burst_1000_writes(self):
        """Handle 1000 writes as fast as possible."""
        with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as f:
            db_path = f.name
        
        try:
            config = MemoryConfig.default()
            mem = Memory(db_path, config=config)
            
            burst_count = 1000
            print(f"\n  Burst write test: {burst_count} writes as fast as possible...")
            
            start_time = time.time()
            errors = 0
            
            for i in range(burst_count):
                try:
                    content = f"Burst write {i}: {time.time()}"
                    mem.add(content, type="episodic", importance=random.uniform(0.1, 0.5))
                except Exception as e:
                    errors += 1
                    if errors <= 5:  # Print first few errors
                        print(f"    Error {errors}: {e}")
            
            burst_time = time.time() - start_time
            burst_rate = burst_count / burst_time
            
            print(f"\n  Results:")
            print(f"    Time: {burst_time:.2f}s")
            print(f"    Rate: {burst_rate:.0f} writes/sec")
            print(f"    Errors: {errors}/{burst_count}")
            
            # All writes should succeed
            assert errors == 0, f"Burst writes had {errors} errors"
            
            # Verify memories were actually stored
            stats = mem.stats()
            assert stats["total_memories"] >= burst_count, "Not all memories were stored"
            
            print(f"\n✓ Burst write test passed")
        
        finally:
            if os.path.exists(db_path):
                os.unlink(db_path)


class TestConcurrentOperations:
    """Test concurrent read/write operations."""
    
    def test_concurrent_reads(self):
        """Multiple readers should work simultaneously."""
        with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as f:
            db_path = f.name
        
        try:
            config = MemoryConfig.default()
            mem = Memory(db_path, config=config)
            
            # Pre-populate with memories
            print("\n  Pre-populating database...")
            for i in range(1000):
                mem.add(f"Memory {i}: {random.randint(1, 10000)}", 
                       type="episodic", 
                       importance=random.uniform(0.1, 0.6))
            
            print(f"  Testing concurrent reads (10 threads)...")
            
            def read_worker(thread_id, num_reads=100):
                """Worker that performs reads."""
                local_mem = Memory(db_path, config=config)
                results = []
                errors = 0
                
                for i in range(num_reads):
                    try:
                        query = f"Memory {random.randint(1, 1000)}"
                        res = local_mem.recall(query, limit=5)
                        results.append(len(res))
                    except Exception as e:
                        errors += 1
                
                return {
                    "thread_id": thread_id,
                    "reads": num_reads,
                    "errors": errors,
                    "avg_results": sum(results) / len(results) if results else 0
                }
            
            # Run concurrent readers
            num_threads = 10
            with ThreadPoolExecutor(max_workers=num_threads) as executor:
                futures = [executor.submit(read_worker, i) for i in range(num_threads)]
                
                total_errors = 0
                for future in as_completed(futures):
                    result = future.result()
                    total_errors += result["errors"]
                    print(f"    Thread {result['thread_id']}: {result['reads']} reads, "
                          f"{result['errors']} errors, "
                          f"avg {result['avg_results']:.1f} results")
            
            print(f"\n  Total errors across all threads: {total_errors}")
            
            # Should have no errors
            assert total_errors == 0, f"Concurrent reads had {total_errors} errors"
            
            print(f"\n✓ Concurrent read test passed")
        
        finally:
            if os.path.exists(db_path):
                os.unlink(db_path)
    
    def test_mixed_read_write(self):
        """Test mixed concurrent reads and writes."""
        with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as f:
            db_path = f.name
        
        try:
            config = MemoryConfig.default()
            mem = Memory(db_path, config=config)
            
            # Pre-populate
            for i in range(500):
                mem.add(f"Initial memory {i}", type="episodic", importance=0.3)
            
            print("\n  Testing mixed read/write (8 readers, 2 writers)...")
            
            def reader_worker(thread_id):
                local_mem = Memory(db_path, config=config)
                errors = 0
                for _ in range(50):
                    try:
                        local_mem.recall(f"memory {random.randint(1, 100)}", limit=5)
                    except Exception as e:
                        errors += 1
                return ("reader", thread_id, errors)
            
            def writer_worker(thread_id):
                local_mem = Memory(db_path, config=config)
                errors = 0
                for i in range(50):
                    try:
                        local_mem.add(
                            f"Writer {thread_id} memory {i}",
                            type="episodic",
                            importance=0.3
                        )
                    except Exception as e:
                        errors += 1
                return ("writer", thread_id, errors)
            
            with ThreadPoolExecutor(max_workers=10) as executor:
                futures = []
                
                # Submit 8 readers
                for i in range(8):
                    futures.append(executor.submit(reader_worker, i))
                
                # Submit 2 writers
                for i in range(2):
                    futures.append(executor.submit(writer_worker, i))
                
                total_errors = 0
                for future in as_completed(futures):
                    worker_type, thread_id, errors = future.result()
                    total_errors += errors
                    print(f"    {worker_type.capitalize()} {thread_id}: {errors} errors")
            
            print(f"\n  Total errors: {total_errors}")
            
            # Some databases may have lock contention, but should complete
            assert total_errors < 10, f"Too many errors in mixed operations: {total_errors}"
            
            print(f"\n✓ Mixed read/write test passed")
        
        finally:
            if os.path.exists(db_path):
                os.unlink(db_path)


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
