#!/usr/bin/env python3
"""
LoCoMo conv-26 sessions 1-3 ingest, post commit d991715
(retrieval cognitive state readback + T9 PipelineRecordProcessor + bench drivers).

Pattern adapted from .gid/issues/ISS-021/pilot/smoke_ingest_v3.py — same
ingestion semantics so we can compare against prior baselines.

Output: locomo-conv26-s1-3-postd991715.db
"""

import json
import subprocess
import sys
import time
from pathlib import Path

ENGRAM_BIN = "/Users/potato/clawd/projects/engram/target/release/engram"
DATASET = "/Users/potato/clawd/projects/cogmembench/datasets/locomo/data/locomo10.json"
OUT_DIR = Path("/Users/potato/clawd/projects/engram/.gid/issues/_smoke-locomo-2026-04-27")
DB_PATH = OUT_DIR / "locomo-conv26-s1-3-postd991715.db"
LOG_PATH = OUT_DIR / "ingest.log"
NAMESPACE = "locomo-conv26-postd991715"

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
print(f"[ingest] Namespace: {NAMESPACE}")
print(f"[ingest] Binary: {ENGRAM_BIN}")

# Clear old DB if present (we want a clean run)
if DB_PATH.exists():
    DB_PATH.unlink()
    for suffix in ("-shm", "-wal"):
        p = DB_PATH.parent / (DB_PATH.name + suffix)
        if p.exists():
            p.unlink()
    print("[ingest] Cleared old DB")

sessions_to_ingest = ["session_1", "session_2", "session_3"]
total_turns = 0
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
            "--extractor", "anthropic",
            "--oauth",
            "--auth-token", OAUTH_TOKEN,
        ]
        env = {"PATH": "/usr/bin:/bin:/usr/local/bin"}
        r = subprocess.run(cmd, capture_output=True, text=True, timeout=90, env=env)
        log_f.write(f"--- {dia_id} ---\nstdout: {r.stdout}\nstderr: {r.stderr}\n\n")
        log_f.flush()
        if r.returncode != 0:
            print(f"  [!] Turn {dia_id} FAILED: {r.stderr[:300]}")
            sys.exit(1)
        total_turns += 1
        print(f"\r  [{i+1}/{len(turns)}] stored ({total_turns} total)", end="", flush=True)
    print()

elapsed = time.time() - start
log_f.close()
print(f"\n[ingest] Total: {total_turns} turns in {elapsed:.1f}s ({elapsed/total_turns:.2f}s/turn)")
print(f"[ingest] DB at {DB_PATH}")
print(f"[ingest] Full log at {LOG_PATH}")
