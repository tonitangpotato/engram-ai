# Design Review r2-part4 — v04-unified-substrate (Infrastructure & Meta)

> **Reviewer:** claude (sub-agent, part 4/4)
> **Date:** 2026-05-12
> **Target:** `.gid/features/v04-unified-substrate/design.md` §6–§10 (lines 982–1675)
> **Scope:** §6 Writer queue architecture, §7 Resolved decisions, §8 Action plan, §9 Risks, §10 Status
> **Method:** 36-check review-design skill, depth=full

## Summary

| Severity   | Count |
|------------|-------|
| 🔴 Critical   | 4   |
| 🟡 Important  | 8   |
| 🟢 Minor      | 2   |
| **Total**  | **14**   |

**Recommendation**: Needs fixes before implementation. The four critical findings (FINDING-A4-1, A4-3, A4-8, A4-14) represent internal contradictions that would cause implementers to build the wrong thing. All are resolvable by aligning the action plan (§8) and risks (§9) with the detailed design (§4.15, §6.9).

**Estimated implementation confidence**: Medium — §6 writer queue architecture is solid in concept but has several specification gaps (missing variants, async/sync mismatch, priority starvation). §7 resolved decisions are well-reasoned. §8 task list needs dependency ordering. §9 risks are comprehensive but contain stale references.

---

## FINDING-A4-1 🔴 Critical — §6.9 contradicts itself on write journal; T66 implements what §6.9 rejects

**Check #5, #7 (logic correctness, error handling)**

§6.9 explicitly states:

> **No write journal beyond SQLite's WAL.** A separate disk journal of pre-commit ops would be a "WAL on top of WAL" — pointless duplication. SQLite's WAL *is* the durable log.

But T66 in §8.15 says:

> **T66** Implement write journal (§6.9): append-only log of pending ops, fsync'd before queue ack — replays on crash recovery before accepting new writes.

And R9 in §9 says:

> (b) §6.9 write journal means in-flight ops survive restart

These are three contradictory positions:
1. §6.9 body: no write journal, SQLite WAL is sufficient, lost in-flight ops are fine.
2. T66: implement a write journal with fsync + crash replay.
3. R9 mitigation: assumes write journal exists.

The §6.9 body argues *against* a journal, then T66 and R9 assume one exists. This is a copy-paste residual — likely T66/R9 were written before §6.9 was finalized, or §6.9 was revised to reject the journal without updating downstream references.

**Impact**: An implementer following T66 would build an unnecessary journal that §6.9 says not to build. Or an implementer reading §6.9 would skip the journal, then wonder why R9's mitigation claims it exists.

**Suggested fix**: Pick one position. The §6.9 analysis is sound — SQLite WAL *is* the durable log, and a pre-WAL journal is redundant for single-process in-memory queue. If the "no journal" position wins:
- **T66**: Delete entirely, renumber or leave gap.
- **R9 mitigation (b)**: Replace with "(b) on writer restart, in-flight ops whose `oneshot` receivers are dropped will be retried by callers who received `Err(WriterCrashed)` — idempotent ops (Hebbian, decay) are safe; non-idempotent ops (WriteMemory) generate new node IDs on retry (correct semantic)."
- **§6.9 body**: No change needed; it's already correct.

If the "yes journal" position wins, rewrite §6.9's last paragraph to describe the journal format, replay semantics, and fsync strategy.

---

## FINDING-A4-2 🟡 Important — §0 TL;DR claims "68 atomic tasks" but there are 70 checklist items; T04 and T44 are duplicates

**Check #2, #4 (references resolve, consistent naming)**

§0 TL;DR says: "§8 has 68 atomic tasks (T01–T68)"
§10 Status says: "68 atomic tasks (T01–T68)"

Actual count: `grep '^\- \[.\] \*\*T' design.md` returns **70 lines**. The discrepancy:
- T26a, T26b, T26c are three separate checklist items but share the T26 namespace → counting them as 1 task gives 68, counting as 3 gives 70. The TL;DR should clarify.
- **T04** ("Update `consolidation-autopilot-DRAFT.md` §2 invariants to reference unified substrate") and **T44** ("Update `consolidation-autopilot-DRAFT.md` to reference unified substrate") appear to be duplicates with near-identical descriptions. One is in §8.1 Setup, the other in §8.8 Cleanup.

**Suggested fix**:
1. §0 TL;DR: change "68 atomic tasks (T01–T68)" to "68 task IDs (T01–T68, with T26 split into T26a–c) across 70 checklist items."
2. Merge T04 and T44 or differentiate them. If T04 is "update §2 invariants" and T44 is "update other sections post-migration", make the scope explicit. If identical, delete T44 and add a `depends_on: T39` note to T04 (since it can't finalize until Phase F).

---

## FINDING-A4-3 🔴 Critical — §6.1 WriteOp enum is missing 5+ write paths defined in §4.12, §4.15, §6.8

**Check #1, #6 (types fully defined, data flow completeness)**

§6.1 declares "every mutation in engram becomes a `WriteOp` variant. The set is closed and audited." But cross-referencing with §4.x writer paths reveals gaps:

**Missing from §6.1 enum but explicitly named as writer-queue ops in other sections:**

| Op name | Defined in | Description |
|---|---|---|
| `WriteAlignmentEdge` | §4.12 | Fires on every ingest, low priority, batchable |
| `WriteActionOutcome` | §4.12 | Fires on heartbeat action completion |
| `UpdateDriveReinforcement` | §4.12 | Increments `last_reinforced` on drive node |
| `LogExternalWrite` | §4.12 | Audit trail for file mutations (SOUL.md etc.) |
| `BackfillBatch` | §6.8 | Low-priority backfill rows through queue |

§4.12 explicitly says "Writer paths through §6 queue:" and lists four ops by name. None appear in the §6.1 enum. Instead, §6.1 has a single `WriteEmpathySignal` variant that doesn't correspond to any of the four.

§6.8 introduces `BackfillBatch { rows: Vec<LegacyRow> }` as a "dedicated low-priority WriteOp variant" but it's absent from §6.1.

**Additionally ambiguous:**

- §4.15 says "dimensions enter as part of `WriteMemory`" — this is internally consistent (dimensions bundled into WriteMemory), but the `WriteMemory` variant in §6.1 carries `dimensions: Dimensions` which implies the *old* flat `Dimensions` struct, not the Tier 2 edge-based model. The enum payload should show the dimension edges will be created, or at least reference §4.15's model.
- §4.10 episode node creation — no explicit WriteOp. Presumably episodes are created during backfill (T19/T22) and during ingest (as part of WriteMemory Batch). But §6.1 doesn't show this.

**Impact**: Implementers of T61 ("Implement WriteOp enum per §6.1") will produce an incomplete enum. Implementers of T49 ("Refactor bus/ to drain into writer queue") will find no matching variant.

**Suggested fix**: Add the 5 missing variants to §6.1's enum definition:
```rust
// §4.12 empathy bus (replace WriteEmpathySignal)
WriteAlignmentEdge { memory_id: NodeId, drive_id: NodeId, score: f64 },
WriteActionOutcome { action_type: String, success: bool, ... },
UpdateDriveReinforcement { drive_id: NodeId, delta: f64 },
LogExternalWrite { target: String, content_hash: String },

// §6.8 migration
BackfillBatch { rows: Vec<LegacyRow>, reply_to: oneshot::Sender<Result<usize>> },
```

Also: either rename the existing `WriteEmpathySignal` to one of the above, or remove it and replace with the four concrete variants from §4.12.

---

## FINDING-A4-4 🟡 Important — §6.2 writer loop mixes async tokio with synchronous rusqlite; blocking the runtime

**Check #13 (separation of concerns), #21 (ambiguous prose)**

§6.2's pseudocode runs as an `async fn` in a tokio task:
```rust
async fn writer_loop(mut rx: mpsc::Receiver<WriteOp>, mut storage: Storage) {
    ...
    let tx = storage.conn_mut().transaction()?;
    for op in batch.drain(..) { apply_op(&tx, op); }
    tx.commit()?;
}
```

`rusqlite::Connection` is synchronous. `transaction()` and `commit()` perform IO (WAL fsync). Running these in an `async fn` on the tokio runtime **blocks the executor thread** for the duration of the SQLite transaction (estimated 5.8ms per batch in §6.6).

At 5.8ms per batch, this blocks one tokio worker thread ~0.6% of the time under realistic load — negligible. But during backfill (§6.8, sustained 11k ops/sec), the writer blocks a thread for ~5.8ms every 5.8ms → **100% blocking of one worker thread**. If the tokio runtime has N worker threads, this reduces capacity by 1/N for all other async tasks during backfill.

This is a well-known footgun in the Rust async ecosystem. The standard solutions are:
1. `tokio::task::spawn_blocking` for the SQLite portion.
2. A dedicated OS thread (not a tokio task) for the writer, communicating via channel.
3. `block_in_place` (if using multi-threaded runtime).

**Suggested fix**: Clarify in §6.2 that the writer loop either:
- Runs on a **dedicated OS thread** (`std::thread::spawn`), not a tokio task — receives from an `mpsc::Receiver` (std, not tokio). This is the simplest and most correct approach for a single-writer pattern.
- Or uses `tokio::task::spawn_blocking` around the transaction commit. But this is awkward because the whole loop is the blocking part.

The dedicated-thread approach is more natural for a single-writer: no async needed, the thread parks on the channel, wakes on message, runs synchronous SQLite, loops.

---

## FINDING-A4-5 🟡 Important — §6.3 priority drain can starve medium/low channels indefinitely

**Check #5, #7 (logic correctness, error handling — deadlocks/starvation)**

§6.3's batch assembly pseudocode:
```rust
while batch.len() < BATCH_MAX {
    // Drain high first
    while batch.len() < BATCH_MAX {
        match rx_high.try_recv() { Ok(op) => batch.push(op), _ => break }
    }
    // Then medium
    // Then low
    if batch.is_empty() { rx_high.recv().await; } // park on high
}
```

If `rx_high` sustains ≥64 ops between commits (e.g., during burst ingest of 100 memories), medium and low channels are **never drained**. This means:
- Metacog feedback events (medium) pile up indefinitely during ingest bursts.
- Decay ticks (low) are silently dropped (drop-oldest on a 256-capacity channel).

For decay, dropping is fine (idempotent). But for metacog feedback, loss during ingest bursts is a silent correctness issue — the agent's self-evaluation becomes sparse during exactly the periods where it's doing the most work.

**Suggested fix**: Reserve a fraction of BATCH_MAX for non-high ops. For example:
```rust
let high_cap = (BATCH_MAX * 3) / 4;  // 48 of 64
// Drain high up to high_cap
// Then drain medium up to remaining capacity
// Then drain low up to remaining capacity
```

Or: after every N batches of pure-high, force-drain one medium + one low op ("aging" / anti-starvation). This is the standard pattern in priority schedulers.

---

## FINDING-A4-6 🟡 Important — §6.3 Hebbian coalescing HashMap has no size bound

**Check #9, #7 (integer/memory overflow, error handling)**

§6.3 says:

> Hebbian coalescing: the writer maintains a small `HashMap<(NodeId, NodeId), f64>` accumulator. Successive `BumpAssociation` ops with the same `(from, to)` add to the accumulator instead of emitting separate edge upserts. Flush on batch commit.

The accumulator is described as "small" but has no size cap. A retrieval returning K results generates O(K²) Hebbian bumps (every pair co-activated). With K=20, that's 190 unique pairs per recall. Under burst recall (10 recalls/sec during a conversation), the accumulator grows to ~1900 entries/sec.

The accumulator flushes on batch commit — but it's only populated by *low-priority* Hebbian ops, which may be starved by high-priority ingest (per FINDING-A4-5). If medium/low channels back up, the coalescing accumulator grows in the channel, not in the HashMap (since `try_recv` on low channel fails during high-priority bursts). So the *channel* absorbs the growth, not the HashMap.

However, the design doesn't specify: **is the coalescing HashMap flushed per-batch or across batches?** If it persists across batches (only flushed when Hebbian ops are actually processed), and Hebbian ops are starved for extended periods, the HashMap silently discards coalesced state when the writer restarts.

**Suggested fix**: Clarify that the HashMap is (a) per-batch scoped (created fresh, flushed as part of `tx.commit()`), and (b) capped at a reasonable size (e.g., 10,000 entries — if exceeded, force-flush the current contents and start a new accumulation window within the same batch).

---

## FINDING-A4-7 🟢 Minor — §6.2 and §6.3 present incompatible pseudocode for the same writer loop

**Check #21 (ambiguous prose — two implementations would differ)**

§6.2 shows a complete writer loop with `mpsc::Receiver<WriteOp>` (single channel) and `tokio::select!` for batch fill with deadline.

§6.3 replaces this with three channels (`rx_high`, `rx_med`, `rx_low`) and a different drain pattern using `try_recv()` in priority order, with `rx_high.recv().await` for parking.

These two pseudocode blocks cannot both be the implementation. §6.2's single-channel design is simpler but doesn't support priority. §6.3's multi-channel design doesn't include the `BATCH_LINGER` timeout from §6.2.

An implementer must mentally merge these into one design, which is error-prone.

**Suggested fix**: §6.2 should present the *final* merged design (three channels + linger timeout + priority drain), and §6.3 should explain the priority *semantics* without re-presenting the loop. Alternatively, §6.2 can be labeled "simplified single-channel sketch" with a forward reference: "see §6.3 for the production multi-channel version."

---

## FINDING-A4-8 🔴 Critical — §6.1 WriteOp reply channel not shown on most variants; Batch nested reply semantics undefined

**Check #1, #6 (types fully defined, data flow completeness)**

§6.1 states: "every `WriteOp` carries a `oneshot::Sender` for its result." But the enum definition only shows `reply_to: oneshot::Sender<Result<NodeId>>` on `WriteMemory`. All other variants (`WriteEntity`, `BumpAssociation`, `ApplyDecayTick`, `WriteAnomalyEvent`, etc.) end with `...` and no reply channel.

More critically: `Batch(Vec<WriteOp>)` nests WriteOps. If each inner op carries its own `reply_to`, do all fire on batch commit? Or does only the Batch's outer reply fire? §6.4 says "the reply oneshot fires only after the full batch commits" — but which oneshot? The inner ops' individual ones, or a Batch-level one not shown in the enum?

**Suggested fix**: (a) Add `reply_to` to every variant or extract it: `struct QueuedOp { op: WriteOp, reply_to: oneshot::Sender<Result<OpResult>> }` — cleaner, separates routing from payload. (b) For `Batch`, specify: inner ops' reply channels are fired individually after commit, OR Batch carries one reply channel and inner ops have none (which means Batch is a different type from `Vec<WriteOp>`).

---

## FINDING-A4-9 🟡 Important — §6.6 throughput math includes embedding blob cost but §6.1 says embedding generation is pre-enqueue

**Check #5 (logic correctness — arithmetic with concrete values)**

§6.6's cost table includes "Embedding blob upsert: ~80µs × N = 5120µs" for a 64-memory batch, totaling 5.8ms → 11k ops/sec ceiling.

But §6.1 `WriteMemory` carries `embedding: Option<Vec<f32>>` and §6.6 notes "embedding generation cost is paid before enqueue, not in the writer." The 80µs is just the blob INSERT — reasonable for a ~3KB blob (768×f32).

However, the 11k ops/sec number assumes **every op is a WriteMemory with an embedding blob**. In practice, most ops are BumpAssociation, ApplyDecayTick, WriteFeedbackEvent — none carry blobs. The "11k ops/sec" figure is a *worst-case* floor, not a ceiling. The real ceiling for mixed workloads is much higher (~100k+ ops/sec for non-blob ops per the 120µs Hebbian estimate).

This isn't wrong, but it's misleading — the TL;DR says "~11k ops/sec" as if it's the steady-state number. Under realistic mixed load, the effective ceiling is 5-10× higher.

**Suggested fix**: §6.6 should clarify: "11k ops/sec is the floor for pure-ingest (worst case). Mixed workload ceiling is ~50-100k ops/sec. The binding constraint in production is ingest rate (~100/hr), making the writer idle >99.9% of the time."

---

## FINDING-A4-10 🟡 Important — §6.9 writer panic recovery via catch_unwind is unsound with rusqlite

**Check #7, #23 (error handling, dependency assumptions)**

§6.9 proposes:
> `tokio::spawn(async move { let _ = std::panic::catch_unwind(...); })`. On panic, the writer logs, transitions the channel to a closed state... A supervisor (`Memory::ensure_writer_alive`) restarts the writer task with a fresh `Storage` handle.

`std::panic::catch_unwind` only catches unwinding panics, not `abort`. More importantly, `rusqlite::Connection` is not `UnwindSafe` — a panic mid-transaction leaves the connection in an undefined state. `catch_unwind` on a `!UnwindSafe` type requires `AssertUnwindSafe` wrapper, which is explicitly opting out of safety guarantees.

The "restart with a fresh `Storage` handle" is the right recovery, but the design should note: the panicked connection is **dropped** (not reused), and the fresh `Storage` opens a new SQLite connection. SQLite's WAL recovery handles the abandoned transaction on re-open.

**Suggested fix**: Replace `catch_unwind` with `std::thread::spawn` (if using dedicated thread per FINDING-A4-4) + monitoring the thread's `JoinHandle`. If the thread panics, the `JoinHandle::join()` returns `Err`. The supervisor spawns a new thread with a new `Storage`. No need for `catch_unwind` at all.

---

## FINDING-A4-11 🟡 Important — §7.2 section numbering: "6.2.1" should be "7.2.1"

**Check #2 (references resolve)**

§7.2 contains a subsection labeled "**6.2.1 Canonical vs clique**" — this should be **7.2.1**. The section is inside §7.2, not §6.2.

**Suggested fix**: Rename "6.2.1" → "7.2.1".

---

## FINDING-A4-12 🟡 Important — §8 tasks missing dependency chains and effort estimates for most items

**Check #25, #21 (testability/implementability, ambiguous prose)**

The task prompt specifies: "Each task: ID, title, depends_on, estimated effort (S/M/L), satisfies (GOAL refs)?" — but most §8 tasks have only ID + title. No `depends_on`, no effort estimate, no GOAL ref.

Exceptions: T02 is marked `[x]` (done). T26a-c have effort hints in their descriptions. But T05-T68 have none of these fields.

This makes the action plan an unordered checklist rather than a dependency-aware execution plan. An implementer cannot determine: which tasks can be parallelized? Which block others? Which are S (1hr) vs L (1 day)?

**Suggested fix**: At minimum, add `depends_on` for tasks with ordering constraints. Key dependencies that are implicit but not stated:
- T10 depends on T05-T08 (types need tables defined first)
- T12-T16 depend on T10 (dual-write needs Rust types)
- T19-T27 depend on T12-T16 passing (backfill needs dual-write verified)
- T61-T66 (writer queue) should precede T12-T16 (dual-write goes through queue)
- T45-T48 (interoception) depend on T61 (writer queue exists)

The last point reveals a **sequencing ambiguity**: §8.15 (writer queue, T61-T68) is listed *after* §8.3 (Phase B dual-write, T12-T18). But §6.8 says Phase B dual-write goes through the queue. So T61 (implement WriteOp enum) must precede T12 (dual-write store_raw). The current T-numbering implies the opposite order.

---

## FINDING-A4-13 🟢 Minor — §9 R8 is well-reasoned; R9 mitigation (b) references non-existent write journal

**Check #30, #31 (technical debt, shortcut detection)**

R8 ("baseline ephemerality") is well-reasoned: the warm-up window after restart is acknowledged, and `MIN_SAMPLES ≥ 30` is a concrete mitigation. No issue.

R9 ("writer SPOF") correctly identifies the single-writer risk. However, mitigation (b) references the write journal that §6.9 explicitly rejects (see FINDING-A4-1). Mitigation (c) references T67/T68 which are good.

R10 ("node_dimensions growth") references a `node_dimensions` table from §4.15 — but §4.15 actually describes dimensions as Tier 1 (attributes JSON), Tier 2 (edges to dimension nodes), and Tier 3 (tags as edges). There is no `node_dimensions` table in §3 schema. T56 in §8.13 references it too. This is a schema artifact that was designed in §4.15 prose but never added to §3.

**Suggested fix**: Either add `node_dimensions` to §3 as a new table (if that's the intended design), or update §4.15/T56/R10 to reflect that dimensions are stored as edges + attributes (no dedicated table), which is what §4.15's body actually describes.

---

## FINDING-A4-14 🔴 Critical — §4.15 dimensional model vs §8.13 T56-T59 and R10 describe incompatible storage designs

**Check #32, #2 (conflicts with existing architecture, references resolve)**

This is the root issue behind FINDING-A4-13. Two incompatible designs coexist:

**§4.15 body** (the detailed design): 3-tier model:
- Tier 1: scalar dimensions in `nodes.attributes` JSON
- Tier 2: narrative fields as `describes_*` edges to `node_kind='dimension'` nodes
- Tier 3: tags as `tagged` edges to `node_kind='tag'` nodes

**§8.13 tasks** (the implementation plan):
- T56: "Schema: `node_dimensions` table (Tier 2 — full dimension vector per node)"
- T57: "Dual-write: writes both legacy `dimensions` table and new `node_dimensions`"
- T58: "Retrieval adapter: dimensional plan reads from `node_dimensions`"

These are different designs. §4.15 stores Tier 2 dimensions as **graph edges** (the whole point of §4.15.5's justification). §8.13 stores them in a **dedicated `node_dimensions` table** — exactly the "separate table per function" pattern §1.2 critiques.

R10 compounds this: "At ~10 dimensions per memory × 10M memories that's 100M rows" — this row math makes sense for a table, not for edges (which are already counted in edges table sizing).

**Impact**: T56-T59 will implement a different design than §4.15 specifies. This is a multi-day implementation mismatch.

**Suggested fix**: Align §8.13 with §4.15. T56 should become "Implement Tier 2 dimension edges: `node_kind='dimension'` nodes + `describes_*` edges per §4.15.2." T57/T58 follow accordingly. R10 should reference edge count growth, not a `node_dimensions` table. Delete all references to `node_dimensions` table.

---

## Passed Checks and Cross-Cutting Verification

---

### ✅ Passed Checks

- **Check #0**: Document size — §6-§10 is within scope (infrastructure + meta). Component count across full doc is ~17 §4.x subsections; already split into 4 review parts. ✅
- **Check #3**: No dead definitions in §6-§10 — every WriteOp variant maps to a §4.x function; every risk maps to a design section. ✅
- **Check #4** (partial): Naming consistency within §6 — `WriteOp`, `NodeId`, `Storage` used consistently. ✅
- **Check #5** (§6.4): Cross-op atomicity — `Batch(Vec<WriteOp>)` + single `tx.commit()` is sound for SQLite's transaction model. The failure semantics (all-or-nothing rollback) are correctly stated. ✅
- **Check #5** (§6.5): Reader snapshot — WAL mode reader isolation is correctly described. Readers get point-in-time consistency, never see partial batches. This is standard SQLite WAL behavior, correctly applied. ✅
- **Check #8**: No string slicing on user text in §6-§10. ✅
- **Check #10**: No `.unwrap()` in pseudocode — all error paths use `?` operator. ✅
- **Check #13**: Separation of concerns — writer queue cleanly separates mutation (queue producer) from persistence (queue consumer). Readers are fully independent. ✅
- **Check #14**: Coupling — WriteOp variants carry payload data, not derived state. Callers don't reach into writer internals. ✅
- **Check #15**: Configuration — BATCH_MAX and BATCH_LINGER are named as tunables with sensible defaults. Priority channel capacities (1024/4096/256) are documented. ✅
- **Check #16**: API surface — WriteOp is the public contract; internal apply_op handlers are private to the writer. ✅
- **Check #17**: Goals explicit — §6 rationale paragraph clearly states the goal (serialize mutations, enable atomicity, support priority). ✅
- **Check #18**: Trade-offs — §6.7 documents 3 sharding alternatives with pros/cons and explicit rejection. §4.13 documents 3 WM options with rationale for chosen. ✅
- **Check #19**: Cross-cutting — performance (§6.6 throughput), failure modes (§6.9), scale ceiling (§6.7) all addressed. ✅
- **Check #20**: Abstraction level — pseudocode clarifies design intent without being copy-pasteable Rust. Appropriate for a design doc. ✅
- **Check #24**: Migration path — §6.8 clearly describes how Phase B dual-write flows through the queue. Phase E removal is a localized diff. ✅
- **Check #25**: Testability — T67 (throughput bench) and T68 (scale ceiling test) provide concrete test plans for §6. ✅
- **Check #26**: Existing functionality — verified `Memory` struct and `Storage` connection model match design claims (storage.rs:157, memory.rs:68). ✅
- **Check #27**: API compatibility — WriteOp preserves the `store_raw(...).await → NodeId` shape via oneshot reply channels. No external API break. ✅
- **Check #28**: Feature flag — `MemoryConfig::unified_substrate` flag (T28) enables gradual rollout. ✅
- **Check #33**: Simplification — §6 does not drop edge cases. Concurrent readers, crash recovery, priority starvation, panic recovery all addressed (some with issues noted above). ✅
- **Check #34**: Breaking-change risk — §6.8 ensures Phase B is invisible to consumers (write fan-out only). §7.7 has hard exit criteria with Jaccard similarity thresholds. ✅
- **Check #35**: Purpose alignment — every §6 subsection serves the stated goal (serialized writes + atomicity + priority). No speculative flexibility. ✅

### Cross-Cutting Verification

- **§0 TL;DR claims 11 risks**: §9 has R1–R11. ✅ Verified, count matches.
- **§0 TL;DR claims "68 atomic tasks (T01–T68)"**: Actual task IDs T01–T68 are all present (68 unique IDs). However, T26 is split into T26a/b/c (70 checklist lines). See FINDING-A4-2. 🟡
- **§7.7 hard exit criteria**: All three criteria are testable with concrete numbers (row-count parity, Jaccard ≥0.99 on 95% of queries, J-score ±1pp). Not vibes. ✅
- **§7.4 episode-is-node**: Consistent with §4.10 spec (containment edges, not columns). ✅
- **§7.1 single DB file**: Rationale convincing — ATTACH DATABASE doesn't provide cross-DB atomicity. ✅
- **Task ID uniqueness**: All T01–T68 are unique. No duplicates in ID namespace. T04/T44 have similar *descriptions* (FINDING-A4-2) but different IDs. ✅
- **T-numbering vs phase ordering**: T01–T44 follow Phase A→F sequence. T45–T60 cover §4.11–§4.16 (can run in parallel with phases). T61–T68 cover §6 writer queue. **But T61–T68 must precede T12–T18** per §6.8 (dual-write goes through queue). See FINDING-A4-12. 🟡

---

<!-- FINDINGS -->

## Applied

(None — awaiting human approval before apply phase.)
