#!/usr/bin/env python3
"""
ISS-021 LoCoMo Dimension Coverage Smoke Test

Ingests first 3 sessions of conv-26 using engram-ai-rust HEAD (dimensional extractor),
then queries the target DB for sub-dimension coverage stats.

Compares to Step 9 baseline (engram-telegram data):
- participants: 6% (filter path) / 0% in target-100
- temporal: 6% in target-100
- causation: 13% in target-100
"""

import json
import subprocess
import sys
import time
from pathlib import Path

ENGRAM_BIN = "/Users/potato/clawd/projects/engram-ai-rust/target/release/engram"
DATASET = "/Users/potato/clawd/projects/cogmembench/datasets/locomo/data/locomo10.json"
OUT_DIR = Path("/Users/potato/clawd/projects/engram-ai-rust/.gid/issues/ISS-021-subdim-extraction-coverage/pilot")
DB_PATH = OUT_DIR / "locomo-conv26-smoke-v2.db"
LOG_PATH = OUT_DIR / "smoke-run-v2.log"
NAMESPACE = "locomo-conv26-smoke-v2"

OUT_DIR.mkdir(parents=True, exist_ok=True)

# OAuth token from keychain (same method as cogmembench adapter)
def get_oauth_token():
    r = subprocess.run(
        ["security", "find-generic-password", "-s", "Claude Code-credentials",
         "-a", "potato", "-w"],
        capture_output=True, text=True, timeout=10,
    )
    r.check_returncode()
    return json.loads(r.stdout.strip())["claudeAiOauth"]["accessToken"]

OAUTH_TOKEN = get_oauth_token()

# Load data
with open(DATASET) as f:
    data = json.load(f)
conv = data[0]
conv_id = conv["sample_id"]
speaker_a = conv["conversation"]["speaker_a"]
speaker_b = conv["conversation"]["speaker_b"]

print(f"[smoke] Conversation: {conv_id} ({speaker_a} & {speaker_b})")
print(f"[smoke] DB: {DB_PATH}")
print(f"[smoke] Namespace: {NAMESPACE}")

# Remove old DB if exists
if DB_PATH.exists():
    DB_PATH.unlink()
    print("[smoke] Cleared old DB")

# Ingest first 3 sessions
sessions_to_ingest = ["session_1", "session_2", "session_3"]
total_turns = 0
log_f = open(LOG_PATH, "w")

start = time.time()
for sk in sessions_to_ingest:
    turns = conv["conversation"].get(sk, [])
    if not isinstance(turns, list):
        continue
    print(f"\n[smoke] {sk}: {len(turns)} turns")
    for i, t in enumerate(turns):
        text = t["text"]
        speaker = t["speaker"]
        dia_id = t["dia_id"]
        # Store as dialogue turn: "<speaker>: <text>"
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
        r = subprocess.run(cmd, capture_output=True, text=True, timeout=60, env=env)
        log_f.write(f"--- {dia_id} ---\nstdout: {r.stdout}\nstderr: {r.stderr}\n\n")
        log_f.flush()
        if r.returncode != 0:
            print(f"  [!] Turn {dia_id} FAILED: {r.stderr[:200]}")
            sys.exit(1)
        total_turns += 1
        print(f"\r  [{i+1}/{len(turns)}] stored ({total_turns} total)", end="", flush=True)
    print()

elapsed = time.time() - start
log_f.close()
print(f"\n[smoke] Total: {total_turns} turns in {elapsed:.1f}s")
print(f"[smoke] DB at {DB_PATH}")
print(f"[smoke] Full log at {LOG_PATH}")
