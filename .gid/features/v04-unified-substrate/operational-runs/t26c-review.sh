#!/usr/bin/env bash
# T26c quality review — runs only SQL analytics, zero LLM cost.
# Designed to run against the T26c clone after the backfill finishes.
#
# Outputs a markdown stats block that the reviewer appends to
# T26c-full-2026-05-14.md alongside the human-judgement section.
#
# Usage:
#   ./t26c-review.sh /path/to/engram-memory-t26c.db [outfile.md]
#
# If outfile is omitted, prints to stdout.

set -euo pipefail

DB="${1:-/Users/potato/rustclaw/engram-memory-t26c.db}"
OUT="${2:-/dev/stdout}"

if [[ ! -f "$DB" ]]; then
  echo "fatal: DB not found at $DB" >&2
  exit 2
fi

sqlite() { sqlite3 -bail "$DB" "$@"; }

scalar() { sqlite "$1"; }

# Use a temp file so we can both print and capture
TMP="$(mktemp /tmp/t26c-review.XXXX.md)"
trap "rm -f $TMP" EXIT

{
  echo "# T26c — automated quality stats"
  echo
  echo "_Generated: $(date -Iseconds)_"
  echo "_Source DB: \`$DB\`_"
  echo

  ## ── volume ──
  echo "## Volume"
  echo
  MEMORIES_TOTAL=$(scalar "SELECT COUNT(*) FROM memories WHERE deleted_at IS NULL;")
  TRIPLES_TOTAL=$(scalar "SELECT COUNT(*) FROM triples;")
  MEMORIES_WITH_TRIPLES=$(scalar "SELECT COUNT(DISTINCT memory_id) FROM triples;")
  MEMORIES_ZERO_TRIPLES=$((MEMORIES_TOTAL - MEMORIES_WITH_TRIPLES))
  AVG_TRIPLES=$(scalar "SELECT ROUND(CAST((SELECT COUNT(*) FROM triples) AS REAL) / NULLIF((SELECT COUNT(DISTINCT memory_id) FROM triples),0), 2);")
  echo "- Memories (non-deleted):   **$MEMORIES_TOTAL**"
  echo "- Triples written:          **$TRIPLES_TOTAL**"
  echo "- Memories with ≥1 triple:  **$MEMORIES_WITH_TRIPLES**  ($MEMORIES_ZERO_TRIPLES with zero)"
  echo "- Avg triples per memory:   **$AVG_TRIPLES**"
  echo

  ## ── backfill run audit ──
  echo "## Backfill audit row"
  echo
  STATUS=$(scalar "SELECT status FROM triple_backfill_checkpoint ORDER BY started_at DESC LIMIT 1;")
  if [[ "$STATUS" == "in_progress" ]]; then
    echo "_⚠️ Backfill still in progress — audit row not yet finalized. Re-run this script after completion._"
    echo
  else
    echo "\`\`\`"
    sqlite "SELECT 'run_id            = ' || run_id || CHAR(10) ||
                   'rows_read         = ' || rows_read || CHAR(10) ||
                   'rows_inserted     = ' || rows_inserted || CHAR(10) ||
                   'rows_skipped      = ' || rows_skipped_existing || CHAR(10) ||
                   'rows_failed       = ' || rows_failed || CHAR(10) ||
                   'elapsed           = ' || ROUND((finished_at - started_at), 1) || 's'
            FROM backfill_runs WHERE legacy_table='triples'
            ORDER BY started_at DESC LIMIT 1;"
    echo "\`\`\`"
    echo
  fi

  ## ── checkpoint final state ──
  echo "## Checkpoint final state"
  echo
  echo "\`\`\`"
  sqlite "SELECT status, memories_processed, triples_inserted, memories_failed,
                 last_memory_id
          FROM triple_backfill_checkpoint
          ORDER BY started_at DESC LIMIT 1;" \
    | tr '|' '\t'
  echo "\`\`\`"
  echo

  ## ── confidence distribution ──
  echo "## Confidence distribution"
  echo
  echo "| bucket | count | pct |"
  echo "|--------|-------|-----|"
  sqlite "WITH t AS (SELECT COUNT(*) AS tot FROM triples)
          SELECT
            CASE
              WHEN confidence < 0.5 THEN '<0.50'
              WHEN confidence < 0.7 THEN '0.50-0.69'
              WHEN confidence < 0.8 THEN '0.70-0.79'
              WHEN confidence < 0.9 THEN '0.80-0.89'
              ELSE '>=0.90'
            END AS bucket,
            COUNT(*) AS n,
            ROUND(100.0*COUNT(*)/(SELECT tot FROM t), 1) AS pct
          FROM triples GROUP BY bucket ORDER BY MIN(confidence);" \
    | awk -F'|' '{printf "| %s | %s | %s%% |\n", $1, $2, $3}'
  echo
  MEAN_CONF=$(scalar "SELECT ROUND(AVG(confidence), 3) FROM triples;")
  STDEV_CONF=$(scalar "SELECT ROUND(SQRT(AVG((confidence - (SELECT AVG(confidence) FROM triples))*(confidence - (SELECT AVG(confidence) FROM triples)))), 3) FROM triples;")
  echo "- Mean confidence: **$MEAN_CONF**  (σ = $STDEV_CONF)"
  echo

  ## ── predicate distribution ──
  echo "## Predicate distribution"
  echo
  echo "| predicate | count | pct |"
  echo "|-----------|-------|-----|"
  sqlite "WITH t AS (SELECT COUNT(*) AS tot FROM triples)
          SELECT predicate, COUNT(*) AS n,
                 ROUND(100.0*COUNT(*)/(SELECT tot FROM t), 1) AS pct
          FROM triples GROUP BY predicate ORDER BY n DESC;" \
    | awk -F'|' '{printf "| %s | %s | %s%% |\n", $1, $2, $3}'
  echo

  ## ── span length distribution ──
  echo "## Span length (subject + object)"
  echo
  echo "| length bucket | count | pct |"
  echo "|---------------|-------|-----|"
  sqlite "WITH t AS (SELECT COUNT(*) AS tot FROM triples)
          SELECT
            CASE
              WHEN MAX(LENGTH(subject), LENGTH(object)) <= 20 THEN 'a) <=20'
              WHEN MAX(LENGTH(subject), LENGTH(object)) <= 40 THEN 'b) 21-40'
              WHEN MAX(LENGTH(subject), LENGTH(object)) <= 60 THEN 'c) 41-60'
              ELSE 'd) >60 (sentence fragment)'
            END AS bucket,
            COUNT(*) AS n,
            ROUND(100.0*COUNT(*)/(SELECT tot FROM t), 1) AS pct
          FROM triples GROUP BY bucket ORDER BY bucket;" \
    | awk -F'|' '{printf "| %s | %s | %s%% |\n", $1, $2, $3}'
  echo

  ## ── known failure modes ──
  echo "## Known failure modes"
  echo
  SELF_LOOPS=$(scalar "SELECT COUNT(*) FROM triples WHERE LOWER(TRIM(subject))=LOWER(TRIM(object));")
  SHORT_SPANS=$(scalar "SELECT COUNT(*) FROM triples WHERE LENGTH(subject)<3 OR LENGTH(object)<3;")
  SENTENCE_SPANS=$(scalar "SELECT COUNT(*) FROM triples WHERE LENGTH(subject)>60 OR LENGTH(object)>60;")
  TAUTOLOGICAL_IS_A=$(scalar "SELECT COUNT(*) FROM triples WHERE predicate='is_a' AND object IN ('task node','task','commit','component','memory','feature','thing','item','entity');")
  CJK_TRIPLES=$(scalar "SELECT COUNT(*) FROM triples WHERE subject GLOB '*[一-龥]*' OR object GLOB '*[一-龥]*';")
  echo "- Self-referential (subject == object):       **$SELF_LOOPS**"
  echo "- Ultra-short spans (<3 chars):               **$SHORT_SPANS**"
  echo "- Sentence-fragment spans (>60 chars):        **$SENTENCE_SPANS**"
  echo "- Tautological \`is_a → {task node, commit, …}\`: **$TAUTOLOGICAL_IS_A**"
  echo "- CJK-containing triples:                     **$CJK_TRIPLES**"
  echo

  ## ── failed memories detail ──
  echo "## Failed memories"
  echo
  FAILED=$(scalar "SELECT memories_failed FROM triple_backfill_checkpoint ORDER BY started_at DESC LIMIT 1;")
  if [[ "$FAILED" == "0" ]]; then
    echo "_None._"
  else
    echo "**$FAILED memories failed extraction** (exhausted max_retries). To inspect:"
    echo "\`\`\`sql"
    echo "-- These memories are *not* in the triples table — they were"
    echo "-- attempted but errored. Identify by checking which memory_ids"
    echo "-- the driver iterated over (per the cursor) but didn't produce"
    echo "-- a triple row. Run separately."
    echo "\`\`\`"
  fi
  echo

  ## ── sample spot-check (deterministic by id ordering) ──
  echo "## Spot-check sample (40 random triples across the corpus)"
  echo
  echo "| memory_id | subject | predicate | object | conf |"
  echo "|-----------|---------|-----------|--------|------|"
  sqlite3 -separator $'\x01' "$DB" "SELECT SUBSTR(memory_id,1,8), subject, predicate, object, ROUND(confidence,2)
          FROM triples
          WHERE id IN (SELECT id FROM triples ORDER BY (id * 2654435761) & 4294967295 LIMIT 40)
          ORDER BY memory_id, subject;" \
    | awk -F$'\x01' '{
        # Escape pipes inside cell values so the markdown table stays well-formed
        gsub(/\|/, "\\|", $2); gsub(/\|/, "\\|", $4);
        # Truncate long subjects/objects for readability
        if (length($2) > 50) $2 = substr($2,1,47) "…";
        if (length($4) > 50) $4 = substr($4,1,47) "…";
        printf "| `%s` | %s | `%s` | %s | %s |\n", $1, $2, $3, $4, $5
      }'
  echo

  ## ── per-memory triple-count histogram ──
  echo "## Triples per memory (histogram)"
  echo
  echo "| triples/memory | memories |"
  echo "|----------------|----------|"
  sqlite "SELECT n_triples, COUNT(*) AS n_memories FROM (
            SELECT memory_id, COUNT(*) AS n_triples FROM triples GROUP BY memory_id
          ) GROUP BY n_triples ORDER BY n_triples;" \
    | awk -F'|' '{printf "| %s | %s |\n", $1, $2}'
  echo
} > "$TMP"

if [[ "$OUT" == "/dev/stdout" ]]; then
  cat "$TMP"
else
  cp "$TMP" "$OUT"
  echo "wrote $OUT"
fi
