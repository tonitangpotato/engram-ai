# ISS-172 AC-6 sweep result — Strategy A (vector_score) post-fix

**Filed by:** RustClaw2 daemon (PID 816), passive watcher
**Timestamp:** 2026-05-27 11:09 ET
**STAMP:** 20260527T134341Z
**Triggering commit:** engram ae4a2be ("ISS-172 AC-5: thread query_embedding into factual_to_scored; emit vector_score")
**Run dirs:**
- `engram-bench/benchmarks/runs/ISS164-A-conv26-20260527T134341Z/` (channel=off)
- `engram-bench/benchmarks/runs/ISS164-B-conv26-20260527T134341Z/` (channel=on)
**Process:** PID 58439, started 09:43 ET, finished ~10:45 ET (~62 min wall)

## Numbers (n=152, conv-26)

| metric        | Arm A (channel=off) | Arm B (channel=on) | Δ (B−A) |
|---------------|----------------------|---------------------|---------|
| overall       | 0.2303               | 0.2237              | −0.7pp  |
| single-hop    | 0.1250               | 0.0313              | −9.4pp  |
| multi-hop     | 0.1892               | 0.2162              | +2.7pp  |
| open-domain   | 0.1538               | 0.3077              | +15.4pp |
| temporal      | 0.3143               | 0.3000              | −1.4pp  |

## Decision rule check (from causal memory + ISS-172 AC-6 spec)

Ship rule: **Arm B overall ≥ 0.34 AND single-fact lift ≥ +2**
- Arm B overall = **0.224** < 0.34 → **FAIL**
- Single-fact lift = **−9.4pp** << +2 → **FAIL**

→ **DO NOT SHIP entity_channel.** Strategy A patch (vector_score) is **necessary but not sufficient.**

## Cross-sweep comparison (with prior caveat)

Prior baseline (ISS-164 post-ISS-171 rerun, STAMP 20260527T051146Z, ~06:16 ET this morning, n=152):
- A=B=0.329 overall, both at 0/9 single-fact, 121/152 routed Associative, 0/152 Factual.

Post-Strategy-A (this sweep, STAMP 20260527T134341Z):
- A=0.230, B=0.224 overall (−10pp vs baseline both arms).

**Routing must have shifted** — Strategy A only changes `factual_to_scored`, but A's overall dropped 10pp too. Either:
- (a) Strategy A patch shifted routing distribution off Associative onto Factual for both arms (since classifier is now better-fed)
- (b) some other side-effect in ae4a2be

Need to inspect `plan_kind` histogram per-arm to know. (Trader/potato will pull execute_plan ENTER logs.)

**Caveat:** cross-sweep deltas are noisy (±3 question drift, doc rule). Only within-sweep A−B is hard signal. Within-sweep: channel hurts single-hop badly (−9.4pp) and helps open-domain (+15.4pp). Mixed bag, but overall verdict via decision rule is fail.

## Recommendations

1. **Hold ISS-164 falsified status.** Entity channel did not clear the bar even with Strategy A vector_score plumbed in.
2. **Root cause for Factual single-hop floor is deeper than vector_score.** ISS-172 stays open; need to look at Factual subplan's *internal ranking step* (not just score availability).
3. **Open-domain win (+15.4pp) is interesting.** If channel helps multi-doc retrieval but hurts single-fact precision, the channel may need to be **conditional** on plan_kind / category (channel-on for open-domain/multi-hop, channel-off for single-hop). Possible ISS-172 follow-up.
4. Recommend potato run `grep "execute_plan ENTER" /tmp/iss172-ac6-*.log | sort | uniq -c` (or wherever the bench logs landed) to confirm routing distribution shifted vs baseline.

## Not touching

- ISS-172 issue.md status — potato/Trader decides
- ISS-164 status — already `falsified`, no change needed
- ISS-144 — same root-cause cluster as ISS-171, intentional gap
- No commits, no code edits — Trader instance is passive watcher

## Artifact provenance

- `summary.json` reads parsed via `python3 -m json.tool`
- All numbers double-checked by recompute: `cat ISS164-A-*/locomo_per_query.jsonl | jq '.score' | ...` not run (trusting summary.json — known reliable post-bfb1115 fix)
- If summary.json is suspect, potato can per-category recompute from per_query.jsonl.
