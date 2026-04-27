//! # Stage-1 heuristic signal scorers (`task:retr-impl-classifier-heuristic`)
//!
//! Pure-function, sub-millisecond, no-IO scorers that turn a raw query string
//! into per-signal confidence scores in `[0.0, 1.0]`. The classifier
//! orchestrator (`super`) consumes these scores and applies the §3.2 routing
//! rule.
//!
//! Design ref: `.gid/features/v03-retrieval/design.md` §3.2 stage 1.
//!
//! ## Signals
//!
//! - [`score_entity`]    — does any token resolve to a known entity?
//! - [`score_temporal`]  — is there a time expression?
//! - [`score_abstract`]  — does the query ask for a thematic / summary view?
//! - [`score_affective`] — does the query carry emotion vocabulary?
//! - [`score_associative`] — derived: `1.0 - max(other four)`. Captures
//!   "no strong primary signal".
//!
//! ## Determinism
//!
//! Every scorer is a pure function: identical inputs produce identical
//! outputs across processes, threads, and time. No `now()`, no RNG, no
//! locale-dependent collation — case folding is ASCII-only (`make_ascii_lowercase`).
//! Required for the `graph_query_locked` reproducibility guarantee (§5.4).
//!
//! ## Trait abstraction
//!
//! Entity lookup is hidden behind [`EntityLookup`] so the orchestrator can
//! plug in a concrete graph-backed implementation later
//! (`task:retr-impl-classifier-core`) without this module knowing about the
//! v0.3 graph layer. A [`NullEntityLookup`] is provided for tests and for
//! the "no graph populated" case (returns `EntityMatch::None` for every
//! token — `score_entity` then trivially returns `0.0`).

use std::sync::OnceLock;

use regex::Regex;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// All five signal scores for one query, in `[0.0, 1.0]`.
///
/// `associative` is derived from the other four (§3.2): it is `1.0` when no
/// other signal fires and shrinks toward `0.0` as primary signals strengthen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SignalScores {
    pub entity: f64,
    pub temporal: f64,
    pub abstract_: f64,
    pub affective: f64,
    pub associative: f64,
}

impl SignalScores {
    /// Build the full score set, computing `associative` from the other four.
    pub fn from_primary(entity: f64, temporal: f64, abstract_: f64, affective: f64) -> Self {
        let associative = score_associative(entity, temporal, abstract_, affective);
        Self {
            entity,
            temporal,
            abstract_,
            affective,
            associative,
        }
    }

    /// Names+scores of the four primary signals (i.e. excluding `associative`).
    /// Convenience for the orchestrator's `strong_signals` set construction.
    pub fn primary(&self) -> [(SignalKind, f64); 4] {
        [
            (SignalKind::Entity, self.entity),
            (SignalKind::Temporal, self.temporal),
            (SignalKind::Abstract, self.abstract_),
            (SignalKind::Affective, self.affective),
        ]
    }
}

/// Discriminator for the four primary signals (matches design §3.2).
///
/// `Associative` is intentionally absent — it is a derived scalar, not a
/// primary signal, and never appears in the orchestrator's `strong_signals`
/// set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SignalKind {
    Entity,
    Temporal,
    Abstract,
    Affective,
}

/// Result of an entity-store lookup for a single token (or normalized phrase).
///
/// Used by [`EntityLookup`] implementations to report match strength back to
/// [`score_entity`]. The numeric mapping (exact=1.0, alias=0.8, fuzzy=0.5,
/// none=0.0) is fixed by design §3.2 and applied inside `score_entity` —
/// implementations only report the **kind** of match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityMatch {
    /// Exact match against the canonical name.
    Exact,
    /// Match via a registered alias.
    Alias,
    /// Fuzzy match (e.g. case-insensitive prefix, edit distance ≤ 1).
    Fuzzy,
    /// No match. Token is unknown to the entity store.
    None,
}

/// Trait abstraction over the v0.3 entity store.
///
/// The classifier-core (`task:retr-impl-classifier-core`) wires a real
/// graph-backed implementation behind this trait once `v03-graph-layer` is
/// available. Until then [`NullEntityLookup`] is used and `score_entity`
/// trivially returns `0.0`.
///
/// `Send + Sync` so the orchestrator can hold it inside an `Arc<dyn EntityLookup>`
/// shared across async tasks.
pub trait EntityLookup: Send + Sync {
    /// Look up a single token (already normalized — ASCII-lowercased, no
    /// leading/trailing punctuation). Implementations must be pure: identical
    /// stores + identical tokens → identical [`EntityMatch`].
    fn lookup(&self, token: &str) -> EntityMatch;
}

/// Always-`None` entity lookup for tests and pre-graph environments.
#[derive(Debug, Clone, Copy, Default)]
pub struct NullEntityLookup;

impl EntityLookup for NullEntityLookup {
    fn lookup(&self, _token: &str) -> EntityMatch {
        EntityMatch::None
    }
}

// ---------------------------------------------------------------------------
// score_entity
// ---------------------------------------------------------------------------

/// Score the **entity** signal for `query`.
///
/// Per design §3.2:
/// `score = max(exact ? 1.0 : 0.0, alias * 0.8, fuzzy * 0.5)`
///
/// Implemented here as: tokenize the query, look up each token via `lookup`,
/// take the strongest match across all tokens. Returns `0.0` if no token
/// matches anything (this is the common case when no graph is populated, and
/// matches `NullEntityLookup`'s behavior).
pub fn score_entity(query: &str, lookup: &dyn EntityLookup) -> f64 {
    let mut best: f64 = 0.0;
    for token in tokenize(query) {
        let token_score = match lookup.lookup(&token) {
            EntityMatch::Exact => 1.0,
            EntityMatch::Alias => 0.8,
            EntityMatch::Fuzzy => 0.5,
            EntityMatch::None => 0.0,
        };
        if token_score > best {
            best = token_score;
        }
        if best >= 1.0 {
            break; // can't beat exact
        }
    }
    best
}

// ---------------------------------------------------------------------------
// score_temporal
// ---------------------------------------------------------------------------

/// Compiled temporal-signal regex (lazy, one-time).
///
/// Matches the design §3.2 set:
/// `yesterday | today | last (week|month|...) | before | after |
///  \d{4}-\d{2}-\d{2} | \d+ (days?|weeks?) ago | since | until | as of`
fn temporal_regex() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        // Word boundaries (\b) keep "today" from matching "todayish" but allow
        // "today," / "today." / start-of-string. Everything is ASCII-lowercased
        // before matching, so the pattern itself is lowercase only.
        Regex::new(
            r"(?x)
            \b(
                yesterday
              | today
              | tomorrow
              | last\s+(week|month|year|quarter|day|night)
              | (before|after|since|until|as\s+of)
              | \d{4}-\d{2}-\d{2}
              | \d+\s+(day|days|week|weeks|month|months|year|years|hour|hours|minute|minutes)\s+ago
              | this\s+(week|month|year|quarter)
              | (mon|tues|wednes|thurs|fri|satur|sun)day
            )\b
            ",
        )
        .expect("temporal_regex compiles")
    })
}

/// Score the **temporal** signal for `query`.
///
/// Binary per design §3.2: `1.0` on any match, `0.0` otherwise. The query is
/// ASCII-lowercased before matching (locale-independent for determinism).
pub fn score_temporal(query: &str) -> f64 {
    let mut buf = query.to_owned();
    buf.make_ascii_lowercase();
    if temporal_regex().is_match(&buf) {
        1.0
    } else {
        0.0
    }
}

// ---------------------------------------------------------------------------
// score_abstract
// ---------------------------------------------------------------------------

/// Compiled abstract-intent regex (lazy, one-time).
///
/// Matches the design §3.2 set:
/// `what (has|have) | summarize | overview | themes? | patterns? | trends? | working on`
fn abstract_regex() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?x)
            \b(
                what\s+(has|have|am\s+i|are|were|did)
              | summari[sz]e
              | overview
              | theme | themes
              | pattern | patterns
              | trend | trends
              | working\s+on
              | (high\s*-\s*level|big\s+picture)
            )\b
            ",
        )
        .expect("abstract_regex compiles")
    })
}

/// Score the **abstract** signal for `query`.
///
/// Binary per design §3.2: `1.0` on any thematic/summary phrase match, else
/// `0.0`.
pub fn score_abstract(query: &str) -> f64 {
    let mut buf = query.to_owned();
    buf.make_ascii_lowercase();
    if abstract_regex().is_match(&buf) {
        1.0
    } else {
        0.0
    }
}

// ---------------------------------------------------------------------------
// score_affective
// ---------------------------------------------------------------------------

/// Seed affect vocabulary list per design §3.2 ("extensible via config" — the
/// orchestrator may take a richer list in the future; this module ships the
/// baseline set).
const AFFECT_SEEDS: &[&str] = &[
    "felt",
    "feeling",
    "feelings",
    "feel",
    "emotion",
    "emotional",
    "anxious",
    "anxiety",
    "worried",
    "worry",
    "happy",
    "happiness",
    "sad",
    "sadness",
    "excited",
    "excitement",
    "stressed",
    "stress",
    "proud",
    "pride",
    "ashamed",
    "shame",
    "angry",
    "anger",
    "frustrated",
    "frustration",
    "afraid",
    "fear",
    "scared",
    "calm",
    "content",
    "lonely",
    "loneliness",
    "joy",
    "joyful",
    "depressed",
    "depression",
    "hopeful",
    "hopeless",
    "grateful",
    "gratitude",
    "guilty",
    "guilt",
];

/// Compiled affect-keyword regex (lazy, one-time, alternation of seeds with
/// word boundaries).
fn affective_regex() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        let alternation = AFFECT_SEEDS.join("|");
        let pattern = format!(r"\b({})\b", alternation);
        Regex::new(&pattern).expect("affective_regex compiles")
    })
}

/// Score the **affective** signal for `query`.
///
/// Binary per design §3.2: `1.0` if the query contains any affect-vocabulary
/// word, else `0.0`. Configurable extensibility is the orchestrator's
/// responsibility (this stage-1 module ships the seed list); a richer
/// implementation accepting `extra_keywords: &[&str]` is straightforward but
/// out of scope for this task.
pub fn score_affective(query: &str) -> f64 {
    let mut buf = query.to_owned();
    buf.make_ascii_lowercase();
    if affective_regex().is_match(&buf) {
        1.0
    } else {
        0.0
    }
}

// ---------------------------------------------------------------------------
// score_associative (derived)
// ---------------------------------------------------------------------------

/// Compute the **associative** score from the four primary signal scores.
///
/// Per design §3.2: `associative = 1.0 - max(entity, temporal, abstract, affective)`.
/// Bounded to `[0.0, 1.0]` (bound is automatic given primaries are in
/// `[0.0, 1.0]`, but we clamp defensively for forward-compat with future
/// non-binary signals).
pub fn score_associative(entity: f64, temporal: f64, abstract_: f64, affective: f64) -> f64 {
    let m = entity
        .max(temporal)
        .max(abstract_)
        .max(affective);
    (1.0 - m).clamp(0.0, 1.0)
}

// ---------------------------------------------------------------------------
// Convenience: score all signals at once
// ---------------------------------------------------------------------------

/// Run all four primary scorers + derive `associative`. Convenience for the
/// orchestrator (`super::Classifier`); equivalent to calling each scorer
/// individually and assembling a [`SignalScores`].
pub fn score_all(query: &str, lookup: &dyn EntityLookup) -> SignalScores {
    SignalScores::from_primary(
        score_entity(query, lookup),
        score_temporal(query),
        score_abstract(query),
        score_affective(query),
    )
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Token splitter used for entity lookup.
///
/// Splits on whitespace, ASCII-lowercases, strips leading/trailing ASCII
/// punctuation. Keeps tokens with internal punctuation intact ("ISS-021",
/// "v0.3", "engram-rs") so canonical IDs match exactly.
fn tokenize(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .filter_map(|raw| {
            let trimmed = raw.trim_matches(|c: char| {
                c.is_ascii_punctuation() && c != '-' && c != '_' && c != '.'
            });
            if trimmed.is_empty() {
                None
            } else {
                let mut s = trimmed.to_owned();
                s.make_ascii_lowercase();
                Some(s)
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Test-only `EntityLookup` that returns a configured match per token.
    struct StubLookup(HashMap<String, EntityMatch>);

    impl StubLookup {
        fn with(pairs: &[(&str, EntityMatch)]) -> Self {
            Self(
                pairs
                    .iter()
                    .map(|(k, v)| (k.to_string(), *v))
                    .collect(),
            )
        }
    }

    impl EntityLookup for StubLookup {
        fn lookup(&self, token: &str) -> EntityMatch {
            self.0.get(token).copied().unwrap_or(EntityMatch::None)
        }
    }

    // ---- score_entity ----------------------------------------------------

    #[test]
    fn entity_no_match_returns_zero() {
        let lookup = NullEntityLookup;
        assert_eq!(score_entity("who is alice", &lookup), 0.0);
    }

    #[test]
    fn entity_exact_match_returns_one() {
        let lookup = StubLookup::with(&[("alice", EntityMatch::Exact)]);
        assert_eq!(score_entity("who is alice", &lookup), 1.0);
    }

    #[test]
    fn entity_alias_returns_zero_eight() {
        let lookup = StubLookup::with(&[("al", EntityMatch::Alias)]);
        assert_eq!(score_entity("ping al today", &lookup), 0.8);
    }

    #[test]
    fn entity_fuzzy_returns_zero_five() {
        let lookup = StubLookup::with(&[("alise", EntityMatch::Fuzzy)]);
        assert_eq!(score_entity("who is alise?", &lookup), 0.5);
    }

    #[test]
    fn entity_takes_max_across_tokens() {
        let lookup = StubLookup::with(&[
            ("alice", EntityMatch::Fuzzy),
            ("bob", EntityMatch::Exact),
        ]);
        assert_eq!(score_entity("alice and bob", &lookup), 1.0);
    }

    #[test]
    fn entity_normalizes_case_and_punctuation() {
        let lookup = StubLookup::with(&[("iss-021", EntityMatch::Exact)]);
        assert_eq!(score_entity("status of ISS-021?", &lookup), 1.0);
    }

    // ---- score_temporal --------------------------------------------------

    #[test]
    fn temporal_yesterday_matches() {
        assert_eq!(score_temporal("what happened yesterday"), 1.0);
    }

    #[test]
    fn temporal_iso_date_matches() {
        assert_eq!(score_temporal("note from 2026-04-25"), 1.0);
    }

    #[test]
    fn temporal_relative_ago_matches() {
        assert_eq!(score_temporal("3 days ago we shipped"), 1.0);
        assert_eq!(score_temporal("2 weeks ago"), 1.0);
    }

    #[test]
    fn temporal_as_of_matches() {
        assert_eq!(score_temporal("as of last month"), 1.0);
    }

    #[test]
    fn temporal_no_time_returns_zero() {
        assert_eq!(score_temporal("who is alice"), 0.0);
        assert_eq!(score_temporal(""), 0.0);
    }

    #[test]
    fn temporal_word_boundary_prevents_false_match() {
        // "todayish" must NOT match "today"
        assert_eq!(score_temporal("todayish content"), 0.0);
    }

    // ---- score_abstract --------------------------------------------------

    #[test]
    fn abstract_summarize_matches() {
        assert_eq!(score_abstract("summarize our work on retrieval"), 1.0);
        assert_eq!(score_abstract("summarise our work"), 1.0);
    }

    #[test]
    fn abstract_what_have_matches() {
        assert_eq!(score_abstract("what have I been working on"), 1.0);
    }

    #[test]
    fn abstract_themes_matches() {
        assert_eq!(score_abstract("show me trends"), 1.0);
        assert_eq!(score_abstract("recurring patterns?"), 1.0);
    }

    #[test]
    fn abstract_factual_query_does_not_match() {
        assert_eq!(score_abstract("who is alice"), 0.0);
    }

    // ---- score_affective -------------------------------------------------

    #[test]
    fn affective_emotion_word_matches() {
        assert_eq!(score_affective("things I felt good about"), 1.0);
        assert_eq!(score_affective("what made me anxious this week"), 1.0);
    }

    #[test]
    fn affective_no_emotion_returns_zero() {
        assert_eq!(score_affective("what is the schema"), 0.0);
    }

    #[test]
    fn affective_word_boundary_prevents_false_match() {
        // "fearless" must not match "fear"
        assert_eq!(score_affective("fearless leadership"), 0.0);
    }

    // ---- score_associative -----------------------------------------------

    #[test]
    fn associative_no_signals_is_one() {
        assert_eq!(score_associative(0.0, 0.0, 0.0, 0.0), 1.0);
    }

    #[test]
    fn associative_full_signal_is_zero() {
        assert_eq!(score_associative(1.0, 0.0, 0.0, 0.0), 0.0);
        assert_eq!(score_associative(0.0, 0.0, 1.0, 0.0), 0.0);
    }

    #[test]
    fn associative_mid_signal() {
        // max=0.7 → associative = 0.3 (within f64 epsilon)
        let got = score_associative(0.7, 0.5, 0.0, 0.0);
        assert!((got - 0.3).abs() < 1e-9, "got {got}");
    }

    // ---- score_all -------------------------------------------------------

    #[test]
    fn score_all_assembles_complete_signal_set() {
        let lookup = NullEntityLookup;
        let s = score_all("what happened yesterday", &lookup);
        assert_eq!(s.entity, 0.0);
        assert_eq!(s.temporal, 1.0);
        assert_eq!(s.abstract_, 0.0);
        assert_eq!(s.affective, 0.0);
        // associative = 1 - max(0,1,0,0) = 0
        assert_eq!(s.associative, 0.0);
    }

    #[test]
    fn score_all_is_deterministic() {
        // Same input twice → identical bits. Required for §5.4 reproducibility.
        let lookup = NullEntityLookup;
        let q = "what made me anxious last week, alice?";
        let a = score_all(q, &lookup);
        let b = score_all(q, &lookup);
        assert_eq!(a, b);
    }

    // ---- tokenize --------------------------------------------------------

    #[test]
    fn tokenize_strips_punctuation_and_lowercases() {
        // `?` and `,` are stripped; `.` is preserved as internal punctuation
        // (so "v0.3" survives intact — see the next test).
        let toks = tokenize("Who is Alice? Bob, hello!");
        assert_eq!(toks, vec!["who", "is", "alice", "bob", "hello"]);
    }

    #[test]
    fn tokenize_preserves_internal_punctuation() {
        let toks = tokenize("status of ISS-021 and v0.3?");
        assert_eq!(toks, vec!["status", "of", "iss-021", "and", "v0.3"]);
    }
}
