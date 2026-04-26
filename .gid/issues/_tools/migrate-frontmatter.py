#!/usr/bin/env python3
"""
Migrate a project's .gid/issues/ to v5.0.0 strict format.

For each ISS-NNN-slug.md (or ISS-NNN-slug/ dir):
1. Parse legacy header (Status:, Priority:, Filed:, Closed:, Component:, Related:, etc.)
2. Build YAML frontmatter
3. Move file to ISS-NNN/issue.md (git mv, preserves history)
4. Prepend frontmatter
5. If a directory ISS-NNN-slug/ already exists, rename to ISS-NNN/ and move sibling .md inside as issue.md
"""
import re
import subprocess
import sys
from pathlib import Path
from collections import defaultdict

if len(sys.argv) < 2:
    print("Usage: migrate-project.py <project_root>")
    sys.exit(1)

PROJECT = Path(sys.argv[1])
ISSUES_DIR = PROJECT / ".gid" / "issues"

if not ISSUES_DIR.is_dir():
    print(f"No issues dir at {ISSUES_DIR}")
    sys.exit(1)

ISSUE_RE = re.compile(r'^(ISS-\d+)(?:-(.+))?$')

def git_mv(src, dst):
    dst.parent.mkdir(parents=True, exist_ok=True)
    # Try git mv first (without -k so we see real failures)
    res = subprocess.run(
        ["git", "mv", str(src), str(dst)],
        cwd=PROJECT, capture_output=True, text=True
    )
    if res.returncode != 0:
        # File is untracked or some other issue — plain move
        import shutil
        shutil.move(str(src), str(dst))

def parse_header(text):
    """Extract metadata from legacy header lines like '**Status:** closed'."""
    meta = {}
    # Title from first line: "# ISS-NNN: Title" or "# ISS-NNN — Title" or "# ISS-NNN Title"
    lines = text.splitlines()
    title = None
    for line in lines[:3]:
        m = re.match(r'^#\s*ISS-\d+\s*[:\—\-]\s*(.+)$', line)
        if m:
            title = m.group(1).strip()
            # Strip trailing markdown like backticks
            break
        m = re.match(r'^#\s*ISS-\d+\s+(.+)$', line)
        if m:
            title = m.group(1).strip()
            break
    if title:
        meta["title"] = title

    # Walk header lines (until first --- or ## section)
    for line in lines[1:60]:
        if line.startswith("##") or line.strip() == "---":
            break
        # **Key:** value
        m = re.match(r'^\*\*([A-Za-z][A-Za-z _/]*)\:\*\*\s*(.+)$', line)
        if not m:
            continue
        key = m.group(1).strip().lower().replace(" ", "_").replace("/", "_")
        val = m.group(2).strip()
        meta[key] = val
    return meta

def normalize_status(raw):
    """Map various status strings to canonical set."""
    if not raw:
        return "open"
    r = raw.lower()
    # Strip parenthesized notes: "closed (2026-04-26)"
    r = re.sub(r'\s*\(.*\)\s*$', '', r).strip()
    if r in ("closed", "done", "resolved", "fixed"):
        return "closed"
    if "superseded" in r:
        return "superseded"
    if "wontfix" in r or "won't fix" in r:
        return "wontfix"
    if "blocked" in r:
        return "blocked"
    if "in_progress" in r or "in progress" in r or "wip" in r:
        return "in_progress"
    return "open"

def extract_date(raw, key="filed"):
    """Find a YYYY-MM-DD in a raw value or in any header that mentions it."""
    if not raw:
        return None
    m = re.search(r'(\d{4}-\d{2}-\d{2})', raw)
    return m.group(1) if m else None

def extract_priority(raw):
    if not raw:
        return None
    m = re.search(r'(P[0-3])', raw)
    return m.group(1) if m else None

def extract_severity(raw):
    if not raw:
        return None
    r = raw.lower()
    for s in ("critical", "high", "medium", "low"):
        if s in r:
            return s
    return None

def extract_related(raw):
    """Extract ISS-NNN refs from a related/cross-repo line."""
    if not raw:
        return []
    return list(dict.fromkeys(re.findall(r'(?:[a-zA-Z\-]+:)?ISS-\d+', raw)))

def fmt_yaml_str(s):
    s = s.replace('"', '\\"')
    return f'"{s}"'

def fmt_yaml_list(items):
    return "[" + ", ".join(fmt_yaml_str(x) for x in items) + "]"

def build_frontmatter(iid, meta, fallback_title):
    title = meta.get("title") or fallback_title
    status = normalize_status(meta.get("status"))
    priority = extract_priority(meta.get("priority", "")) or "P2"
    filed = extract_date(meta.get("filed") or meta.get("discovered") or meta.get("created") or meta.get("date"))
    closed = extract_date(meta.get("closed") or (meta.get("status") if status == "closed" else None))
    severity = extract_severity(meta.get("severity"))
    component = meta.get("component")
    related = extract_related(
        " ".join(filter(None, [
            meta.get("related"), meta.get("cross-repo"), meta.get("cross_repo"),
            meta.get("see_also"),
        ]))
    )
    # Filter out self-refs
    related = [r for r in related if r != iid and not r.endswith(":" + iid) and not r == iid]

    lines = ["---", f'id: {fmt_yaml_str(iid)}', f'title: {fmt_yaml_str(title)}',
             f'status: {status}', f'priority: {priority}']
    if filed:
        lines.append(f'created: {filed}')
    else:
        lines.append('created: 2026-04-26')  # fallback to today
    if closed and status in ("closed", "superseded"):
        lines.append(f'closed: {closed}')
    if severity:
        lines.append(f'severity: {severity}')
    if component:
        # strip backticks/markdown for cleanliness
        c = re.sub(r'`', '', component)
        lines.append(f'component: {fmt_yaml_str(c[:200])}')
    if related:
        lines.append(f'related: {fmt_yaml_list(related)}')
    lines.append("---")
    lines.append("")
    return "\n".join(lines)

# ---- Discover all entries ----
# Group by ISS-NNN (some have multiple files, some are dirs)
entries_by_id = defaultdict(list)  # iid -> list of (path, kind) where kind is 'file' or 'dir'
for entry in ISSUES_DIR.iterdir():
    if entry.name.startswith("_") or entry.name == "reviews":
        continue
    name = entry.stem if entry.is_file() else entry.name
    m = ISSUE_RE.match(name)
    if not m:
        print(f"SKIP (no ISS prefix): {entry.name}")
        continue
    iid = m.group(1)
    entries_by_id[iid].append(entry)

print(f"Found {len(entries_by_id)} unique issue IDs")

for iid in sorted(entries_by_id.keys()):
    entries = entries_by_id[iid]
    target_dir = ISSUES_DIR / iid
    target_issue = target_dir / "issue.md"

    # Skip if already canonical (target_dir exists with issue.md that has frontmatter)
    if target_issue.exists():
        head = target_issue.read_text()[:10]
        if head.startswith("---"):
            print(f"SKIP {iid}: already canonical")
            continue
        # Has issue.md but no frontmatter → parse + prepend
        if not files and not (existing_dir_for_iid := [d for d in entries if d.is_dir() and d.name != iid]):
            text = target_issue.read_text()
            meta = parse_header(text)
            fm = build_frontmatter(iid, meta, fallback_title=iid)
            target_issue.write_text(fm + text)
            print(f"  FRONTMATTER_ONLY {iid}/issue.md (status={normalize_status(meta.get('status'))})")
            continue

    # Find the "primary" file: the one with longest content matching ISS-NNN-slug.md
    files = [e for e in entries if e.is_file()]
    dirs = [e for e in entries if e.is_dir()]

    primary = None
    if files:
        # Prefer one whose name has a slug after ISS-NNN (the "main" file)
        # If multiple, pick the largest
        files.sort(key=lambda p: (-(p.stat().st_size)))
        primary = files[0]

    # If there's an existing dir (e.g. ISS-021-subdim-extraction-coverage/),
    # we want to rename it to ISS-NNN/ and put primary inside as issue.md
    existing_dir = None
    for d in dirs:
        if d.name != iid:
            existing_dir = d

    # Step 1: handle dirs
    if existing_dir and not target_dir.exists():
        # Rename old slug-dir to canonical iid
        git_mv(existing_dir, target_dir)
        print(f"  RENAME_DIR {existing_dir.name} -> {iid}")
    elif existing_dir and target_dir.exists():
        # Move all contents from existing_dir into target_dir
        for child in existing_dir.iterdir():
            git_mv(child, target_dir / child.name)
        existing_dir.rmdir() if existing_dir.exists() and not any(existing_dir.iterdir()) else None
        print(f"  MERGE_DIR {existing_dir.name} -> {iid}")
    elif not target_dir.exists():
        target_dir.mkdir()

    # Step 2: handle the primary file
    if primary and primary.exists():
        text = primary.read_text()
        meta = parse_header(text)
        fm = build_frontmatter(iid, meta, fallback_title=primary.stem.replace(f"{iid}-", "").replace("-", " "))
        # Move primary to issue.md (or merge if issue.md exists from a prior dir)
        if target_issue.exists() and primary.resolve() != target_issue.resolve():
            # An issue.md already exists in target_dir (from existing_dir merge).
            # The bare .md is the "real" issue body — replace.
            target_issue.unlink()
        if primary.resolve() != target_issue.resolve():
            git_mv(primary, target_issue)
        # Prepend frontmatter if not already present
        body = target_issue.read_text()
        if not body.lstrip().startswith("---"):
            target_issue.write_text(fm + body)
        print(f"  PRIMARY {primary.name} -> {iid}/issue.md ({normalize_status(meta.get('status'))})")

    # Step 3: handle additional files (e.g. ISS-003-investigation.md → ISS-003/investigation.md)
    for extra in files:
        if not extra.exists():
            continue  # already moved
        if extra.resolve() == (target_dir / "issue.md").resolve():
            continue
        # Strip leading "ISS-NNN-" prefix
        new_name = re.sub(rf'^{iid}-', '', extra.stem) + extra.suffix
        if not new_name or new_name == extra.suffix:
            new_name = "supplementary" + extra.suffix
        dest = target_dir / new_name
        # If dest exists, suffix with _2
        i = 2
        while dest.exists():
            dest = target_dir / f"{Path(new_name).stem}_{i}{Path(new_name).suffix}"
            i += 1
        git_mv(extra, dest)
        print(f"  EXTRA {extra.name} -> {iid}/{dest.name}")

    # Step 4: ensure issue.md exists (might be a dir-only entry like engram ISS-017)
    if not target_issue.exists():
        # Build a stub from any siblings
        siblings = sorted([p.name for p in target_dir.iterdir() if p.is_file()])
        title = iid.replace("ISS-", "Issue ")
        # Try to derive title from one of the existing entry names
        for e in entries:
            if e.is_dir() and e.name != iid:
                slug = e.name.replace(f"{iid}-", "").replace("-", " ")
                title = slug.title()
                break
        meta = {"title": title, "status": "open"}
        fm = build_frontmatter(iid, meta, fallback_title=title)
        body_lines = [f"# {iid}: {title}", "",
                      "_(Stub — see sibling files for design / requirements / artifacts.)_", ""]
        if siblings:
            body_lines.append("## Files in this directory")
            for s in siblings:
                body_lines.append(f"- `{s}`")
            body_lines.append("")
        target_issue.write_text(fm + "\n".join(body_lines) + "\n")
        print(f"  STUB {iid}/issue.md (dir-only entry)")

print("\nDone.")
