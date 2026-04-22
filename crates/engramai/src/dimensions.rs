//! Dimensional signature of a memory — the typed replacement for
//! raw JSON metadata blobs.
//!
//! Every memory row in the main `memories` table carries a `Dimensions`
//! value. Narrative fields are `Option<_>` (absent by design if the
//! extractor didn't find them); scalar fields always have defaults.
//!
//! Construction is the only place these fields can be missing —
//! once a `Dimensions` is in the system, every later operation is
//! expressed in terms of this type, not raw JSON.
//!
//! See design §2.1, §2.2 of ISS-019.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use thiserror::Error;

use crate::type_weights::TypeWeights;

// ---------------------------------------------------------------------
// NonEmptyString — core_fact guard
// ---------------------------------------------------------------------

/// A string guaranteed to be non-empty and non-whitespace-only.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct NonEmptyString(String);

#[derive(Debug, Clone, Error, PartialEq, Eq)]
#[error("core_fact must be non-empty (got empty or whitespace-only string)")]
pub struct EmptyCoreFactError;

impl NonEmptyString {
    pub fn new(s: impl Into<String>) -> Result<Self, EmptyCoreFactError> {
        let s = s.into();
        if s.trim().is_empty() {
            Err(EmptyCoreFactError)
        } else {
            Ok(Self(s))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl AsRef<str> for NonEmptyString {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

// Custom Deserialize: enforce non-empty invariant when decoding from JSON.
impl<'de> Deserialize<'de> for NonEmptyString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::new(s).map_err(serde::de::Error::custom)
    }
}

// ---------------------------------------------------------------------
// Valence — clamped scalar [-1.0, 1.0]
// ---------------------------------------------------------------------

/// Emotional valence, clamped to [-1.0, 1.0]. The only constructor
/// clamps — eliminates scattered `.clamp()` calls in extractor paths.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Valence(f64);

impl Valence {
    pub const ZERO: Valence = Valence(0.0);

    pub fn new(v: f64) -> Self {
        // NaN maps to 0.0; infinities clamp to ±1.0.
        let v = if v.is_nan() { 0.0 } else { v.clamp(-1.0, 1.0) };
        Self(v)
    }

    pub fn get(self) -> f64 {
        self.0
    }
}

impl Default for Valence {
    fn default() -> Self {
        Self::ZERO
    }
}

impl<'de> Deserialize<'de> for Valence {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let v = f64::deserialize(deserializer)?;
        Ok(Self::new(v))
    }
}

// ---------------------------------------------------------------------
// Importance — clamped scalar [0.0, 1.0]
// ---------------------------------------------------------------------

/// Importance score, clamped to [0.0, 1.0].
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct Importance(f64);

impl Importance {
    pub const ZERO: Importance = Importance(0.0);
    pub const DEFAULT: Importance = Importance(0.5);

    pub fn new(v: f64) -> Self {
        let v = if v.is_nan() { 0.5 } else { v.clamp(0.0, 1.0) };
        Self(v)
    }

    pub fn get(self) -> f64 {
        self.0
    }
}

impl Default for Importance {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl<'de> Deserialize<'de> for Importance {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let v = f64::deserialize(deserializer)?;
        Ok(Self::new(v))
    }
}

// ---------------------------------------------------------------------
// Domain — typed enum replacing free-string domain
// ---------------------------------------------------------------------

/// Memory domain. Common cases are typed; anything else goes in `Other`
/// without requiring a code change.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum Domain {
    Coding,
    Trading,
    Research,
    Communication,
    #[default]
    General,
    Other(String),
}

impl Domain {
    /// Is this a concrete variant (not `Other`)?
    pub fn is_concrete(&self) -> bool {
        !matches!(self, Domain::Other(_))
    }

    /// Parse the free-form string used by `ExtractedFact::domain`.
    /// Unknown strings become `Other(s)`; empty becomes `General`.
    pub fn from_loose_str(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "general" => Domain::General,
            "coding" | "code" => Domain::Coding,
            "trading" | "trade" => Domain::Trading,
            "research" => Domain::Research,
            "communication" | "comms" => Domain::Communication,
            _ => Domain::Other(s.trim().to_string()),
        }
    }
}

// ---------------------------------------------------------------------
// Confidence — ordered enum (Uncertain < Likely < Confident)
// ---------------------------------------------------------------------

/// Extraction confidence. `Ord` is Uncertain < Likely < Confident,
/// so `max` picks the most confident and `min` the most cautious.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    #[default]
    Uncertain,
    Likely,
    Confident,
}

impl Confidence {
    /// Parse the loose strings emitted by `ExtractedFact::confidence`.
    /// Unknown values default to `Uncertain` (cautious).
    pub fn from_loose_str(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "confident" => Confidence::Confident,
            "likely" => Confidence::Likely,
            _ => Confidence::Uncertain,
        }
    }
}

// ---------------------------------------------------------------------
// TemporalMark — typed temporal precision
// ---------------------------------------------------------------------

/// Temporal reference with precision tracking. The extractor produces
/// temporal strings of varying precision; parsing at extraction time
/// lets `Dimensions::union()` pick the more precise one without
/// string-length heuristics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum TemporalMark {
    /// Timestamp accurate to seconds.
    Exact(DateTime<Utc>),
    /// Calendar day.
    Day(NaiveDate),
    /// A closed range of days.
    Range { start: NaiveDate, end: NaiveDate },
    /// Unparseable natural-language temporal reference.
    Vague(String),
}

impl TemporalMark {
    /// Precision ordering (higher = more precise): Exact > Range > Day > Vague.
    ///
    /// Matches design §2.2. Used by `Dimensions::union` to prefer the
    /// more precise mark when both inputs are present.
    pub fn precision_rank(&self) -> u8 {
        match self {
            TemporalMark::Exact(_) => 4,
            TemporalMark::Range { .. } => 3,
            TemporalMark::Day(_) => 2,
            TemporalMark::Vague(_) => 1,
        }
    }
}

// ---------------------------------------------------------------------
// Dimensions — the typed metadata signature
// ---------------------------------------------------------------------

/// The complete dimensional signature of a memory.
///
/// See design §2.1. Invariant: `core_fact` is non-empty (enforced
/// at construction by `NonEmptyString`). Every later operation on
/// a memory is expressed in terms of `Dimensions`, not raw JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Dimensions {
    // Core (required, non-empty)
    pub core_fact: NonEmptyString,

    // Narrative dimensions — Option<_> means "extractor didn't find one"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub participants: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temporal: Option<TemporalMark>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub causation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relations: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sentiment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stance: Option<String>,

    // Scalars — always present
    #[serde(default)]
    pub valence: Valence,
    #[serde(default)]
    pub domain: Domain,
    #[serde(default)]
    pub confidence: Confidence,
    #[serde(default)]
    pub tags: BTreeSet<String>,

    // Inferred type affinity (default = all-ones, no bias)
    #[serde(default)]
    pub type_weights: TypeWeights,
}

impl Dimensions {
    /// Build a minimal `Dimensions` from content only.
    ///
    /// Used when no extractor is configured, or during migration of
    /// pre-extractor rows (FINDING-4 / FINDING-5).
    ///
    /// `core_fact = content`; all narrative dimensions `None`; scalars
    /// at their defaults (valence 0.0, domain General, confidence
    /// Uncertain, tags empty, type_weights all 1.0).
    ///
    /// This is legitimate low-dimensional memory, not an error state.
    pub fn minimal(content: &str) -> Result<Self, EmptyCoreFactError> {
        Ok(Self {
            core_fact: NonEmptyString::new(content)?,
            participants: None,
            temporal: None,
            location: None,
            context: None,
            causation: None,
            outcome: None,
            method: None,
            relations: None,
            sentiment: None,
            stance: None,
            valence: Valence::ZERO,
            domain: Domain::General,
            confidence: Confidence::Uncertain,
            tags: BTreeSet::new(),
            type_weights: TypeWeights::default(),
        })
    }
}

// ---------------------------------------------------------------------
// Dimensions::union — merge two signatures without information loss.
// See design §5.1 and §5.3 of ISS-019.
// ---------------------------------------------------------------------

impl Dimensions {
    /// Union two dimensional signatures.
    ///
    /// `self` is the **existing** side and `other` is the **incoming**
    /// side — the operation is non-commutative by design (§5.3): for
    /// speaker-specific fields (`sentiment`, `stance`) the existing
    /// side wins when both are populated.
    ///
    /// Invariants guaranteed (proptest-asserted):
    /// - Idempotence: `a.union(a, w).eq(&a)` (for any `w`).
    /// - Associativity (modulo consistent weights).
    /// - Monotonicity: `info_content(union) ≥ max(info_content(a), info_content(b))`.
    ///
    /// Per-field rules (see design §5.1):
    ///
    /// - `core_fact`: longer wins (proxy for fuller extraction).
    /// - `participants`: set-union of comma-separated names.
    /// - `temporal`: higher `precision_rank` wins; tie → existing.
    /// - `location` / `context` / `causation` / `outcome` / `method`:
    ///   longer wins; tie → existing.
    /// - `relations`: set-union (semicolon-separated).
    /// - `sentiment`: existing wins if present, else incoming fills the `None`.
    /// - `stance`: same as `sentiment`.
    /// - `valence`: importance-weighted average, clamped.
    /// - `domain` (FINDING-6):
    ///   1. concrete variant beats `Other(_)` regardless of side;
    ///   2. two concretes → existing wins (stable);
    ///   3. two `Other(_)` → longer string wins; tie → existing.
    /// - `confidence`: `min` (most cautious).
    /// - `tags`: `BTreeSet` union.
    /// - `type_weights`: per-variant `max` (affinity never decays).
    pub fn union(self, other: Dimensions, weights: crate::merge_types::MergeWeights) -> Dimensions {
        let core_fact = pick_longer_non_empty(self.core_fact, other.core_fact);

        let participants = merge_csv_set(self.participants, other.participants, ',');
        let temporal = pick_higher_temporal(self.temporal, other.temporal);
        let location = pick_longer(self.location, other.location);
        let context = pick_longer(self.context, other.context);
        let causation = pick_longer(self.causation, other.causation);
        let outcome = pick_longer(self.outcome, other.outcome);
        let method = pick_longer(self.method, other.method);
        let relations = merge_csv_set(self.relations, other.relations, ';');

        // sentiment / stance: existing wins if Some, else fall back to incoming.
        let sentiment = self.sentiment.or(other.sentiment);
        let stance = self.stance.or(other.stance);

        let valence = weighted_valence(self.valence, other.valence, weights);
        let domain = merge_domain(self.domain, other.domain);
        let confidence = self.confidence.min(other.confidence);

        let mut tags = self.tags;
        tags.extend(other.tags);

        let type_weights = merge_type_weights(self.type_weights, other.type_weights);

        Dimensions {
            core_fact,
            participants,
            temporal,
            location,
            context,
            causation,
            outcome,
            method,
            relations,
            sentiment,
            stance,
            valence,
            domain,
            confidence,
            tags,
            type_weights,
        }
    }

    /// Information-content proxy used by the monotonicity invariant.
    ///
    /// Counts populated narrative fields + tag cardinality. Deliberately
    /// ignores scalar fields (valence/domain/confidence) because those
    /// always have values and their "information" is orthogonal to the
    /// grow-only property we assert on `union`.
    pub fn info_content(&self) -> usize {
        let mut n = 0usize;
        if self.participants.is_some() {
            n += 1;
        }
        if self.temporal.is_some() {
            n += 1;
        }
        if self.location.is_some() {
            n += 1;
        }
        if self.context.is_some() {
            n += 1;
        }
        if self.causation.is_some() {
            n += 1;
        }
        if self.outcome.is_some() {
            n += 1;
        }
        if self.method.is_some() {
            n += 1;
        }
        if self.relations.is_some() {
            n += 1;
        }
        if self.sentiment.is_some() {
            n += 1;
        }
        if self.stance.is_some() {
            n += 1;
        }
        n + self.tags.len()
    }
}

// ---------------------------------------------------------------------
// Union helpers (private; exercised through `Dimensions::union` only)
// ---------------------------------------------------------------------

/// "Longer wins; tie → existing" over `Option<String>`.
fn pick_longer(existing: Option<String>, incoming: Option<String>) -> Option<String> {
    match (existing, incoming) {
        (Some(a), Some(b)) => {
            if b.chars().count() > a.chars().count() {
                Some(b)
            } else {
                Some(a)
            }
        }
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

/// "Longer wins; tie → existing" over `NonEmptyString` (used for `core_fact`).
fn pick_longer_non_empty(existing: NonEmptyString, incoming: NonEmptyString) -> NonEmptyString {
    if incoming.as_str().chars().count() > existing.as_str().chars().count() {
        incoming
    } else {
        existing
    }
}

/// Set-union of a delimiter-separated `Option<String>` field. Tokens
/// are trimmed; empty tokens dropped; output re-serialized with a
/// stable, sorted order so union is idempotent and associative.
fn merge_csv_set(
    existing: Option<String>,
    incoming: Option<String>,
    delim: char,
) -> Option<String> {
    let mut set: BTreeSet<String> = BTreeSet::new();
    for src in existing.iter().chain(incoming.iter()) {
        for token in src.split(delim) {
            let t = token.trim();
            if !t.is_empty() {
                set.insert(t.to_string());
            }
        }
    }
    if set.is_empty() {
        None
    } else {
        let joined = set.into_iter().collect::<Vec<_>>().join(match delim {
            ',' => ", ",
            ';' => "; ",
            _ => ", ",
        });
        Some(joined)
    }
}

/// Higher precision wins; tie → existing.
fn pick_higher_temporal(
    existing: Option<TemporalMark>,
    incoming: Option<TemporalMark>,
) -> Option<TemporalMark> {
    match (existing, incoming) {
        (Some(a), Some(b)) => {
            if b.precision_rank() > a.precision_rank() {
                Some(b)
            } else {
                Some(a)
            }
        }
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

/// Importance-weighted average of two valence values, clamped.
///
/// Short-circuits when both sides hold the same bit pattern — this
/// preserves exact idempotence (`a.union(a) == a`) across weight
/// choices, which floating-point rounding would otherwise violate
/// (e.g. `(v*w1 + v*w2) / (w1+w2)` can differ from `v` by 1 ulp).
fn weighted_valence(
    existing: Valence,
    incoming: Valence,
    weights: crate::merge_types::MergeWeights,
) -> Valence {
    if existing.get().to_bits() == incoming.get().to_bits() {
        return existing;
    }
    let total = weights.total();
    // `MergeWeights::new` guarantees `total > 0`, but if a caller built
    // the struct literally we still defend against zero-sum.
    if total <= 0.0 {
        return existing;
    }
    let mixed = (existing.get() * weights.existing_importance
        + incoming.get() * weights.incoming_importance)
        / total;
    Valence::new(mixed)
}

/// FINDING-6 domain merge rule.
fn merge_domain(existing: Domain, incoming: Domain) -> Domain {
    match (existing.is_concrete(), incoming.is_concrete()) {
        // Rule 2: two concretes → existing wins (stable).
        (true, true) => existing,
        // Rule 1: concrete beats Other(_) regardless of side.
        (true, false) => existing,
        (false, true) => incoming,
        // Rule 3: two Other(_) → longer string wins; tie → existing.
        (false, false) => match (&existing, &incoming) {
            (Domain::Other(a), Domain::Other(b)) => {
                if b.chars().count() > a.chars().count() {
                    incoming
                } else {
                    existing
                }
            }
            // Unreachable: is_concrete() already ruled these out, but
            // fall back deterministically to keep the function total.
            _ => existing,
        },
    }
}

/// Per-variant `max` over the seven type-weight fields.
fn merge_type_weights(a: TypeWeights, b: TypeWeights) -> TypeWeights {
    TypeWeights {
        factual: a.factual.max(b.factual),
        episodic: a.episodic.max(b.episodic),
        procedural: a.procedural.max(b.procedural),
        relational: a.relational.max(b.relational),
        emotional: a.emotional.max(b.emotional),
        opinion: a.opinion.max(b.opinion),
        causal: a.causal.max(b.causal),
    }
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- NonEmptyString --

    #[test]
    fn non_empty_string_rejects_empty() {
        assert_eq!(NonEmptyString::new(""), Err(EmptyCoreFactError));
    }

    #[test]
    fn non_empty_string_rejects_whitespace_only() {
        assert_eq!(NonEmptyString::new("   \t\n"), Err(EmptyCoreFactError));
    }

    #[test]
    fn non_empty_string_accepts_real_content() {
        let s = NonEmptyString::new("hello").unwrap();
        assert_eq!(s.as_str(), "hello");
    }

    #[test]
    fn non_empty_string_deserialize_rejects_empty() {
        let err = serde_json::from_str::<NonEmptyString>("\"\"");
        assert!(err.is_err(), "empty string should fail to deserialize");
    }

    // -- Valence --

    #[test]
    fn valence_clamps_out_of_range() {
        assert_eq!(Valence::new(2.0).get(), 1.0);
        assert_eq!(Valence::new(-2.0).get(), -1.0);
        assert_eq!(Valence::new(0.5).get(), 0.5);
    }

    #[test]
    fn valence_handles_nan() {
        assert_eq!(Valence::new(f64::NAN).get(), 0.0);
    }

    #[test]
    fn valence_handles_infinity() {
        assert_eq!(Valence::new(f64::INFINITY).get(), 1.0);
        assert_eq!(Valence::new(f64::NEG_INFINITY).get(), -1.0);
    }

    // -- Importance --

    #[test]
    fn importance_clamps_out_of_range() {
        assert_eq!(Importance::new(2.0).get(), 1.0);
        assert_eq!(Importance::new(-0.5).get(), 0.0);
        assert_eq!(Importance::new(0.7).get(), 0.7);
    }

    #[test]
    fn importance_default_is_half() {
        assert_eq!(Importance::default().get(), 0.5);
    }

    // -- Domain --

    #[test]
    fn domain_from_loose_str_known() {
        assert_eq!(Domain::from_loose_str("Coding"), Domain::Coding);
        assert_eq!(Domain::from_loose_str(" trading "), Domain::Trading);
        assert_eq!(Domain::from_loose_str("research"), Domain::Research);
    }

    #[test]
    fn domain_from_loose_str_unknown_becomes_other() {
        assert_eq!(
            Domain::from_loose_str("ml"),
            Domain::Other("ml".to_string())
        );
    }

    #[test]
    fn domain_from_loose_str_empty_is_general() {
        assert_eq!(Domain::from_loose_str(""), Domain::General);
        assert_eq!(Domain::from_loose_str("   "), Domain::General);
    }

    #[test]
    fn domain_is_concrete() {
        assert!(Domain::Coding.is_concrete());
        assert!(Domain::General.is_concrete());
        assert!(!Domain::Other("x".to_string()).is_concrete());
    }

    // -- Confidence --

    #[test]
    fn confidence_ord_cautious_is_least() {
        assert!(Confidence::Uncertain < Confidence::Likely);
        assert!(Confidence::Likely < Confidence::Confident);
    }

    #[test]
    fn confidence_from_loose_str() {
        assert_eq!(Confidence::from_loose_str("confident"), Confidence::Confident);
        assert_eq!(Confidence::from_loose_str("likely"), Confidence::Likely);
        assert_eq!(Confidence::from_loose_str("uncertain"), Confidence::Uncertain);
        assert_eq!(Confidence::from_loose_str("???"), Confidence::Uncertain);
    }

    // -- TemporalMark --

    #[test]
    fn temporal_precision_order_matches_design() {
        let exact = TemporalMark::Exact(Utc::now());
        let day = TemporalMark::Day(NaiveDate::from_ymd_opt(2026, 4, 22).unwrap());
        let range = TemporalMark::Range {
            start: NaiveDate::from_ymd_opt(2026, 4, 22).unwrap(),
            end: NaiveDate::from_ymd_opt(2026, 4, 23).unwrap(),
        };
        let vague = TemporalMark::Vague("a while back".to_string());

        // design §2.2: Exact > Range > Day > Vague
        assert!(exact.precision_rank() > range.precision_rank());
        assert!(range.precision_rank() > day.precision_rank());
        assert!(day.precision_rank() > vague.precision_rank());
    }

    // -- Dimensions::minimal --

    #[test]
    fn dimensions_minimal_defaults() {
        let d = Dimensions::minimal("hello world").unwrap();
        assert_eq!(d.core_fact.as_str(), "hello world");
        assert!(d.participants.is_none());
        assert!(d.temporal.is_none());
        assert_eq!(d.valence, Valence::ZERO);
        assert_eq!(d.domain, Domain::General);
        assert_eq!(d.confidence, Confidence::Uncertain);
        assert!(d.tags.is_empty());
    }

    #[test]
    fn dimensions_minimal_rejects_empty() {
        assert!(Dimensions::minimal("").is_err());
        assert!(Dimensions::minimal("   ").is_err());
    }

    // -- Serde round-trip --

    #[test]
    fn dimensions_serde_round_trip() {
        let d = Dimensions::minimal("roundtrip test").unwrap();
        let json = serde_json::to_string(&d).unwrap();
        let decoded: Dimensions = serde_json::from_str(&json).unwrap();
        assert_eq!(d, decoded);
    }

    #[test]
    fn dimensions_round_trip_with_fields() {
        let mut d = Dimensions::minimal("test").unwrap();
        d.participants = Some("alice, bob".to_string());
        d.temporal = Some(TemporalMark::Day(
            NaiveDate::from_ymd_opt(2026, 4, 22).unwrap(),
        ));
        d.valence = Valence::new(0.3);
        d.domain = Domain::Coding;
        d.confidence = Confidence::Likely;
        d.tags.insert("rust".to_string());
        d.tags.insert("testing".to_string());

        let json = serde_json::to_string(&d).unwrap();
        let decoded: Dimensions = serde_json::from_str(&json).unwrap();
        assert_eq!(d, decoded);
    }

    #[test]
    fn domain_other_serde_round_trip() {
        let d = Domain::Other("ml".to_string());
        let json = serde_json::to_string(&d).unwrap();
        let decoded: Domain = serde_json::from_str(&json).unwrap();
        assert_eq!(d, decoded);
    }

    #[test]
    fn temporal_mark_serde_all_variants() {
        for tm in [
            TemporalMark::Exact(Utc::now()),
            TemporalMark::Day(NaiveDate::from_ymd_opt(2026, 4, 22).unwrap()),
            TemporalMark::Range {
                start: NaiveDate::from_ymd_opt(2026, 4, 22).unwrap(),
                end: NaiveDate::from_ymd_opt(2026, 4, 28).unwrap(),
            },
            TemporalMark::Vague("later".to_string()),
        ] {
            let json = serde_json::to_string(&tm).unwrap();
            let decoded: TemporalMark = serde_json::from_str(&json).unwrap();
            assert_eq!(tm, decoded);
        }
    }

    // -----------------------------------------------------------------
    // Dimensions::union — targeted unit tests (design §5.1, §5.3)
    // -----------------------------------------------------------------

    use crate::merge_types::MergeWeights;

    fn dims(core: &str) -> Dimensions {
        Dimensions::minimal(core).unwrap()
    }

    // core_fact: longer wins

    #[test]
    fn union_core_fact_longer_wins() {
        let a = dims("short");
        let b = dims("a longer statement");
        let merged = a.clone().union(b.clone(), MergeWeights::EQUAL);
        assert_eq!(merged.core_fact.as_str(), "a longer statement");

        // reverse roles → still longer wins (content rule is commutative)
        let merged2 = b.union(a, MergeWeights::EQUAL);
        assert_eq!(merged2.core_fact.as_str(), "a longer statement");
    }

    #[test]
    fn union_core_fact_tie_keeps_existing() {
        let a = dims("aaa");
        let b = dims("bbb"); // same length
        let merged = a.clone().union(b, MergeWeights::EQUAL);
        assert_eq!(merged.core_fact.as_str(), "aaa");
    }

    // participants: csv set-union

    #[test]
    fn union_participants_set_union() {
        let mut a = dims("x");
        a.participants = Some("alice, bob".to_string());
        let mut b = dims("x");
        b.participants = Some("bob, carol".to_string());
        let merged = a.union(b, MergeWeights::EQUAL);
        // BTreeSet ordering → "alice, bob, carol"
        assert_eq!(
            merged.participants.as_deref(),
            Some("alice, bob, carol")
        );
    }

    #[test]
    fn union_participants_drops_empty_tokens() {
        let mut a = dims("x");
        a.participants = Some("alice,, bob ".to_string());
        let mut b = dims("x");
        b.participants = None;
        let merged = a.union(b, MergeWeights::EQUAL);
        assert_eq!(merged.participants.as_deref(), Some("alice, bob"));
    }

    // relations: semicolon set-union

    #[test]
    fn union_relations_semicolon_set_union() {
        let mut a = dims("x");
        a.relations = Some("rel1; rel2".to_string());
        let mut b = dims("x");
        b.relations = Some("rel2; rel3".to_string());
        let merged = a.union(b, MergeWeights::EQUAL);
        assert_eq!(merged.relations.as_deref(), Some("rel1; rel2; rel3"));
    }

    // temporal: precision ordering (design §2.2 — Exact > Range > Day > Vague)

    #[test]
    fn union_temporal_vague_loses_to_exact() {
        let mut a = dims("x");
        a.temporal = Some(TemporalMark::Vague("someday".to_string()));
        let mut b = dims("x");
        b.temporal = Some(TemporalMark::Exact(Utc::now()));
        let merged = a.union(b.clone(), MergeWeights::EQUAL);
        assert!(matches!(merged.temporal, Some(TemporalMark::Exact(_))));
    }

    #[test]
    fn union_temporal_day_loses_to_range() {
        let mut a = dims("x");
        a.temporal = Some(TemporalMark::Day(
            NaiveDate::from_ymd_opt(2026, 4, 22).unwrap(),
        ));
        let mut b = dims("x");
        b.temporal = Some(TemporalMark::Range {
            start: NaiveDate::from_ymd_opt(2026, 4, 22).unwrap(),
            end: NaiveDate::from_ymd_opt(2026, 4, 23).unwrap(),
        });
        let merged = a.union(b, MergeWeights::EQUAL);
        assert!(matches!(merged.temporal, Some(TemporalMark::Range { .. })));
    }

    #[test]
    fn union_temporal_same_precision_keeps_existing() {
        let d1 = NaiveDate::from_ymd_opt(2026, 4, 22).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2026, 5, 1).unwrap();
        let mut a = dims("x");
        a.temporal = Some(TemporalMark::Day(d1));
        let mut b = dims("x");
        b.temporal = Some(TemporalMark::Day(d2));
        let merged = a.union(b, MergeWeights::EQUAL);
        assert_eq!(merged.temporal, Some(TemporalMark::Day(d1)));
    }

    // sentiment: existing wins if present (non-commutative)

    #[test]
    fn union_sentiment_existing_wins_when_both_present() {
        let mut a = dims("x");
        a.sentiment = Some("happy".to_string());
        let mut b = dims("x");
        b.sentiment = Some("sad".to_string());
        let merged = a.union(b, MergeWeights::EQUAL);
        assert_eq!(merged.sentiment.as_deref(), Some("happy"));
    }

    #[test]
    fn union_sentiment_incoming_fills_none() {
        let a = dims("x"); // sentiment None
        let mut b = dims("x");
        b.sentiment = Some("sad".to_string());
        let merged = a.union(b, MergeWeights::EQUAL);
        assert_eq!(merged.sentiment.as_deref(), Some("sad"));
    }

    #[test]
    fn union_stance_existing_wins_when_both_present() {
        let mut a = dims("x");
        a.stance = Some("pro".to_string());
        let mut b = dims("x");
        b.stance = Some("con".to_string());
        let merged = a.union(b, MergeWeights::EQUAL);
        assert_eq!(merged.stance.as_deref(), Some("pro"));
    }

    // valence: importance-weighted average

    #[test]
    fn union_valence_weighted_average() {
        let mut a = dims("x");
        a.valence = Valence::new(0.9);
        let mut b = dims("x");
        b.valence = Valence::new(-0.3);
        let w = MergeWeights::new(1.0, 0.1);
        let merged = a.union(b, w);
        // (0.9*1.0 + (-0.3)*0.1) / 1.1 ≈ 0.7909
        let expected = (0.9 * 1.0 + -0.3 * 0.1) / 1.1;
        assert!((merged.valence.get() - expected).abs() < 1e-9);
    }

    #[test]
    fn union_valence_equal_weights_midpoint() {
        let mut a = dims("x");
        a.valence = Valence::new(0.4);
        let mut b = dims("x");
        b.valence = Valence::new(-0.2);
        let merged = a.union(b, MergeWeights::EQUAL);
        assert!((merged.valence.get() - 0.1).abs() < 1e-9);
    }

    // domain: FINDING-6

    #[test]
    fn union_domain_concrete_beats_other_either_side() {
        let mut a = dims("x");
        a.domain = Domain::Coding;
        let mut b = dims("x");
        b.domain = Domain::Other("misc".to_string());
        let merged = a.clone().union(b.clone(), MergeWeights::EQUAL);
        assert_eq!(merged.domain, Domain::Coding);

        // reverse roles → still the concrete one wins
        let merged2 = b.union(a, MergeWeights::EQUAL);
        assert_eq!(merged2.domain, Domain::Coding);
    }

    #[test]
    fn union_domain_two_concretes_existing_wins() {
        let mut a = dims("x");
        a.domain = Domain::Coding;
        let mut b = dims("x");
        b.domain = Domain::Trading;
        let merged = a.union(b, MergeWeights::EQUAL);
        assert_eq!(merged.domain, Domain::Coding);
    }

    #[test]
    fn union_domain_two_others_longer_wins() {
        let mut a = dims("x");
        a.domain = Domain::Other("a".to_string());
        let mut b = dims("x");
        b.domain = Domain::Other("longer".to_string());
        let merged = a.union(b, MergeWeights::EQUAL);
        assert_eq!(merged.domain, Domain::Other("longer".to_string()));
    }

    #[test]
    fn union_domain_two_others_same_length_existing_wins() {
        let mut a = dims("x");
        a.domain = Domain::Other("alpha".to_string());
        let mut b = dims("x");
        b.domain = Domain::Other("omega".to_string());
        let merged = a.union(b, MergeWeights::EQUAL);
        assert_eq!(merged.domain, Domain::Other("alpha".to_string()));
    }

    // confidence: min picks most cautious

    #[test]
    fn union_confidence_min() {
        let mut a = dims("x");
        a.confidence = Confidence::Confident;
        let mut b = dims("x");
        b.confidence = Confidence::Uncertain;
        let merged = a.union(b, MergeWeights::EQUAL);
        assert_eq!(merged.confidence, Confidence::Uncertain);
    }

    // tags: BTreeSet union

    #[test]
    fn union_tags_set_union() {
        let mut a = dims("x");
        a.tags.insert("rust".to_string());
        a.tags.insert("dim".to_string());
        let mut b = dims("x");
        b.tags.insert("dim".to_string());
        b.tags.insert("union".to_string());
        let merged = a.union(b, MergeWeights::EQUAL);
        let got: Vec<&str> = merged.tags.iter().map(|s| s.as_str()).collect();
        assert_eq!(got, vec!["dim", "rust", "union"]);
    }

    // type_weights: per-variant max

    #[test]
    fn union_type_weights_per_variant_max() {
        let mut a = dims("x");
        a.type_weights = TypeWeights {
            factual: 0.9,
            episodic: 0.2,
            procedural: 0.3,
            relational: 0.4,
            emotional: 0.5,
            opinion: 0.1,
            causal: 0.1,
        };
        let mut b = dims("x");
        b.type_weights = TypeWeights {
            factual: 0.1,
            episodic: 0.8,
            procedural: 0.3,
            relational: 0.2,
            emotional: 0.5,
            opinion: 0.9,
            causal: 0.1,
        };
        let merged = a.union(b, MergeWeights::EQUAL);
        assert_eq!(merged.type_weights.factual, 0.9);
        assert_eq!(merged.type_weights.episodic, 0.8);
        assert_eq!(merged.type_weights.procedural, 0.3);
        assert_eq!(merged.type_weights.relational, 0.4);
        assert_eq!(merged.type_weights.emotional, 0.5);
        assert_eq!(merged.type_weights.opinion, 0.9);
        assert_eq!(merged.type_weights.causal, 0.1);
    }

    // info_content monotonicity spot-check

    #[test]
    fn union_info_content_never_shrinks() {
        let mut a = dims("x");
        a.participants = Some("alice".to_string());
        a.tags.insert("rust".to_string());
        let mut b = dims("x");
        b.causation = Some("because".to_string());
        b.tags.insert("memory".to_string());

        let ai = a.info_content();
        let bi = b.info_content();
        let merged = a.union(b, MergeWeights::EQUAL);
        assert!(merged.info_content() >= ai.max(bi));
    }

    // Sanity: self-union is identity on information.

    #[test]
    fn union_self_is_idempotent() {
        let mut a = dims("a fact");
        a.participants = Some("alice, bob".to_string());
        a.tags.insert("rust".to_string());
        a.domain = Domain::Coding;
        a.confidence = Confidence::Likely;
        a.valence = Valence::new(0.4);
        let merged = a.clone().union(a.clone(), MergeWeights::EQUAL);
        assert_eq!(merged, a);
    }
}
