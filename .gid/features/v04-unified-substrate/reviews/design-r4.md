# Design Review r4 — Focused Post-Commit-7 Review

> **Reviewer:** sub-agent (coder)
> **Date:** 2026-05-12
> **Target:** `.gid/features/v04-unified-substrate/design.md` — 8 in-scope sections changed by commit 7 (628dfc4)
> **Prior reviews:** r1 (16 findings, all applied), r2 (4 parts), r3 (5 parts — 4 Criticals raised: F2, F5, F6, F7)
> **Method:** 36-check review-design skill, depth=full, focused on commit-7 changes only
> **r3 Criticals under verification:** F2 (WM cold_start), F5 (panic recovery / WriterSupervisor), F6 (WriteMemory macro-op), F7 (UNIQUE on tagged/describes_*)

## Scope

Only reviewing sections changed by commit 7:
1. §3.2 `edges` table (taxonomy table, UNIQUE on containment)
2. §3.5 Audit tables (UNIQUE mention)
3. §4.13 Working memory (cold_start | warm flag)
4. §4.15 Dimensional signature (§4.15.2–§4.15.3 UNIQUE, §4.15.6 write amp)
5. §4.8 Retrieval plans (factual sub-plan rewrite)
6. §6.1 Write op enum (WriteMemory macro-op, WriteMemoryReply)
7. §6.9 Failure modes / WriterSupervisor
8. §8 Action plan (T57, T58, T66)

## Summary

| Severity   | Count |
|------------|-------|
| 🔴 Critical   | 0     |
| 🟡 Important  | 3     |
| 🟢 Minor      | 2     |
| **Total**  | **5** |

**Recommendation:** Implementation-ready. All 4 r3 Criticals are closed by commit 7. No new Criticals found. The 3 Important findings (FINDING-1, FINDING-2, FINDING-4) are prose clarifications and missing fields — they can be fixed in a quick commit 8 before T01, or addressed during T61/T63 implementation with the design as guidance. None blocks schema creation (T05–T11).

**Estimated implementation confidence:** High — the macro-op, supervisor, and taxonomy are internally consistent. The gaps are at the boundary (episode edge routing, reply ownership semantics, memory_type derivation) and are small enough that an implementer with this review in hand can resolve them inline.

---

## FINDING-1 🟡 Important — §6.9 WriterSupervisor HashMap<OpId, oneshot::Sender> is under-specified: reply type erasure and completion signaling missing

**Check:** #1 (Every type fully defined) + #21 (Ambiguous prose)
**Location:** §6.9 lines ~1730–1745, T66 lines ~2082–2095

**Issue:** The supervisor recovery design says: "It keeps a copy of the `oneshot::Sender` reply (or a stable per-op `op_id` indexing into a `HashMap<OpId, oneshot::Sender>`) before forwarding."

Two problems:

1. **Type erasure**: `oneshot::Sender` is generic — each WriteOp variant has a different reply type (`Sender<Result<WriteMemoryReply>>`, `Sender<Result<NodeId>>`, `Sender<Result<()>>`, etc.). A `HashMap<OpId, oneshot::Sender<???>>`  can't hold heterogeneous sender types. The design needs to specify either (a) a type-erased wrapper (`Box<dyn ReplySlot>` with a `send_error` method for the crash path), or (b) a single `Result<WriteOpResult>` reply type shared across all variants (matching the Batch result pattern from §6.4), or (c) the supervisor stores opaque `Box<dyn FnOnce(Error)>` closures that each op-specific extractor constructs.

2. **Completion signaling**: the design doesn't specify how the writer thread signals "op X is done with result Y" back to the supervisor. If the supervisor extracts the `oneshot::Sender` before forwarding, the writer has no way to deliver the result. Two implementation paths exist:
   - **Proxy pair**: supervisor creates a new `oneshot` pair, gives writer the `Sender`, keeps the `Receiver`, and listens for completions. On receive, supervisor forwards to the original sender. But this doubles channel overhead and requires the supervisor to poll N receivers.
   - **Direct send**: the writer keeps the original `oneshot::Sender` (supervisor does NOT extract it). On panic, the sender is dropped, and the receiver gets `RecvError`. This is actually the simplest design — but then the `HashMap<OpId, oneshot::Sender>` in §6.9/T66 is unnecessary, and the "iterate in-flight map" recovery can't work (the senders died with the thread).

The **direct-send** path is far simpler and is what actually happens when a thread panics: all values owned by the thread are dropped, `oneshot::Sender::drop()` closes the channel, and the receiver gets `RecvError`. The supervisor doesn't need to explicitly send `Err(WriterCrashed)` — it's implicit in the channel closure.

If direct-send is the intent, then §6.9's claim "on writer panic... iterate in-flight map sending `Err(WriterCrashed { generation, cause })` to every pending reply" is misleading — the supervisor can't send a *typed error with generation/cause* because it doesn't own the senders. The receiver just gets a generic `RecvError`.

**Impact:** Two competent engineers would implement different supervisor architectures. One would build the complex proxy-pair system described in prose; the other would realize the direct-send path is simpler and correct. The prose needs to commit to one.

**Suggested fix:** Commit to direct-send. Replace the HashMap/in-flight-map language with:

> When the writer thread panics, all `oneshot::Sender`s in the thread's batch + coalescer are dropped. Each async caller's `oneshot::Receiver` resolves to `RecvError`. The supervisor's job on panic is: (1) detect via `JoinHandle::join() → Err`, (2) log the panic cause, (3) drain the public channels sending `Err(WriterCrashed { generation, cause })` to any ops that haven't been forwarded yet, (4) `Storage::reopen()`, (5) spawn fresh writer thread, (6) increment generation. Ops already forwarded to the private channel and consumed by the dead writer get `RecvError` on their reply — callers treat `RecvError` and `Err(WriterCrashed)` identically (retry or propagate).

This eliminates the HashMap, the type-erasure problem, and the completion-signaling ambiguity. Update T66 accordingly.

---

## FINDING-2 🟡 Important — §6.1 WriteMemory macro-op is missing episode_id field; §4.1 episode edge creation orphaned

**Check:** #6 (Data flow completeness) + #2 (References resolve)
**Location:** §6.1 WriteMemory variant (line ~1192), §6.1 macro-op steps (line ~1407), §4.1 (line ~437)

**Issue:** §4.1 (Memory ingest) shows an episode edge being created atomically with the memory node:

```
INSERT INTO edges (id, source_id=memory_id, target_id=episode_node_id,
                   edge_kind='containment', predicate='belongs_to_episode', ...);
```

But the `WriteMemory` variant in §6.1 has no `episode_id: Option<NodeId>` field, and the macro-op step list (steps 1–3) makes no mention of episode edges. This means:

- A caller who wants to assign a memory to an episode must either (a) use a separate `Batch([WriteMemory, WriteEdge])` (which defeats the macro-op's purpose of encapsulating all memory-related writes), or (b) construct the edge separately via `WriteDimensionEdge` or similar (non-atomic).
- §4.1's pseudocode is unreachable through WriteMemory as currently specified.

Additionally, the `WriteMemoryReply` struct has `dimension_edges` and `tag_edges` but no `episode_edge_id`, consistent with the omission.

§4.15.6's write amp budget table also doesn't include episode edges (typically 1 per memory if episodes are active), understating the true per-memory op count by ~1 at P50.

**Impact:** Episode containment edges are a core part of the substrate design (§7.4 explicitly chose episodes-as-nodes over episode-as-column). Without a clear write path through WriteMemory, episode assignment is either non-atomic or requires caller-side Batch construction — contradicting the macro-op rationale.

**Suggested fix:**
1. Add `episode_id: Option<NodeId>` to WriteMemory variant fields.
2. Add step 4 to macro-op: "If `episode_id` is Some, INSERT edge `containment/belongs_to_episode` from memory → episode."
3. Add `episode_edge: Option<EdgeId>` to WriteMemoryReply.
4. Update §4.15.6 budget table to include "+1 episode edge (when episode context active)".

---

## FINDING-3 🟢 Minor — §4.13 `wm_state` cold/warm flag lacks initialization specification

**Check:** #21 (Ambiguous prose)
**Location:** §4.13 lines ~748–750

**Issue:** §4.13 defines `wm_state` in the snapshot attributes as "the cold/warm flag at capture time, persisted so downstream analysis can distinguish 'agent had genuinely empty WM' from 'agent had just restarted and not yet recalled anything'." This is the correct semantics (closes r3-F2).

However, the design doesn't specify:
- How the in-memory `WorkingMemory` struct tracks cold vs warm state.
- What transitions it from `cold` to `warm` (first memory added? first recall? first user interaction?).
- T51 ("Implement in-memory `WorkingMemory`") doesn't mention the cold/warm flag.

Two implementers might choose different transition points (first attention shift vs first explicit recall vs time-based warmup).

**Suggested fix:** Add one sentence to §4.13: "WM initializes in `cold_start` state. Transitions to `warm` on the first `attend()` call that adds a memory to the ring buffer. The flag is read-only after that — it never reverts to cold within a session." Add `wm_state: cold_start | warm` to T51.

---

## FINDING-4 🟡 Important — §6.1 WriteMemory variant missing `memory_type` and `layer` fields; derivation logic unspecified

**Check:** #6 (Data flow completeness) + #1 (Every type fully defined)
**Location:** §6.1 WriteMemory variant (line ~1192), §3.1 nodes DDL

**Issue:** The `WriteMemory` variant carries: `content`, `dimensions`, `occurred_at`, `embedding`, `namespace`, `agent_id`. But `nodes` requires:
- `memory_type TEXT` (first-class column with dedicated index `idx_nodes_memory_type`) — not inside `attributes`
- `layer TEXT` ('core'|'working'|'archive')

Neither field appears in the WriteMemory payload. The macro-op steps don't specify how these columns are populated. Two paths exist:

1. `memory_type` could be derived from `dimensions.type_weights` (argmax). But this derivation isn't mentioned.
2. `layer` defaults to 'working' on ingest (all new memories start in working memory). But this isn't stated.

The `source` column (default '') and `importance` (default 0.3) have DDL defaults, so they're OK. But `memory_type` and `layer` have no DDL defaults and no payload fields — the writer would INSERT NULL, which is technically valid (both are nullable) but defeats the purpose of the `idx_nodes_memory_type` index and makes `nodes WHERE memory_type='factual'` miss freshly ingested memories.

**Suggested fix:** Either:
- (A) Add `memory_type: Option<String>` and `layer: String` to WriteMemory variant. Layer defaults to `"working"` at call site.
- (B) Document in macro-op steps: "Step 1a: derive `memory_type` from `dimensions.type_weights` (argmax), set `layer='working'`." Callers don't need to pass these — the writer derives them.

Option (B) is cleaner (fewer caller concerns), but must be documented.

---

## FINDING-5 🟢 Minor — T66 references HashMap<OpId, oneshot::Sender> that conflicts with FINDING-1 resolution

**Check:** #2 (References resolve) + #4 (Consistent naming)
**Location:** §8.15 T66 (line ~2082)

**Issue:** T66 says: "owns the public per-priority mpsc receivers + a `HashMap<OpId, oneshot::Sender>` of in-flight replies. Forwarder task moves ops from public → private channels into the writer thread." This matches §6.9's current prose but inherits the type-erasure and completion-signaling problems from FINDING-1.

If FINDING-1's suggested fix (direct-send) is adopted, T66 needs rewriting to remove the HashMap/forwarder-task language and describe the simpler model: supervisor owns public receivers, forwards whole WriteOps (with reply intact) to private channels, and on panic just drains public channels with `Err(WriterCrashed)`.

**Suggested fix:** Update T66 after resolving FINDING-1.

---

<!-- FINDINGS -->

## ✅ Passed Checks (in-scope sections only)

### r3 Critical Closure Verification

- **r3-F2 (WM cold_start flag):** ✅ CLOSED — §4.13 now includes `wm_state: cold_start | warm` in `wm_snapshot` attributes. Semantics clear. Minor gap in initialization logic (FINDING-3).
- **r3-F5 (panic recovery / WriterSupervisor):** ✅ CLOSED with caveat — §6.9 now specifies supervisor-owned channels + thread respawn + generation counter. Core recovery model is sound. Implementation ambiguity in reply-channel ownership (FINDING-1, Important not Critical — the design works, just needs prose clarification).
- **r3-F6 (WriteMemory macro-op vs separate edge writes):** ✅ CLOSED — §6.1 now has full macro-op specification with steps 1–3, WriteMemoryReply struct, and clear "WriteDimensionEdge/WriteTagEdge are backfill-only" stance. Episode edge gap flagged (FINDING-2).
- **r3-F7 (UNIQUE on tagged/describes_*):** ✅ CLOSED — Commit 7 moved `tagged` and `describes_*` from `edge_kind='structural'` to `edge_kind='containment'`. §3.2 taxonomy table, §4.15.2, §4.15.3 all consistently reference `containment` edge_kind. The existing `idx_edges_containment_unique` partial UNIQUE index covers both. §4.15.2 includes explicit rationale ("Why `containment` and not `structural`"). No remaining contradiction.

### Per-Check Results (applicable checks against in-scope sections)

- **Check #0** (Document size): Out of scope — §3.x count unchanged from r3. ✅
- **Check #1** (Types fully defined): WriteMemoryReply struct fully defined with 3 fields. WriterSupervisor architecture diagrammed. `wm_snapshot` attributes enumerated. Missing `memory_type`/`layer` in WriteMemory (FINDING-4). ⚠️
- **Check #2** (References resolve): §3.2 taxonomy → §4.15 ✅. §4.15.2 → §3.2 containment UNIQUE ✅. §6.1 macro-op → §4.15 tiers ✅. §4.8 factual plan → edges WHERE containment ✅. T57 → §6.1 WriteMemory macro-op ✅. T58 → §3.2 idx_edges_containment_unique ✅. T66 → §6.9 ✅. §4.1 episode edge → §6.1 WriteMemory ❌ (FINDING-2).
- **Check #3** (No dead definitions): WriteDimensionEdge/WriteTagEdge standalone variants justified for backfill. No dead definitions found. ✅
- **Check #4** (Consistent naming): `containment` used consistently for tagged/describes_* across §3.2, §4.15, §6.1. `wm_snapshot` consistent in §4.13, §4.14, §6.1. ✅
- **Check #5** (State machine / flow): Supervisor lifecycle traced: spawn → forward ops → detect panic via JoinHandle → drain channels → reopen Storage → respawn → increment generation. Forward progress guaranteed (generation always increments). No deadlocks (panic kills thread, supervisor always respawns). ✅
- **Check #6** (Data flow): Tier 1 scalars: Dimensions → nodes.attributes ✅. Tier 2: Dimensions → dimension nodes + containment edges ✅. Tier 3: Dimensions.tags → tag nodes + containment edges ✅. memory_type/layer: unspecified derivation (FINDING-4). ⚠️
- **Check #7** (Error handling): Writer errors → oneshot reply ✅. Panic → thread death + channel closure → RecvError ✅. Queue overflow → per-priority policy (§6.3) ✅. No unbounded retries ✅.
- **Check #8** (String ops): No string slicing in any in-scope section. ✅
- **Check #9** (Integer overflow): Generation counter is unbounded u64 — overflow at 2^64 not practical. ✅
- **Check #10** (Option/None): `occurred_at: Option` correctly nullable. `episode_id` absent (FINDING-2). ✅
- **Check #11** (Match exhaustiveness): WriteOp enum is closed; macro-op expansion is internal. ✅
- **Check #13** (Separation of concerns): Macro-op expansion writer-internal. Supervisor separate from writer thread. ✅
- **Check #14** (Coupling): WriteMemory carries raw Dimensions, not pre-computed edges. Writer derives edges — correct. ✅
- **Check #15** (Configuration): BATCH_MAX, queue caps — documented as configurable. ✅
- **Check #16** (API surface): WriteMemoryReply minimal. Standalone edge variants kept for backfill. ✅
- **Check #17–20** (Doc quality): Clear rationale, trade-offs documented (write amp budget). ✅
- **Check #21** (Ambiguous prose): Supervisor reply ownership (FINDING-1). WM cold_start transition (FINDING-3). ⚠️
- **Check #30** (Tech debt): No "temporary" language. Macro-op is permanent design. ✅
- **Check #31** (Shortcut detection): Supervisor addresses root cause (rusqlite not UnwindSafe). ✅
- **Check #32** (Architecture conflicts): Containment edges align with existing containment usage. ✅
- **Check #33** (Simplification): Write amp budget has concrete P50/P95. Not simplified away. ✅
- **Check #34** (Breaking-change risk): Macro-op is new code. Standalone variants preserved. ✅
- **Check #35** (Purpose alignment): Every component traces to a stated goal. ✅

## Applied

**Commit 8** (2026-05-12, applied by main agent — not sub-agent, in-scope was ≤300 lines):

- **F1** ✅ §6.9: rewrote supervisor architecture to direct-send model. Writer keeps the original `oneshot::Sender<R>` inside each WriteOp variant; supervisor wraps it in `Arc<Mutex<Option<oneshot::Sender<R>>>>` on the public→private hop, and stores `Box<dyn FnOnce(WriterCrashed) + Send>` crash-notifier closures in `HashMap<OpId, …>`. Added explicit completion-signaling via separate `mpsc::UnboundedSender<OpId>` tick channel. Clarified type-erasure rationale (kept replies strongly typed at variant level; no heterogeneous sender map). Public §6.1 WriteOp surface unchanged.
- **F2** ✅ §6.1: added `episode_edge: Option<EdgeId>` to `WriteMemoryReply`. §4.15.6: added `in_episode` edge row to write-amp budget table (P50 0.7, P95 1.0), updated totals (11→12 P50, 24→25 P95), updated peak throughput projection (1200→1250 ops/sec, headroom 9×→8.8×). `episode_id` field on WriteMemory variant and macro-op step 4 were already in place from commit 7.
- **F3** ✅ T51: rewrote task description to make the `cold_start | warm` initialization, transition triggers, and persistence into wm_snapshot explicit (was previously only in §4.13 prose, not surfaced in the task list).
- **F4** ✅ §6.1: `memory_type`/`layer` fields and macro-op step-1 derivation rule ("writer populates directly from op payload, does NOT derive from dimensions") + caller-responsibility note were already added in commit 7. No additional change required — verified consistent with `WriteMemory` variant fields.
- **F5** ✅ T66: rewrote task description to match the direct-send model from F1 (typed-closure crash-notifier map instead of heterogeneous sender map, explicit Arc<Mutex<Option<…>>> sharing, OpDone tick channel for cleanup).

**Net change**: design.md 2210 → 2232 lines (+22). Diff: +52 / −23.

**Carried-forward debt** (acceptable, per r4 recommendation): none — all 3 Important + 2 Minor closed.
