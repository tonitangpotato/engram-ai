#!/bin/bash
# RUN-0016: J-score validation for ISS-099 dia_id metadata fix.
#
# Compare to RUN-0013 baseline:
#   - evidence_recall = 1%, J-score = 8% (with broken metadata.user.dia_id)
#
# This run:
#   - .engram_dbs/conv-26.db moved to .before-RUN-0015 (forces re-ingest)
#   - Current engram binary (smoke-tested: --meta dia_id writes to metadata.user.dia_id)
#   - Current cogmembench adapter (Apr 30 code, traverses meta dict to --meta flags)
#   - --max-questions 25 to keep total wall time ~30 min
set -euo pipefail
cd /Users/potato/clawd/projects/cogmembench
export PYTHONUNBUFFERED=1
LOG=/Users/potato/clawd/projects/engram/.gid/eval-runs/RUN-0016-jscore-iss099-validate/RUN-0016.log
python3 run_locomo.py \
    --system engram \
    --conversations conv-26 \
    --no-resume \
    --max-questions 25 \
    >> "$LOG" 2>&1
