#!/usr/bin/env python3
"""
ISS-021 Step 0 — Haiku vs Sonnet Extractor Comparison

Parameterized smoke test: same conv-26 sessions 1-3, different extraction model.
Used to diagnose whether sub-field coverage (participants/temporal/causation)
is bottlenecked by prompt or by model capability.

Usage:
    python3 smoke_ingest_model_compare.py --model claude-haiku-4-5-20251001
    python3 smoke_ingest_model_compare.py --model claude-sonnet-4-5-20250929

Output: locomo-conv26-smoke-{tag}.db + smoke-run-{tag}.log
"""

import argparse
import json
import subprocess
import sys
import time
from pathlib import Path

ENGRAM_BIN = "/Users/potato/clawd/projects/engram/target/release/engram"
DATASET = "/Users/potato/clawd/projects/cogmembench/datasets/locomo/data/locomo10.json"
OUT_DIR = Path("/Users/potato/clawd/projects/engram/.gid/issues/ISS-021-subdim-extraction-coverage/pilot")
OUT_DIR.mkdir(parents=True, exist_ok=True)

parser = argparse.ArgumentParser()
parser.add_argument("--model", required=True,
                    help="Extractor model, e.g. claude-haiku-4-5-20251001 or claude-sonnet-4-5-20250929")
parser.add_argument("--tag", default=None,
                    help="Short tag used in DB / log / namespace names (default: derived from model)")
parser.add_argument("--sessions", default="session_1,session_2,session_3",
                    help="Comma-separated session keys to ingest")
args = parser.parse_args()

# Derive tag from model if not provided
if args.tag is None:
    if "haiku" in args.model:
        args.tag = "haiku"
    elif "sonnet" in args.model:
        args.tag = "sonnet"
    elif "opus" in args.model:
        args.tag = "opus"
    else:
        args.tag = args.model.split("-")[1] if "-" in args.model else args.model

DB_PATH = OUT_DIR / f"locomo-conv26-smoke-{args.tag}.db"
LOG_PATH = OUT_DIR / f"smoke-run-{args.tag}.log"
NAMESPACE = f"locomo-conv26-smoke-{args.tag}"
sessions_to_ingest = args.sessions.split(",")


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

print(f"[smoke-{args.tag}] Model: {args.model}")
print(f"[smoke-{args.tag}] Conversation: {conv_id} ({speaker_a} & {speaker_b})")
print(f"[smoke-{args.tag}] DB: {DB_PATH}")
print(f"[smoke-{args.tag}] Namespace: {NAMESPACE}")
print(f"[smoke-{args.tag}] Sessions: {sessions_to_ingest}")

if DB_PATH.exists():
    DB_PATH.unlink()
    for suffix in ("-shm", "-wal"):
        p = DB_PATH.parent / (DB_PATH.name + suffix)
        if p.exists():
            p.unlink()
    print(f"[smoke-{args.tag}] Cleared old DB")

total_turns = 0
log_f = open(LOG_PATH, "w")

start = time.time()
for sk in sessions_to_ingest:
    turns = conv["conversation"].get(sk, [])
    if not isinstance(turns, list):
        continue
    print(f"\n[smoke-{args.tag}] {sk}: {len(turns)} turns")
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
            "--extractor-model", args.model,
            "--oauth",
            "--auth-token", OAUTH_TOKEN,
        ]
        env = {"PATH": "/usr/bin:/bin:/usr/local/bin"}
        r = subprocess.run(cmd, capture_output=True, text=True, timeout=90, env=env)
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
print(f"\n[smoke-{args.tag}] Total: {total_turns} turns in {elapsed:.1f}s")
print(f"[smoke-{args.tag}] DB at {DB_PATH}")
print(f"[smoke-{args.tag}] Full log at {LOG_PATH}")
