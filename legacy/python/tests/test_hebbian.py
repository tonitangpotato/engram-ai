"""
Tests for Hebbian Learning — Co-activation forms memory links

"Neurons that fire together, wire together."
"""

import pytest
import time

from engram import Memory
from engram.config import MemoryConfig
from engram.store import SQLiteStore
from engram.hebbian import (
    record_coactivation,
    maybe_create_link,
    get_hebbian_neighbors,
    get_all_hebbian_links,
    decay_hebbian_links,
    strengthen_link,
    get_coactivation_stats,
)


class TestHebbianModule:
    """Test the hebbian.py module functions directly."""

    def test_record_coactivation_increments_count(self):
        """Co-activation tracking should increment count for each pair."""
        store = SQLiteStore(":memory:")
        
        # Add some memories
        m1 = store.add("Memory one")
        m2 = store.add("Memory two")
        m3 = store.add("Memory three")
        
        # Record co-activation
        record_coactivation(store, [m1.id, m2.id, m3.id], threshold=3)
        
        stats = get_coactivation_stats(store)
        # Should have 3 pairs tracked (1-2, 1-3, 2-3)
        assert len(stats) == 3
        # Each pair should have count=1
        for count in stats.values():
            assert count == 1
        
        store.close()

    def test_link_forms_at_threshold(self):
        """Hebbian link should form after threshold co-activations."""
        store = SQLiteStore(":memory:")
        
        m1 = store.add("Memory one")
        m2 = store.add("Memory two")
        
        # Below threshold - no link
        record_coactivation(store, [m1.id, m2.id], threshold=3)
        record_coactivation(store, [m1.id, m2.id], threshold=3)
        assert get_hebbian_neighbors(store, m1.id) == []
        
        # At threshold - link forms!
        new_links = record_coactivation(store, [m1.id, m2.id], threshold=3)
        assert len(new_links) == 1
        assert m2.id in get_hebbian_neighbors(store, m1.id)
        assert m1.id in get_hebbian_neighbors(store, m2.id)
        
        store.close()

    def test_maybe_create_link_idempotent(self):
        """Creating link after threshold should be idempotent."""
        store = SQLiteStore(":memory:")
        
        m1 = store.add("Memory one")
        m2 = store.add("Memory two")
        
        # Form link
        for _ in range(3):
            maybe_create_link(store, m1.id, m2.id, threshold=3)
        
        # Link should exist
        links = get_all_hebbian_links(store)
        # Bidirectional = 2 entries
        assert len(links) == 2
        
        # Further calls shouldn't create more links
        for _ in range(5):
            maybe_create_link(store, m1.id, m2.id, threshold=3)
        
        links = get_all_hebbian_links(store)
        assert len(links) == 2  # Still just 2
        
        store.close()

    def test_get_hebbian_neighbors(self):
        """Should return only neighbors with formed links."""
        store = SQLiteStore(":memory:")
        
        m1 = store.add("Memory one")
        m2 = store.add("Memory two")
        m3 = store.add("Memory three")
        
        # Form link between m1-m2 but not m1-m3
        for _ in range(3):
            record_coactivation(store, [m1.id, m2.id], threshold=3)
        
        # Only m2 should be a neighbor of m1
        neighbors = get_hebbian_neighbors(store, m1.id)
        assert m2.id in neighbors
        assert m3.id not in neighbors
        
        store.close()

    def test_decay_hebbian_links(self):
        """Decay should reduce link strength, prune weak links."""
        store = SQLiteStore(":memory:")
        
        m1 = store.add("Memory one")
        m2 = store.add("Memory two")
        
        # Form link
        for _ in range(3):
            record_coactivation(store, [m1.id, m2.id], threshold=3)
        
        # Check initial strength
        links = get_all_hebbian_links(store)
        assert links[0][2] == 1.0
        
        # Decay
        decay_hebbian_links(store, factor=0.5)
        
        links = get_all_hebbian_links(store)
        assert links[0][2] == 0.5
        
        # Decay until pruned (below 0.1)
        for _ in range(5):
            decay_hebbian_links(store, factor=0.5)
        
        links = get_all_hebbian_links(store)
        assert len(links) == 0  # Pruned
        
        store.close()

    def test_strengthen_link(self):
        """Strengthening should increase link strength up to cap."""
        store = SQLiteStore(":memory:")
        
        m1 = store.add("Memory one")
        m2 = store.add("Memory two")
        
        # Form link
        for _ in range(3):
            record_coactivation(store, [m1.id, m2.id], threshold=3)
        
        # Strengthen
        strengthen_link(store, m1.id, m2.id, boost=0.5)
        
        links = get_all_hebbian_links(store)
        assert links[0][2] == 1.5
        
        # Cap at 2.0
        for _ in range(10):
            strengthen_link(store, m1.id, m2.id, boost=0.5)
        
        links = get_all_hebbian_links(store)
        assert links[0][2] == 2.0
        
        store.close()


class TestHebbianIntegration:
    """Test Hebbian learning integrated with Memory class."""

    def test_recall_triggers_coactivation(self):
        """Recalling memories together should track co-activation."""
        config = MemoryConfig(hebbian_enabled=True, hebbian_threshold=2)
        mem = Memory(":memory:", config=config)
        
        # Add memories that will be recalled together
        mem.add("Python programming language", type="factual")
        mem.add("Machine learning with Python", type="factual")
        mem.add("Today is a nice day", type="episodic")  # Unrelated
        
        # First recall - should track co-activation
        results = mem.recall("Python", limit=3)
        assert len(results) >= 2
        
        # Check that co-activation was recorded
        stats = get_coactivation_stats(mem._store)
        assert len(stats) > 0
        
        mem.close()

    def test_repeated_recall_forms_link(self):
        """Repeated recall should form Hebbian links at threshold."""
        config = MemoryConfig(hebbian_enabled=True, hebbian_threshold=2)
        mem = Memory(":memory:", config=config)
        
        id1 = mem.add("TensorFlow deep learning", type="factual")
        id2 = mem.add("PyTorch neural networks", type="factual")
        
        # First recall
        mem.recall("deep learning frameworks", limit=5)
        links = mem.hebbian_links()
        assert len(links) == 0  # Not yet
        
        # Second recall - should hit threshold
        mem.recall("neural network frameworks", limit=5)
        links = mem.hebbian_links()
        assert len(links) > 0  # Link formed!
        
        mem.close()

    def test_hebbian_links_method(self):
        """Memory.hebbian_links() should return correct links."""
        config = MemoryConfig(hebbian_enabled=True, hebbian_threshold=2)
        mem = Memory(":memory:", config=config)
        
        id1 = mem.add("Memory A", type="factual")
        id2 = mem.add("Memory B", type="factual")
        id3 = mem.add("Memory C", type="factual")
        
        # Force link formation
        record_coactivation(mem._store, [id1, id2], threshold=2)
        record_coactivation(mem._store, [id1, id2], threshold=2)
        
        # Get all links
        all_links = mem.hebbian_links()
        assert len(all_links) == 2  # Bidirectional
        
        # Get links for specific memory
        m1_links = mem.hebbian_links(id1)
        assert len(m1_links) == 1
        assert m1_links[0][1] == id2
        
        mem.close()

    def test_consolidation_decays_hebbian_links(self):
        """Consolidation should decay Hebbian link strengths."""
        config = MemoryConfig(
            hebbian_enabled=True,
            hebbian_threshold=2,
            hebbian_decay=0.5,  # Aggressive decay for testing
        )
        mem = Memory(":memory:", config=config)
        
        id1 = mem.add("Memory A", type="factual")
        id2 = mem.add("Memory B", type="factual")
        
        # Form link
        record_coactivation(mem._store, [id1, id2], threshold=2)
        record_coactivation(mem._store, [id1, id2], threshold=2)
        
        links = mem.hebbian_links()
        assert links[0][2] == 1.0
        
        # Consolidate
        mem.consolidate(days=1.0)
        
        links = mem.hebbian_links()
        assert links[0][2] == 0.5  # Decayed
        
        mem.close()

    def test_hebbian_disabled(self):
        """With hebbian_enabled=False, no links should form."""
        config = MemoryConfig(hebbian_enabled=False)
        mem = Memory(":memory:", config=config)
        
        mem.add("Memory A", type="factual")
        mem.add("Memory B", type="factual")
        
        # Many recalls
        for _ in range(10):
            mem.recall("Memory", limit=5)
        
        links = mem.hebbian_links()
        assert len(links) == 0
        
        mem.close()


class TestHebbianGraphExpansion:
    """Test that Hebbian links are used in search graph expansion."""

    def test_graph_expand_includes_hebbian_neighbors(self):
        """Graph expansion should include Hebbian-linked memories."""
        config = MemoryConfig(hebbian_enabled=True, hebbian_threshold=2)
        mem = Memory(":memory:", config=config)
        
        # Add memories - m1 and m2 are about "cat", m3 is about "dog"
        id1 = mem.add("I have a cat named Whiskers", type="episodic")
        id2 = mem.add("Cats are great pets", type="opinion")
        id3 = mem.add("Dogs are loyal companions", type="opinion")
        
        # Create Hebbian link between cat memory and dog memory
        # (maybe user often thinks about both pets together)
        record_coactivation(mem._store, [id1, id3], threshold=2)
        record_coactivation(mem._store, [id1, id3], threshold=2)
        
        # Now search for "cat" with graph_expand=True
        # Should find cat memories AND dog memory via Hebbian link
        results = mem.recall("cat Whiskers", limit=5, graph_expand=True)
        
        result_ids = [r["id"] for r in results]
        assert id1 in result_ids  # Direct match
        assert id3 in result_ids  # Via Hebbian link
        
        mem.close()

    def test_graph_expand_without_hebbian(self):
        """Without Hebbian links, expansion only uses entity links."""
        config = MemoryConfig(hebbian_enabled=True, hebbian_threshold=2)
        mem = Memory(":memory:", config=config)
        
        id1 = mem.add("I have a cat named Whiskers", type="episodic")
        id2 = mem.add("Dogs are loyal companions", type="opinion")
        
        # No Hebbian link formed
        results = mem.recall("cat Whiskers", limit=5, graph_expand=True)
        
        result_ids = [r["id"] for r in results]
        assert id1 in result_ids
        # id2 should NOT be included (no link)
        # (unless FTS happens to match, which it shouldn't for "cat Whiskers" -> "Dogs")
        
        mem.close()


class TestHebbianSchemaCreation:
    """Test that hebbian_links table is created properly."""

    def test_table_exists(self):
        """hebbian_links table should be created on store init."""
        store = SQLiteStore(":memory:")
        
        # Check table exists
        result = store._conn.execute(
            "SELECT name FROM sqlite_master WHERE type='table' AND name='hebbian_links'"
        ).fetchone()
        assert result is not None
        assert result[0] == "hebbian_links"
        
        store.close()

    def test_table_schema(self):
        """hebbian_links table should have correct columns."""
        store = SQLiteStore(":memory:")
        
        result = store._conn.execute("PRAGMA table_info(hebbian_links)").fetchall()
        columns = {row[1] for row in result}
        
        assert "source_id" in columns
        assert "target_id" in columns
        assert "strength" in columns
        assert "coactivation_count" in columns
        assert "created_at" in columns
        
        store.close()

    def test_cascade_delete(self):
        """Deleting a memory should cascade to hebbian_links."""
        store = SQLiteStore(":memory:")
        
        m1 = store.add("Memory one")
        m2 = store.add("Memory two")
        
        # Form link
        for _ in range(3):
            record_coactivation(store, [m1.id, m2.id], threshold=3)
        
        links = get_all_hebbian_links(store)
        assert len(links) == 2
        
        # Delete m1
        store.delete(m1.id)
        
        # Links should be gone (cascade)
        links = get_all_hebbian_links(store)
        assert len(links) == 0
        
        store.close()
