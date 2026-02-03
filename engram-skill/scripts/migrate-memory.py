#!/usr/bin/env python3
"""
Migrate existing Clawdbot memory files (MEMORY.md + memory/YYYY-MM-DD.md) into Engram.

Usage:
    python3 migrate-memory.py --memory-dir /Users/potato/clawd --output ./engram.db
    python3 migrate-memory.py --memory-dir /Users/potato/clawd --output ./engram.db --dry-run
"""

import argparse
import glob
import os
import re
import sys

_script_dir = os.path.dirname(os.path.abspath(__file__))
_project_root = os.path.abspath(os.path.join(_script_dir, "..", ".."))
sys.path.insert(0, _project_root)
sys.path.insert(0, os.path.join(_project_root, "engram"))

from engram.memory import Memory

# Keywords for classification heuristics
CLASSIFICATION_RULES = [
    # (type, importance, patterns)
    ("emotional", 0.8, [
        r"\b(feel|feeling|felt|love|hate|angry|happy|sad|frustrated|excited|worried|afraid)\b",
        r"\b(kinda like|appreciate|trust|care about)\b",
        r"❤|💙|😊|😢|😡|🥰",
    ]),
    ("relational", 0.65, [
        r"\b(prefer|likes?|dislikes?|wants?|personality|relationship|friend|partner)\b",
        r"\b(potato|user|human)\s+(is|prefers|likes|wants|said)\b",
        r"\b(name is|calls? (me|him|her|them))\b",
    ]),
    ("procedural", 0.7, [
        r"\b(how to|steps?|workflow|process|always use|never use|make sure|don't forget)\b",
        r"\b(command|script|run|execute|install|deploy|build|config)\b",
        r"```",
        r"\b(URL|API|endpoint|base url|redirect)\b",
    ]),
    ("opinion", 0.4, [
        r"\b(think|believe|opinion|assessment|seems? like|probably|might be)\b",
        r"\b(better|worse|best|worst|should|shouldn't)\b",
    ]),
    ("episodic", 0.35, [
        r"\b(today|yesterday|last (week|night|time)|earlier|just now|happened)\b",
        r"\b(session|conversation|discussed|talked about|worked on)\b",
        r"\d{4}-\d{2}-\d{2}",
    ]),
    # factual is the fallback
]


def classify_entry(text: str) -> tuple[str, float]:
    """Classify a memory entry by content. Returns (type, importance)."""
    text_lower = text.lower()
    for mem_type, importance, patterns in CLASSIFICATION_RULES:
        for pat in patterns:
            if re.search(pat, text_lower):
                return mem_type, importance
    return "factual", 0.5


def parse_memory_md(filepath: str) -> list[dict]:
    """Parse MEMORY.md into individual memory entries."""
    if not os.path.exists(filepath):
        return []

    with open(filepath, "r") as f:
        content = f.read()

    entries = []
    current_section = ""
    current_lines = []

    for line in content.split("\n"):
        # Section headers
        if line.startswith("#"):
            # Flush previous
            if current_lines:
                text = "\n".join(current_lines).strip()
                if text:
                    entries.append({"content": text, "source": f"MEMORY.md:{current_section}"})
            current_section = line.lstrip("#").strip()
            current_lines = []
            continue

        # Bullet points are individual entries
        if re.match(r"^\s*[-*]\s+", line):
            # Flush any accumulated non-bullet text
            if current_lines and not any(re.match(r"^\s*[-*]\s+", l) for l in current_lines):
                text = "\n".join(current_lines).strip()
                if text:
                    entries.append({"content": text, "source": f"MEMORY.md:{current_section}"})
                current_lines = []

            bullet_text = re.sub(r"^\s*[-*]\s+", "", line).strip()
            if bullet_text:
                entries.append({"content": bullet_text, "source": f"MEMORY.md:{current_section}"})
        else:
            if line.strip():
                current_lines.append(line)

    # Flush remaining
    if current_lines:
        text = "\n".join(current_lines).strip()
        if text:
            entries.append({"content": text, "source": f"MEMORY.md:{current_section}"})

    return entries


def parse_daily_md(filepath: str) -> list[dict]:
    """Parse a daily memory/YYYY-MM-DD.md file into entries."""
    if not os.path.exists(filepath):
        return []

    basename = os.path.basename(filepath)
    with open(filepath, "r") as f:
        content = f.read()

    entries = []
    current_section = basename
    buffer = []

    for line in content.split("\n"):
        if line.startswith("#"):
            if buffer:
                text = "\n".join(buffer).strip()
                if text and len(text) > 10:
                    entries.append({"content": text, "source": f"{basename}:{current_section}"})
                buffer = []
            current_section = line.lstrip("#").strip()
            continue

        if re.match(r"^\s*[-*]\s+", line):
            if buffer:
                text = "\n".join(buffer).strip()
                if text and len(text) > 10:
                    entries.append({"content": text, "source": f"{basename}:{current_section}"})
                buffer = []
            bullet_text = re.sub(r"^\s*[-*]\s+", "", line).strip()
            if bullet_text and len(bullet_text) > 10:
                entries.append({"content": bullet_text, "source": f"{basename}:{current_section}"})
        else:
            if line.strip():
                buffer.append(line)

    if buffer:
        text = "\n".join(buffer).strip()
        if text and len(text) > 10:
            entries.append({"content": text, "source": f"{basename}:{current_section}"})

    return entries


def migrate(memory_dir: str, output_db: str, dry_run: bool = False):
    """Run the full migration."""
    all_entries = []

    # Parse MEMORY.md
    memory_md = os.path.join(memory_dir, "MEMORY.md")
    if os.path.exists(memory_md):
        entries = parse_memory_md(memory_md)
        print(f"  MEMORY.md: {len(entries)} entries")
        all_entries.extend(entries)
    else:
        print(f"  MEMORY.md: not found at {memory_md}")

    # Parse daily files
    daily_dir = os.path.join(memory_dir, "memory")
    daily_files = sorted(glob.glob(os.path.join(daily_dir, "*.md")))
    for f in daily_files:
        entries = parse_daily_md(f)
        if entries:
            print(f"  {os.path.basename(f)}: {len(entries)} entries")
            all_entries.extend(entries)

    if not all_entries:
        print("No entries found to migrate.")
        return

    # Classify
    classified = []
    type_counts = {}
    for entry in all_entries:
        mem_type, importance = classify_entry(entry["content"])
        entry["type"] = mem_type
        entry["importance"] = importance
        classified.append(entry)
        type_counts[mem_type] = type_counts.get(mem_type, 0) + 1

    print(f"\nClassification:")
    for t, c in sorted(type_counts.items()):
        print(f"  {t:12s}: {c}")
    print(f"  {'TOTAL':12s}: {len(classified)}")

    if dry_run:
        print("\n[DRY RUN] Would import the above. Use without --dry-run to execute.")
        print("\nSample entries:")
        for entry in classified[:10]:
            print(f"  [{entry['type']:12s} imp={entry['importance']:.1f}] "
                  f"{entry['content'][:80]}{'...' if len(entry['content']) > 80 else ''}")
        return

    # Import
    mem = Memory(output_db)
    imported = 0
    for entry in classified:
        try:
            mem.add(
                content=entry["content"],
                type=entry["type"],
                importance=entry["importance"],
                source=entry["source"],
            )
            imported += 1
        except Exception as e:
            print(f"  Warning: failed to import entry: {e}", file=sys.stderr)

    mem.close()
    print(f"\nImported {imported}/{len(classified)} entries into {output_db}")


def main():
    parser = argparse.ArgumentParser(description="Migrate Clawdbot memory files to Engram")
    parser.add_argument("--memory-dir", required=True, help="Path to Clawdbot workspace (contains MEMORY.md and memory/)")
    parser.add_argument("--output", required=True, help="Output Engram database path")
    parser.add_argument("--dry-run", action="store_true", help="Preview without importing")
    args = parser.parse_args()

    print(f"Migrating from {args.memory_dir} → {args.output}\n")
    migrate(args.memory_dir, args.output, dry_run=args.dry_run)


if __name__ == "__main__":
    main()
