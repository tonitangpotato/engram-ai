#!/usr/bin/env python3
"""
neuromemory-ai CLI

Usage:
    neuromem add "memory content" [--type TYPE] [--importance IMPORTANCE]
    neuromem recall "query" [--limit LIMIT]
    neuromem stats
    neuromem consolidate
    neuromem forget [--threshold THRESHOLD]
    neuromem export OUTPUT_PATH
    neuromem list [--limit LIMIT] [--type TYPE]
"""

import argparse
import sys
import os
import json
from pathlib import Path

# Try to import from the package
try:
    from engram import Memory
    from engram.config import MemoryConfig
except ImportError:
    # If running from source directory
    sys.path.insert(0, str(Path(__file__).parent.parent))
    from engram import Memory
    from engram.config import MemoryConfig


DEFAULT_DB = os.environ.get("NEUROMEM_DB", "./neuromem.db")


def get_memory(db_path: str = DEFAULT_DB) -> Memory:
    """Get or create a memory instance."""
    return Memory(db_path)


def cmd_add(args):
    """Add a new memory."""
    mem = get_memory(args.db)
    
    kwargs = {}
    if args.type:
        kwargs["type"] = args.type
    if args.importance:
        kwargs["importance"] = float(args.importance)
    
    mem_id = mem.add(args.content, **kwargs)
    print(f"✓ Added memory: {mem_id[:8]}...")
    print(f"  Content: {args.content[:60]}{'...' if len(args.content) > 60 else ''}")
    
    mem.close()


def cmd_recall(args):
    """Recall memories matching a query."""
    mem = get_memory(args.db)
    
    results = mem.recall(args.query, limit=args.limit)
    
    if not results:
        print("No memories found.")
    else:
        print(f"Found {len(results)} memories:\n")
        for i, r in enumerate(results, 1):
            conf = r.get("confidence_label", "?")
            typ = r.get("type", "?")[:4]
            content = r["content"]
            if len(content) > 80:
                content = content[:77] + "..."
            print(f"  {i}. [{conf:8}] [{typ}] {content}")
    
    mem.close()


def cmd_stats(args):
    """Show memory statistics."""
    mem = get_memory(args.db)
    stats = mem.stats()
    
    print("=== neuromemory-ai Stats ===\n")
    print(f"Total memories: {stats['total_memories']}")
    print(f"Pinned: {stats['pinned']}")
    print(f"Uptime: {stats['uptime_hours']:.1f} hours")
    
    print("\nBy layer:")
    for layer, data in stats["layers"].items():
        if data["count"] > 0:
            print(f"  {layer}: {data['count']} memories")
    
    print("\nBy type:")
    for typ, data in stats["by_type"].items():
        print(f"  {typ}: {data['count']} (avg importance: {data['avg_importance']:.2f})")
    
    mem.close()


def cmd_consolidate(args):
    """Run a consolidation cycle (like sleep)."""
    mem = get_memory(args.db)
    result = mem.consolidate(days=args.days)
    
    print(f"✓ Consolidation complete ({args.days} day(s))")
    if result:
        print(f"  {result}")
    
    mem.close()


def cmd_forget(args):
    """Prune weak memories."""
    mem = get_memory(args.db)
    
    # Get count before
    before = mem.stats()["total_memories"]
    
    mem.forget(threshold=args.threshold)
    
    # Get count after
    after = mem.stats()["total_memories"]
    archived = before - after
    
    print(f"✓ Archived {archived} memories below threshold {args.threshold}")
    
    mem.close()


def cmd_export(args):
    """Export memory database."""
    mem = get_memory(args.db)
    mem.export(args.output)
    
    size = os.path.getsize(args.output)
    print(f"✓ Exported to {args.output} ({size} bytes)")
    
    mem.close()


def cmd_list(args):
    """List memories."""
    mem = get_memory(args.db)
    
    all_mems = list(mem._store.all())
    
    # Filter by type if specified
    if args.type:
        all_mems = [m for m in all_mems if m.memory_type.value == args.type]
    
    # Sort by created_at descending
    all_mems.sort(key=lambda m: m.created_at, reverse=True)
    
    # Limit
    all_mems = all_mems[:args.limit]
    
    if not all_mems:
        print("No memories found.")
    else:
        print(f"Listing {len(all_mems)} memories:\n")
        for m in all_mems:
            content = m.content
            if len(content) > 70:
                content = content[:67] + "..."
            typ = m.memory_type.value[:4]
            layer = m.layer.value[:4]
            print(f"  [{typ}] [{layer}] {content}")
    
    mem.close()


def cmd_hebbian(args):
    """Show Hebbian links for a memory."""
    mem = get_memory(args.db)
    
    # Find memory by content match
    results = mem.recall(args.query, limit=1)
    if not results:
        print(f"No memory found matching: {args.query}")
        mem.close()
        return
    
    mem_id = results[0]["id"]
    links = mem.hebbian_links(mem_id)
    
    print(f"Memory: {results[0]['content'][:60]}...")
    print(f"Hebbian links: {len(links)}")
    
    for link_id in links[:10]:
        linked = mem._store.get(link_id)
        if linked:
            print(f"  → {linked.content[:60]}...")
    
    mem.close()


def main():
    parser = argparse.ArgumentParser(
        description="neuromemory-ai: Neuroscience-grounded memory for AI agents",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument("--db", default=DEFAULT_DB, help="Database path")
    
    subparsers = parser.add_subparsers(dest="command", help="Commands")
    
    # add
    add_parser = subparsers.add_parser("add", help="Add a memory")
    add_parser.add_argument("content", help="Memory content")
    add_parser.add_argument("--type", "-t", choices=["factual", "episodic", "relational", "emotional", "procedural", "opinion"])
    add_parser.add_argument("--importance", "-i", type=float, help="Importance (0-1)")
    
    # recall
    recall_parser = subparsers.add_parser("recall", help="Recall memories")
    recall_parser.add_argument("query", help="Search query")
    recall_parser.add_argument("--limit", "-l", type=int, default=5)
    
    # stats
    subparsers.add_parser("stats", help="Show statistics")
    
    # consolidate
    cons_parser = subparsers.add_parser("consolidate", help="Run consolidation")
    cons_parser.add_argument("--days", "-d", type=float, default=1.0)
    
    # forget
    forget_parser = subparsers.add_parser("forget", help="Prune weak memories")
    forget_parser.add_argument("--threshold", "-t", type=float, default=0.01)
    
    # export
    export_parser = subparsers.add_parser("export", help="Export database")
    export_parser.add_argument("output", help="Output path")
    
    # list
    list_parser = subparsers.add_parser("list", help="List memories")
    list_parser.add_argument("--limit", "-l", type=int, default=20)
    list_parser.add_argument("--type", "-t", choices=["factual", "episodic", "relational", "emotional", "procedural", "opinion"])
    
    # hebbian
    hebb_parser = subparsers.add_parser("hebbian", help="Show Hebbian links")
    hebb_parser.add_argument("query", help="Query to find memory")
    
    args = parser.parse_args()
    
    if args.command is None:
        parser.print_help()
        sys.exit(1)
    
    commands = {
        "add": cmd_add,
        "recall": cmd_recall,
        "stats": cmd_stats,
        "consolidate": cmd_consolidate,
        "forget": cmd_forget,
        "export": cmd_export,
        "list": cmd_list,
        "hebbian": cmd_hebbian,
    }
    
    commands[args.command](args)


if __name__ == "__main__":
    main()
