#!/usr/bin/env python3
"""Second pass: any ISS-NNN/issue.md without frontmatter gets parsed + frontmattered."""
import re, sys
from pathlib import Path

# Reuse parsing from migrate-project
sys.path.insert(0, "/tmp")
import importlib.util
spec = importlib.util.spec_from_file_location("mig", "/tmp/migrate-project.py")
mig = importlib.util.module_from_spec(spec)
# We need PROJECT to be set as a CLI arg before the module loads. Easier: just inline.

if len(sys.argv) < 2:
    print("Usage: pass2.py <project_root>")
    sys.exit(1)

PROJECT = Path(sys.argv[1])
ISSUES_DIR = PROJECT / ".gid" / "issues"

def parse_header(text):
    meta = {}
    lines = text.splitlines()
    title = None
    for line in lines[:5]:
        m = re.match(r'^#\s*ISS-\d+\s*[:\—\-]\s*(.+)$', line)
        if m: title = m.group(1).strip(); break
        m = re.match(r'^#\s*ISS-\d+\s+(.+)$', line)
        if m: title = m.group(1).strip(); break
    if title: meta["title"] = title
    for line in lines[1:80]:
        if line.startswith("##") or line.strip() == "---":
            break
        # **Key:** value form
        m = re.match(r'^\*\*([A-Za-z][A-Za-z _/]*)\:\*\*\s*(.+)$', line)
        if m:
            meta[m.group(1).strip().lower().replace(" ", "_")] = m.group(2).strip()
            continue
        # ## Status: Open form (engram ISS-017 style)
    # Also check ## Status: lines outside the header block
    for line in lines[:30]:
        m = re.match(r'^##\s*([A-Za-z]+)\s*:\s*(.+)$', line)
        if m:
            k = m.group(1).strip().lower()
            if k in ("status", "priority", "component", "severity", "filed", "closed"):
                meta.setdefault(k, m.group(2).strip())
    return meta

def normalize_status(raw):
    if not raw: return "open"
    r = raw.lower()
    r = re.sub(r'\s*\(.*\)\s*$', '', r).strip()
    if r in ("closed", "done", "resolved", "fixed"): return "closed"
    if "superseded" in r: return "superseded"
    if "wontfix" in r or "won't fix" in r: return "wontfix"
    if "blocked" in r: return "blocked"
    if "in_progress" in r or "in progress" in r or "wip" in r: return "in_progress"
    return "open"

def extract_date(raw):
    if not raw: return None
    m = re.search(r'(\d{4}-\d{2}-\d{2})', raw)
    return m.group(1) if m else None

def extract_priority(raw):
    if not raw: return None
    m = re.search(r'(P[0-3])', raw)
    return m.group(1) if m else None

def extract_severity(raw):
    if not raw: return None
    r = raw.lower()
    for s in ("critical", "high", "medium", "low"):
        if s in r: return s
    return None

def extract_related(raw):
    if not raw: return []
    return list(dict.fromkeys(re.findall(r'(?:[a-zA-Z\-]+:)?ISS-\d+', raw)))

def fmt_str(s):
    return '"' + s.replace('"', '\\"') + '"'

def build_fm(iid, meta):
    title = meta.get("title") or iid
    status = normalize_status(meta.get("status"))
    priority = extract_priority(meta.get("priority", "")) or "P2"
    filed = extract_date(meta.get("filed") or meta.get("discovered") or meta.get("created") or meta.get("date"))
    closed = extract_date(meta.get("closed") or (meta.get("status") if status == "closed" else None))
    severity = extract_severity(meta.get("severity"))
    component = meta.get("component")
    related = extract_related(" ".join(filter(None, [
        meta.get("related"), meta.get("cross-repo"), meta.get("cross_repo")])))
    related = [r for r in related if r != iid]
    lines = ["---", f"id: {fmt_str(iid)}", f"title: {fmt_str(title)}",
             f"status: {status}", f"priority: {priority}",
             f"created: {filed or '2026-04-26'}"]
    if closed and status in ("closed", "superseded"):
        lines.append(f"closed: {closed}")
    if severity: lines.append(f"severity: {severity}")
    if component:
        c = re.sub(r'`', '', component)[:200]
        lines.append(f"component: {fmt_str(c)}")
    if related: lines.append("related: [" + ", ".join(fmt_str(r) for r in related) + "]")
    lines.append("---")
    lines.append("")
    return "\n".join(lines)

count = 0
for d in sorted(ISSUES_DIR.iterdir()):
    if not d.is_dir() or not d.name.startswith("ISS-"):
        continue
    f = d / "issue.md"
    if not f.exists():
        continue
    text = f.read_text()
    if text.lstrip().startswith("---"):
        continue
    meta = parse_header(text)
    fm = build_fm(d.name, meta)
    f.write_text(fm + text)
    print(f"FRONTMATTER {d.name} (status={normalize_status(meta.get('status'))})")
    count += 1
print(f"\n{count} files frontmattered.")
