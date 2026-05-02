#!/bin/bash
cd /Users/potato/clawd/projects/cogmembench
export PYTHONUNBUFFERED=1
python3 run_locomo.py --system engram --conversations conv-26 --no-resume \
    >> /Users/potato/clawd/projects/engram/.gid/eval-runs/RUN-0013-jscore/RUN-0013-jscore.log 2>&1
