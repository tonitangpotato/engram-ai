# ISS-172 AC-6 addendum — routing histogram confirmed

**Filed by:** rustclaw2 (current session, parent PID 816 — heartbeat at 11:35 EDT)
**Status:** addendum to `heartbeat-1109-ac6-result.md` (already filed by parallel instance ~26 min ago)
**Purpose:** answer the "need to inspect plan_kind histogram per-arm" recommendation in the 1109 artifact, no other re-analysis.

## ⚠️ Filing-collision flag

The 1109 artifact in this directory claims author "RustClaw2 daemon (PID 816), passive watcher" — that's my identity. But this current session did NOT file it; I had no record of it in working memory when I checked the runs at 11:35. Either:

1. A parallel session of rustclaw2 (same persona/namespace/PID) ran during the same window and filed it without my session knowing, or
2. The author tag is misattributed.

Numbers in the 1109 artifact match what I independently recomputed at 11:35 EDT — so the analysis itself is sound. Flagging the provenance concern for potato. No data harm; one artifact, two confirms.

## Routing histogram (per-query top-level execute_plan ENTER, from /tmp/iss164-bench/iss164-{A,B}.log)

```
                   A (channel=off)   B (channel=on)
factual            113               114
hybrid              30                29
associative          8                 7
abstract             1                 2
                  ---               ---
                  152               152
```

**Verdict on the 1109 question (a) vs (b):** ✅ (a) confirmed. Strategy A's vector_score wiring at api.rs:789 did shift routing significantly — both arms now route ~74% to Factual, identical to the 11:27Z sweep's 75%. The entity channel itself does NOT affect classifier routing (as expected — it only affects what Factual *does with* its candidates, not whether Factual fires).

This means the ISS-164 entity_channel A/B test is now correctly isolating the channel's effect, with routing held constant. The within-sweep delta is fully attributable to channel-on vs channel-off behavior inside the Factual plan.

## Implications

1. **The 1109 verdict "DO NOT SHIP entity_channel" is correct.** Routing is held constant; the channel actively hurts single-hop by -9.4pp at the same factual routing rate. q37's earlier 0→1 flip in the 11:27Z sweep was likely a side effect of the broken ranker, not the channel.

2. **Open-domain +15.4pp from the channel is real** — but on n=13 it's only +2 wins (2→4). Small sample, but worth investigating whether channel-conditional-on-category is a real path (the 1109 artifact's recommendation #3). For single-fact, channel-off is unambiguous.

3. **ISS-172 didn't recover the absolute floor.** A overall 0.230 vs pre-ISS-171 baseline 0.362 = still -13pp. vector_score wiring is necessary but, as the 1109 artifact noted, not sufficient. Something else in the Factual plan's path (graph_score tying, anchor expansion fan-out, fusion weight balance) still under-ranks gold vs the old Associative path.

## Not doing

- Not filing a new top-level verdict (1109 artifact already covers it)
- Not touching ISS-164/171/172 issue lifecycle
- Not running cargo / extra benches
- Just filed this addendum + flag for potato visibility

— rustclaw2 (heartbeat 11:35 EDT)
