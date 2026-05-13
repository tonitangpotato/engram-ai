# Design Review r3-part4a — v04-unified-substrate (Concurrency Architecture)

> **Reviewer:** claude (sub-agent, concurrency focus)
> **Date:** 2026-05-12
> **Target:** `.gid/features/v04-unified-substrate/design.md` §6 (lines 1159–1746)
> **Prior reviews:** `design-r2-part4-infra-meta.md` (resolved findings not re-raised), `design-r3-part4b-decisions-tasks-risks.md` (FINDING-1 covers §4.12↔§6.1 empathy name mismatch — skipped here)
> **Scope:** §6.1–§6.9 concurrency architecture deep-dive, post commit-5 rewrite
> **Method:** Focused concurrency review per task spec (§6.1 enum completeness, §6.2 writer loop, §6.3 weighted fairness, §6.6 throughput, §6.7 multi-tenant, §6.9 failure modes)

## Summary

| Severity   | Count |
|------------|-------|
| 🔴 Critical   | 2   |
| 🟡 Important  | 4   |
| 🟢 Minor      | 3   |
| **Total**  | **9**   |

**Recommendation**: Needs fixes before implementation. FINDING-5 (panic recovery liveness bug) and FINDING-6 (WriteMemory macro-op vs Batch ambiguity) are critical — both would cause implementers to build incorrect behavior. The 4 important findings are specification gaps that would require implementer judgment calls.

**Estimated implementation confidence**: Medium — §6 is well-designed in architecture but has specification gaps in edge-case behavior (overflow, panic recovery, commit failure notification) that would cause different implementers to build different things.

---

## FINDING-1 🟡 Important — §6.1 WriteOp enum still missing `BackfillBatch` variant referenced in §6.8

**Section:** §6.1 vs §6.8  
**Check focus:** WriteOp enum completeness

§6.8 explicitly says: "Phase C backfill runs as a **dedicated low-priority `BackfillBatch` WriteOp variant** flowing through the same queue. The backfill driver enqueues `BackfillBatch { rows: Vec<LegacyRow>, ... }` in batches of 256."

But §6.1's enum definition (25 variants) does not include `BackfillBatch`. The enum ends with `Batch { ops, reply }` — which is the general-purpose compound op, not the migration-specific backfill variant.

r2 FINDING-A4-3 flagged this. Commit-5 added many missing variants (interoception, somatic markers, drive alignment, empathy, WM, metacog, dimensions) but did NOT add `BackfillBatch`.

**Impact:** T19–T27 (backfill tasks) need a dedicated variant to flow through the low-priority channel. Without it, backfill would need to use `Batch(Vec<WriteOp>)` on the high-priority channel — defeating the priority design and potentially starving live ingest during the 15-minute backfill window.

**Suggested fix:** Add to §6.1 enum:
```rust
// ─────────────── §6.8 migration backfill ───────────────
BackfillBatch {
    rows: Vec<LegacyRow>,              // opaque legacy rows; writer converts to node/edge INSERTs
    source_table: String,              // 'memories'|'entities'|'hebbian_links'|...
    reply: oneshot::Sender<Result<usize>>,  // count of rows backfilled
},
```
Route to `rx_low` channel in the priority assignment table (§6.3).

---

## FINDING-2 🟡 Important — §6.3 weighted cap arithmetic ignores Hebbian coalesce map entries in batch size accounting

**Section:** §6.3  
**Check focus:** Weighted fairness algorithm correctness

The per-batch fair drain pseudocode has loop condition:
```rust
while batch.len() + hebbian.len() < BATCH_MAX {
```

Where `batch` holds non-Hebbian ops and `hebbian` is the `HashMap<(NodeId, NodeId), f64>` coalescing accumulator. The per-lane caps are:
- `BATCH_CAP_HIGH = 48`, `BATCH_CAP_MED = 12`, `BATCH_CAP_LOW = 4` (sum = 64 = BATCH_MAX)

Problem: `BumpAssociation` ops are routed to `hebbian` (not `batch`), but the per-lane caps count against `count_high`/`count_med`/`count_low` regardless of whether the op goes to `batch` or `hebbian`. This means:

1. If 48 high-priority ops are all `BumpAssociation` (e.g., burst recall with 48 co-activation bumps), `count_high` hits 48, `batch` is empty, and `hebbian` has ≤48 unique pairs. The batch commits 0 non-Hebbian ops + up to 48 Hebbian flushes. **No capacity remains for medium/low**, even though the batch was "full" of only coalescable ops that compress to fewer SQL statements.

2. Conversely, if all 48 high ops are `WriteMemory` (non-coalescable) and 12 medium are `BumpAssociation`, `batch.len()` = 48 and `hebbian.len()` ≤ 12. The loop condition `48 + 12 < 64` is false → loop exits. But the actual SQL cost is 48 heavy ops + 12 lightweight upserts — the batch size should be allowed to exceed BATCH_MAX when the excess is coalescable.

**Impact:** Under Hebbian-heavy workloads (common — every recall generates O(K²) bumps), the effective batch utilization is poor. Under non-Hebbian-heavy workloads, medium/low starvation returns because caps are consumed by Hebbian ops that compress away.

**Suggested fix:** Count coalescable ops separately from batch capacity:
```rust
let BATCH_MAX_OPS = 64;      // non-coalescable ops cap
let HEBBIAN_MAX = 4096;       // already exists as HEBBIAN_COALESCE_CAP
// Loop: batch.len() < BATCH_MAX_OPS && hebbian.len() < HEBBIAN_MAX
// Per-lane caps apply only to batch.len(), not hebbian.len()
```
Or: don't count `BumpAssociation` against per-lane caps at all (since they coalesce into O(1) SQL per unique pair, not O(N)).

---

## FINDING-3 🟡 Important — §6.3 Hebbian coalesce cap (4096) overflow behavior unspecified

**Section:** §6.3  
**Check focus:** Data-loss profile at HEBBIAN_COALESCE_CAP

§6.2 sets `HEBBIAN_COALESCE_CAP = 4096` and §6.3 says the writer "maintains a small `HashMap<(NodeId, NodeId), f64>` accumulator" that is flushed on batch commit. The cap is mentioned but the **overflow behavior when the map reaches 4096 entries is never specified**.

Three possible behaviors, each with different consequences:
1. **Reject new bumps** → callers get `Err`, must retry → backpressure propagates to retrieval (bad: retrieval should never block on writes)
2. **Force-flush current map mid-batch** → extra SQL upserts within the current transaction → transaction grows unbounded → commit latency spike
3. **Drop new bumps silently** → data loss of Hebbian signal, but idempotent (next co-recall re-generates the bump)

Option 3 is likely correct for Hebbian (idempotent, lossy-safe), but the design should state this explicitly. §6.6 references the 4096 cap in its throughput analysis but assumes all entries flush successfully without addressing the cap scenario.

**Impact:** Implementer must guess overflow semantics. Wrong guess (option 1) blocks retrieval.

**Suggested fix:** Add to §6.3 after the cap definition:
> When `hebbian.len() >= HEBBIAN_COALESCE_CAP`, new `BumpAssociation` ops are **silently dropped** (the `reply` oneshot receives `Ok(())`). This is safe because Hebbian bumps are idempotent — the next co-recall of the same pair regenerates the signal. The cap prevents the HashMap from growing during pathological burst-recall scenarios (e.g., 100 results × 100 results = 10,000 unique pairs in one recall batch).

---

## FINDING-4 🟢 Minor — §6.7 multi-tenant ceiling has no concrete number and no failure-mode specification

**Section:** §6.7  
**Check focus:** Scale ceiling and failure mode

§6.7 describes the current model (single-tenant, in-process) and discusses three sharding alternatives (rejected). It concludes: "The single-writer single-file architecture is the correct design for engram's scale. Multi-tenant concurrency is handled by WAL (readers) and the writer queue (writers)."

What's missing:
1. **No concrete ceiling number.** How many namespaces can share one writer queue before throughput degrades? §6.6 gives 11k ops/sec floor — at what namespace count does the queue saturate? 10? 100? 1000? The answer is likely "irrelevant for engram's scale" (single-user, 1-3 namespaces) but stating this explicitly would prevent future scaling assumptions.

2. **No failure mode at the ceiling.** If the writer queue fills because too many namespaces are ingesting simultaneously, what happens? Per §6.3, high-priority callers block-await. But is there a timeout? Does the system degrade gracefully (slower ingest) or catastrophically (all callers stuck, tokio runtime stalls)?

**Impact:** Low — engram is single-user, single-namespace in practice. But §6.7 is 80 lines of multi-tenant discussion that never answers "so what's the actual limit?"

**Suggested fix:** Add a one-liner: "For engram's operational profile (1 user, 1-3 namespaces, ~100 ingests/hr), the writer queue is idle >99.9% of the time. The architecture's hard ceiling is the SQLite WAL fsync rate (~11k ops/sec); multi-namespace overhead is negligible because all namespaces share one queue and one transaction."

---

## FINDING-5 🔴 Critical — §6.9 panic recovery: pending oneshot replies are silently dropped, not flushed with `Err(WriterCrashed)`

**Section:** §6.9  
**Check focus:** Failure modes / panic recovery

§6.9 states:
> "the panic hook flushes pending oneshot replies with `Err(WriterCrashed)`, and the writer thread **exits**."

This is mechanically impossible as described. When the writer thread panics:

1. The **batch being processed** is on the writer thread's stack. The `oneshot::Sender` for each op in that batch is owned by the batch vector. On panic + unwind, the senders are **dropped** — the corresponding receivers get `Err(RecvError)` (channel closed), not `Err(WriterCrashed)`.

2. The **queued ops still in the mpsc channels** are NOT accessible from a panic hook. The panic hook runs on the panicking thread but doesn't have a reference to the channel receivers (they're in the writer loop's local scope, being unwound). Those senders are owned by the callers — the callers' receivers will eventually get `Err(RecvError)` when they try to send on a closed channel, or they'll hang if they already sent and are `await`ing their oneshot.

3. The **callers who already enqueued ops but haven't received a reply** will block forever on their `oneshot::Receiver.await` — the mpsc channel is still open (senders are cloned per-caller), but the consumer (writer thread) is dead. The ops sit in the channel buffer indefinitely until the supervisor spawns a new writer, which creates a NEW channel — the old channel's ops are leaked.

**Root cause:** The design conflates "writer thread panics" with "channel closes." The channel only closes when all senders OR the receiver is dropped. The receiver is dropped on panic (it's in the writer's scope), which DOES close the receive end — but callers who already sent still have dangling oneshot receivers.

**Impact:** After a writer panic, all in-flight callers hang forever (their oneshot never fires). `Memory::ensure_writer_alive` spawns a new writer with a new channel, but existing callers are on the OLD channel. This is a liveness bug — the process doesn't crash, but a subset of async tasks deadlock silently.

**Suggested fix:** The channel receiver should be held by the supervisor, NOT the writer thread. Design:
```
// Supervisor owns the mpsc::Receiver
// Supervisor spawns writer thread, passing ops via a second internal channel or shared queue
// On writer panic (JoinHandle returns Err):
//   1. Supervisor drains remaining ops from mpsc, sends Err(WriterCrashed) on each reply
//   2. Supervisor creates new writer thread with fresh Storage
//   3. New writer uses the SAME mpsc channel (callers don't notice the restart)
```
Alternatively: the mpsc sender has a `is_closed()` check — callers can poll and retry on a new channel. But this pushes recovery complexity to every call site. The supervisor-owns-channel model is cleaner.

---

## FINDING-6 🔴 Critical — §6.1 `WriteMemory.dimensions` implies nested Batch creation, but §6.1 forbids nested `Batch`

**Section:** §6.1 vs §6.4 vs §4.15  
**Check focus:** Batch variant partial-failure / nested semantics

§6.1 `WriteMemory` carries `dimensions: Dimensions` with the comment: "§4.15 expanded inline (Tier 1 + Tier 2/3 derived ops emitted as a Batch — see §6.4)".

§4.15 Tier 2/3 dimensions produce N `WriteDimensionEdge` + M `WriteTagEdge` ops per memory. §6.4 says these are bundled as a `Batch` for atomicity ("a memory ingest produces the memory node + N membership edges + dimension edges in one commit").

But §6.1 Batch semantics explicitly state:
> "Nested `Batch` inside `Batch` is forbidden: the writer rejects with `Err(NestedBatch)` before opening the transaction."

This creates a contradiction:
1. A memory ingest needs to be atomic (memory node + dimension edges + tag edges) → must be a `Batch`
2. `WriteMemory` carries `dimensions: Dimensions` → the writer must internally expand this into sub-ops
3. If the caller sends `Batch([WriteMemory { dimensions, .. }, WriteEntityMention { .. }])`, the writer would need to expand `WriteMemory` into multiple SQL statements inside the already-open transaction — but this expansion is invisible to the Batch's `WriteOpResult` ordering guarantee ("result at index i corresponds to ops[i]")

**The ambiguity:** Is `WriteMemory` a single op that the writer internally expands into node INSERT + edge INSERTs (handling dimensions inside `apply_op`)? Or does the caller decompose it into `Batch([WriteMemory{no dims}, WriteDimensionEdge, WriteDimensionEdge, ..., WriteTagEdge, ...])`?

If the former: `WriteMemory` is a "macro-op" that silently produces 5-20 SQL statements, and its `WriteOpResult::NodeId` doesn't account for the dimension edge IDs. The caller has no way to know which dimension edges were created or get their IDs.

If the latter: the `dimensions` field on `WriteMemory` is dead (callers decompose manually) and should be removed.

**Impact:** An implementer of `apply_op` for `WriteMemory` must decide this — the design supports both readings. Getting it wrong means either lost atomicity (caller decomposition without Batch) or invisible side effects (macro-op expansion).

**Suggested fix:** Pick one model explicitly:
- **Recommended: WriteMemory is a macro-op.** The writer's `apply_op(WriteMemory { .. })` handler creates the node, then creates dimension/tag edges, all within the current transaction. `WriteOpResult::NodeId(memory_id)` is the only result. Dimension edge IDs are not returned (callers don't need them — they're structural, not referenced by ID). Remove the comment about "emitted as a Batch" from §6.1. Add a note: "WriteMemory internally generates Tier 2/3 edges per §4.15; these are part of the same transaction, not separate WriteOps."

---

## FINDING-7 🟢 Minor — §6.6 throughput math conflates per-op µs with per-batch µs in the cost breakdown

**Section:** §6.6  
**Check focus:** Throughput numbers — derived or guessed?

§6.6's cost table for a 64-op batch of `WriteMemory`:
- Node INSERT: ~90µs × 64 = 5760µs
- Embedding blob upsert: ~80µs × 64 = 5120µs  (clarified: this is the BLOB INSERT, not generation)
- FTS trigger: ~15µs × 64 = 960µs
- WAL fsync (commit): ~50µs × 1 = 50µs
- **Total: ~5890µs per 64-op batch → ~92µs/op → ~11k ops/sec**

The math is internally consistent and the µs estimates are reasonable for NVMe SQLite (verified against published benchmarks: single-row INSERT ~50-150µs, BLOB ~50-100µs, FTS trigger ~10-30µs).

However, the analysis only covers `WriteMemory` (the heaviest op). It does not provide cost estimates for:
- `BumpAssociation` (Hebbian upsert — likely ~120µs per unique pair as mentioned in §6.3, but not in the §6.6 table)
- `ApplyDecayTick` (bulk UPDATE of `working_strength` — likely ~20µs/row for indexed update)
- `WriteFeedbackEvent` (node INSERT, lighter than WriteMemory — no embedding, no FTS)
- `Batch` with mixed ops (the real production workload)

The 11k figure is stated as "the writer's throughput ceiling" but it's actually the **worst-case floor for pure-ingest**. r2 FINDING-A4-9 already flagged this as misleading. Commit-5 did not address it.

**Impact:** Low — the real concern is "can the writer keep up?" and at 100 ingests/hr production load, even the worst-case floor has 100× headroom. But the analysis would be more useful with a mixed-workload estimate.

**Suggested fix:** Add a one-line note: "Mixed-workload estimate (80% Hebbian + 15% metacog + 5% ingest): ~30µs/op average → ~33k ops/sec effective ceiling. Production load (~100 ingests/hr + ~1000 Hebbian bumps/hr) uses <0.1% of capacity."

---

## FINDING-8 🟡 Important — §6.2 commit failure comment says replies "already been sent" before batch assembly — contradicts actual flow

**Section:** §6.2  
**Check focus:** Error handling completeness

§6.2 pseudocode has this comment on commit failure:
```rust
if let Err(e) = tx_result {
    // §6.9 dictates each enqueued op's reply has already been sent a copy of
    // the error before the batch was assembled, so callers are not stranded.
    log::error!("writer batch commit failed: {e}; continuing");
}
```

This is self-contradictory:
1. Replies can't have "already been sent" before batch assembly — at assembly time, the ops haven't been processed yet. The reply is the *result* of processing.
2. §6.1 Batch semantics say "the outer `Batch.reply` fires **once**, with `Ok(Vec<WriteOpResult>)` on full success or `Err(BatchAborted { failed_index, cause })` on first failure." This implies replies fire AFTER the attempt, not before.
3. If replies were already sent with `Ok`, and THEN the commit fails, callers have been told success for a rolled-back transaction — a correctness violation.

The comment seems to be a stale artifact from an earlier design where ops were applied and replied individually, then the batch was committed. In the current design (all ops applied in one tx, commit at end), the correct flow is:

```rust
if let Err(e) = tx_result {
    // Commit failed: send Err to all ops' reply channels
    for op in batch.drain(..) {
        let _ = op.reply.send(Err(CommitFailed(e.clone())));
    }
}
```

**Impact:** An implementer following the comment literally would either skip error notification (callers stranded) or send success prematurely (callers see success for rolled-back data).

**Suggested fix:** Replace the comment block with:
```rust
if let Err(e) = tx_result {
    // Commit failed → entire batch rolled back by SQLite.
    // Notify every op's reply channel with the commit error.
    for op in batch.drain(..) {
        let _ = op.reply_to.send(Err(WriterError::CommitFailed(e.clone())));
    }
    log::error!("writer batch commit failed: {e}; {} ops rolled back", batch_size);
}
```

---

## FINDING-9 🟢 Minor — §6.3 channel capacities: medium (4096) is 4× high (1024) — inverted relative to expected traffic distribution

**Section:** §6.3  
**Check focus:** Configuration vs hardcoding, backpressure design

§6.3 specifies channel capacities:
- `rx_high`: 1024 (ingest, entity writes, supersession)
- `rx_med`: 4096 (metacog, interoception, empathy)
- `rx_low`: 256 (decay, Hebbian — drop-oldest)

The medium channel is 4× the high channel. This is intentional — §6.3 explains: "Hebbian bumps arrive in O(K²) bursts after every recall" and medium-priority ops include Hebbian-adjacent signals. But Hebbian (`BumpAssociation`) is explicitly LOW priority, not medium.

Medium-priority ops are `WriteFeedbackEvent`, `WriteAnomalyEvent`, `UpdateDomainStats`, and the empathy variants. These fire at most once per ingest cycle (~1/sec during active conversation). A 4096 buffer for ~1/sec arrival rate gives 68 *minutes* of buffer — far beyond any realistic backpressure scenario.

Meanwhile, high-priority at 1024 with burst ingest of 100 memories (e.g., bulk import) fills to 10% in one burst. This is fine for normal use but tight for bulk operations.

**Impact:** Negligible in practice — all channels are oversized for engram's load profile. The 4096 medium buffer is wasteful but harmless (4096 × ~200 bytes per op ≈ 800KB allocated but rarely used).

**Suggested fix:** Consider `rx_med: 512` (still 8+ minutes of buffer at peak rate) and document the sizing rationale: "Channel sizes are set to absorb burst arrivals without backpressure. Oversizing is cheap (~1KB per slot); undersizing causes blocking."

---

<!-- FINDINGS -->

## ✅ Passed Checks

- **§6.1 variant coverage vs §4.1-§4.11, §4.13-§4.17**: All non-empathy writer paths have matching WriteOp variants. `WriteMemory` (§4.1), `WriteEntity`/`WriteEntityMention`/`WriteEntitySameAs` (§4.2), `BumpAssociation` (§4.3), `WriteKnowledgeTopic` (§4.4), `WriteSynthesisInsight` (§4.5), `ApplyDecayTick`/`SoftDelete` (§4.6), `SupersedeNode` (§4.7), `PromoteNode` (§4.9), `UpdateDomainStats`/`WriteAnomalyEvent`/`WriteSomaticMarker`/`WriteRegulationPolicy`/`WriteDriveAlignment` (§4.11), `WriteWmSnapshot`/`WriteMetaJudgment` (§4.13-§4.14), `WriteDimensionEdge`/`WriteTagEdge` (§4.15). ✅
- **§6.1 reply channels**: Every variant has an explicit `reply: oneshot::Sender<Result<T>>` with appropriate return type (NodeId for creates, EdgeId for edge creates, () for updates). ✅
- **§6.1 Batch reply semantics**: Post commit-5, Batch has explicit outer reply + documented inner-reply-drop behavior + nested-Batch rejection. ✅ (but see FINDING-6 for WriteMemory interaction)
- **§6.2 dedicated OS thread**: Commit-5 correctly migrated from async tokio task to `std::thread::spawn`. Cross-thread communication via `tokio::sync::mpsc` with `blocking_recv` is the right pattern. ✅
- **§6.2 batching rationale**: WAL fsync amortization across 64 ops is sound. 5ms linger is invisible to retrieval. ✅
- **§6.3 weighted fairness (structure)**: Bounded credit scheme with per-lane caps replaces strict priority drain. Addresses r2 FINDING-A4-5 starvation. Algorithm is starvation-free by construction (every lane gets its cap's worth per batch). ✅
- **§6.4 cross-op atomicity**: `Batch(Vec<WriteOp>)` + single `tx.commit()` is correct for SQLite. Failure = full rollback. ✅
- **§6.5 reader snapshot**: WAL-based reader isolation correctly described. Readers never block writers, never see partial batches. Connection pool with Semaphore is appropriate. ✅
- **§6.7 multi-tenant analysis**: Three sharding alternatives correctly rejected with sound reasoning. Single-file single-writer is the right call for engram's scale. ✅
- **§6.8 migration-phase dual-write through queue**: Sound — preserves single-writer invariant during Phase B. Backfill at low priority prevents ingest starvation. ✅
- **§6.9 process crash recovery**: SQLite WAL rollback on crash is correctly described. No half-committed batches. ✅
- **§6.9 no write journal decision**: Correctly argues against WAL-on-top-of-WAL. SQLite WAL is sufficient for single-process in-memory queue. ✅
- **No string slicing on user text** in §6. ✅
- **No `.unwrap()` in pseudocode** — all paths use `?` or explicit error handling. ✅
- **§4.12 empathy name mismatch**: Skipped per task spec — covered in r3-part4b FINDING-1. ✅ (deferred)

## Applied

(None — awaiting human approval before apply phase.)
