## Review: supersession/design.md

**Review depth:** standard (Phases 0–5, checks 0–20)
**Reviewed:** 2026-04-19

---

### 🔴 Critical (blocks implementation)

1. **FINDING-1 [Check #6] `recall_recent()` bypasses the Rust-level safety net.** The design's defense-in-depth strategy places `candidates.retain(|r| r.superseded_by.is_none())` inside `recall_from_namespace()`. However, `Memory::recall_recent()` (line ~1812 in memory.rs) does NOT go through `recall_from_namespace()` — it directly calls `self.storage.fetch_recent()` and returns the raw records with zero filtering. If the SQL filter on `fetch_recent()` is missed or has a bug, superseded memories leak into `recall_recent` results with no safety net.

   **Suggested fix:** Add a Rust-level filter in `Memory::recall_recent()`:
   ```rust
   pub fn recall_recent(&self, limit: usize, namespace: Option<&str>) -> Result<Vec<MemoryRecord>, ...> {
       let mut records = self.storage.fetch_recent(limit, namespace)?;
       records.retain(|r| r.superseded_by.is_none());
       Ok(records)
   }
   ```
   Also update §2.2 to explicitly list `recall_recent` as needing the Rust-level safety net, not just `recall_from_namespace`.

2. **FINDING-2 [Check #6] `hybrid_recall()` path completely missing from filter table.** `Memory::hybrid_recall()` (line ~2576 in memory.rs) delegates to `crate::hybrid_search::hybrid_search()`, which has its own SQL queries. This recall path is not mentioned in the §2.2 SQL filter table at all. Since `hybrid_recall` is listed in GOAL-ss.2's acceptance criteria ("hybrid_recall"), this is a coverage gap — superseded memories will appear in hybrid recall results.

   **Suggested fix:** Add `hybrid_search` module queries to the §2.2 table. Read `hybrid_search.rs` to identify which SQL queries need the filter clause. Also add a Rust-level safety net in `Memory::hybrid_recall()` before returning results.

3. **FINDING-3 [Check #6] `recall_associated_ns()` has a direct `storage.search_by_type_ns()` path.** When `cause_query` is `None`, `recall_associated_ns()` (line ~2672) calls `self.storage.search_by_type_ns()` directly, bypassing `recall_from_namespace()` entirely. The SQL filter on `search_by_type_ns` would catch it, but there's no Rust-level safety net for this path. If the SQL filter is incomplete (e.g., the `*` namespace branch), superseded causal memories leak through.

   **Suggested fix:** Add `recall_associated_ns` to the safety net discussion in §2.2. Either add `.retain()` in `recall_associated_ns` or ensure the design explicitly documents that the SQL filter is the sole barrier for this path.

4. **FINDING-4 [Check #29 partial / Check #2] Design references wrong method names.** The §2.2 SQL filter table lists `by_type()` / `by_type_in_namespace()` but the actual method names in storage.rs are `search_by_type()` and `search_by_type_ns()`. An implementer following the design literally would fail to find these methods.

   **Suggested fix:** Replace `by_type()` with `search_by_type()` and `by_type_in_namespace()` with `search_by_type_ns()` in the §2.2 table.

---

### 🟡 Important (should fix before implementation)

5. **FINDING-5 [Check #6] MemoryRecord construction sites undercounted.** §6 says "~8 in `association/candidate.rs`, `association/former.rs`, `synthesis/*.rs`, `lifecycle.rs`" need `superseded_by: None`. Actual count from codebase grep of `contradicted_by: None` is **17 sites** across: `association/candidate.rs`, `association/former.rs`, `promotion.rs`, `memory.rs` (×6), `models/actr.rs`, `synthesis/provenance.rs`, `synthesis/gate.rs`, `synthesis/insight.rs`, `synthesis/cluster.rs`, `synthesis/engine.rs`, `storage.rs`. Notably missing from the design's list: `promotion.rs`, `memory.rs` (6 instances — likely tests + `add_to_namespace`), and `models/actr.rs`.

   **Suggested fix:** Replace "~8" with the actual count (17) and list all files. Missing even one will cause a compile error since the struct field is not `Option` with a default.

6. **FINDING-6 [Check #6] `fetch_recent()` wildcard branch has no `deleted_at IS NULL` check.** The `ns == "*"` branch of `storage.fetch_recent()` is `SELECT * FROM memories ORDER BY created_at DESC LIMIT ?` — no `deleted_at IS NULL` filter. This is a pre-existing bug, but the design should note it. When adding the supersession filter, the implementer should add `WHERE (superseded_by IS NULL OR superseded_by = '') AND deleted_at IS NULL` to the wildcard branch, fixing both issues together.

   **Suggested fix:** Note this pre-existing bug in §2.2 and fix both `deleted_at` and `superseded_by` filters in the same pass for `fetch_recent()`'s wildcard branch.

7. **FINDING-7 [Check #14] `correct_bulk()` couples search and supersession in a surprising way.** `correct_bulk()` calls `recall_from_namespace(query, limit)` to find matches, but `recall_from_namespace` applies the full 6-channel scoring pipeline (embedding + FTS + ACT-R + entity + temporal + Hebbian). This means:
   - Results are ranked by relevance, not by content match — a memory with high importance but low text match could be superseded over a memory with exact text match but low importance.
   - The `limit` parameter caps results, so if there are 100 wrong memories but `limit=50`, only 50 get superseded.
   - After the first `correct_bulk` call, the newly created correction would appear in future recalls, potentially interfering with a second call.

   This is architecturally fine but should be documented as a known behavior — the user may expect "all memories matching this text" but gets "top N memories by relevance score."

   **Suggested fix:** Add a note in §2.3 that `correct_bulk` uses relevance-ranked recall (not exact text match) and that multiple passes may be needed for very large correction sets. Consider offering an FTS-only search variant for bulk corrections.

8. **FINDING-8 [Check #7] `supersede()` namespace validation has a subtle issue with `get_namespace()` returning `None`.** `get_namespace()` returns `Result<Option<String>>` where `None` means the ID was not found (`.optional()` on the query). The design validates existence via `get()` first, so `get_namespace()` should always return `Some(...)`. But if there's a TOCTOU race (memory deleted between `get()` and `get_namespace()`), `get_namespace` returns `None` and the comparison `old_ns != new_ns` would pass if both are `None` (deleted), allowing supersession of deleted memories. This is a minor edge case but worth noting.

   **Suggested fix:** Either (a) combine existence check and namespace retrieval into a single query, or (b) document that this is acceptable since superseding a just-deleted memory is harmless.

9. **FINDING-9 [Check #15] Auto-detection negation tokens are hardcoded in English and Chinese only.** §2.4 lists negation tokens ("not", "doesn't", "no longer", "isn't", "不", "没有", "不再", "并非") and correction markers ("actually", "correction", "其实", "更正"). While the `SupersessionConfig` makes these configurable, the defaults only cover English and Chinese. Users of other languages get no auto-detection.

   **Suggested fix:** This is fine for P2, but add a note in §2.4 that the default token lists are English/Chinese only, and users should configure tokens for other languages.

10. **FINDING-10 [Check #3] `SupersessionCandidate.reason` is defined but never shown to the user.** The `SupersessionCandidate` struct has a `reason` field, but §2.4 only says "return candidates (caller decides whether to supersede)". The CLI section (§4) doesn't mention displaying auto-detection results at all. The `store` command doesn't show candidates.

    **Suggested fix:** Either add CLI output for auto-detection candidates in §4 (e.g., "store" prints suggestions if `--detect` flag is passed), or mark `reason` as internal/debug-only. Currently it's a dead field from the user's perspective.

11. **FINDING-11 [Check #16] `StoreResult` widens the public API surface for a P2 feature.** `StoreResult` is a new public type returned by `add_with_detection()`. Since auto-detection is P2 and may ship in a separate PR (per §8), this type shouldn't be part of the P0/P1 implementation. If it's defined early, it becomes part of the public API before the feature is ready.

    **Suggested fix:** Move `StoreResult`, `SupersessionCandidate`, and `SupersessionConfig` definitions to §2.4 and mark them as P2-only. The P0/P1 implementation should not include these types.

---

### 🟢 Minor (can fix during implementation)

12. **FINDING-12 [Check #4] Inconsistent naming: "supersessor" vs "superseding memory".** The design uses "supersessor" in `list_superseded()` return type description ("with their supersessor") but "superseding memory" isn't used. The term "supersessor" is non-standard English. Consider standardizing on "replacement" or "successor".

13. **FINDING-13 [Check #1] `MemoryTypeArg` referenced in §4 CLI but never defined in the design.** The `Correct` command uses `Option<MemoryTypeArg>` for the `--type` flag. This type presumably already exists in the CLI (it's used by `Store`), but the design doesn't note that it's an existing type being reused. An implementer unfamiliar with the codebase might try to create it.

    **Suggested fix:** Add a brief note: "Uses existing `MemoryTypeArg` from the CLI."

14. **FINDING-14 [Check #4] `superseded_by` column default `''` vs `Option<String>` semantics.** The column uses `DEFAULT ''` (empty string) and `row_to_record` maps `''` → `None`. The SQL filter checks `superseded_by IS NULL OR superseded_by = ''`. This is correct but slightly redundant — since the default is `''` and the code always writes strings (never NULL), the `IS NULL` check is pure defense. Worth a code comment explaining why both checks exist.

15. **FINDING-15 [Check #2] §1 Requirements Coverage table references GOAL-ss.1 through GOAL-ss.8 but doesn't include the full list of recall paths from GOAL-ss.2.** GOAL-ss.2 lists: "recall, recall_from_namespace, hybrid_recall, recall_recent, recall_with_associations, recall_associated". The design's §2.2 SQL filter table doesn't cover `hybrid_recall` or `recall_with_associations` (see FINDING-2). The requirements coverage table in §1 says GOAL-ss.2 is covered by §2.2, but §2.2 is incomplete.

---

### ✅ Passed Checks

- **Check #0: Document size** ✅ — 8 sections (§1–§8), components in §2 are 5 subsections (§2.1–§2.5). Under the 8-component limit.
- **Check #1: Types fully defined** ✅ — `SupersessionError`, `SupersessionCandidate`, `StoreResult`, `BulkCorrectionResult`, `SupersessionInfo`, `SupersessionConfig` all have complete field definitions. `MemoryRecord` change is explicit. (Minor: `MemoryTypeArg` noted in FINDING-13.)
- **Check #2: References resolve** ✅ — §1 table references §2.1–§2.5 and §3, all exist. Internal references ("line ~1643") are approximate but verifiable. (FINDING-4 and FINDING-15 are resolution issues.)
- **Check #3: No dead definitions** ✅ — All types are used in method signatures or CLI commands. (FINDING-10 notes `reason` field is unused at CLI level but used in the struct.)
- **Check #5: State machine** ✅ — No state machine in this design. Memory states are binary (active/superseded) with a single transition (`supersede`) and reverse (`unsupersede`). No complex state graph.
- **Check #7: Error handling** ✅ — `SupersessionError` covers not-found, self-supersession, cross-namespace, invalid IDs, and DB errors. Bulk operation has rollback semantics. `resolve_chain_head` handles cycles with `None` return + log warning.
- **Check #8: String operations** ✅ — No string slicing on user content. Content is passed through as-is. Negation detection (§2.4) uses `.contains()` which is UTF-8 safe.
- **Check #9: Integer overflow** ✅ — No unbounded counters. `visited` HashSet in `resolve_chain_head` prevents infinite loops. `limit` parameters are bounded by caller.
- **Check #10: Option/None handling** ✅ — `get()` returns `Option` checked before use. `unwrap_or` used for defaults. No raw `.unwrap()` on uncertain values.
- **Check #11: Match exhaustiveness** ✅ — No match statements in the design. Enum variants for `SupersessionError` use `#[derive(Error)]` with explicit formatting.
- **Check #12: Ordering sensitivity** ✅ — No order-dependent guard chains. Validation checks in `supersede()` are independent (self-check, existence, namespace — order doesn't matter).
- **Check #13: Separation of concerns** ✅ — Pure logic (validation, chain resolution) is in `Storage`. Side effects (creating new memories) are in `Memory`. CLI handles UX (confirmation prompts). Clean layering.
- **Check #14: Coupling** ✅ — `superseded_by` is a simple ID reference, not derived state. Events carry only what they need. (FINDING-7 is an architectural note, not a coupling violation.)
- **Check #16: API surface** ✅ — Public API additions are minimal: `supersede`, `unsupersede`, `correct`, `correct_bulk`, `list_superseded`, `resolve_chain_head`. Internal helpers stay private. `add()` return type unchanged (GUARD-ss.2). (FINDING-11 notes P2 types leaking early.)
- **Check #17: Goals and non-goals** ✅ — Requirements doc has explicit "Out of Scope" section: no auto-supersession, no conflict resolution UI, no LLM-based detection, no versioning. Design respects these boundaries.
- **Check #18: Trade-offs documented** ✅ — §1 documents SQL-level vs Rust-level filtering trade-off with clear decision rationale. §2.4 documents breaking vs non-breaking return type change. Both have alternatives considered.
- **Check #19: Cross-cutting concerns** ✅ — Performance addressed in GUARD-ss.4 with concrete estimates. Security addressed in SEC-ss.1 with namespace scoping. Observability addressed in OBS-ss.1 with `list_superseded`.
- **Check #20: Appropriate abstraction level** ✅ — Pseudocode is at the right level — shows signatures, key logic, SQL patterns. Not over-specified (no exact line numbers for edits) but specific enough that two engineers would implement similarly.

---

### Summary

- **Critical: 4** (FINDING-1 through FINDING-4)
- **Important: 7** (FINDING-5 through FINDING-11)
- **Minor: 4** (FINDING-12 through FINDING-15)
- **Total: 15 findings**

**Recommendation: needs fixes first** — The 4 critical findings represent real recall filter gaps. FINDING-1 (recall_recent bypass) and FINDING-2 (hybrid_recall missing) mean superseded memories WILL leak into results through paths the design doesn't cover. FINDING-4 (wrong method names) will cause implementation confusion. These must be fixed before implementation begins.

**Estimated implementation confidence: medium** — The core design is sound (SQL filter + Rust safety net is the right architecture), but the recall path coverage has gaps. Once the missing paths are added to the filter table and safety nets are placed in all `Memory`-level recall methods (not just `recall_from_namespace`), confidence would be high.
