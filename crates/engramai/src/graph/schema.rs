//! Schema registry for graph predicates — see design §3.3.
//!
//! Implements the hybrid canonical/proposed predicate model mandated by
//! Master §3.5: a closed set of [`CanonicalPredicate`] variants with known
//! structural semantics (inverse, symmetry, cardinality) and an open space
//! of LLM-authored [`Predicate::Proposed`] strings preserved verbatim
//! (modulo case/whitespace normalization — GOAL-1.8).

use serde::{Deserialize, Serialize};

/// A typed predicate. Either canonical (has structural semantics) or
/// proposed (opaque label, preserved verbatim).
// NOTE on serde shape: the design (§3.3) specifies
// `#[serde(tag = "kind", rename_all = "snake_case")]` (internally tagged).
// Internal tagging requires every variant's payload to serialize as a
// self-describing map; a newtype variant wrapping a primitive `String`
// (`Proposed(String)`) fails at runtime with "cannot serialize tagged
// newtype variant ... containing a string". The minimal-deviation fix that
// keeps the variant shape from the design verbatim is to switch to the
// **adjacently-tagged** representation by adding `content = "value"`. The
// wire format becomes `{"kind":"canonical","value":"is_a"}` and
// `{"kind":"proposed","value":"works_with"}` — the `kind` discriminant
// promised by the design is preserved.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Predicate {
    Canonical(CanonicalPredicate),
    /// LLM- or agent-proposed predicate. Stored as lowercase, whitespace-
    /// normalized string; never participates in inverse/symmetric traversal
    /// (GOAL-1.9). The exact string is preserved (GOAL-1.8 — no info loss).
    Proposed(String),
}

impl Predicate {
    /// Construct a [`Predicate::Proposed`] from a raw string, applying the
    /// sanctioned normalization: lowercase, internal whitespace runs
    /// collapsed to a single space, leading/trailing whitespace trimmed.
    ///
    /// This is the **only** sanctioned constructor for `Proposed` — building
    /// the variant directly bypasses normalization and may break the
    /// invariant that "two proposed strings differing only in
    /// whitespace/case collapse to one" (design §3.3, evolution rule 2).
    pub fn proposed(s: &str) -> Self {
        let lowered = s.to_lowercase();
        let normalized = lowered.split_whitespace().collect::<Vec<_>>().join(" ");
        Predicate::Proposed(normalized)
    }
}

/// Canonical predicates seeded with known semantics. This list is the
/// v0.3 baseline; extending it is a schema change owned by a minor release.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CanonicalPredicate {
    // identity / type
    IsA,
    PartOf,
    // social / organizational
    WorksAt,
    MemberOf,
    MarriedTo,
    ParentOf,
    // technical
    DependsOn,
    Uses,
    Implements,
    // causal / temporal
    CausedBy,
    LeadsTo,
    PrecededBy,
    // authored / sourced
    CreatedBy,
    MentionedIn,
    // dialectical
    Contradicts,
    Supports,
    // generic fallback (avoid when possible)
    RelatedTo,
}

/// Structural hint used by `GraphStore::traverse`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Directionality {
    /// `A p B` implies `B inverse(p) A` — walker can traverse both ways.
    Directed { inverse: Option<CanonicalPredicate> },
    /// `A p B` implies `B p A` — same predicate both directions.
    Symmetric,
}

/// Returns the [`Directionality`] for a canonical predicate.
///
/// This is the static lookup table backing `GraphStore::traverse`.
/// Exhaustive over all [`CanonicalPredicate`] variants — adding a variant
/// requires updating this match (the compiler will enforce it).
pub fn directionality(p: &CanonicalPredicate) -> Directionality {
    use CanonicalPredicate::*;
    match p {
        // Symmetric relations — same predicate both directions.
        MarriedTo => Directionality::Symmetric,
        RelatedTo => Directionality::Symmetric,
        Contradicts => Directionality::Symmetric,

        // Directed with a known inverse canonical predicate.
        ParentOf => Directionality::Directed { inverse: None }, // ChildOf not in baseline
        CausedBy => Directionality::Directed { inverse: Some(LeadsTo) },
        LeadsTo => Directionality::Directed { inverse: Some(CausedBy) },

        // Directed without a baseline inverse. A future minor release may
        // introduce inverse variants (e.g. `ContainedBy` for `PartOf`); per
        // evolution rule 1 these are append-only additions.
        IsA => Directionality::Directed { inverse: None },
        PartOf => Directionality::Directed { inverse: None },
        WorksAt => Directionality::Directed { inverse: None },
        MemberOf => Directionality::Directed { inverse: None },
        DependsOn => Directionality::Directed { inverse: None },
        Uses => Directionality::Directed { inverse: None },
        Implements => Directionality::Directed { inverse: None },
        PrecededBy => Directionality::Directed { inverse: None },
        CreatedBy => Directionality::Directed { inverse: None },
        MentionedIn => Directionality::Directed { inverse: None },
        Supports => Directionality::Directed { inverse: None },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proposed_normalizes_case_and_whitespace() {
        let p = Predicate::proposed("  Foo   BAR  baz ");
        assert_eq!(p, Predicate::Proposed("foo bar baz".into()));
    }

    #[test]
    fn proposed_preserves_internal_punctuation() {
        // Only whitespace collapses; punctuation (hyphens, underscores, etc.)
        // is preserved verbatim aside from lowercasing.
        let p = Predicate::proposed("Has-A");
        assert_eq!(p, Predicate::Proposed("has-a".into()));
    }

    #[test]
    fn proposed_handles_tabs_and_newlines() {
        // `split_whitespace` collapses any unicode whitespace, including
        // tabs and newlines, into a single ASCII space.
        let p = Predicate::proposed("foo\t\nbar");
        assert_eq!(p, Predicate::Proposed("foo bar".into()));
    }

    #[test]
    fn serde_roundtrip_canonical() {
        let original = Predicate::Canonical(CanonicalPredicate::IsA);
        let json = serde_json::to_string(&original).unwrap();
        // With `#[serde(tag = "kind", rename_all = "snake_case")]` on the
        // outer enum and `rename_all = "snake_case"` on `CanonicalPredicate`,
        // the wire shape is `{"kind":"canonical","Canonical":"is_a"}` — the
        // outer tag adapter keeps the variant payload under the variant
        // name (PascalCase by default). Document whatever serde produces.
        let decoded: Predicate = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, original);
        // Sanity: tag is present and discriminant is snake_case.
        assert!(json.contains("\"kind\":\"canonical\""));
        assert!(json.contains("is_a"));
    }

    #[test]
    fn serde_roundtrip_proposed() {
        let original = Predicate::Proposed("works_with".into());
        let json = serde_json::to_string(&original).unwrap();
        let decoded: Predicate = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, original);
        assert!(json.contains("\"kind\":\"proposed\""));
        assert!(json.contains("works_with"));
    }

    #[test]
    fn directionality_covers_all_canonical() {
        use CanonicalPredicate::*;
        // Exhaustive list — if a variant is added to CanonicalPredicate,
        // this test must be extended (and `directionality`'s match too).
        let all = [
            IsA, PartOf, WorksAt, MemberOf, MarriedTo, ParentOf, DependsOn,
            Uses, Implements, CausedBy, LeadsTo, PrecededBy, CreatedBy,
            MentionedIn, Contradicts, Supports, RelatedTo,
        ];
        for p in &all {
            // Must not panic and must return a sensible value.
            let _ = directionality(p);
        }

        // Spot-check the design's stated semantics.
        assert_eq!(directionality(&MarriedTo), Directionality::Symmetric);
        assert_eq!(directionality(&RelatedTo), Directionality::Symmetric);
        assert!(matches!(
            directionality(&ParentOf),
            Directionality::Directed { .. }
        ));
        assert!(matches!(
            directionality(&CreatedBy),
            Directionality::Directed { .. }
        ));

        // CausedBy/LeadsTo are mutual inverses.
        assert_eq!(
            directionality(&CausedBy),
            Directionality::Directed { inverse: Some(LeadsTo) }
        );
        assert_eq!(
            directionality(&LeadsTo),
            Directionality::Directed { inverse: Some(CausedBy) }
        );
    }

    #[test]
    fn is_a_inverse_is_none_or_specific() {
        // The v0.3 baseline does not include a `SubtypeOf`/`InstanceOf`
        // inverse for `IsA`, so the design leaves the inverse unspecified
        // (None). A future minor release may add one (evolution rule 1).
        match directionality(&CanonicalPredicate::IsA) {
            Directionality::Directed { inverse } => {
                assert!(
                    inverse.is_none(),
                    "IsA has no baseline inverse in v0.3; got {:?}",
                    inverse
                );
            }
            Directionality::Symmetric => {
                panic!("IsA must be Directed, not Symmetric");
            }
        }
    }
}
