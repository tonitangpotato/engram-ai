#!/usr/bin/env python3
"""
RUN-0010: LoCoMo conv-26 FULL ingest with --occurred-at (ISS-087 anchoring).

Delta vs RUN-0009: each turn now passes --occurred-at derived from the
session's date_time field ("1:56 pm on 8 May, 2023" → RFC3339), so temporal
recall sees the original 2023 dates instead of wall-clock now.

Output: locomo-conv26-full.{db,graph.db}
Namespace: locomo-conv26-full
"""

import datetime
import json
import subprocess
import sys
import time
from pathlib import Path

ENGRAM_BIN = "/Users/potato/clawd/projects/engram/target/release/engram"
DATASET = "/Users/potato/clawd/projects/cogmembench/datasets/locomo/data/locomo10.json"
OUT_DIR = Path("/Users/potato/clawd/projects/engram/.gid/eval-runs/RUN-0010-occurred-at")
DB_PATH = OUT_DIR / "locomo-conv26-full.db"
GRAPH_DB_PATH = OUT_DIR / "locomo-conv26-full.graph.db"
LOG_PATH = OUT_DIR / "ingest.log"
NAMESPACE = "locomo-conv26-full"

OUT_DIR.mkdir(parents=True, exist_ok=True)


def parse_session_dt(s: str) -> str:
    """"1:56 pm on 8 May, 2023" → "2023-05-08T13:56:00Z" (RFC3339, UTC).

    LoCoMo session date_times are wall-clock with no tz; we treat them as UTC
    for ingestion (relative anchoring is what matters for recall, not absolute tz).
    """
    return datetime.datetime.strptime(s.strip(), "%I:%M %p on %d %B, %Y").strftime(
        "%Y-%m-%dT%H:%M:%SZ"
    )


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

# All sessions in chronological order
sessions_to_ingest = sorted(
    [k for k in conv["conversation"].keys()
     if k.startswith("session_") and not k.endswith("_date_time")],
    key=lambda s: int(s.split("_")[1]),
)
print(f"[ingest] Sessions: {len(sessions_to_ingest)} ({sessions_to_ingest[0]}..{sessions_to_ingest[-1]})")

total_turns = 0
failures = []
log_f = open(LOG_PATH, "w")

start = time.time()
for sk in sessions_to_ingest:
    turns = conv["conversation"].get(sk, [])
    if not isinstance(turns, list):
        continue
    session_dt_raw = conv["conversation"].get(f"{sk}_date_time")
    occurred_at = parse_session_dt(session_dt_raw) if session_dt_raw else None
    print(f"\n[ingest] {sk}: {len(turns)} turns  occurred_at={occurred_at}")
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
        if occurred_at:
            cmd += ["--occurred-at", occurred_at]
        env = {"PATH": "/usr/bin:/bin:/usr/local/bin"}
        r = subprocess.run(cmd, capture_output=True, text=True, timeout=180, env=env)
        log_f.write(f"--- {dia_id} ({sk}) ---\nstdout: {r.stdout}\nstderr: {r.stderr}\n\n")
        log_f.flush()
        if r.returncode != 0:
            full_cmd_repr = repr(cmd)
            failures.append((dia_id, r.stderr[:300], full_cmd_repr))
            print(f"\n  [!] {dia_id} FAILED rc={r.returncode}: {r.stderr[:200]}")
            log_f.write(f"FAILURE_FULL_CMD: {full_cmd_repr}\n\n")
            log_f.flush()
        total_turns += 1
        print(f"\r  [{i+1}/{len(turns)}] processed ({total_turns} attempted)", end="", flush=True)
    print()

elapsed = time.time() - start
log_f.close()
print(f"\n[ingest] Done: {total_turns} attempted in {elapsed:.1f}s ({elapsed/total_turns:.2f}s/turn)")
print(f"[ingest] Failures: {len(failures)}")
for fid, err, _cmd in failures:
    print(f"  - {fid}: {err}")
print(f"[ingest] Log: {LOG_PATH}")
