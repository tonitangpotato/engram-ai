#!/usr/bin/env python3
"""
Engram CLI — standalone command-line interface to the Engram memory system.
No MCP server required. Directly imports from the engram package.

Usage:
    python3 engram-cli.py add "memory content" --type factual --importance 0.5
    python3 engram-cli.py recall "query" --limit 5
    python3 engram-cli.py consolidate [--days 1.0]
    python3 engram-cli.py stats
    python3 engram-cli.py export ./backup.db
    python3 engram-cli.py pin <memory-id>
    python3 engram-cli.py unpin <memory-id>
    python3 engram-cli.py forget --id <memory-id>
    python3 engram-cli.py forget --prune --threshold 0.01
    python3 engram-cli.py reward "feedback text"
"""

import argparse
import json
import sys
import os

# Add engram package to path
_script_dir = os.path.dirname(os.path.abspath(__file__))
_project_root = os.path.abspath(os.path.join(_script_dir, "..", ".."))
sys.path.insert(0, _project_root)
sys.path.insert(0, os.path.join(_project_root, "engram"))

from engram.memory import Memory

DEFAULT_DB = os.environ.get("ENGRAM_DB", os.path.join(_project_root, "engram.db"))

VALID_TYPES = ["factual", "episodic", "relational", "procedural", "emotional", "opinion"]


def cmd_add(args, mem: Memory):
    content = args.content
    mem_type = args.type or "factual"
    if mem_type not in VALID_TYPES:
        print(f"Error: invalid type '{mem_type}'. Must be one of: {', '.join(VALID_TYPES)}", file=sys.stderr)
        sys.exit(1)
    importance = args.importance
    tags = args.tags.split(",") if args.tags else None
    source = args.source or ""

    mid = mem.add(content, type=mem_type, importance=importance, source=source, tags=tags)
    print(f"Stored: {mid}")


def cmd_recall(args, mem: Memory):
    results = mem.recall(
        args.query,
        limit=args.limit,
        types=args.types.split(",") if args.types else None,
        min_confidence=args.min_confidence,
    )
    if not results:
        print("No memories found.")
        return

    if args.json:
        print(json.dumps(results, indent=2))
        return

    for r in results:
        label = r["confidence_label"]
        conf = r["confidence"]
        age = r["age_days"]
        print(f"  [{label:10s}] conf={conf:.2f} age={age:.1f}d | {r['content']}")
        if args.verbose:
            print(f"             id={r['id']} type={r['type']} layer={r['layer']} "
                  f"str={r['strength']:.3f} imp={r['importance']:.2f}")


def cmd_consolidate(args, mem: Memory):
    days = args.days or 1.0
    mem.consolidate(days=days)
    print(f"Consolidation complete ({days} day{'s' if days != 1 else ''}).")


def cmd_stats(args, mem: Memory):
    s = mem.stats()
    if args.json:
        print(json.dumps(s, indent=2, default=str))
        return

    print(f"Total memories: {s['total_memories']}")
    print(f"Pinned: {s['pinned']}")
    print(f"Uptime: {s['uptime_hours']}h")
    print(f"Layers: {json.dumps(s['layers'])}")
    if s["by_type"]:
        print("By type:")
        for t, info in s["by_type"].items():
            print(f"  {t:12s}: {info['count']} entries, "
                  f"avg_str={info['avg_strength']:.3f}, avg_imp={info['avg_importance']:.2f}")


def cmd_export(args, mem: Memory):
    mem.export(args.path)
    print(f"Exported to {args.path}")


def cmd_pin(args, mem: Memory):
    mem.pin(args.id)
    print(f"Pinned: {args.id}")


def cmd_unpin(args, mem: Memory):
    mem.unpin(args.id)
    print(f"Unpinned: {args.id}")


def cmd_forget(args, mem: Memory):
    if args.id:
        mem.forget(memory_id=args.id)
        print(f"Forgot: {args.id}")
    elif args.prune:
        mem.forget(threshold=args.threshold)
        print(f"Pruned memories below threshold {args.threshold}")
    else:
        print("Specify --id or --prune", file=sys.stderr)
        sys.exit(1)


def cmd_reward(args, mem: Memory):
    mem.reward(args.feedback)
    print("Reward applied.")


def main():
    parser = argparse.ArgumentParser(description="Engram Memory CLI")
    parser.add_argument("--db", default=DEFAULT_DB, help=f"Database path (default: {DEFAULT_DB})")
    sub = parser.add_subparsers(dest="command", required=True)

    # add
    p = sub.add_parser("add", help="Store a new memory")
    p.add_argument("content", help="Memory content")
    p.add_argument("--type", "-t", default="factual", help=f"Type: {', '.join(VALID_TYPES)}")
    p.add_argument("--importance", "-i", type=float, default=None, help="Importance 0-1")
    p.add_argument("--source", "-s", default="", help="Source identifier")
    p.add_argument("--tags", default=None, help="Comma-separated tags")

    # recall
    p = sub.add_parser("recall", help="Recall memories")
    p.add_argument("query", help="Search query")
    p.add_argument("--limit", "-l", type=int, default=5)
    p.add_argument("--types", default=None, help="Comma-separated type filter")
    p.add_argument("--min-confidence", type=float, default=0.0)
    p.add_argument("--json", action="store_true", help="JSON output")
    p.add_argument("--verbose", "-v", action="store_true")

    # consolidate
    p = sub.add_parser("consolidate", help="Run consolidation cycle")
    p.add_argument("--days", type=float, default=1.0)

    # stats
    p = sub.add_parser("stats", help="Show memory statistics")
    p.add_argument("--json", action="store_true")

    # export
    p = sub.add_parser("export", help="Export database")
    p.add_argument("path", help="Output file path")

    # pin / unpin
    p = sub.add_parser("pin", help="Pin a memory")
    p.add_argument("id", help="Memory ID")

    p = sub.add_parser("unpin", help="Unpin a memory")
    p.add_argument("id", help="Memory ID")

    # forget
    p = sub.add_parser("forget", help="Forget memories")
    p.add_argument("--id", default=None, help="Specific memory ID")
    p.add_argument("--prune", action="store_true", help="Prune all weak memories")
    p.add_argument("--threshold", type=float, default=0.01)

    # reward
    p = sub.add_parser("reward", help="Apply feedback reward")
    p.add_argument("feedback", help="Feedback text")

    args = parser.parse_args()
    mem = Memory(args.db)

    try:
        {
            "add": cmd_add,
            "recall": cmd_recall,
            "consolidate": cmd_consolidate,
            "stats": cmd_stats,
            "export": cmd_export,
            "pin": cmd_pin,
            "unpin": cmd_unpin,
            "forget": cmd_forget,
            "reward": cmd_reward,
        }[args.command](args, mem)
    finally:
        mem.close()


if __name__ == "__main__":
    main()
