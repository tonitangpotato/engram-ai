//! Property-based tests for `Dimensions::union`.
//!
//! Locks in the three algebraic invariants from design §5.3 of ISS-019:
//!   1. Idempotence:   `a.union(a, w) == a`
//!   2. Associativity: `(a.u(b)).u(c) == a.u(b.u(c))` (same weights both sides)
//!   3. Monotonicity:  `info_content(a.u(b)) >= max(info_content(a), info_content(b))`
//!
//! NOT asserted (and would fail): commutativity. `sentiment` / `stance` /
//! domain tie-break / valence-weighted-average all make `a.u(b) != b.u(a)`
//! in general, which is a deliberate design choice (see FINDING-7).

use chrono::{NaiveDate, TimeZone, Utc};
use engramai::dimensions::{
    Confidence, Dimensions, Domain, TemporalMark, Valence,
};
use engramai::merge_types::MergeWeights;
use engramai::type_weights::TypeWeights;
use proptest::prelude::*;
use std::collections::BTreeSet;

// ---------------------------------------------------------------------
// Generators
// ---------------------------------------------------------------------

/// Generate a small non-empty ASCII-alphanumeric string so tokens
/// never contain the split delimiters (, or ;). This keeps the
/// "set-union" fields well-defined across normalization cycles.
fn arb_token() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9_]{1,8}".prop_map(|s| s)
}

/// A single "short" string (possibly containing spaces, but no commas
/// or semicolons), used for longer-wins narrative fields.
fn arb_short_text() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9 _]{0,20}".prop_map(|s| s.trim().to_string())
        .prop_filter("non-empty", |s| !s.is_empty())
}

fn arb_opt_short_text() -> impl Strategy<Value = Option<String>> {
    prop::option::of(arb_short_text())
}

/// Comma-separated participants. Generate a **canonical** form
/// (BTreeSet-sorted, single-space after comma) so the generator
/// matches the output of `merge_csv_set`. Non-canonical inputs
/// are valid in real use but would defeat the idempotence check
/// here — idempotence is a property of canonicalized values.
fn arb_opt_csv() -> impl Strategy<Value = Option<String>> {
    prop::option::of(
        prop::collection::btree_set(arb_token(), 1..4)
            .prop_map(|set| set.into_iter().collect::<Vec<_>>().join(", ")),
    )
}

/// Semicolon-separated relations in canonical (BTreeSet-sorted) form.
fn arb_opt_ssv() -> impl Strategy<Value = Option<String>> {
    prop::option::of(
        prop::collection::btree_set(arb_token(), 1..4)
            .prop_map(|set| set.into_iter().collect::<Vec<_>>().join("; ")),
    )
}

fn arb_temporal() -> impl Strategy<Value = TemporalMark> {
    prop_oneof![
        (1i64..2_000_000_000i64).prop_map(|t| {
            TemporalMark::Exact(Utc.timestamp_opt(t, 0).single().unwrap())
        }),
        (1900i32..2100i32, 1u32..13u32, 1u32..28u32).prop_map(|(y, m, d)| {
            TemporalMark::Day(NaiveDate::from_ymd_opt(y, m, d).unwrap())
        }),
        (1900i32..2100i32, 1u32..12u32, 1u32..20u32, 1u32..8u32)
            .prop_map(|(y, m, d, span)| {
                let start = NaiveDate::from_ymd_opt(y, m, d).unwrap();
                let end = start
                    .checked_add_days(chrono::Days::new(span.into()))
                    .unwrap_or(start);
                TemporalMark::Range { start, end }
            }),
        arb_short_text().prop_map(TemporalMark::Vague),
    ]
}

fn arb_opt_temporal() -> impl Strategy<Value = Option<TemporalMark>> {
    prop::option::of(arb_temporal())
}

fn arb_domain() -> impl Strategy<Value = Domain> {
    prop_oneof![
        Just(Domain::Coding),
        Just(Domain::Trading),
        Just(Domain::Research),
        Just(Domain::Communication),
        Just(Domain::General),
        arb_token().prop_map(Domain::Other),
    ]
}

fn arb_confidence() -> impl Strategy<Value = Confidence> {
    prop_oneof![
        Just(Confidence::Uncertain),
        Just(Confidence::Likely),
        Just(Confidence::Confident),
    ]
}

fn arb_valence() -> impl Strategy<Value = Valence> {
    (-1.0f64..=1.0f64).prop_map(Valence::new)
}

fn arb_tags() -> impl Strategy<Value = BTreeSet<String>> {
    prop::collection::vec(arb_token(), 0..5).prop_map(|v| v.into_iter().collect())
}

fn arb_type_weights() -> impl Strategy<Value = TypeWeights> {
    (
        0.0f64..=1.0f64,
        0.0f64..=1.0f64,
        0.0f64..=1.0f64,
        0.0f64..=1.0f64,
        0.0f64..=1.0f64,
        0.0f64..=1.0f64,
        0.0f64..=1.0f64,
    )
        .prop_map(|(f, e, p, r, em, o, c)| TypeWeights {
            factual: f,
            episodic: e,
            procedural: p,
            relational: r,
            emotional: em,
            opinion: o,
            causal: c,
        })
}

fn arb_dimensions() -> impl Strategy<Value = Dimensions> {
    // proptest tuple impls cap out at 10-arity; nest two groups.
    let group_a = (
        arb_short_text(),
        arb_opt_csv(),
        arb_opt_temporal(),
        arb_opt_short_text(),
        arb_opt_short_text(),
        arb_opt_short_text(),
        arb_opt_short_text(),
        arb_opt_short_text(),
    );
    let group_b = (
        arb_opt_ssv(),
        arb_opt_short_text(),
        arb_opt_short_text(),
        arb_valence(),
        arb_domain(),
        arb_confidence(),
        arb_tags(),
        arb_type_weights(),
    );

    (group_a, group_b).prop_map(
        |(
            (
                core,
                participants,
                temporal,
                location,
                context,
                causation,
                outcome,
                method,
            ),
            (
                relations,
                sentiment,
                stance,
                valence,
                domain,
                confidence,
                tags,
                type_weights,
            ),
        )| {
            let mut d = Dimensions::minimal(&core).expect("non-empty by construction");
            d.participants = participants;
            d.temporal = temporal;
            d.location = location;
            d.context = context;
            d.causation = causation;
            d.outcome = outcome;
            d.method = method;
            d.relations = relations;
            d.sentiment = sentiment;
            d.stance = stance;
            d.valence = valence;
            d.domain = domain;
            d.confidence = confidence;
            d.tags = tags;
            d.type_weights = type_weights;
            d
        },
    )
}

fn arb_merge_weights() -> impl Strategy<Value = MergeWeights> {
    (0.0f64..=1.0f64, 0.0f64..=1.0f64).prop_map(|(a, b)| MergeWeights::new(a, b))
}

// ---------------------------------------------------------------------
// Properties
// ---------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Idempotence: `a.union(a, w) == a` for any weights `w`.
    ///
    /// All non-commutative rules ("existing wins" on ties, sentiment
    /// fallback, domain tie-break) collapse to a no-op when both sides
    /// are literally the same value.
    #[test]
    fn prop_idempotent(a in arb_dimensions(), w in arb_merge_weights()) {
        let merged = a.clone().union(a.clone(), w);
        prop_assert_eq!(merged, a);
    }

    /// Associativity: `(a.u(b)).u(c) == a.u(b.u(c))` under consistent weights,
    /// **modulo valence** (see note below).
    ///
    /// Rationale: every field except `valence` uses an associative combiner
    /// (set-union, max, min, longer-wins-with-existing-tiebreak). `valence`
    /// uses a weighted arithmetic mean, which is *not* algebraically
    /// associative — `((a+b)/2 + c)/2 ≠ (a + (b+c)/2)/2` in general,
    /// regardless of floating-point precision. Design §5.3's associativity
    /// claim therefore holds "modulo valence weighting" — we check
    /// valence with a coarse tolerance and every other field exactly.
    #[test]
    fn prop_associative(
        a in arb_dimensions(),
        b in arb_dimensions(),
        c in arb_dimensions(),
    ) {
        let w = MergeWeights::EQUAL;
        let left = a.clone().union(b.clone(), w).union(c.clone(), w);
        let right = a.union(b.union(c, w), w);

        // Exact equality for every non-valence field.
        prop_assert_eq!(&left.core_fact, &right.core_fact);
        prop_assert_eq!(&left.participants, &right.participants);
        prop_assert_eq!(&left.temporal, &right.temporal);
        prop_assert_eq!(&left.location, &right.location);
        prop_assert_eq!(&left.context, &right.context);
        prop_assert_eq!(&left.causation, &right.causation);
        prop_assert_eq!(&left.outcome, &right.outcome);
        prop_assert_eq!(&left.method, &right.method);
        prop_assert_eq!(&left.relations, &right.relations);
        prop_assert_eq!(&left.sentiment, &right.sentiment);
        prop_assert_eq!(&left.stance, &right.stance);
        prop_assert_eq!(&left.domain, &right.domain);
        prop_assert_eq!(left.confidence, right.confidence);
        prop_assert_eq!(&left.tags, &right.tags);
        prop_assert_eq!(&left.type_weights, &right.type_weights);

        // Valence: weighted mean is not associative; the two parenthesizations
        // differ by at most one full averaging step, which bounds the deviation
        // at 0.5 on the [-1, 1] range. In practice it's usually far smaller.
        let dv = (left.valence.get() - right.valence.get()).abs();
        prop_assert!(
            dv <= 0.5 + 1e-9,
            "valence deviation {} exceeds loose bound",
            dv,
        );
    }

    /// Monotonicity: information only grows.
    ///
    /// `info_content` counts populated narrative fields + tag cardinality.
    /// Unioning with any other signature must not drop populated fields,
    /// drop tags, or otherwise shrink the information content.
    #[test]
    fn prop_monotone(
        a in arb_dimensions(),
        b in arb_dimensions(),
        w in arb_merge_weights(),
    ) {
        let ai = a.info_content();
        let bi = b.info_content();
        let merged = a.union(b, w);
        prop_assert!(
            merged.info_content() >= ai.max(bi),
            "info_content shrunk: merged={} a={} b={}",
            merged.info_content(), ai, bi,
        );
    }
}
