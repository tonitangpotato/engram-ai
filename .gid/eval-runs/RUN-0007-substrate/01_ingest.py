#!/usr/bin/env python3
"""
RUN-0007: LoCoMo conv-26 sessions 1-3 ingest, post ISS-072 A-clean fix.

ISS-072 plumbed extractor kind/summary/attributes/importance through
the resolution pipeline into graph_entities, replacing the previous
default-only path that yielded ~99% kind=other("unknown").

Acceptance (GOAL-2): kind=other("unknown") ratio MUST drop from
~99% (RUN-0006 baseline: 213/215 = 99.07%) to <= 30%.

Output: locomo-conv26-iss072.{db,graph.db}
Namespace: locomo-conv26-iss072
"""

import json
import subprocess
import sys
import time
from pathlib import Path

ENGRAM_BIN = "/Users/potato/clawd/projects/engram/target/release/engram"
DATASET = "/Users/potato/clawd/projects/cogmembench/datasets/locomo/data/locomo10.json"
OUT_DIR = Path("/Users/potato/clawd/projects/engram/.gid/eval-runs/RUN-0007-substrate")
DB_PATH = OUT_DIR / "locomo-conv26-iss072.db"
GRAPH_DB_PATH = OUT_DIR / "locomo-conv26-iss072.graph.db"
LOG_PATH = OUT_DIR / "ingest.log"
NAMESPACE = "locomo-conv26-iss072"

OUT_DIR.mkdir(parents=True, exist_ok=True)


def get_oauth_token():
    r = subprocess.run(
        ["security", "find-generic-password", "-s", "Claude Code-credentials",
         "-a", "potato", "-w"],
        capture_output=True, text=True, timeout=10,
    )
    r.check_returncode()
    return json.loads(r.stdout.strip())["claudeAiOauth"]["accessToken"]


OAUTH_TOKEN = get_oauth_token()

with open(DATASET) as f:
    data = json.load(f)
conv = data[0]
conv_id = conv["sample_id"]
speaker_a = conv["conversation"]["speaker_a"]
speaker_b = conv["conversation"]["speaker_b"]

print(f"[ingest] Conversation: {conv_id} ({speaker_a} & {speaker_b})")
print(f"[ingest] DB: {DB_PATH}")
print(f"[ingest] Graph DB: {GRAPH_DB_PATH}")
print(f"[ingest] Namespace: {NAMESPACE}")
print(f"[ingest] Binary: {ENGRAM_BIN}")

# Clear old DB if present
for db in (DB_PATH, GRAPH_DB_PATH):
    if db.exists():
        db.unlink()
    for suffix in ("-shm", "-wal"):
        p = db.parent / (db.name + suffix)
        if p.exists():
            p.unlink()
print("[ingest] Cleared old DB(s)")

sessions_to_ingest = ["session_1", "session_2", "session_3"]
total_turns = 0
failures = []
log_f = open(LOG_PATH, "w")

start = time.time()
for sk in sessions_to_ingest:
    turns = conv["conversation"].get(sk, [])
    if not isinstance(turns, list):
        continue
    print(f"\n[ingest] {sk}: {len(turns)} turns")
    for i, t in enumerate(turns):
        text = t["text"]
        speaker = t["speaker"]
        dia_id = t["dia_id"]
        content = f"{speaker}: {text}"
        cmd = [
            ENGRAM_BIN,
            "--database", str(DB_PATH),
            "store", content,
            "-n", NAMESPACE,
            "-t", "episodic",
            "-i", "0.6",
            "-s", f"locomo/{conv_id}/{dia_id}",
            "--graph-db", str(GRAPH_DB_PATH),
            "--graph-drain-timeout-secs", "120",
            "--extractor", "anthropic",
            "--oauth",
            "--auth-token", OAUTH_TOKEN,
        ]
        env = {"PATH": "/usr/bin:/bin:/usr/local/bin"}
        r = subprocess.run(cmd, capture_output=True, text=True, timeout=120, env=env)
        log_f.write(f"--- {dia_id} ({sk}) ---\nstdout: {r.stdout}\nstderr: {r.stderr}\n\n")
        log_f.flush()
        if r.returncode != 0:
            failures.append((dia_id, r.stderr[:200]))
            print(f"\n  [!] {dia_id} FAILED rc={r.returncode}: {r.stderr[:200]}")
        total_turns += 1
        print(f"\r  [{i+1}/{len(turns)}] processed ({total_turns} attempted)", end="", flush=True)
    print()

elapsed = time.time() - start
log_f.close()
print(f"\n[ingest] Done: {total_turns} attempted in {elapsed:.1f}s ({elapsed/total_turns:.2f}s/turn)")
print(f"[ingest] Failures: {len(failures)}")
for fid, err in failures:
    print(f"  - {fid}: {err}")
print(f"[ingest] Log: {LOG_PATH}")
