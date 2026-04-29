#!/usr/bin/env python3
"""
ISS-068 fix verification smoke.

Re-runs LoCoMo conv-26 session_1 (18 turns) ingest with the fixed
binary. Expected outcome: distinct dia_ids in the `memories` table
should jump from 7 (pre-fix) to ~18 (every turn admitted, even
those for which the LLM extracts zero facts).

This is the minimum-cost verification — only session_1, only
~18 LLM extractor calls. Don't run sessions 2/3 unless the
session_1 result needs corroboration.
"""

import json
import sqlite3
import subprocess
import sys
import time
from pathlib import Path

ENGRAM_BIN = "/Users/potato/clawd/projects/engram/target/release/engram"
DATASET = "/Users/potato/clawd/projects/cogmembench/datasets/locomo/data/locomo10.json"
OUT_DIR = Path(
    "/Users/potato/clawd/projects/engram/.gid/issues/ISS-068/_smoke_2026-04-29"
)
DB_PATH = OUT_DIR / "verify.db"
GRAPH_DB_PATH = OUT_DIR / "verify.graph.db"
LOG_PATH = OUT_DIR / "verify.log"
NAMESPACE = "iss068-verify-conv26-s1"

OUT_DIR.mkdir(parents=True, exist_ok=True)


def get_oauth_token():
    r = subprocess.run(
        [
            "security",
            "find-generic-password",
            "-s",
            "Claude Code-credentials",
            "-a",
            "potato",
            "-w",
        ],
        capture_output=True,
        text=True,
        timeout=10,
    )
    r.check_returncode()
    return json.loads(r.stdout.strip())["claudeAiOauth"]["accessToken"]


OAUTH_TOKEN = get_oauth_token()

with open(DATASET) as f:
    data = json.load(f)
conv = data[0]
conv_id = conv["sample_id"]

# Clear stale state
for db in (DB_PATH, GRAPH_DB_PATH):
    if db.exists():
        db.unlink()
    for suffix in ("-shm", "-wal"):
        p = db.parent / (db.name + suffix)
        if p.exists():
            p.unlink()
print(f"[smoke] Cleared {DB_PATH.parent}")
print(f"[smoke] Binary: {ENGRAM_BIN}")
print(f"[smoke] Conv: {conv_id}, session_1 only")

turns = conv["conversation"].get("session_1", [])
print(f"[smoke] {len(turns)} turns")

start = time.time()
log_f = open(LOG_PATH, "w")
uuid_count = 0
skipped_count = 0
for i, t in enumerate(turns):
    text = t["text"]
    speaker = t["speaker"]
    dia_id = t["dia_id"]
    content = f"{speaker}: {text}"
    cmd = [
        ENGRAM_BIN,
        "--database",
        str(DB_PATH),
        "store",
        content,
        "-n",
        NAMESPACE,
        "-t",
        "episodic",
        "-i",
        "0.6",
        "-s",
        f"locomo/{conv_id}/{dia_id}",
        "--graph-db",
        str(GRAPH_DB_PATH),
        "--graph-drain-timeout-secs",
        "120",
        "--extractor",
        "anthropic",
        "--oauth",
        "--auth-token",
        OAUTH_TOKEN,
    ]
    env = {"PATH": "/usr/bin:/bin:/usr/local/bin"}
    r = subprocess.run(cmd, capture_output=True, text=True, timeout=90, env=env)
    log_f.write(f"--- {dia_id} ---\nstdout: {r.stdout}\nstderr: {r.stderr}\n\n")
    log_f.flush()
    if r.returncode != 0:
        print(f"  [!] {dia_id} FAILED: {r.stderr[:200]}")
        sys.exit(1)
    out = r.stdout.strip()
    if out.startswith("skipped:"):
        skipped_count += 1
        marker = "S"
    else:
        uuid_count += 1
        marker = "U"
    print(f"  [{i+1}/{len(turns)}] {dia_id} {marker}", end="  ", flush=True)

print()
elapsed = time.time() - start
log_f.close()
print(f"[smoke] CLI summary: {uuid_count} UUIDs, {skipped_count} skipped")
print(f"[smoke] Elapsed: {elapsed:.1f}s ({elapsed/len(turns):.2f}s/turn)")

# Now query the DB
print()
print("[verify] Querying memories table...")
conn = sqlite3.connect(DB_PATH)
rows = conn.execute(
    "SELECT DISTINCT source FROM memories WHERE source LIKE 'locomo/conv-26/D1:%' ORDER BY source"
).fetchall()
got = [r[0].split("/")[-1] for r in rows]
print(f"[verify] Distinct D1 sources persisted: {len(got)}")
print(f"[verify] dia_ids: {got}")

expected_min = 17  # legitimate to skip D1:1 greeting; everything else must land
critical = ["D1:12"]  # gold evidence turn
missing_critical = [c for c in critical if c not in got]
print()
if len(got) >= expected_min and not missing_critical:
    print(f"✅ PASS: {len(got)} ≥ {expected_min} D1 turns persisted, gold {critical} present")
else:
    if missing_critical:
        print(f"❌ FAIL: critical gold turns still missing: {missing_critical}")
    if len(got) < expected_min:
        print(f"❌ FAIL: only {len(got)} D1 turns persisted, expected ≥ {expected_min}")
    sys.exit(2)

# Confirm extraction failures got recorded
print()
print("[verify] Querying graph_extraction_failures...")
g = sqlite3.connect(GRAPH_DB_PATH)
no_fact_rows = g.execute(
    "SELECT COUNT(*) FROM graph_extraction_failures WHERE error_category='no_facts_extracted'"
).fetchone()[0]
print(f"[verify] no_facts_extracted failure rows: {no_fact_rows}")
if no_fact_rows == 0:
    print("⚠️  WARN: no extraction-failure rows recorded. If skipped_count from CLI was 0, this is fine.")
else:
    print(f"✅ Observability preserved: {no_fact_rows} extraction failures recorded")

print()
print("[smoke] DONE — fix verified.")
