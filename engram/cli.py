#!/usr/bin/env python3
"""
Engram CLI

Usage:
    engram init                          # First-time setup
    engram status                        # Show current config
    engram add "memory content" [--type TYPE] [--importance IMPORTANCE]
    engram recall "query" [--limit LIMIT]
    engram stats
    engram consolidate
    engram forget [--threshold THRESHOLD]
    engram export OUTPUT_PATH
    engram list [--limit LIMIT] [--type TYPE]
    engram import PATH [PATH...] [--verbose]
    engram update ID NEW_CONTENT
    engram pin ID
    engram unpin ID
    engram reward "feedback text"
    engram info ID
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
    from engram.cli_config import (
        load_config, save_config, resolve_embedding, resolve_db,
        detect_embedding_models, CONFIG_FILE,
    )
except ImportError:
    # If running from source directory
    sys.path.insert(0, str(Path(__file__).parent.parent))
    from engram import Memory
    from engram.config import MemoryConfig
    from engram.cli_config import (
        load_config, save_config, resolve_embedding, resolve_db,
        detect_embedding_models, CONFIG_FILE,
    )


DEFAULT_DB = os.environ.get("NEUROMEM_DB", "./neuromem.db")


def get_memory(db_path: str = DEFAULT_DB, embedding: str = None) -> Memory:
    """Get or create a memory instance using config priority chain."""
    resolved = resolve_embedding(embedding)
    db = resolve_db(db_path)
    return Memory(db, embedding=resolved)


def cmd_init(args):
    """Interactive first-time setup."""
    print("🧠 Engram Setup\n")

    # Detect available providers
    models = detect_embedding_models()

    cfg = load_config()

    # DB path
    current_db = cfg.get("db", DEFAULT_DB)
    db = input(f"Database path [{current_db}]: ").strip() or current_db
    cfg["db"] = db

    # Embedding
    if models:
        print("\nDetected embedding providers:")
        for i, m in enumerate(models, 1):
            tag = "local, free" if m["free"] else "API key required"
            print(f"  {i}. {m['provider']} ({m['model']}) — {tag}")
        print(f"  {len(models) + 1}. None (FTS5 text search only)")

        choice = input(f"\nSelect embedding provider [1]: ").strip()
        if not choice or choice == "1":
            cfg["embedding"] = models[0]["provider"]
            cfg["embedding_model"] = models[0]["model"]
        elif choice == str(len(models) + 1):
            cfg["embedding"] = None
            cfg.pop("embedding_model", None)
        else:
            idx = int(choice) - 1
            if 0 <= idx < len(models):
                cfg["embedding"] = models[idx]["provider"]
                cfg["embedding_model"] = models[idx]["model"]
    else:
        print("\nNo embedding providers detected.")
        print("  Install Ollama (ollama.ai) + `ollama pull nomic-embed-text` for semantic search.")
        print("  Or set OPENAI_API_KEY for OpenAI embeddings.")
        print("  Continuing with FTS5 text search only.\n")
        cfg["embedding"] = None

    # LLM Extractor
    print("\n--- LLM Extraction (optional) ---")
    print("Extract structured facts from raw text using an LLM.")
    print("Auth tokens come from environment variables (never stored in config).")
    print("  ANTHROPIC_AUTH_TOKEN → OAuth (Claude Max)")
    print("  ANTHROPIC_API_KEY   → API key")

    ext_choices = ["anthropic", "ollama", "none"]
    ext_current = cfg.get("extractor", {}).get("provider", "none")
    ext_choice = input(f"\nExtractor provider [{ext_current}]: ").strip() or ext_current

    if ext_choice == "anthropic":
        ext_model = input("  Model [claude-haiku-4-5-20251001]: ").strip() or "claude-haiku-4-5-20251001"
        cfg["extractor"] = {"provider": "anthropic", "model": ext_model}
    elif ext_choice == "ollama":
        ext_model = input("  Model [llama3.2:3b]: ").strip() or "llama3.2:3b"
        ext_host = input("  Host [http://localhost:11434]: ").strip() or "http://localhost:11434"
        cfg["extractor"] = {"provider": "ollama", "model": ext_model, "host": ext_host}
    else:
        cfg.pop("extractor", None)

    save_config(cfg)
    print(f"\n✅ Config saved to {CONFIG_FILE}")
    print(f"   DB: {cfg['db']}")
    print(f"   Embedding: {cfg.get('embedding') or 'none (FTS5)'}")
    if cfg.get("embedding_model"):
        print(f"   Model: {cfg['embedding_model']}")
    ext_info = cfg.get("extractor", {}).get("provider", "none")
    print(f"   Extractor: {ext_info}")


def cmd_status(args):
    """Show current configuration and status."""
    cfg = load_config()
    resolved_embed = resolve_embedding(getattr(args, "embedding", None))
    resolved_db = resolve_db(args.db)

    print("🧠 Engram Status\n")

    # Config source
    if CONFIG_FILE.exists():
        print(f"Config: {CONFIG_FILE}")
    else:
        print(f"Config: not initialized (run `engram init`)")

    # DB
    db_source = "flag" if args.db != DEFAULT_DB else ("env" if os.environ.get("NEUROMEM_DB") else ("config" if cfg.get("db") else "default"))
    print(f"DB: {resolved_db} (from {db_source})")

    # Embedding
    if getattr(args, "embedding", None):
        embed_source = "flag"
    elif os.environ.get("ENGRAM_EMBEDDING"):
        embed_source = "env"
    elif cfg.get("embedding"):
        embed_source = "config"
    else:
        embed_source = "none"
    print(f"Embedding: {resolved_embed or 'none (FTS5 only)'} (from {embed_source})")

    # Check DB exists
    db_path = Path(resolved_db).expanduser()
    if db_path.exists():
        size_mb = db_path.stat().st_size / 1024 / 1024
        try:
            mem = Memory(str(db_path))
            stats = mem.stats()
            print(f"\nMemories: {stats['total_memories']}")
            print(f"DB size: {size_mb:.1f} MB")
            mem.close()
        except Exception as e:
            print(f"\nDB exists ({size_mb:.1f} MB) but error reading: {e}")
    else:
        print(f"\nDB not found at {resolved_db}")

    # Check providers
    models = detect_embedding_models()
    if models:
        print(f"\nAvailable providers:")
        for m in models:
            active = " ← active" if m["provider"] == resolved_embed else ""
            print(f"  • {m['provider']} ({m['model']}){active}")


def cmd_add(args):
    """Add a new memory."""
    mem = get_memory(args.db, getattr(args, "embedding", None))

    kwargs = {}
    if args.type:
        kwargs["type"] = args.type
    if args.importance:
        kwargs["importance"] = float(args.importance)

    mem_id = mem.add(args.content, **kwargs)
    print(f"✓ Added memory: {mem_id[:8]}...")
    print(f"  Content: {args.content[:120]}{'...' if len(args.content) > 120 else ''}")

    mem.close()


def cmd_recall(args):
    """Recall memories matching a query."""
    mem = get_memory(args.db, getattr(args, "embedding", None))

    results = mem.recall(args.query, limit=args.limit)

    if not results:
        print("No memories found.")
    else:
        print(f"Found {len(results)} memories:\n")
        for i, r in enumerate(results, 1):
            conf = r.get("confidence_label", "?")
            typ = r.get("type", "?")[:4]
            content = r["content"]
            print(f"  {i}. [{conf:8}] [{typ}] {content}")

    mem.close()


def cmd_stats(args):
    """Show memory statistics."""
    mem = get_memory(args.db, getattr(args, "embedding", None))
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
    mem = get_memory(args.db, getattr(args, "embedding", None))
    result = mem.consolidate(days=args.days)

    print(f"✓ Consolidation complete ({args.days} day(s))")
    if result:
        print(f"  {result}")

    mem.close()


def cmd_forget(args):
    """Prune weak memories."""
    mem = get_memory(args.db, getattr(args, "embedding", None))

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
    mem = get_memory(args.db, getattr(args, "embedding", None))
    mem.export(args.output)

    size = os.path.getsize(args.output)
    print(f"✓ Exported to {args.output} ({size} bytes)")

    mem.close()


def cmd_list(args):
    """List memories."""
    mem = get_memory(args.db, getattr(args, "embedding", None))

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
    mem = get_memory(args.db, getattr(args, "embedding", None))

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
            print(f"  → {linked.content}")

    mem.close()


def cmd_update(args):
    """Update a memory's content."""
    mem = get_memory(args.db, getattr(args, "embedding", None))

    new_id = mem.update_memory(args.id, args.new_content)
    print(f"✓ Updated memory {args.id[:8]}...")
    print(f"  New memory: {new_id[:8]}...")
    print(f"  Content: {args.new_content[:120]}{'...' if len(args.new_content) > 120 else ''}")

    mem.close()


def cmd_pin(args):
    """Pin a memory (won't decay or be pruned)."""
    mem = get_memory(args.db, getattr(args, "embedding", None))
    mem.pin(args.id)
    print(f"✓ Pinned memory {args.id[:8]}...")
    mem.close()


def cmd_unpin(args):
    """Unpin a memory (resumes normal decay)."""
    mem = get_memory(args.db, getattr(args, "embedding", None))
    mem.unpin(args.id)
    print(f"✓ Unpinned memory {args.id[:8]}...")
    mem.close()


def cmd_reward(args):
    """Reward recent memories based on feedback."""
    mem = get_memory(args.db, getattr(args, "embedding", None))
    mem.reward(args.feedback)
    print(f"✓ Applied reward feedback: {args.feedback[:120]}{'...' if len(args.feedback) > 120 else ''}")
    mem.close()


def cmd_info(args):
    """Show full details of a single memory."""
    mem = get_memory(args.db, getattr(args, "embedding", None))

    entry = mem._store.get(args.id)
    if entry is None:
        print(f"Memory {args.id} not found.")
        mem.close()
        sys.exit(1)

    print(f"=== Memory Details ===\n")
    print(f"ID:         {entry.id}")
    print(f"Content:    {entry.content}")
    print(f"Type:       {entry.memory_type.value}")
    print(f"Layer:      {entry.layer.value}")
    print(f"Importance: {entry.importance:.3f}")
    print(f"Created:    {entry.created_at}")
    access_count = len(entry.access_times) if hasattr(entry, 'access_times') and entry.access_times else 0
    print(f"Accessed:   {access_count} times")
    print(f"Pinned:     {entry.pinned}")

    # Hebbian links
    links = mem.hebbian_links(entry.id)
    if links:
        print(f"\nHebbian links ({len(links)}):")
        for link_id in links[:10]:
            linked = mem._store.get(link_id)
            if linked:
                content = linked.content
                if len(content) > 60:
                    content = content[:57] + "..."
                print(f"  → [{linked.memory_type.value[:4]}] {content}")
    else:
        print(f"\nHebbian links: none")

    mem.close()


def cmd_migrate(args):
    """Migrate database to v1 unified schema."""
    from engram.migrate import migrate_to_v1
    db = resolve_db(args.db)
    migrate_to_v1(db, dry_run=args.dry_run)


def cmd_import(args):
    """Import memories from markdown files."""
    from .import_markdown import import_memories

    result = import_memories(
        paths=args.paths,
        db_path=args.db,
        consolidate=not args.no_consolidate,
        verbose=args.verbose,
    )

    print(f"\n✓ Import complete")
    print(f"  Imported: {result['imported']}")
    if result['failed']:
        print(f"  Failed: {result['failed']}")
    print(f"  Total memories: {result['total_memories']}")
    print(f"  By type: {result['by_type']}")


def main():
    parser = argparse.ArgumentParser(
        description="engram: Neuroscience-grounded memory for AI agents",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument("--db", default=DEFAULT_DB, help="Database path")
    parser.add_argument("--embedding", "-e", choices=["ollama", "openai"], default=None,
                       help="Embedding provider (overrides config)")

    subparsers = parser.add_subparsers(dest="command", help="Commands")

    # init
    subparsers.add_parser("init", help="First-time setup")

    # status
    subparsers.add_parser("status", help="Show current config and status")

    # add
    add_parser = subparsers.add_parser("add", help="Add a memory")
    add_parser.add_argument("content", help="Memory content")
    add_parser.add_argument("--type", "-t", choices=["factual", "episodic", "relational", "emotional", "procedural", "opinion", "causal"])
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
    list_parser.add_argument("--type", "-t", choices=["factual", "episodic", "relational", "emotional", "procedural", "opinion", "causal"])

    # hebbian
    hebb_parser = subparsers.add_parser("hebbian", help="Show Hebbian links")
    hebb_parser.add_argument("query", help="Query to find memory")

    # update
    update_parser = subparsers.add_parser("update", help="Update a memory's content")
    update_parser.add_argument("id", help="Memory ID to update")
    update_parser.add_argument("new_content", help="New content for the memory")

    # pin
    pin_parser = subparsers.add_parser("pin", help="Pin a memory (prevents decay)")
    pin_parser.add_argument("id", help="Memory ID to pin")

    # unpin
    unpin_parser = subparsers.add_parser("unpin", help="Unpin a memory (resumes decay)")
    unpin_parser.add_argument("id", help="Memory ID to unpin")

    # reward
    reward_parser = subparsers.add_parser("reward", help="Reward recent memories with feedback")
    reward_parser.add_argument("feedback", help="Feedback text (positive/negative)")

    # info
    info_parser = subparsers.add_parser("info", help="Show full details of a memory")
    info_parser.add_argument("id", help="Memory ID to inspect")

    # import
    import_parser = subparsers.add_parser("import", help="Import from markdown files")
    import_parser.add_argument("paths", nargs="+", help="Files or directories to import")
    import_parser.add_argument("--no-consolidate", action="store_true", help="Skip consolidation")
    import_parser.add_argument("-v", "--verbose", action="store_true", help="Verbose output")

    # migrate
    migrate_parser = subparsers.add_parser("migrate", help="Migrate database to v1 schema")
    migrate_parser.add_argument("--dry-run", action="store_true", help="Print SQL without executing")

    # mcp
    subparsers.add_parser("mcp", help="Start MCP server (stdio transport)")

    args = parser.parse_args()

    if args.command is None:
        parser.print_help()
        sys.exit(1)

    # MCP server gets special handling — pass config via env before import
    if args.command == "mcp":
        resolved_db_path = resolve_db(args.db)
        resolved_embed = resolve_embedding(getattr(args, "embedding", None))
        os.environ["ENGRAM_DB"] = resolved_db_path
        if resolved_embed:
            os.environ["ENGRAM_EMBEDDING"] = resolved_embed
        from engram.mcp_server import run_server
        run_server()
        return

    commands = {
        "init": cmd_init,
        "status": cmd_status,
        "add": cmd_add,
        "recall": cmd_recall,
        "stats": cmd_stats,
        "consolidate": cmd_consolidate,
        "forget": cmd_forget,
        "export": cmd_export,
        "list": cmd_list,
        "hebbian": cmd_hebbian,
        "import": cmd_import,
        "migrate": cmd_migrate,
        "update": cmd_update,
        "pin": cmd_pin,
        "unpin": cmd_unpin,
        "reward": cmd_reward,
        "info": cmd_info,
    }

    commands[args.command](args)


if __name__ == "__main__":
    main()
