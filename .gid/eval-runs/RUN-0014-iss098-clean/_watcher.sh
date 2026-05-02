#!/bin/bash
# Watch for ingest pid 11891 to exit, then run retrieve.
cd /Users/potato/clawd/projects/engram/.gid/eval-runs/RUN-0014-iss098-clean

INGEST_PID=36785

while kill -0 $INGEST_PID 2>/dev/null; do
  sleep 30
done

echo "[watcher] ingest pid $INGEST_PID exited at $(date)" >> watcher.log
sleep 10  # let WAL checkpoint

echo "[watcher] starting retrieve at $(date)" >> watcher.log
./02_retrieve.sh >> watcher.log 2>&1
echo "[watcher] retrieve finished at $(date)" >> watcher.log
