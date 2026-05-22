//! ReextractReport — v0.3 retry surface contract (ISS-042).
//!
//! `ReextractReport` is the value returned from `Memory::reextract_episodes`
//! (see v03-graph-layer design §5). The graph-layer method is a thin shim:
//! the actual retry loop lives in the resolution worker pool
//! (`task:res-impl-worker`). This struct is the contract between the worker
//! and any caller driving a retry batch.
//!
//! ## Why this matters
//!
//! v0.3 resolution has three failure modes a caller must distinguish:
//!
//! 1. **Succeeded** — the episode was re-extracted in this pass. Edges /
//!    entities are now in the graph.
//! 2. **Still failed** — re-extraction ran but errored. The caller may
//!    retry later, escalate, or drop the episode. The error string is
//!    surfaced verbatim (GOAL-2.2 / 2.3 — failures must be visible, not
//!    silently swallowed).
//! 3. **Skipped (idempotent)** — the episode was already resolved on a
//!    prior pass. No work was done this time. This is the GOAL-2.1
//!    idempotence guarantee: re-calling `reextract_episodes` on an
//!    already-resolved id is a no-op, not a re-run that double-writes
//!    entities.
//!
//! `requested` is the input batch size, kept so callers can sanity-check
//! `succeeded.len() + still_failed.len() + skipped_idempotent.len() ==
//! requested` without re-counting their own input slice.
//!
//! ## Wire format
//!
//! All fields serialize. `Vec` fields serialize as JSON arrays even when
//! empty (no `skip_serializing_if`) — callers parsing the report should not
//! have to special-case absent fields. `deny_unknown_fields` is intentionally
//! NOT set: this is an internal contract, not a public API surface, and the
//! resolution worker may extend the report shape (e.g. add per-episode
//! timing) without breaking older callers.
//!
//! ## Consumer
//!
//! The consumer is [`Memory::reextract_episodes`] (shipped in ISS-133):
//! given a `Vec<Uuid>` of memory ids, that method enqueues a retry job
//! for each, polls until terminal, and returns this report with the
//! requested ids bucketed into `succeeded` / `still_failed` /
//! `skipped_idempotent`.
//!
//! [`Memory::reextract_episodes`]: ../../memory/struct.Memory.html#method.reextract_episodes
//!
//! ## Example
//!
//! ```
//! use engramai::resolution::ReextractReport;
//! use uuid::Uuid;
//!
//! let succeeded = Uuid::new_v4();
//! let failed = Uuid::new_v4();
//! let skipped = Uuid::new_v4();
//!
//! let report = ReextractReport {
//!     requested: 3,
//!     succeeded: vec![succeeded],
//!     still_failed: vec![(failed, "extractor timeout".into())],
//!     skipped_idempotent: vec![skipped],
//! };
//!
//! assert_eq!(report.total_processed(), 3);
//! assert_eq!(report.requested, report.total_processed());
//! ```

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Report returned from a re-extraction batch (ISS-042).
///
/// See module-level docs for field semantics and idempotence guarantees.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ReextractReport {
    /// Input batch size (number of episode ids passed in).
    pub requested: usize,

    /// Episode ids that were successfully re-extracted this pass.
    pub succeeded: Vec<Uuid>,

    /// Episode ids that errored. The string is the failure reason as
    /// surfaced by the resolution worker — verbatim, not interpreted.
    pub still_failed: Vec<(Uuid, String)>,

    /// Episode ids that were already resolved and so no work was done
    /// (GOAL-2.1 idempotence). Distinct from `succeeded`: the caller can
    /// see "no-op" vs "actually retried" to drive metrics or back-off.
    pub skipped_idempotent: Vec<Uuid>,
}

impl ReextractReport {
    /// Empty report for `requested` episodes — convenience for the worker
    /// to start a batch and accumulate results into.
    pub fn new(requested: usize) -> Self {
        Self {
            requested,
            succeeded: Vec::new(),
            still_failed: Vec::new(),
            skipped_idempotent: Vec::new(),
        }
    }

    /// Total episodes accounted for across all three buckets.
    ///
    /// Should equal `requested` after the worker finishes the batch; a
    /// mismatch means the worker dropped an episode without classifying it
    /// (a bug). Callers can `assert_eq!(report.total_processed(),
    /// report.requested)` as a defensive check.
    pub fn total_processed(&self) -> usize {
        self.succeeded.len() + self.still_failed.len() + self.skipped_idempotent.len()
    }

    /// True if every requested episode was classified.
    pub fn is_complete(&self) -> bool {
        self.total_processed() == self.requested
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_starts_empty() {
        let r = ReextractReport::new(5);
        assert_eq!(r.requested, 5);
        assert!(r.succeeded.is_empty());
        assert!(r.still_failed.is_empty());
        assert!(r.skipped_idempotent.is_empty());
        assert_eq!(r.total_processed(), 0);
        assert!(!r.is_complete()); // 5 requested, 0 processed
    }

    #[test]
    fn default_is_zero_requested_and_complete() {
        let r = ReextractReport::default();
        assert_eq!(r.requested, 0);
        assert_eq!(r.total_processed(), 0);
        // 0 requested + 0 processed = trivially complete.
        assert!(r.is_complete());
    }

    #[test]
    fn total_processed_sums_all_buckets() {
        let r = ReextractReport {
            requested: 3,
            succeeded: vec![Uuid::new_v4()],
            still_failed: vec![(Uuid::new_v4(), "boom".into())],
            skipped_idempotent: vec![Uuid::new_v4()],
        };
        assert_eq!(r.total_processed(), 3);
        assert!(r.is_complete());
    }

    #[test]
    fn is_complete_false_when_buckets_undercounted() {
        let r = ReextractReport {
            requested: 5,
            succeeded: vec![Uuid::new_v4()],
            still_failed: vec![],
            skipped_idempotent: vec![],
        };
        assert!(!r.is_complete());
    }

    #[test]
    fn idempotent_skip_is_not_success() {
        // GOAL-2.1: skipped_idempotent and succeeded must be distinct
        // buckets. A caller asking "did anything get re-extracted?" must
        // be able to answer accurately.
        let id = Uuid::new_v4();
        let r = ReextractReport {
            requested: 1,
            succeeded: vec![],
            still_failed: vec![],
            skipped_idempotent: vec![id],
        };
        assert!(!r.succeeded.contains(&id));
        assert!(r.skipped_idempotent.contains(&id));
    }

    #[test]
    fn still_failed_carries_error_reason() {
        // GOAL-2.2 / 2.3: failures must surface a reason, not just the id.
        let id = Uuid::new_v4();
        let r = ReextractReport {
            requested: 1,
            succeeded: vec![],
            still_failed: vec![(id, "extractor timeout".into())],
            skipped_idempotent: vec![],
        };
        assert_eq!(r.still_failed.len(), 1);
        assert_eq!(r.still_failed[0].0, id);
        assert_eq!(r.still_failed[0].1, "extractor timeout");
    }

    #[test]
    fn serde_roundtrip_preserves_all_fields() {
        // ISS-042 AC: serde round-trip tested.
        let original = ReextractReport {
            requested: 4,
            succeeded: vec![Uuid::new_v4(), Uuid::new_v4()],
            still_failed: vec![(Uuid::new_v4(), "bad input".into())],
            skipped_idempotent: vec![Uuid::new_v4()],
        };
        let json = serde_json::to_string(&original).expect("serialize");
        let decoded: ReextractReport =
            serde_json::from_str(&json).expect("deserialize");
        assert_eq!(original, decoded);
    }

    #[test]
    fn serde_empty_buckets_roundtrip() {
        // Empty Vecs serialize as `[]`, not omitted. Round-trip preserves
        // them so callers can distinguish "no data" from "missing field".
        let original = ReextractReport::new(0);
        let json = serde_json::to_string(&original).expect("serialize");
        assert!(json.contains("\"succeeded\":[]"));
        assert!(json.contains("\"still_failed\":[]"));
        assert!(json.contains("\"skipped_idempotent\":[]"));
        let decoded: ReextractReport =
            serde_json::from_str(&json).expect("deserialize");
        assert_eq!(original, decoded);
    }
}
