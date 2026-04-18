## Review: architecture.md

**Review depth**: standard (Phase 0–5, Checks 0–20)
**Reviewed**: 2026-04-17
**Reviewer**: design-review skill (automated)

---

### 🔴 Critical (blocks implementation)

1. **FINDING-1 [Check #6] `ChangeSet.affected_topic_ids` has no upstream writer** — `ChangeSet` (§4.4) has a field `affected_topic_ids: Vec<TopicId>`, but the architecture never specifies *who computes* this mapping. For incremental recompilation, something must map changed memory IDs → affected topic IDs by looking up the `source_memory_ids` on existing `TopicPage` rows. The architecture says "content hashing" (D5) drives recompilation, but then `ChangeSet` implies a memory-ID-based approach. These are two different strategies and neither is fully specified.

   **Suggested fix:** Choose one strategy and specify it. If memory-ID-based: add a method signature like `fn affected_topics(changed: &[i64], store: &dyn TopicStore) -> Vec<TopicId>` and note that it queries the `source_memory_ids` index. If content-hash-based: remove `affected_topic_ids` from `ChangeSet` and instead describe the scan-and-compare flow. Don't leave both half-specified.

2. **FINDING-2 [Check #1] `TopicStore` trait referenced but never defined** — §1.4 D2 says: *"The existing `Storage` struct gains new methods via `impl` blocks or a `TopicStore` trait."* This is an either/or without resolution. `TopicStore` is never defined anywhere — no methods, no signatures. Feature designs downstream (compilation, maintenance) cannot implement against this until the persistence API is specified.

   **Suggested fix:** Define the trait in §4 Shared Types, even if minimal:
   ```rust
   pub trait TopicStore {
       fn save_topic(&self, page: &TopicPage) -> Result<(), StorageError>;
       fn get_topic(&self, id: &TopicId) -> Result<Option<TopicPage>, StorageError>;
       fn list_topics(&self) -> Result<Vec<TopicPage>, StorageError>;
       fn topics_for_memories(&self, memory_ids: &[i64]) -> Result<Vec<TopicId>, StorageError>;
       fn mark_stale(&self, ids: &[TopicId]) -> Result<(), StorageError>;
   }
   ```
   Or explicitly decide it's `impl Storage` and defer the trait to feature design.

3. **FINDING-3 [Check #7] Feedback loop error/conflict handling unspecified** — The Feedback Loop stage (§3.1) says input is "user rating/correction" and output is "updated TopicPage + adjusted weights." But there is no error path defined. What happens if:
   - User corrects a topic but the underlying memories contradict the correction?
   - Adjusted weights produce a degenerate recompile (all memories excluded)?
   - User feedback conflicts with a prior feedback on the same topic?

   §2.1 error handling only covers LLM and storage failures. Feedback semantic errors are unaddressed.

   **Suggested fix:** Add a `FeedbackError` or `FeedbackConflict` variant in the error handling section, and specify that user corrections are stored as override metadata (not mutations to source memories), with a conflict resolution rule (e.g., latest-wins, or flag for manual review).

---

### 🟡 Important (should fix before implementation)

4. **FINDING-4 [Check #4] Inconsistent naming: `SynthesizedInsight` ID type** — `TopicPage.source_insight_ids` is `Vec<String>`, while `TopicPage.source_memory_ids` is `Vec<i64>`. The architecture doesn't define what type `SynthesizedInsight` uses for its ID. If the existing synthesis engine uses a different ID type (e.g., `i64`, UUID, or a newtype), downstream features will have a type mismatch. The inconsistency between `String` for insights and `i64` for memories is suspicious — is this intentional?

   **Suggested fix:** Either (a) verify the existing `SynthesizedInsight` ID type and match it, or (b) define an `InsightId` newtype parallel to `TopicId` and use it consistently.

5. **FINDING-5 [Check #14] `TopicCandidate` carries derived state** — `TopicCandidate.entity_ids` duplicates information that's already derivable from `memory_ids` (since entities are extracted from memories). If a memory's entities change (re-extraction), the `entity_ids` in the candidate become stale. This is the coupling smell from Check #14 — the candidate carries derived data that's already available from the source.

   **Suggested fix:** Either (a) remove `entity_ids` and have the consumer look them up from `memory_ids`, or (b) document that `entity_ids` is a snapshot taken at discovery time and may diverge, and specify when it's refreshed.

6. **FINDING-6 [Check #15] Performance budgets are hardcoded, not configurable** — §2.4 specifies concrete targets (<2s, <5s, <50MB) but these aren't exposed as configuration. If a user has 100k memories on a slow disk, the 2s target for discovery is unrealistic. More critically, `--limit N` is mentioned for batch operations but there's no `KcConfig` field for it — it's only a CLI flag. This means library API callers have no way to set batch limits.

   **Suggested fix:** Add to `KcConfig`:
   ```rust
   /// Maximum memories to process per batch operation (default: unlimited)
   pub batch_limit: Option<usize>,
   /// Timeout per LLM call in seconds (default: 30)
   pub llm_timeout_secs: u64,
   ```

7. **FINDING-7 [Check #13] `compile()` purity claim is questionable** — §2.5 claims `fn compile(memories: &[Memory], config: &CompileConfig) -> TopicPage` is pure. But §3.1 shows Topic Rendering has an "Optional: LLM enhancement pass." If `compile()` is truly pure (no IO), then it can't call LLM. That's fine — but then who orchestrates the two-pass flow (pure compile → LLM enhance)? There's no orchestrator/coordinator defined.

   **Suggested fix:** Define the orchestration layer explicitly. Something like:
   ```rust
   // In src/compiler/mod.rs
   pub async fn compile_topic(
       candidate: &TopicCandidate,
       memories: &[Memory],
       config: &CompileConfig,
       llm: Option<&dyn LlmProvider>,
   ) -> Result<TopicPage, CompileError> {
       let mut page = compile(memories, config);      // pure
       if let Some(llm) = llm {
           enhance(&mut page, llm).await?;            // IO
       }
       Ok(page)
   }
   ```

8. **FINDING-8 [Check #10] `TopicMetadata.user_rating` is `Option<f32>` — semantics unclear** — What's the valid range? Is it 0.0–1.0? 1–5 stars? Negative allowed? Without bounds, downstream code will handle it inconsistently. Also, `revision: u32` starts at what value? 0 or 1? An `u32` can overflow if somehow recompiled 4 billion times (unlikely, but `revision + 1` without check is technically unbounded).

   **Suggested fix:** Add doc comments specifying:
   ```rust
   /// User feedback score in range [0.0, 5.0], None = no feedback
   pub user_rating: Option<f32>,
   /// Revision counter starting at 1 (first compilation)
   pub revision: u32,
   ```
   And note that revision overflow is not a practical concern (u32 max is ~4 billion).

9. **FINDING-9 [Check #3] `RecompileMode` is defined but connection to triggering is dead** — `RecompileMode` (§4.4) is defined as `Incremental | Full` and referenced in `KcConfig.recompile_mode`. But the architecture never specifies *where* `recompile_mode` is read or how it influences the pipeline. Is it checked in Topic Discovery? Rendering? The maintenance trigger path says it "optionally triggers incremental recompile" but doesn't reference `RecompileMode`. This type is defined but its integration point is unspecified.

   **Suggested fix:** In §3.1 or §3.3, add: "The compilation pipeline checks `config.recompile_mode` at the discovery stage: if `Incremental`, compute `ChangeSet` and only process affected topics; if `Full`, process all topic candidates."

10. **FINDING-10 [Check #2] §2.2 references `LlmError` but it's never defined** — The `LlmProvider` trait returns `Result<String, LlmError>`, but `LlmError` is not defined in this document. §2.1 defines the error handling *pattern* (use `EngineError`) but doesn't clarify the relationship between `LlmError` and `EngineError`. Is `LlmError` a variant of `EngineError`? A separate type? Does it already exist in the codebase?

    **Suggested fix:** Add a note: "LlmError is the existing error type from `src/models/`. It is converted to `EngineError::Llm(LlmError)` at module boundaries." Or define the variant explicitly.

11. **FINDING-11 [Check #16] `TopicPage` has large public API surface** — `TopicPage` has 10 public fields, `TopicMetadata` has 4, and both derive `Serialize, Deserialize`. This means the serialized format is effectively a public API. Any field rename or type change breaks stored data. For a SQLite-persisted type, this is a forward-compatibility concern.

    **Suggested fix:** Add a note in §2.7 (Migration) that topic table schema includes a `schema_version` column, and that `TopicPage` field changes require a migration step. Alternatively, store the content as a JSON blob with explicit version field.

---

### 🟢 Minor (can fix during implementation)

12. **FINDING-12 [Check #4] Naming inconsistency: `kc` vs full names** — §2.6 uses `KcConfig` (abbreviated), §2.7 mentions `engram kc` namespace, but the features are called "Knowledge Compiler" / "Topic Compiler" / "Knowledge Maintenance" everywhere else. The abbreviation "KC" is introduced without definition and used inconsistently (sometimes `kc`, sometimes full name). Minor, but could confuse new contributors.

    **Suggested fix:** Add at the top of §1.1: "Knowledge Compiler (KC)" as the canonical abbreviation, then use it consistently.

13. **FINDING-13 [Check #4] Module line counts in Appendix A use suspicious units** — `storage.rs` is listed at "104k" lines and `memory.rs` at "132k" lines. If these are literal line counts, a 132,000-line single Rust file would be extraordinary (the entire Rust compiler `rustc` is ~500k lines across thousands of files). These might be byte counts (104KB) mislabeled as line counts, or estimates. This is misleading for effort estimation.

    **Suggested fix:** Verify actual line counts with `wc -l` and correct the table. If they're byte counts, label them as such (e.g., "104KB").

14. **FINDING-14 [Check #1] `MaintenanceSchedule` type used in `KcConfig` but never defined** — §2.6 `KcConfig` has `pub maintenance_schedule: MaintenanceSchedule` but this type has no definition in §4 Shared Types. What are its variants? Is it a cron expression? An enum of `OnConsolidate | Daily | Manual`?

    **Suggested fix:** Add to §4 Shared Types:
    ```rust
    #[derive(Debug, Clone)]
    pub enum MaintenanceSchedule {
        /// Run maintenance during consolidation (default)
        OnConsolidate,
        /// Run on explicit command only
        Manual,
    }
    ```

15. **FINDING-15 [Check #1] `CompileConfig` referenced in §2.5 but never defined** — The pure function signature is `fn compile(memories: &[Memory], config: &CompileConfig) -> TopicPage`. `CompileConfig` is distinct from `KcConfig` but never defined. What fields does it have?

    **Suggested fix:** Either define `CompileConfig` (likely a subset of `KcConfig` relevant to a single compilation), or replace with `&KcConfig` if they're the same.

16. **FINDING-16 [Check #20] Appendix A is implementation-level detail in an architecture doc** — Listing exact line counts and file names is useful context, but module line counts will be stale immediately. This is maintenance burden with low value.

    **Suggested fix:** Keep the file listing but remove line counts, or mark them as "approximate as of [date]".

17. **FINDING-17 [Check #18] D4 (offline-first) trade-offs not documented** — The design decision says "every operation has a non-LLM fallback path" and "rendering falls back to template-based output." But the trade-off isn't discussed: how much quality degradation is acceptable? If the template fallback produces significantly worse output, users might never use offline mode. What's the expected quality delta? This is a design decision that should document the trade-off explicitly.

    **Suggested fix:** Add to D4: "Trade-off: offline/template output covers ~70% of LLM quality for structured topics (lists, timelines) but degrades significantly for narrative synthesis. This is acceptable because [reason]." Or defer this analysis to the feature design doc and note that explicitly.

---

### ✅ Passed Checks

- **Check #0: Document size** ✅ — 8 components listed in §1.3, exactly at the ≤8 limit. No split needed.
- **Check #2: References resolve** ✅ (partial) — §3.2 references Compilation/Maintenance/Platform which are defined in §5. §1.4 D1–D5 reference sections that exist. §2 cross-cutting concerns are self-contained. (Exceptions: `LlmError`, `TopicStore`, `CompileConfig`, `MaintenanceSchedule` flagged separately.)
- **Check #3: No dead definitions** ✅ (partial) — Most types are referenced in prose or pipeline. `RecompileMode` integration is weak (flagged as FINDING-9) but not fully dead. `LinkType` variants are all plausible for `BrokenLink`. `ConflictSeverity` is used in `ConflictRecord`.
- **Check #5: State machine invariants** ✅ N/A — No explicit state machine in this architecture doc. The pipeline is a DAG, not a state machine. Individual feature designs may have state machines.
- **Check #8: String operations** ✅ — No string slicing operations specified. `TopicId` is a hash string, not user-manipulated. Content is stored as full strings.
- **Check #9: Integer overflow** ✅ — `revision: u32` noted in FINDING-8 but not practically dangerous. `confidence: f64` has no arithmetic defined at this level. No counter increment logic specified in the architecture doc.
- **Check #11: Match exhaustiveness** ✅ — All enums (`DiscoveryMethod`, `AccessLevel`, `ConflictSeverity`, `RecompileMode`, `LinkType`) have small, closed variant sets. No catch-all branches specified.
- **Check #12: Ordering sensitivity** ✅ N/A — No match/if-else chains with guards in the architecture doc.
- **Check #13: Separation of concerns** ✅ (partial) — The three-layer architecture (CLI / Pipeline / Foundation) has clean separation. Pure compile + IO enhance split is good (modulo FINDING-7 on orchestration). Maintenance goes through compilation pipeline, never edits directly — good boundary.
- **Check #17: Goals and non-goals** ✅ — Appendix B lists 5 explicit non-goals (real-time collab, web UI, distributed storage, custom embeddings, plugin system). These are concrete and don't conflict with stated goals. Goals are referenced by ID (GOAL-comp.*, GOAL-maint.*, GOAL-plat.*) from requirements docs.
- **Check #19: Cross-cutting concerns** ✅ — Privacy (§2.3), performance (§2.4), testability (§2.5), configuration (§2.6), migration (§2.7) are all addressed. Security is covered implicitly (no external calls beyond LLM, no telemetry). Observability is not mentioned but is a reasonable omission for a CLI tool.
- **Check #20: Appropriate abstraction level** ✅ (mostly) — The doc is at the right level for an architecture doc: structural decisions, type definitions, data flow, and cross-cutting concerns. Pseudocode clarifies intent without over-specifying. Appendix A is slightly too detailed (FINDING-16).

---

### Summary

| Severity | Count |
|----------|-------|
| 🔴 Critical | 3 |
| 🟡 Important | 8 |
| 🟢 Minor | 6 |

**Recommendation: needs fixes first** — The 3 critical findings (FINDING-1: recompilation strategy ambiguity, FINDING-2: undefined persistence API, FINDING-3: feedback error handling) each represent gaps that would force feature designers to guess or make incompatible assumptions. The important findings are real but can be resolved during feature design if the architecture doc explicitly defers them.

**Estimated implementation confidence: medium** — The architecture is well-structured and the pipeline is clear. The gaps are in the seams between components (persistence API, recompile triggering, feedback conflict handling) rather than in the components themselves. Two competent engineers would agree on the broad structure but might implement the incremental recompile mechanism very differently.
