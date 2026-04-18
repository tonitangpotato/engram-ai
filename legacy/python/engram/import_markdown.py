#!/usr/bin/env python3
"""
Import memories from markdown files into Engram.

Supports common agent memory formats:
- MEMORY.md (long-term curated memory)
- Daily logs (memory/YYYY-MM-DD.md)
- Generic markdown with bullet points

Usage:
    # Import from a directory
    engram import ./memory/
    
    # Import specific files
    engram import MEMORY.md memory/2024-01-15.md
    
    # With custom database
    ENGRAM_DB_PATH=./agent.db engram import ./memory/
"""

import re
import os
from pathlib import Path
from typing import Iterator

from .memory import Memory

# Type inference based on content patterns
TYPE_PATTERNS = [
    (r"(?i)(prefer|like|want|hate|dislike|love)", "relational"),
    (r"(?i)(learned|lesson|mistake|insight|realized)", "procedural"),
    (r"(?i)(feel|emotion|happy|sad|frustrat|excit)", "emotional"),
    (r"(?i)(on \d{4}-\d{2}-\d{2}|today|yesterday|last week)", "episodic"),
    (r"(?i)(opinion|think|believe|should)", "opinion"),
]


def infer_type(content: str) -> str:
    """Infer memory type from content."""
    for pattern, mem_type in TYPE_PATTERNS:
        if re.search(pattern, content):
            return mem_type
    return "factual"


def infer_importance(content: str, source: str) -> float:
    """Infer importance based on content and source."""
    importance = 0.5
    
    # Boost for curated memory files
    if "MEMORY" in source.upper():
        importance += 0.2
    
    # Boost for key words
    if re.search(r"(?i)(important|critical|key|essential|must|always|never)", content):
        importance += 0.15
    
    # Boost for lessons/insights
    if re.search(r"(?i)(learned|lesson|insight|realized|mistake)", content):
        importance += 0.1
    
    # Boost for preferences
    if re.search(r"(?i)(prefer|like|want|love)", content):
        importance += 0.1
    
    return min(importance, 1.0)


def parse_markdown_file(path: Path) -> Iterator[dict]:
    """
    Parse a markdown file into memory entries.
    
    Extracts bullet points as individual memories,
    tracking the section context.
    """
    if not path.exists():
        return
    
    content = path.read_text()
    filename = path.name
    
    # Check if it's a date-based file
    date_match = re.match(r"(\d{4}-\d{2}-\d{2})", filename)
    date_prefix = f"[{date_match.group(1)}] " if date_match else ""
    
    current_section = ""
    
    for line in content.splitlines():
        line = line.strip()
        
        # Track sections (## or ###)
        if line.startswith("## "):
            current_section = line[3:].strip()
            continue
        if line.startswith("### "):
            current_section = line[4:].strip()
            continue
        
        # Skip headers and empty lines
        if not line or line.startswith("#"):
            continue
        
        # Extract bullet points as memories
        if line.startswith("- "):
            entry = line[2:].strip()
            
            # Skip very short entries (likely not meaningful)
            min_length = 15 if date_prefix else 10
            if len(entry) < min_length:
                continue
            
            # Skip entries that are just links or references
            if entry.startswith("[") and "](" in entry and entry.endswith(")"):
                continue
            
            yield {
                "content": f"{date_prefix}{entry}" if date_prefix else entry,
                "section": current_section,
                "source": f"{filename}/{current_section}" if current_section else filename,
            }


def import_path(
    mem: Memory,
    path: Path,
    verbose: bool = False,
) -> tuple[int, int]:
    """
    Import memories from a path (file or directory).
    
    Returns (success_count, failed_count).
    """
    success = 0
    failed = 0
    seen_content = set()  # Deduplication
    
    # Collect files to process
    files: list[Path] = []
    
    if path.is_file():
        files = [path]
    elif path.is_dir():
        # Look for markdown files
        files = list(path.glob("*.md"))
        # Also check for MEMORY.md in parent
        parent_memory = path.parent / "MEMORY.md"
        if parent_memory.exists() and parent_memory not in files:
            files.insert(0, parent_memory)
    
    if verbose:
        print(f"Found {len(files)} markdown files to process")
    
    for filepath in sorted(files):
        if verbose:
            print(f"  Processing: {filepath.name}")
        
        for entry in parse_markdown_file(filepath):
            # Deduplicate by content prefix
            content_key = entry["content"][:100].lower()
            if content_key in seen_content:
                continue
            seen_content.add(content_key)
            
            # Infer type and importance
            mem_type = infer_type(entry["content"])
            importance = infer_importance(entry["content"], entry["source"])
            
            try:
                mem.add(
                    content=entry["content"],
                    type=mem_type,
                    importance=importance,
                    source=entry["source"],
                )
                success += 1
            except Exception as e:
                failed += 1
                if verbose and failed <= 3:
                    print(f"    Error: {e}")
    
    return success, failed


def import_memories(
    paths: list[str],
    db_path: str | None = None,
    consolidate: bool = True,
    verbose: bool = False,
) -> dict:
    """
    Import memories from markdown files into Engram.
    
    Args:
        paths: List of file or directory paths to import
        db_path: Database path (default: ENGRAM_DB_PATH or ./engram.db)
        consolidate: Run consolidation after import to form Hebbian links
        verbose: Print progress
    
    Returns:
        Dict with import statistics
    """
    if db_path is None:
        db_path = os.environ.get("ENGRAM_DB_PATH", "./engram.db")
    
    mem = Memory(db_path)
    
    total_success = 0
    total_failed = 0
    
    for path_str in paths:
        path = Path(path_str).expanduser().resolve()
        
        if not path.exists():
            if verbose:
                print(f"Path not found: {path}")
            continue
        
        success, failed = import_path(mem, path, verbose=verbose)
        total_success += success
        total_failed += failed
    
    # Run consolidation to form Hebbian links
    if consolidate and total_success > 0:
        if verbose:
            print("Running consolidation...")
        mem.consolidate()
    
    stats = mem.stats()
    
    return {
        "imported": total_success,
        "failed": total_failed,
        "total_memories": stats["total_memories"],
        "by_type": {k: v["count"] for k, v in stats["by_type"].items()},
    }


# CLI integration
def add_import_command(subparsers):
    """Add import subcommand to CLI."""
    parser = subparsers.add_parser(
        "import",
        help="Import memories from markdown files",
    )
    parser.add_argument(
        "paths",
        nargs="+",
        help="Files or directories to import",
    )
    parser.add_argument(
        "--no-consolidate",
        action="store_true",
        help="Skip consolidation after import",
    )
    parser.add_argument(
        "-v", "--verbose",
        action="store_true",
        help="Print progress",
    )
    
    def handler(args):
        result = import_memories(
            paths=args.paths,
            consolidate=not args.no_consolidate,
            verbose=args.verbose,
        )
        
        print(f"Imported: {result['imported']}")
        if result['failed']:
            print(f"Failed: {result['failed']}")
        print(f"Total memories: {result['total_memories']}")
        print(f"By type: {result['by_type']}")
    
    parser.set_defaults(func=handler)
