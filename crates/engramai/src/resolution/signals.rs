//! Multi-signal scoring (s1–s8) for entity resolution.
//!
//! Each signal scores the *match* between a [`DraftEntity`] mention and a
//! candidate [`Entity`]. Higher = more likely the same identity.
//!
//! Signals:
//!  - `s1` `SemanticSimilarity`  cosine(mention_embedding, candidate.embedding)
//!  - `s2` `NameMatch`           Jaro-Winkler over (canonical_name, aliases)
//!  - `s3` `GraphContext`        co-mentioned-entity overlap
//!  - `s4` `Recency`             time decay over `last_seen`
//!  - `s5` `Cooccurrence`        Hebbian link strength to other mentions
//!  - `s6` `AffectiveContinuity` valence distance
//!  - `s7` `IdentityHint`        structural hints (same speaker, etc.)
//!  - `s8` `SomaticMatch`        1 − euclidean(mention_fp, candidate_fp)
//!
//! Each scorer returns `Option<f64>`:
//!  - `Some(v)` with `v ∈ [0.0, 1.0]` when the signal could be computed.
//!  - `None` when input data is unavailable (missing embedding, no
//!    fingerprint, no Hebbian record). The fusion stage records the missing
//!    signal in [`crate::resolution::trace::CandidateScore::signals_missing`]
//!    and redistributes its weight (per §3.4.2 / GOAL-2.11).
//!
//! All scorers are **pure** (no IO, no panics, total functions) so they can be
//! unit-tested without a database. Inputs that require IO (embeddings, graph
//! neighborhoods, Hebbian rows) are passed in as parameters; the *fetching*
//! is the fusion driver's responsibility (see `fusion.rs`).
//!
//! See `.gid/features/v03-resolution/design.md` §3.4.2.

use std::collections::HashSet;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::graph::{Entity, SomaticFingerprint};

use super::context::DraftEntity;

/// Stable identifier for one of the eight resolution signals.
///
/// The variant order is `s1..s8` per master DESIGN §4.3 / resolution design
/// §3.4.2. **Append-only** — re-ordering is a breaking change to trace JSON.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Signal {
    /// s1 — cosine similarity over name+summary embeddings.
    SemanticSimilarity,
    /// s2 — Jaro-Winkler over canonical_name and aliases.
    NameMatch,
    /// s3 — co-mentioned-entity overlap between memory and candidate.
    GraphContext,
    /// s4 — time-decayed `last_seen` recency.
    Recency,
    /// s5 — Hebbian co-occurrence with other entities in this memory.
    Cooccurrence,
    /// s6 — valence distance between memory affect and candidate aggregate.
    AffectiveContinuity,
    /// s7 — structural hints (same session speaker, same source path).
    IdentityHint,
    /// s8 — 1 − euclidean over 8-dim somatic fingerprints.
    SomaticMatch,
}

impl Signal {
    /// All eight signals in canonical (s1..s8) order. Useful for iterating
    /// weight maps or asserting completeness in tests.
    pub const ALL: [Signal; 8] = [
        Signal::SemanticSimilarity,
        Signal::NameMatch,
        Signal::GraphContext,
        Signal::Recency,
        Signal::Cooccurrence,
        Signal::AffectiveContinuity,
        Signal::IdentityHint,
        Signal::SomaticMatch,
    ];

    /// Stable string label used in metric tags / DB rows.
    pub fn as_str(&self) -> &'static str {
        match self {
            Signal::SemanticSimilarity => "semantic_similarity",
            Signal::NameMatch => "name_match",
            Signal::GraphContext => "graph_context",
            Signal::Recency => "recency",
            Signal::Cooccurrence => "cooccurrence",
            Signal::AffectiveContinuity => "affective_continuity",
            Signal::IdentityHint => "identity_hint",
            Signal::SomaticMatch => "somatic_match",
        }
    }
}

/// Clamp a float into `[0.0, 1.0]`, mapping `NaN` to `0.0`.
///
/// Used by every scorer as the last step so callers can treat outputs as
/// well-formed probabilities without re-checking.
fn clamp01(x: f64) -> f64 {
    if x.is_nan() {
        0.0
    } else {
        x.clamp(0.0, 1.0)
    }
}

// ============================================================
// s1 — SemanticSimilarity
// ============================================================

/// Cosine similarity between two embedding vectors mapped into `[0, 1]`
/// (cosine ranges in `[-1, 1]` so we use `0.5 * (cos + 1)`).
///
/// Returns `None` when:
///  - either vector is empty,
///  - vectors have mismatched lengths,
///  - either vector has zero norm (would produce NaN).
pub fn semantic_similarity(mention_emb: Option<&[f32]>, candidate_emb: Option<&[f32]>) -> Option<f64> {
    let a = mention_emb?;
    let b = candidate_emb?;
    if a.is_empty() || a.len() != b.len() {
        return None;
    }
    let mut dot = 0.0_f64;
    let mut na = 0.0_f64;
    let mut nb = 0.0_f64;
    for (x, y) in a.iter().zip(b.iter()) {
        let xf = *x as f64;
        let yf = *y as f64;
        dot += xf * yf;
        na += xf * xf;
        nb += yf * yf;
    }
    if na == 0.0 || nb == 0.0 {
        return None;
    }
    let cos = dot / (na.sqrt() * nb.sqrt());
    Some(clamp01(0.5 * (cos + 1.0)))
}

// ============================================================
// s2 — NameMatch
// ============================================================

/// Best Jaro-Winkler similarity between the mention's normalized aliases /
/// canonical_name and the candidate's canonical_name + aliases.
///
/// We take the **maximum** similarity over all (mention_str × candidate_str)
/// pairs. Rationale: an entity with multiple aliases ("OpenAI", "OAI") should
/// score high on either spelling; we don't want to penalize for having other
/// aliases.
///
/// Returns `None` only when both sides are empty (no strings to compare). An
/// empty single side still allows comparison against the canonical_name.
pub fn name_match(
    mention: &DraftEntity,
    candidate: &Entity,
    candidate_aliases: &[String],
) -> Option<f64> {
    let mut mention_strs: Vec<&str> = Vec::new();
    if !mention.canonical_name.is_empty() {
        mention_strs.push(mention.canonical_name.as_str());
    }
    mention_strs.extend(mention.aliases.iter().map(|s| s.as_str()));

    let mut cand_strs: Vec<&str> = Vec::new();
    if !candidate.canonical_name.is_empty() {
        cand_strs.push(candidate.canonical_name.as_str());
    }
    cand_strs.extend(candidate_aliases.iter().map(|s| s.as_str()));

    if mention_strs.is_empty() || cand_strs.is_empty() {
        return None;
    }

    let mut best = 0.0_f64;
    for m in &mention_strs {
        for c in &cand_strs {
            // strsim::jaro_winkler returns f64 in [0, 1]; case-fold first
            // because mentions are user-text and casing is uninformative.
            let m_lc = m.to_lowercase();
            let c_lc = c.to_lowercase();
            let s = strsim::jaro_winkler(&m_lc, &c_lc);
            if s > best {
                best = s;
            }
        }
    }
    Some(clamp01(best))
}

// ============================================================
// s3 — GraphContext
// ============================================================

/// Jaccard overlap between the set of *other* entity names mentioned in this
/// memory and the names of entities currently linked to `candidate`'s
/// neighborhood.
///
/// Returns `None` when **both** sets are empty (no signal). Returns `Some(0.0)`
/// when one set is non-empty and they share no overlap (informative zero —
/// "we have context but it doesn't match").
pub fn graph_context(
    other_mention_names: &[String],
    candidate_neighborhood_names: &[String],
) -> Option<f64> {
    if other_mention_names.is_empty() && candidate_neighborhood_names.is_empty() {
        return None;
    }
    let lhs: HashSet<String> = other_mention_names
        .iter()
        .map(|s| s.to_lowercase())
        .collect();
    let rhs: HashSet<String> = candidate_neighborhood_names
        .iter()
        .map(|s| s.to_lowercase())
        .collect();
    if lhs.is_empty() && rhs.is_empty() {
        return None;
    }
    let inter = lhs.intersection(&rhs).count() as f64;
    let union = lhs.union(&rhs).count() as f64;
    if union == 0.0 {
        return Some(0.0);
    }
    Some(clamp01(inter / union))
}

// ============================================================
// s4 — Recency
// ============================================================

/// Half-life used by [`recency`]. 30 days matches the v0.2 working-memory
/// decay window — recent entities should look "hot", month-old ones "cool".
pub const DEFAULT_RECENCY_HALF_LIFE: Duration = Duration::days(30);

/// Exponential-decay recency score: `2^(-Δt / half_life)`.
///
/// `now` is typically `ctx.memory.occurred_at` (the moment we are resolving
/// for, not wall-clock-now — re-extracts must score recency relative to the
/// memory's own time, per GOAL-2.1 idempotence).
///
/// Returns `None` only if `candidate.last_seen` is in the future relative to
/// `now` (a clock skew or test-fixture problem) — we don't want to "pump up"
/// the recency of future-dated entities silently.
pub fn recency(
    now: DateTime<Utc>,
    candidate: &Entity,
    half_life: Duration,
) -> Option<f64> {
    let dt = now.signed_duration_since(candidate.last_seen);
    if dt < Duration::zero() {
        return None;
    }
    let dt_secs = dt.num_milliseconds() as f64 / 1000.0;
    let hl_secs = half_life.num_milliseconds() as f64 / 1000.0;
    if hl_secs <= 0.0 {
        return None;
    }
    let score = 2.0_f64.powf(-dt_secs / hl_secs);
    Some(clamp01(score))
}

// ============================================================
// s5 — Cooccurrence (Hebbian)
// ============================================================

/// Average Hebbian link strength between the candidate's id and each of
/// `cooccurring_entity_ids`. Strengths are expected in `[0, 1]`.
///
/// Returns `None` when no co-occurring entities exist (empty input). Returns
/// `Some(0.0)` when entities exist but none have prior Hebbian links to the
/// candidate (informative zero).
pub fn cooccurrence(strengths_to_candidate: &[f64]) -> Option<f64> {
    if strengths_to_candidate.is_empty() {
        return None;
    }
    let n = strengths_to_candidate.len() as f64;
    let sum: f64 = strengths_to_candidate.iter().filter(|x| !x.is_nan()).sum();
    Some(clamp01(sum / n))
}

// ============================================================
// s6 — AffectiveContinuity
// ============================================================

/// Distance between the memory's valence (`memory_valence`, in `[-1, 1]`) and
/// the candidate's aggregate valence (`candidate_valence`, in `[-1, 1]`),
/// inverted so closer = higher score.
///
/// Distance is normalized: `|a - b| / 2` lives in `[0, 1]`, so the score is
/// `1 - |a - b| / 2`.
///
/// Returns `None` when either valence is missing (the candidate has no
/// aggregate, or the memory has no affect snapshot for valence extraction).
pub fn affective_continuity(
    memory_valence: Option<f64>,
    candidate_valence: Option<f64>,
) -> Option<f64> {
    let a = memory_valence?.clamp(-1.0, 1.0);
    let b = candidate_valence?.clamp(-1.0, 1.0);
    let d = (a - b).abs() / 2.0;
    Some(clamp01(1.0 - d))
}

// ============================================================
// s7 — IdentityHint
// ============================================================

/// Boolean structural hints — same session speaker, same file path origin,
/// same external id. We accept a vector of booleans (the resolution driver
/// computes them from session/source metadata) and score `present / total`.
///
/// Returns `None` when no hints were checked (caller passed an empty slice —
/// genuinely no hint data, vs. all-false which is informative zero).
pub fn identity_hint(hints: &[bool]) -> Option<f64> {
    if hints.is_empty() {
        return None;
    }
    let n = hints.len() as f64;
    let hits = hints.iter().filter(|b| **b).count() as f64;
    Some(clamp01(hits / n))
}

// ============================================================
// s8 — SomaticMatch
// ============================================================

/// `1 − euclidean(mention_fp, candidate_fp) / max_distance` over the 8-dim
/// somatic fingerprint, where `max_distance = sqrt(8)` (each component lives
/// in `[0, 1]`).
///
/// Returns `None` when either side has no fingerprint.
pub fn somatic_match(
    mention_fp: Option<&SomaticFingerprint>,
    candidate_fp: Option<&SomaticFingerprint>,
) -> Option<f64> {
    let a = mention_fp?;
    let b = candidate_fp?;
    let mut acc = 0.0_f64;
    for (x, y) in a.0.iter().zip(b.0.iter()) {
        let d = (*x as f64) - (*y as f64);
        acc += d * d;
    }
    let dist = acc.sqrt();
    let max_dist = (a.0.len() as f64).sqrt();
    if max_dist == 0.0 {
        return None;
    }
    let normalized = (dist / max_dist).clamp(0.0, 1.0);
    Some(clamp01(1.0 - normalized))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{EntityKind, SomaticFingerprint};
    use chrono::TimeZone;
    use uuid::Uuid;

    fn make_entity(name: &str, last_seen: DateTime<Utc>) -> Entity {
        Entity {
            id: Uuid::new_v4(),
            canonical_name: name.into(),
            kind: EntityKind::Person,
            summary: String::new(),
            attributes: serde_json::json!({}),
            history: Vec::new(),
            merged_into: None,
            first_seen: last_seen,
            last_seen,
            created_at: last_seen,
            updated_at: last_seen,
            episode_mentions: Vec::new(),
            memory_mentions: Vec::new(),
            activation: 0.0,
            agent_affect: None,
            arousal: 0.0,
            importance: 0.0,
            identity_confidence: 0.5,
            somatic_fingerprint: None,
            embedding: None,
        }
    }

    fn make_draft(name: &str) -> DraftEntity {
        DraftEntity {
            canonical_name: name.into(),
            kind: EntityKind::Person,
            aliases: Vec::new(),
            subtype_hint: None,
            first_seen: Utc::now(),
            last_seen: Utc::now(),
            somatic_fingerprint: None,
        }
    }

    // ----- signal enum -----
    #[test]
    fn signal_all_has_eight_unique_in_order() {
        assert_eq!(Signal::ALL.len(), 8);
        let mut seen = HashSet::new();
        for s in Signal::ALL {
            assert!(seen.insert(s));
        }
        assert_eq!(Signal::ALL[0], Signal::SemanticSimilarity);
        assert_eq!(Signal::ALL[7], Signal::SomaticMatch);
    }

    #[test]
    fn signal_as_str_stable_labels() {
        assert_eq!(Signal::NameMatch.as_str(), "name_match");
        assert_eq!(Signal::SomaticMatch.as_str(), "somatic_match");
    }

    // ----- s1 semantic_similarity -----
    #[test]
    fn s1_identical_vectors_score_one() {
        let v = [1.0, 0.0, 0.0];
        let s = semantic_similarity(Some(&v), Some(&v)).unwrap();
        assert!((s - 1.0).abs() < 1e-6);
    }

    #[test]
    fn s1_orthogonal_vectors_score_half() {
        let a = [1.0, 0.0];
        let b = [0.0, 1.0];
        let s = semantic_similarity(Some(&a), Some(&b)).unwrap();
        assert!((s - 0.5).abs() < 1e-6);
    }

    #[test]
    fn s1_opposite_vectors_score_zero() {
        let a = [1.0, 0.0];
        let b = [-1.0, 0.0];
        let s = semantic_similarity(Some(&a), Some(&b)).unwrap();
        assert!(s < 1e-6);
    }

    #[test]
    fn s1_missing_or_mismatched_returns_none() {
        assert!(semantic_similarity(None, Some(&[1.0])).is_none());
        assert!(semantic_similarity(Some(&[1.0, 2.0]), Some(&[1.0])).is_none());
        let zero = [0.0, 0.0];
        assert!(semantic_similarity(Some(&zero), Some(&zero)).is_none());
        let empty: [f32; 0] = [];
        assert!(semantic_similarity(Some(&empty), Some(&empty)).is_none());
    }

    // ----- s2 name_match -----
    #[test]
    fn s2_exact_match_high() {
        let m = make_draft("Alice Smith");
        let c = make_entity("Alice Smith", Utc::now());
        let s = name_match(&m, &c, &[]).unwrap();
        assert!(s > 0.99, "exact match should be ≈1.0, got {s}");
    }

    #[test]
    fn s2_case_insensitive() {
        let m = make_draft("ALICE");
        let c = make_entity("alice", Utc::now());
        let s = name_match(&m, &c, &[]).unwrap();
        assert!(s > 0.99);
    }

    #[test]
    fn s2_alias_boosts_match() {
        let m = make_draft("OAI");
        let c = make_entity("OpenAI", Utc::now());
        let s_no_alias = name_match(&m, &c, &[]).unwrap();
        let s_with_alias = name_match(&m, &c, &["OAI".into()]).unwrap();
        assert!(s_with_alias > s_no_alias);
        assert!(s_with_alias > 0.99);
    }

    #[test]
    fn s2_unrelated_low() {
        let m = make_draft("Alice");
        let c = make_entity("Zebra", Utc::now());
        let s = name_match(&m, &c, &[]).unwrap();
        assert!(s < 0.5, "unrelated names should score low, got {s}");
    }

    // ----- s3 graph_context -----
    #[test]
    fn s3_full_overlap_one() {
        let lhs = vec!["bob".into(), "carol".into()];
        let rhs = vec!["Bob".into(), "Carol".into()];
        assert!((graph_context(&lhs, &rhs).unwrap() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn s3_no_overlap_zero_when_present() {
        let lhs = vec!["alice".into()];
        let rhs = vec!["bob".into()];
        assert_eq!(graph_context(&lhs, &rhs).unwrap(), 0.0);
    }

    #[test]
    fn s3_both_empty_none() {
        let empty: Vec<String> = Vec::new();
        assert!(graph_context(&empty, &empty).is_none());
    }

    // ----- s4 recency -----
    #[test]
    fn s4_just_now_scores_one() {
        let now = Utc.with_ymd_and_hms(2025, 6, 1, 0, 0, 0).unwrap();
        let cand = make_entity("x", now);
        let s = recency(now, &cand, DEFAULT_RECENCY_HALF_LIFE).unwrap();
        assert!((s - 1.0).abs() < 1e-9);
    }

    #[test]
    fn s4_one_half_life_scores_half() {
        let now = Utc.with_ymd_and_hms(2025, 6, 1, 0, 0, 0).unwrap();
        let cand = make_entity("x", now - Duration::days(30));
        let s = recency(now, &cand, DEFAULT_RECENCY_HALF_LIFE).unwrap();
        assert!((s - 0.5).abs() < 1e-3);
    }

    #[test]
    fn s4_far_past_scores_near_zero() {
        let now = Utc.with_ymd_and_hms(2025, 6, 1, 0, 0, 0).unwrap();
        let cand = make_entity("x", now - Duration::days(365));
        let s = recency(now, &cand, DEFAULT_RECENCY_HALF_LIFE).unwrap();
        assert!(s < 0.001, "year-old entity should be near zero, got {s}");
    }

    #[test]
    fn s4_future_dated_returns_none() {
        let now = Utc.with_ymd_and_hms(2025, 6, 1, 0, 0, 0).unwrap();
        let cand = make_entity("x", now + Duration::days(1));
        assert!(recency(now, &cand, DEFAULT_RECENCY_HALF_LIFE).is_none());
    }

    // ----- s5 cooccurrence -----
    #[test]
    fn s5_average_strength() {
        let s = cooccurrence(&[0.4, 0.6, 0.8]).unwrap();
        assert!((s - 0.6).abs() < 1e-9);
    }

    #[test]
    fn s5_empty_returns_none() {
        assert!(cooccurrence(&[]).is_none());
    }

    // ----- s6 affective_continuity -----
    #[test]
    fn s6_same_valence_scores_one() {
        let s = affective_continuity(Some(0.5), Some(0.5)).unwrap();
        assert!((s - 1.0).abs() < 1e-9);
    }

    #[test]
    fn s6_opposite_valence_scores_zero() {
        let s = affective_continuity(Some(1.0), Some(-1.0)).unwrap();
        assert!(s < 1e-9);
    }

    #[test]
    fn s6_missing_either_returns_none() {
        assert!(affective_continuity(None, Some(0.0)).is_none());
        assert!(affective_continuity(Some(0.0), None).is_none());
    }

    // ----- s7 identity_hint -----
    #[test]
    fn s7_all_true_scores_one() {
        assert_eq!(identity_hint(&[true, true, true]).unwrap(), 1.0);
    }

    #[test]
    fn s7_partial_true_scores_fraction() {
        let s = identity_hint(&[true, false, false, true]).unwrap();
        assert!((s - 0.5).abs() < 1e-9);
    }

    #[test]
    fn s7_empty_returns_none() {
        assert!(identity_hint(&[]).is_none());
    }

    // ----- s8 somatic_match -----
    #[test]
    fn s8_identical_fingerprints_score_one() {
        let fp = SomaticFingerprint([0.5; 8]);
        let s = somatic_match(Some(&fp), Some(&fp)).unwrap();
        assert!((s - 1.0).abs() < 1e-9);
    }

    #[test]
    fn s8_max_distance_scores_zero() {
        let a = SomaticFingerprint([0.0; 8]);
        let b = SomaticFingerprint([1.0; 8]);
        let s = somatic_match(Some(&a), Some(&b)).unwrap();
        assert!(s < 1e-6, "max-distance fingerprints should score 0, got {s}");
    }

    #[test]
    fn s8_missing_returns_none() {
        let fp = SomaticFingerprint([0.5; 8]);
        assert!(somatic_match(None, Some(&fp)).is_none());
        assert!(somatic_match(Some(&fp), None).is_none());
    }

    // ----- clamp behavior on NaN -----
    #[test]
    fn clamp_handles_nan_and_out_of_range() {
        assert_eq!(clamp01(f64::NAN), 0.0);
        assert_eq!(clamp01(-0.5), 0.0);
        assert_eq!(clamp01(1.5), 1.0);
        assert_eq!(clamp01(0.7), 0.7);
    }
}
