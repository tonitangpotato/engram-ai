#!/bin/bash
# RUN-0012 launcher — detach properly so RustClaw exec doesn't block
cd /Users/potato/clawd/projects/engram/.gid/eval-runs/RUN-0012-iss091
export PATH="$HOME/.cargo/bin:$PATH"
exec python3 01_ingest.py > ingest.stdout.log 2>&1 < /dev/null
