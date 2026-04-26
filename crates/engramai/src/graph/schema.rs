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

    /// Returns the cardinality of this predicate.
    ///
    /// - For `Canonical(p)`: looks up the registered cardinality (see
    ///   [`cardinality`]).
    /// - For `Proposed(_)`: defaults to `ManyToMany` — safer than
    ///   assuming functional for unknown predicates (a wrong
    ///   `OneToOne` guess would cause spurious supersession).
    ///
    /// References: v03-graph-layer/design.md §3.3, ISS-035.
    pub fn cardinality(&self) -> Cardinality {
        match self {
            Predicate::Canonical(p) => cardinality(p),
            Predicate::Proposed(_) => Cardinality::ManyToMany,
        }
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

/// Cardinality declaration — **normative; write-time consulted by the
/// resolution pipeline.** The resolution pipeline MUST query
/// [`cardinality`] before computing `EdgeDecision` (v03-resolution §3.4.4):
///
/// - `OneToOne` predicates route through the **functional** match arms:
///   at most one live edge per `(subject, predicate)` slot. A new edge
///   with a different object supersedes the existing one (Replace).
/// - `OneToMany` / `ManyToMany` predicates route through the
///   **multi-valued** match arms: multiple live edges per slot is the
///   normal state. Slot lookup returns 0-N edges; new objects route to
///   Add, existing objects route to None (no-op).
///
/// For `Predicate::Proposed` (not in canonical catalog), the default is
/// `ManyToMany` — safer to assume multi-valued than functional for
/// unknown predicates (avoids spurious supersession).
///
/// See [`cardinality`] for the per-variant mapping table.
///
/// References: v03-graph-layer/design.md §3.3, ISS-035.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cardinality {
    /// At most one live edge per `(subject, predicate)` slot.
    /// New object supersedes the existing edge.
    OneToOne,
    /// Many live edges per `(subject, predicate)` slot, but each subject
    /// has a structural fan-out (one parent → many children).
    OneToMany,
    /// Many live edges per `(subject, predicate)` slot, fully unconstrained.
    ManyToMany,
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

/// Returns the [`Cardinality`] for a canonical predicate (ISS-035).
///
/// This is the static lookup table backing the resolution pipeline's
/// `EdgeDecision` (v03-resolution §3.4.4). Exhaustive over all
/// [`CanonicalPredicate`] variants — adding a variant requires updating
/// this match (the compiler will enforce it).
///
/// **Cardinality mapping (v0.3 baseline, design §3.3):**
///
/// | Predicate      | Cardinality   | Rationale                                            |
/// |----------------|---------------|------------------------------------------------------|
/// | `IsA`          | `ManyToMany`  | An entity can be many kinds; kinds have many members |
/// | `PartOf`       | `ManyToMany`  | Parts can belong to multiple wholes                  |
/// | `WorksAt`      | `OneToOne`    | Functional: one current employer per entity          |
/// | `MemberOf`     | `OneToMany`   | One entity, many memberships                         |
/// | `MarriedTo`    | `OneToOne`    | Functional (current spouse)                          |
/// | `ParentOf`     | `OneToMany`   | One parent, many children                            |
/// | `DependsOn`    | `ManyToMany`  | Many-to-many dependency graph                        |
/// | `Uses`         | `OneToMany`   | One entity uses many tools/technologies              |
/// | `Implements`   | `ManyToMany`  | Many-to-many                                         |
/// | `CausedBy`     | `ManyToMany`  | Multiple causes, multiple effects                    |
/// | `LeadsTo`      | `OneToMany`   | One cause, many consequences                         |
/// | `PrecededBy`   | `OneToOne`    | Functional in a sequence                             |
/// | `CreatedBy`    | `OneToOne`    | Functional: one creator                              |
/// | `MentionedIn`  | `ManyToMany`  | Many mentions across many sources                    |
/// | `Contradicts`  | `ManyToMany`  | Many-to-many                                         |
/// | `Supports`     | `ManyToMany`  | Many-to-many                                         |
/// | `RelatedTo`    | `ManyToMany`  | Generic fallback, assumed multi-valued               |
pub fn cardinality(p: &CanonicalPredicate) -> Cardinality {
    use CanonicalPredicate::*;
    match p {
        // OneToOne — functional, at most one live edge per slot.
        WorksAt => Cardinality::OneToOne,
        MarriedTo => Cardinality::OneToOne,
        PrecededBy => Cardinality::OneToOne,
        CreatedBy => Cardinality::OneToOne,

        // OneToMany — structural fan-out from subject.
        MemberOf => Cardinality::OneToMany,
        ParentOf => Cardinality::OneToMany,
        Uses => Cardinality::OneToMany,
        LeadsTo => Cardinality::OneToMany,

        // ManyToMany — fully unconstrained.
        IsA => Cardinality::ManyToMany,
        PartOf => Cardinality::ManyToMany,
        DependsOn => Cardinality::ManyToMany,
        Implements => Cardinality::ManyToMany,
        CausedBy => Cardinality::ManyToMany,
        MentionedIn => Cardinality::ManyToMany,
        Contradicts => Cardinality::ManyToMany,
        Supports => Cardinality::ManyToMany,
        RelatedTo => Cardinality::ManyToMany,
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

    // ----- ISS-035: cardinality lookup -----

    #[test]
    fn cardinality_covers_all_canonical() {
        use CanonicalPredicate::*;
        // Exhaustive enumeration — if a variant is added to
        // CanonicalPredicate, this test must be extended (and
        // `cardinality`'s match too — the compiler will enforce that).
        let all = [
            IsA, PartOf, WorksAt, MemberOf, MarriedTo, ParentOf, DependsOn,
            Uses, Implements, CausedBy, LeadsTo, PrecededBy, CreatedBy,
            MentionedIn, Contradicts, Supports, RelatedTo,
        ];
        assert_eq!(all.len(), 17, "CanonicalPredicate should have 17 variants");
        for p in &all {
            // Must not panic.
            let _ = cardinality(p);
        }
    }

    #[test]
    fn cardinality_one_to_one_predicates() {
        use CanonicalPredicate::*;
        // Functional predicates per design §3.3 table.
        assert_eq!(cardinality(&WorksAt), Cardinality::OneToOne);
        assert_eq!(cardinality(&MarriedTo), Cardinality::OneToOne);
        assert_eq!(cardinality(&PrecededBy), Cardinality::OneToOne);
        assert_eq!(cardinality(&CreatedBy), Cardinality::OneToOne);
    }

    #[test]
    fn cardinality_one_to_many_predicates() {
        use CanonicalPredicate::*;
        // Structural fan-out from subject.
        assert_eq!(cardinality(&MemberOf), Cardinality::OneToMany);
        assert_eq!(cardinality(&ParentOf), Cardinality::OneToMany);
        assert_eq!(cardinality(&Uses), Cardinality::OneToMany);
        assert_eq!(cardinality(&LeadsTo), Cardinality::OneToMany);
    }

    #[test]
    fn cardinality_many_to_many_predicates() {
        use CanonicalPredicate::*;
        // Fully unconstrained relations.
        assert_eq!(cardinality(&IsA), Cardinality::ManyToMany);
        assert_eq!(cardinality(&PartOf), Cardinality::ManyToMany);
        assert_eq!(cardinality(&DependsOn), Cardinality::ManyToMany);
        assert_eq!(cardinality(&Implements), Cardinality::ManyToMany);
        assert_eq!(cardinality(&CausedBy), Cardinality::ManyToMany);
        assert_eq!(cardinality(&MentionedIn), Cardinality::ManyToMany);
        assert_eq!(cardinality(&Contradicts), Cardinality::ManyToMany);
        assert_eq!(cardinality(&Supports), Cardinality::ManyToMany);
        assert_eq!(cardinality(&RelatedTo), Cardinality::ManyToMany);
    }

    #[test]
    fn predicate_cardinality_canonical_dispatches() {
        // The `Predicate::cardinality()` method should delegate to the
        // free-function lookup for canonical variants.
        let p = Predicate::Canonical(CanonicalPredicate::WorksAt);
        assert_eq!(p.cardinality(), Cardinality::OneToOne);

        let p = Predicate::Canonical(CanonicalPredicate::MemberOf);
        assert_eq!(p.cardinality(), Cardinality::OneToMany);

        let p = Predicate::Canonical(CanonicalPredicate::IsA);
        assert_eq!(p.cardinality(), Cardinality::ManyToMany);
    }

    #[test]
    fn predicate_cardinality_proposed_defaults_many_to_many() {
        // Per design §3.3: unknown/proposed predicates default to
        // ManyToMany — wrong OneToOne guess would cause spurious
        // supersession in the resolution pipeline.
        let p = Predicate::proposed("collaborates_with");
        assert_eq!(p.cardinality(), Cardinality::ManyToMany);

        let p = Predicate::proposed("anything_else");
        assert_eq!(p.cardinality(), Cardinality::ManyToMany);
    }

    #[test]
    fn cardinality_serde_roundtrip() {
        // Snake_case wire format.
        let c = Cardinality::OneToOne;
        let json = serde_json::to_string(&c).unwrap();
        assert_eq!(json, "\"one_to_one\"");
        let decoded: Cardinality = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, c);

        for (variant, expected) in [
            (Cardinality::OneToOne, "\"one_to_one\""),
            (Cardinality::OneToMany, "\"one_to_many\""),
            (Cardinality::ManyToMany, "\"many_to_many\""),
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            assert_eq!(json, expected);
            let decoded: Cardinality = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded, variant);
        }
    }
}
