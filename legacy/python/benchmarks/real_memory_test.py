#!/usr/bin/env python3
"""
Real Memory Test

Tests engram with actual Clawdbot memory files.
Validates that the layer system and consolidation work as designed.
"""

import os
import re
import sys
import time
from pathlib import Path
from datetime import datetime, timedelta

sys.path.insert(0, str(Path(__file__).parent.parent))

from engram import Memory, MemoryType
from engram.core import MemoryLayer


def parse_memory_file(filepath: Path) -> list[dict]:
    """Parse a markdown memory file into chunks."""
    content = filepath.read_text()
    
    # Extract date from filename (YYYY-MM-DD.md)
    date_match = re.search(r'(\d{4}-\d{2}-\d{2})', filepath.name)
    if date_match:
        file_date = datetime.strptime(date_match.group(1), '%Y-%m-%d')
    else:
        file_date = datetime.now()
    
    # Split by headers
    chunks = []
    current_section = ""
    current_content = []
    
    for line in content.split('\n'):
        if line.startswith('## ') or line.startswith('### '):
            if current_content:
                text = '\n'.join(current_content).strip()
                if len(text) > 50:  # Skip tiny sections
                    chunks.append({
                        'section': current_section,
                        'content': text[:500],  # Truncate long sections
                        'date': file_date,
                        'source': filepath.name,
                    })
            current_section = line.lstrip('#').strip()
            current_content = []
        else:
            current_content.append(line)
    
    # Last section
    if current_content:
        text = '\n'.join(current_content).strip()
        if len(text) > 50:
            chunks.append({
                'section': current_section,
                'content': text[:500],
                'date': file_date,
                'source': filepath.name,
            })
    
    return chunks


def estimate_importance(content: str, section: str) -> float:
    """Estimate importance based on content."""
    content_lower = content.lower()
    section_lower = section.lower()
    
    # High importance signals
    high_signals = ['important', 'critical', 'remember', 'key', 'decision', 
                    'learned', 'insight', 'always', 'never', 'rule']
    # Low importance signals  
    low_signals = ['random', 'misc', 'note', 'todo', 'minor', 'maybe']
    
    score = 0.5
    
    for signal in high_signals:
        if signal in content_lower or signal in section_lower:
            score += 0.1
    
    for signal in low_signals:
        if signal in content_lower or signal in section_lower:
            score -= 0.1
    
    # Longer content might be more important
    if len(content) > 300:
        score += 0.1
    
    return max(0.1, min(0.9, score))


def run_test():
    """Run the real memory test."""
    
    memory_dir = Path("/Users/potato/clawd/memory")
    memory_file = Path("/Users/potato/clawd/MEMORY.md")
    
    print("=" * 70)
    print("REAL MEMORY TEST")
    print("Testing engram with actual Clawdbot memories")
    print("=" * 70)
    
    # Collect all memory chunks
    all_chunks = []
    
    # Parse daily notes
    for md_file in sorted(memory_dir.glob("*.md")):
        chunks = parse_memory_file(md_file)
        all_chunks.extend(chunks)
        print(f"Parsed {md_file.name}: {len(chunks)} chunks")
    
    # Parse MEMORY.md
    if memory_file.exists():
        chunks = parse_memory_file(memory_file)
        # MEMORY.md content is more important (curated)
        for c in chunks:
            c['importance_boost'] = 0.2
        all_chunks.extend(chunks)
        print(f"Parsed MEMORY.md: {len(chunks)} chunks")
    
    print(f"\nTotal chunks: {len(all_chunks)}")
    
    # Create engram memory
    mem = Memory(":memory:")
    
    # Import all chunks with appropriate timestamps
    print("\n--- Importing memories ---")
    base_time = time.time()
    
    for chunk in all_chunks:
        # Calculate timestamp based on date
        days_ago = (datetime.now() - chunk['date']).days
        created_at = base_time - (days_ago * 24 * 3600)
        
        importance = estimate_importance(chunk['content'], chunk['section'])
        if chunk.get('importance_boost'):
            importance += chunk['importance_boost']
        importance = min(0.95, importance)
        
        mem.add(
            content=f"[{chunk['section']}] {chunk['content'][:300]}",
            importance=importance,
            created_at=created_at,
        )
    
    print(f"Imported {len(all_chunks)} memories")
    
    # Check initial layer distribution
    print("\n--- Initial Layer Distribution ---")
    all_memories = mem._store.all()
    layer_counts = {}
    for m in all_memories:
        layer = m.layer.value
        layer_counts[layer] = layer_counts.get(layer, 0) + 1
    for layer, count in sorted(layer_counts.items()):
        print(f"  {layer}: {count}")
    
    # Run consolidation (simulate 7 days)
    print("\n--- Running 7 days of consolidation ---")
    for day in range(7):
        mem.consolidate(days=1.0)
    
    # Check layer distribution after consolidation
    print("\n--- Layer Distribution After Consolidation ---")
    all_memories = mem._store.all()
    layer_counts = {}
    strength_by_layer = {}
    for m in all_memories:
        layer = m.layer.value
        layer_counts[layer] = layer_counts.get(layer, 0) + 1
        if layer not in strength_by_layer:
            strength_by_layer[layer] = []
        strength_by_layer[layer].append(m.working_strength + m.core_strength)
    
    for layer, count in sorted(layer_counts.items()):
        avg_strength = sum(strength_by_layer[layer]) / len(strength_by_layer[layer])
        print(f"  {layer}: {count} memories (avg strength: {avg_strength:.3f})")
    
    # Test retrieval
    print("\n--- Retrieval Tests ---")
    
    test_queries = [
        "projects working",
        "SaltyHall",
        "learn lesson",
        "important decision",
        "consciousness identity",
    ]
    
    for query in test_queries:
        print(f"\nQuery: '{query}'")
        results = mem.recall(query, limit=3)
        for i, r in enumerate(results):
            layer = "?"
            entry = mem._store.get(r["id"])
            if entry:
                layer = entry.layer.value
            print(f"  {i+1}. [{layer}] {r['content'][:60]}...")
            print(f"     confidence={r['confidence']:.2f}, activation={r['activation']:.2f}")
    
    # Test: Can we find old but important info?
    print("\n--- Critical Test: Old but Important Info ---")
    results = mem.recall("consciousness identity", limit=5)
    found_consciousness = any("conscious" in r["content"].lower() for r in results)
    print(f"Found consciousness discussion: {'✅' if found_consciousness else '❌'}")
    
    # Summary
    print("\n" + "=" * 70)
    print("SUMMARY")
    print("=" * 70)
    
    core_count = layer_counts.get("core", 0)
    working_count = layer_counts.get("working", 0)
    archive_count = layer_counts.get("archive", 0)
    
    print(f"Total memories: {len(all_memories)}")
    print(f"  Core (always loaded): {core_count}")
    print(f"  Working (recent): {working_count}")
    print(f"  Archive (on-demand): {archive_count}")
    
    if core_count > 0:
        print(f"\n✅ Consolidation promoted {core_count} memories to core")
    if archive_count > 0:
        print(f"✅ Consolidation archived {archive_count} old/weak memories")
    
    # Estimate context savings
    avg_tokens_per_memory = 100  # rough estimate
    full_context = len(all_memories) * avg_tokens_per_memory
    reduced_context = (core_count + working_count) * avg_tokens_per_memory
    savings = (1 - reduced_context / full_context) * 100 if full_context > 0 else 0
    
    print(f"\nContext savings: {savings:.0f}% (only load core+working, not archive)")


if __name__ == "__main__":
    run_test()
