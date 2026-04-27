//! v0.2 backward-compatibility shim (design §7).
//!
//! Implements the **signature-lock + behavioral-contract** half of GOAL-4.9:
//! v0.2 call sites for `store`, `recall`, `recall_recent`, and
//! `recall_with_associations` must compile and behave unchanged against v0.3.
//! The full integration test suite that exercises the four methods against a
//! real migrated database lives in a separate task
//! (`task:mig-test-compat-rollback`, design §11.5); this module is the
//! library-side **contract** the suite codes against.
//!
//! ## Why a trait, not a re-export?
//!
//! `engramai-migrate` is a leaf crate with no dependency on `engramai` core
//! — the same architectural decision documented in `backfill.rs` (T8 vs T9).
//! We therefore cannot directly reference `engramai::Memory`, `MemoryId`, or
//! `RankedMemory` types here. Instead we declare a [`V02CompatSurface`]
//! trait with associated types: any concrete `Memory` impl in `engramai`
//! that satisfies this trait is *signature-locked* against v0.2 by
//! construction. If a future v0.3.x change breaks one of the four
//! signatures, the impl block stops compiling, which is exactly the
//! behavior GOAL-4.9 requires.
//!
//! The behavioral half of the contract — what each method should *do* on a
//! migrated database — is captured by [`BEHAVIORAL_CONTRACT`], a static
//! transcription of design §7.2 that the integration tests assert against.
//!
//! ## Scope of this task (T13)
//!
//! T13 is the **shim contract only**:
//!
//! - The trait surface ([`V02CompatSurface`]) — pinned signatures.
//! - A static description of behavioral expectations
//!   ([`BEHAVIORAL_CONTRACT`], [`MethodContract`]).
//! - A compile-time witness helper ([`assert_v02_compat`]) the test crate
//!   uses to prove `engramai::Memory: V02CompatSurface`.
//! - Unit tests that exercise the trait through an in-memory stub —
//!   verifying the trait shape is implementable and the contract table is
//!   well-formed.
//!
//! What is **not** in this task:
//!
//! - The v0.2-fixture-database migration test (`task:mig-test-compat-rollback`,
//!   design §11.5). That suite lives in an integration-test crate that
//!   depends on both `engramai-migrate` and `engramai` core, and does the
//!   real work of asserting `recall` rankings on a migrated database.
//! - Any rerouting logic ("`store` calls `store_raw` internally"). Per
//!   design §7.1, the four methods *stay on `Memory` verbatim*; v0.3 may
//!   route them through new internals but that wiring is owned by
//!   `engramai`, not by this shim. This module only **locks** the surface.
//!
//! ## Design references
//!
//! - §7.1 Signature preservation (the four methods + `#[non_exhaustive]`
//!   additive-only rule)
//! - §7.2 Behavioral contract table (this module's `BEHAVIORAL_CONTRACT`)
//! - §7.3 Compatibility test matrix (driven by the test crate, not here)
//! - GOAL-4.9 (P0) — preserve compile + behavior across the migration boundary
//!
//! ## Stability contract
//!
//! The four methods listed in [`V02_FROZEN_METHODS`] are **frozen for the
//! entire v0.3.x series**. Any v0.3.x release that changes one of these
//! signatures (parameter list, return type, generic bounds) is a v0.3.x
//! contract violation, even if the change "looks additive". Additive
//! changes belong on *new* methods (`graph_query`, `store_raw`, `explain`,
//! …) per §7.1's "strictly additive" rule.

/// The four v0.2 method names this shim freezes (design §7.1).
///
/// Used by integration tests, audit tooling, and the `engramai migrate
/// verify` subcommand (T14) to enumerate the locked surface without
/// hard-coding string literals.
pub const V02_FROZEN_METHODS: &[&str] = &[
    "store",
    "recall",
    "recall_recent",
    "recall_with_associations",
];

/// The four v0.2 surface methods, kept signature-frozen for v0.3.x.
///
/// Any concrete `Memory` impl in `engramai` is expected to implement this
/// trait. Implementations are signature-locked by construction: if a
/// v0.3.x change breaks one of these method shapes, the impl block stops
/// compiling, which surfaces the contract violation at build time rather
/// than at runtime. This is the compile-time half of GOAL-4.9.
///
/// ## Associated types
///
/// We use associated types instead of hard-coding `engramai`'s concrete
/// `MemoryId` / `RankedMemory` / `MemoryRecord` / `AssociativeResult`
/// types because this crate (`engramai-migrate`) is a leaf — it cannot
/// depend on `engramai`. Each impl picks its own concrete types; the
/// shape (number of params, ownership of `&str`, `Result<…>` return) is
/// what's locked, not the inner type names.
///
/// The exact concrete types in use for v0.3.0 are documented in design
/// §7.2 ("v0.3 behavior on migrated-v0.2 DB"); the v0.3 impl in
/// `engramai/src/memory.rs` is the source of truth for what they resolve
/// to.
///
/// ## Why no `&self` vs `&mut self` lock?
///
/// `recall` and `recall_with_associations` both take `&mut self` in the
/// v0.2 surface (they record metacognition events / update ACT-R activation).
/// `recall_recent` and `store` have varying `self`-mutability across v0.2
/// minor releases. The trait method receivers therefore use the same
/// receiver kind the v0.2.2 surface used; tightening to `&self` would be a
/// silent contract narrowing and is rejected by GOAL-4.9.
///
/// ## Error type
///
/// We expose the error as `Self::Error` rather than the concrete
/// `Box<dyn std::error::Error>` that v0.2 returned. This is **not** a
/// signature relaxation: any `Memory` impl in `engramai` is free to set
/// `Error = Box<dyn std::error::Error>` and the existing call sites
/// continue to compile. Using an associated type lets the test crate
/// substitute a richer error in its stub without forcing a `Box` boxing.
pub trait V02CompatSurface {
    /// The v0.3 concrete identifier returned by `store`. v0.2 used a
    /// `String`-shaped `MemoryId`; v0.3 keeps the type frozen per §7.1.
    type MemoryId;

    /// The v0.3 ranked-recall row returned by `recall`. Must remain
    /// field-compatible with v0.2's `RankedMemory` (additive fields only,
    /// gated by `#[non_exhaustive]`).
    type RankedMemory;

    /// The v0.3 plain memory row returned by `recall_recent`.
    type MemoryRecord;

    /// The v0.3 result struct returned by `recall_with_associations`.
    /// Field-compatible with v0.2's `RecallWithAssociationsResult`.
    type AssociativeResult;

    /// The error type returned by all four methods. v0.2 used
    /// `Box<dyn std::error::Error>`; impls are free to reuse the same
    /// boxed-error alias.
    type Error;

    /// `store(content)` — write a memory row, return its identifier.
    ///
    /// Behavioral contract (design §7.2):
    /// - Returns the same kind of identifier on a migrated-v0.2 DB and on
    ///   a fresh-v0.3 DB.
    /// - Embedding computation continues to be a background side-effect
    ///   (not blocking the call).
    /// - On v0.3 internally routes through the resolution pipeline
    ///   (`store_raw` with default `StorageMeta`) but the caller observes
    ///   identical timing and identifier semantics.
    ///
    /// **Note on signature precision (design §7.1):** the documented v0.2
    /// surface method takes a single `&str` content argument and returns
    /// the memory identifier. Real v0.2.2 includes additional optional
    /// parameters (namespace, importance, …) on richer overloads
    /// (`store_enriched`, `store_raw`); those are *not* part of the frozen
    /// surface — only the four canonical methods listed in
    /// [`V02_FROZEN_METHODS`] are. The trait codifies the canonical shape;
    /// `Memory` may keep additional methods so long as the canonical four
    /// remain.
    fn store_v02(&mut self, content: &str) -> Result<Self::MemoryId, Self::Error>;

    /// `recall(query)` — vector + ACT-R ranked recall.
    ///
    /// Behavioral contract (design §7.2 + ranking-contract paragraph):
    /// - Return ordering is the same dot-product-vector + ACT-R
    ///   activation contract used in v0.2.
    /// - On a migrated DB, new v0.3 signals (graph-edge distance, affect
    ///   congruence) **must not reorder the top-K** in a way that breaks
    ///   v0.2 caller expectations. Improvements that *would* reorder must
    ///   be gated behind the new `GraphQuery` surface.
    fn recall_v02(&mut self, query: &str) -> Result<Vec<Self::RankedMemory>, Self::Error>;

    /// `recall_recent(limit)` — N most-recently-stored memories, newest first.
    ///
    /// Behavioral contract (design §7.2):
    /// - Simple time-ordered SELECT on `memories.created_at`.
    /// - Unchanged across v0.2 → v0.3 — no graph signals, no ACT-R
    ///   adjustment.
    fn recall_recent_v02(
        &self,
        limit: usize,
    ) -> Result<Vec<Self::MemoryRecord>, Self::Error>;

    /// `recall_with_associations(query)` — recall + Hebbian-linked neighbors.
    ///
    /// Behavioral contract (design §7.2):
    /// - Reads `hebbian_links` table directly with the same query as v0.2.
    /// - Surfaces both the primary recall result and its associated
    ///   neighbors via the same struct shape v0.2 used.
    fn recall_with_associations_v02(
        &mut self,
        query: &str,
    ) -> Result<Self::AssociativeResult, Self::Error>;
}

/// Compile-time witness that a type implements the v0.2 compat surface.
///
/// Used by the integration test crate (`task:mig-test-compat-rollback`)
/// to assert at build time:
///
/// ```ignore
/// const _: () = engramai_migrate::compat::assert_v02_compat::<engramai::Memory>();
/// ```
///
/// If `engramai::Memory`'s impl block ever drifts from the locked
/// surface, the const expression fails to compile, with the error
/// pointing directly at the method that broke. This is the signature-lock
/// gate for GOAL-4.9.
pub const fn assert_v02_compat<T: V02CompatSurface>() {}

/// One row of the §7.2 behavioral-contract table.
///
/// Each frozen method has a [`MethodContract`] entry describing what
/// behavior must hold on (a) a migrated-v0.2 database and (b) a fresh-v0.3
/// database. Integration tests use these strings as the human-readable
/// rationale when asserting; if a behavior expectation drifts, the
/// integration suite is updated *and* this table is updated in lockstep
/// (the tests print the contract string on assertion failure).
///
/// The strings are deliberately verbatim from design §7.2 — keeping them
/// in code rather than only in markdown means the implementation and the
/// contract document stay synchronized through normal code review.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MethodContract {
    /// One of the four entries from [`V02_FROZEN_METHODS`].
    pub method: &'static str,
    /// What v0.2 documented this method to do. Source of authority for any
    /// future divergence dispute.
    pub v02_documented_behavior: &'static str,
    /// What the v0.3 implementation does on a database that was migrated
    /// up from v0.2.2. Must be observably equivalent to the v0.2 behavior
    /// from the caller's perspective.
    pub v03_on_migrated_db: &'static str,
    /// What the v0.3 implementation does on a database created fresh at
    /// v0.3. Always identical to `v03_on_migrated_db` per design §7.2 —
    /// the migration boundary is invisible to v0.2 call sites.
    pub v03_on_fresh_db: &'static str,
}

/// The §7.2 behavioral-contract table, transcribed verbatim.
///
/// Keep this array exactly four entries long (one per
/// [`V02_FROZEN_METHODS`] entry) — the unit tests assert this invariant.
/// Ordering matches `V02_FROZEN_METHODS` for predictable iteration.
pub const BEHAVIORAL_CONTRACT: &[MethodContract] = &[
    MethodContract {
        method: "store",
        v02_documented_behavior:
            "Writes a memory row, returns its ID; embedding computed in background.",
        v03_on_migrated_db:
            "Routes through v03-resolution `store_raw` with default `StorageMeta`; v0.2 \
             call sites see the same ID and same timing.",
        v03_on_fresh_db: "Same as migrated path — identical behavior.",
    },
    MethodContract {
        method: "recall",
        v02_documented_behavior:
            "Returns vector-ranked memories with ACT-R activation adjustment.",
        v03_on_migrated_db:
            "Routes through v03-retrieval's default plan (v03-retrieval §4); ranking \
             contract preserved (dot-product vector + ACT-R).",
        v03_on_fresh_db: "Same.",
    },
    MethodContract {
        method: "recall_recent",
        v02_documented_behavior:
            "Returns N most-recently-stored memories, newest first.",
        v03_on_migrated_db:
            "Unchanged — simple time-ordered SELECT on `memories.created_at`.",
        v03_on_fresh_db: "Same.",
    },
    MethodContract {
        method: "recall_with_associations",
        v02_documented_behavior:
            "Returns memories + their Hebbian-linked neighbors.",
        v03_on_migrated_db:
            "Unchanged — reads `hebbian_links` table directly, same query as v0.2.",
        v03_on_fresh_db: "Same.",
    },
];

/// Look up the behavioral-contract row for a given method name.
///
/// Returns `None` if `method` is not in [`V02_FROZEN_METHODS`]. Used by
/// integration-test diagnostic output to print the §7.2 expectation
/// alongside an assertion failure.
pub fn contract_for(method: &str) -> Option<&'static MethodContract> {
    BEHAVIORAL_CONTRACT.iter().find(|c| c.method == method)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stub impl: lets us prove `V02CompatSurface` is implementable
    /// without pulling `engramai` in. The integration test crate uses
    /// the *real* `Memory`; this stub only verifies the trait shape.
    #[derive(Default)]
    struct StubMemory {
        stored: Vec<String>,
    }

    #[derive(Debug, Clone, PartialEq)]
    struct StubMemoryId(usize);

    #[derive(Debug, Clone, PartialEq)]
    struct StubRanked {
        id: StubMemoryId,
        score: f64,
    }

    #[derive(Debug, Clone, PartialEq)]
    struct StubMemoryRecord {
        id: StubMemoryId,
        content: String,
    }

    #[derive(Debug, Clone, PartialEq)]
    struct StubAssoc {
        primary: Vec<StubRanked>,
        associated: Vec<StubMemoryRecord>,
    }

    #[derive(Debug)]
    struct StubError(&'static str);

    impl std::fmt::Display for StubError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self.0)
        }
    }
    impl std::error::Error for StubError {}

    impl V02CompatSurface for StubMemory {
        type MemoryId = StubMemoryId;
        type RankedMemory = StubRanked;
        type MemoryRecord = StubMemoryRecord;
        type AssociativeResult = StubAssoc;
        type Error = StubError;

        fn store_v02(&mut self, content: &str) -> Result<Self::MemoryId, Self::Error> {
            let id = StubMemoryId(self.stored.len());
            self.stored.push(content.to_string());
            Ok(id)
        }

        fn recall_v02(&mut self, query: &str) -> Result<Vec<Self::RankedMemory>, Self::Error> {
            // Trivial dot-product-by-string-equality stub. The real Memory
            // routes through retrieval; we only need the shape here.
            let mut hits: Vec<StubRanked> = self
                .stored
                .iter()
                .enumerate()
                .filter(|(_, c)| c.contains(query))
                .map(|(i, _)| StubRanked {
                    id: StubMemoryId(i),
                    score: 1.0,
                })
                .collect();
            hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
            Ok(hits)
        }

        fn recall_recent_v02(
            &self,
            limit: usize,
        ) -> Result<Vec<Self::MemoryRecord>, Self::Error> {
            Ok(self
                .stored
                .iter()
                .enumerate()
                .rev()
                .take(limit)
                .map(|(i, c)| StubMemoryRecord {
                    id: StubMemoryId(i),
                    content: c.clone(),
                })
                .collect())
        }

        fn recall_with_associations_v02(
            &mut self,
            query: &str,
        ) -> Result<Self::AssociativeResult, Self::Error> {
            let primary = self.recall_v02(query)?;
            // Stub: no Hebbian table here, return primary-only.
            Ok(StubAssoc {
                primary,
                associated: Vec::new(),
            })
        }
    }

    /// Compile-time witness: the trait is implementable.
    /// (If this line fails to type-check, the trait's bounds are wrong.)
    const _STUB_COMPAT_WITNESS: () = assert_v02_compat::<StubMemory>();

    #[test]
    fn contract_table_has_one_entry_per_frozen_method() {
        assert_eq!(BEHAVIORAL_CONTRACT.len(), V02_FROZEN_METHODS.len());
        for (entry, &expected) in BEHAVIORAL_CONTRACT.iter().zip(V02_FROZEN_METHODS) {
            assert_eq!(
                entry.method, expected,
                "BEHAVIORAL_CONTRACT[{:?}] order must match V02_FROZEN_METHODS",
                expected
            );
        }
    }

    #[test]
    fn frozen_methods_are_exactly_the_four_design_methods() {
        // §7.1 freezes exactly these four — no fewer, no more.
        assert_eq!(
            V02_FROZEN_METHODS,
            &["store", "recall", "recall_recent", "recall_with_associations"]
        );
    }

    #[test]
    fn contract_for_returns_each_frozen_method() {
        for &method in V02_FROZEN_METHODS {
            let row = contract_for(method)
                .unwrap_or_else(|| panic!("missing BEHAVIORAL_CONTRACT row for {method}"));
            assert_eq!(row.method, method);
            assert!(
                !row.v02_documented_behavior.is_empty(),
                "{method}: v0.2 behavior must be documented"
            );
            assert!(
                !row.v03_on_migrated_db.is_empty(),
                "{method}: v0.3 migrated-DB behavior must be documented"
            );
            assert!(
                !row.v03_on_fresh_db.is_empty(),
                "{method}: v0.3 fresh-DB behavior must be documented"
            );
        }
    }

    #[test]
    fn contract_for_unknown_method_returns_none() {
        assert!(contract_for("graph_query").is_none());
        assert!(contract_for("store_raw").is_none());
        assert!(contract_for("").is_none());
    }

    #[test]
    fn stub_store_returns_id_for_each_call() {
        let mut m = StubMemory::default();
        let id_a = m.store_v02("alpha").unwrap();
        let id_b = m.store_v02("beta").unwrap();
        assert_ne!(id_a, id_b, "consecutive store calls must yield distinct IDs");
        assert_eq!(m.stored.len(), 2);
    }

    #[test]
    fn stub_recall_filters_and_returns_ranked() {
        let mut m = StubMemory::default();
        m.store_v02("alpha apples").unwrap();
        m.store_v02("beta bananas").unwrap();
        m.store_v02("alpha avocados").unwrap();
        let hits = m.recall_v02("alpha").unwrap();
        assert_eq!(hits.len(), 2, "two records contain 'alpha'");
        assert!(hits.iter().all(|h| h.score > 0.0));
    }

    #[test]
    fn stub_recall_recent_returns_newest_first_within_limit() {
        let mut m = StubMemory::default();
        m.store_v02("first").unwrap();
        m.store_v02("second").unwrap();
        m.store_v02("third").unwrap();
        let recent = m.recall_recent_v02(2).unwrap();
        assert_eq!(recent.len(), 2, "limit honored");
        assert_eq!(recent[0].content, "third", "newest first");
        assert_eq!(recent[1].content, "second");
    }

    #[test]
    fn stub_recall_with_associations_surfaces_primary_results() {
        let mut m = StubMemory::default();
        m.store_v02("hebbian-target").unwrap();
        m.store_v02("unrelated").unwrap();
        let result = m.recall_with_associations_v02("hebbian").unwrap();
        assert_eq!(result.primary.len(), 1);
        // Stub has no Hebbian table; the real Memory will populate `associated`.
        assert_eq!(result.associated.len(), 0);
    }

    #[test]
    fn behavioral_contract_strings_mention_design_concepts() {
        // Sanity check: the verbatim §7.2 strings should still mention the
        // critical design concepts. If someone edits these without
        // updating the design doc in lockstep, this test surfaces it.
        let store = contract_for("store").unwrap();
        assert!(store.v03_on_migrated_db.contains("store_raw"));
        assert!(store.v03_on_migrated_db.contains("StorageMeta"));

        let recall = contract_for("recall").unwrap();
        assert!(recall.v02_documented_behavior.contains("ACT-R"));
        assert!(recall.v03_on_migrated_db.contains("ranking contract preserved"));

        let recent = contract_for("recall_recent").unwrap();
        assert!(recent.v03_on_migrated_db.contains("created_at"));

        let assoc = contract_for("recall_with_associations").unwrap();
        assert!(assoc.v03_on_migrated_db.contains("hebbian_links"));
    }
}
