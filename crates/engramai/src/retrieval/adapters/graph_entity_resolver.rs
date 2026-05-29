//! `GraphEntityResolver` — Factual plan's [`EntityResolver`] backed by the
//! v0.3 graph layer.
//!
//! Resolves a free-form query string to a set of candidate
//! [`ResolvedAnchor`]s by:
//!
//! 1. Extracting candidate **mentions** from the query — token n-grams
//!    of length 1..=`MAX_NGRAM_N` after Unicode-tokenization
//!    (see [`extract_mentions`]).
//! 2. Looking up each mention against
//!    [`GraphRead::search_candidates`] across every namespace the
//!    graph knows about.
//! 3. Deduplicating by `entity_id`, keeping the highest match strength.
//!
//! ## Why mention extraction (ISS-165)
//!
//! `search_candidates` does **exact-equality** alias matching against
//! `graph_entity_aliases.normalized` (`WHERE normalized = ?`). Without
//! a mention-extraction step, the resolver passes the entire query
//! string as the alias key — e.g. `"Where did Caroline move from
//! before settling down?"` is normalized as a single 50-char alias
//! and never matches any entity alias. This silently disabled the
//! Factual plan for every natural-language LoCoMo query (root cause
//! of ISS-164 entity-channel falsification — see ISS-165).
//!
//! The fix is intentionally cheap and deterministic: slide an n-gram
//! window over query tokens, call `search_candidates` once per
//! mention. Each call is two indexed point-lookups (alias hit +
//! entity row), microseconds. For a 10-token question with n ∈ 1..=4
//! this is ~34 lookups per namespace — well below the per-query
//! retrieval budget (design §5.4).
//!
//! ## Why scan all namespaces
//!
//! The Factual plan's `EntityResolver::resolve(&str) -> Vec<ResolvedAnchor>`
//! contract has no namespace parameter — see
//! `crates/engramai/src/retrieval/plans/factual.rs`. The graph store
//! requires one (`CandidateQuery::namespace`). We bridge by iterating
//! [`GraphRead::list_namespaces`] and merging the per-namespace candidate
//! sets. This matches the Factual plan's design: the user issues a
//! query like "Who is Caroline?" without naming a namespace; the
//! resolver finds Carolines across the whole graph.
//!
//! Costs are bounded by `top_k` (we cap at 5 anchors per the Factual
//! plan's `max_anchors=5` default), `MAX_TOP_K` enforced inside
//! `search_candidates`, and namespace count (typically O(1)). For
//! realistic graphs this is acceptable; if it becomes a bottleneck the
//! resolver can grow a namespace filter parameter without breaking the
//! `EntityResolver` trait.
//!
//! ## Why not embed the query
//!
//! `CandidateQuery::mention_embedding` is `Option`. We pass `None` —
//! the v0.3 query-time embedding path lives one layer up
//! (`HybridSeedRecaller` for Associative). The Factual resolver runs on
//! exact-alias and recency only, which keeps it deterministic
//! (design §5.4) and side-effect-free.
//!
//! ## Send + Sync
//!
//! The `EntityResolver` trait requires `Send + Sync`. We impose those
//! bounds on the wrapped `&dyn GraphRead` (callers feed
//! `&dyn GraphRead + Send + Sync`). The orchestrator only constructs
//! `GraphEntityResolver` inside a single synchronous closure
//! (`Memory::with_graph_read`), so the bounds are never observably
//! exercised — they're a typesystem requirement, not a runtime one.

use chrono::Utc;

use crate::graph::store::{CandidateQuery, GraphRead};
use crate::retrieval::plans::factual::{EntityResolver, ResolvedAnchor};

/// Per-namespace cap for a single mention lookup. Each n-gram pulls
/// at most this many candidates from `search_candidates`. Multiple
/// mentions can resolve to overlapping anchors; dedupe runs at the
/// resolver level. `FactualPlanInputs::max_anchors` defaults to 5
/// (factual.rs ≈ line 140); over-fetching here lets the per-namespace
/// top-K be merged by score before truncation.
const PER_NAMESPACE_TOP_K: usize = 8;

/// Maximum number of namespaces we'll scan in one `resolve` call.
/// Bounds latency on graphs with pathological namespace fan-out.
const MAX_NAMESPACES_SCANNED: usize = 32;

/// Maximum n-gram length when extracting mentions from a query.
/// Covers entity names up to 4 tokens like
/// `"adoption advice assistance group"` while keeping the scan
/// O(tokens × MAX_NGRAM_N) bounded.
const MAX_NGRAM_N: usize = 4;

/// Hard cap on total candidate mentions emitted per query. Defends
/// against pathologically long queries; for normal LoCoMo questions
/// (10–20 tokens) the natural count is well below this.
const MAX_MENTIONS: usize = 64;

/// Minimum character length for a 1-gram mention. Filters
/// near-universal single-char tokens like `"a"` / `"I"` / `"1"`
/// that would otherwise generate per-query alias-lookup noise.
/// Multi-token n-grams have no per-token length floor (a bigram
/// like `"is_a"` or `"to_do"` could legitimately be an alias).
const MIN_UNIGRAM_CHARS: usize = 2;

/// A candidate mention: a token n-gram with its position in the
/// tokenized query. The span `[start, start + len)` is in **token**
/// units (not byte/char offsets) and is used for specificity dedup
/// (ISS-192): a mention whose span is strictly subsumed by a longer
/// mention that *also* resolved to an entity is a less-specific
/// fragment (e.g. `"support"` / `"group"` inside `"LGBTQ support
/// group"`) and is dropped so it cannot crowd the precise entity out
/// of the Factual plan's `max_anchors` cap.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Mention {
    /// The mention surface text (space-joined tokens, original case).
    text: String,
    /// Index of the first token in the tokenized query.
    start: usize,
    /// Number of tokens (the n-gram order).
    len: usize,
}

impl Mention {
    /// Token span end (exclusive).
    fn end(&self) -> usize {
        self.start + self.len
    }

    /// True if `self`'s span is **strictly** subsumed by `other`'s:
    /// `other` covers `self` and is genuinely longer (covers at least
    /// one token `self` does not). Equal spans are not subsumed.
    fn subsumed_by(&self, other: &Mention) -> bool {
        other.start <= self.start && self.end() <= other.end() && other.len > self.len
    }
}

/// Extract candidate mentions from a query as token n-grams.
///
/// Tokenization: split on Unicode whitespace and ASCII punctuation
/// (`,`, `.`, `?`, `!`, `:`, `;`, `(`, `)`, `[`, `]`, `{`, `}`,
/// `"`, `\``). Apostrophes are preserved inside tokens but stripped
/// at token end (`"Caroline's"` → `"Caroline"`) because possessive
/// `'s` is rarely part of an entity alias. NFKC-folding and
/// lowercasing happen later inside `search_candidates`'s
/// `normalize_alias` call — we keep the original case here so unit
/// tests can read naturally.
///
/// Emits n-grams of length `1..=MAX_NGRAM_N` in order
/// (length-ascending, then left-to-right) for determinism. Stops
/// emitting once `MAX_MENTIONS` is reached.
///
/// Pure function: no I/O, no clock, no allocation reuse — safe for
/// the `EntityResolver` determinism contract.
///
/// Each returned [`Mention`] carries its token span `[start, start+len)`
/// so the resolver can apply specificity dedup (ISS-192).
fn extract_mentions(query: &str) -> Vec<Mention> {
    // Tokenize. We use `char::is_alphanumeric` as the "in-token"
    // predicate plus the apostrophe (U+0027) so contractions stay
    // together; everything else is a separator. This sidesteps the
    // unicode-segmentation dep — `is_alphanumeric` already handles
    // CJK + Latin + Cyrillic etc. via Unicode classification.
    let mut tokens: Vec<&str> = Vec::new();
    let mut start: Option<usize> = None;
    for (i, ch) in query.char_indices() {
        let is_word = ch.is_alphanumeric() || ch == '\'';
        match (start, is_word) {
            (None, true) => start = Some(i),
            (Some(s), false) => {
                tokens.push(&query[s..i]);
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        tokens.push(&query[s..]);
    }

    // Trim trailing/leading apostrophes; drop tokens that become
    // empty after trim, and strip possessive `'s` / `'S`.
    let cleaned: Vec<String> = tokens
        .into_iter()
        .filter_map(|t| {
            let trimmed = t.trim_matches('\'');
            if trimmed.is_empty() {
                return None;
            }
            // Strip possessive `'s` / `'S`.
            let stripped = trimmed
                .strip_suffix("'s")
                .or_else(|| trimmed.strip_suffix("'S"))
                .unwrap_or(trimmed);
            if stripped.is_empty() {
                None
            } else {
                Some(stripped.to_string())
            }
        })
        .collect();

    let n_tokens = cleaned.len();
    if n_tokens == 0 {
        return Vec::new();
    }

    // Emit n-grams length-ascending so 1-grams (cheapest, highest hit
    // rate for person names) come first. This ordering is part of
    // the determinism contract — callers may observe via test logs.
    // Each mention carries its token span [start, start+len) so the
    // resolver can apply specificity dedup (ISS-192): a mention whose
    // span is subsumed by a *longer* mention that also resolves is a
    // less-specific fragment (e.g. "support" / "group" inside
    // "LGBTQ support group") and must not crowd the precise entity out
    // of the `max_anchors` cap.
    let mut out: Vec<Mention> = Vec::new();
    for n in 1..=MAX_NGRAM_N {
        if n > n_tokens {
            break;
        }
        for (start, window) in cleaned.windows(n).enumerate() {
            if out.len() >= MAX_MENTIONS {
                return out;
            }
            // 1-gram length filter — see MIN_UNIGRAM_CHARS doc.
            if n == 1 && window[0].chars().count() < MIN_UNIGRAM_CHARS {
                continue;
            }
            out.push(Mention {
                text: window.join(" "),
                start,
                len: n,
            });
        }
    }
    out
}

/// Graph-backed [`EntityResolver`] for the Factual plan.
///
/// Holds a borrowed `&dyn GraphRead` whose lifetime is tied to a single
/// `Memory::graph_query` call (via `PlanCollaborators<'a>`).
pub struct GraphEntityResolver<'a> {
    pub graph: &'a dyn GraphRead,
}

impl<'a> GraphEntityResolver<'a> {
    pub fn new(graph: &'a dyn GraphRead) -> Self {
        Self { graph }
    }
}

impl<'a> EntityResolver for GraphEntityResolver<'a> {
    fn resolve(&self, query: &str) -> Vec<ResolvedAnchor> {
        // Empty / whitespace queries cannot resolve.
        if query.trim().is_empty() {
            return Vec::new();
        }

        // List namespaces. Failure → return empty (the Factual plan
        // surfaces `DowngradedNoEntity`, which is the correct behaviour
        // for "graph layer unavailable").
        let namespaces = match self.graph.list_namespaces() {
            Ok(ns) => ns,
            Err(_) => return Vec::new(),
        };

        // Deterministic `now` reference for recency scoring. We use the
        // current wall clock — see "no clock sampling" caveat in the
        // trait docstring. The resolver is expected to be "as of read
        // time" because Factual itself accepts `query_time` separately
        // for traversal; resolution-stage recency just orders the
        // anchor candidates and is reproducible *given* the same now.
        let now = Utc::now().timestamp() as f64;

        // ISS-165 fix: extract candidate mentions (token n-grams)
        // from the query, then look up each mention separately.
        // `search_candidates` does exact-equality matching on
        // `graph_entity_aliases.normalized`, so feeding it the whole
        // question (the pre-ISS-165 behaviour) cannot hit any alias
        // for natural-language queries.
        let mentions = extract_mentions(query);
        if mentions.is_empty() {
            return Vec::new();
        }

        let mut hits: Vec<(Mention, ResolvedAnchor)> = Vec::new();
        for ns in namespaces.into_iter().take(MAX_NAMESPACES_SCANNED) {
            for mention in &mentions {
                let candidate_query = CandidateQuery {
                    mention_text: mention.text.clone(),
                    mention_embedding: None,
                    kind_filter: None,
                    namespace: ns.clone(),
                    top_k: PER_NAMESPACE_TOP_K,
                    recency_window: None,
                    now,
                };

                let matches = match self.graph.search_candidates(&candidate_query) {
                    Ok(rows) => rows,
                    // Per-namespace / per-mention failure is non-fatal —
                    // keep scanning. An erroring lookup is observably
                    // identical to "no candidates here".
                    Err(_) => continue,
                };

                for m in matches {
                    // Score combines alias match (binary boost) +
                    // embedding (none here) + recency. We don't have
                    // embedding so the alias bit is dominant — this is
                    // fine for the Factual plan (it's a name-resolution
                    // step, not vector retrieval; see
                    // `HybridSeedRecaller` for the embedding path).
                    let alias_boost: f32 = if m.alias_match { 0.7 } else { 0.0 };
                    let recency_score = m.recency_score; // [0.0, 1.0]
                    // Final strength in [0.0, 1.0]: weight alias 70%,
                    // recency 30%. Tuned to keep alias-only hits above
                    // 0.5 (so the default `min_confidence` filter in
                    // Factual keeps them) while letting recency break
                    // ties between two equally alias-matched candidates.
                    let match_strength = alias_boost + 0.3 * recency_score;

                    // Skip candidates with neither signal — they're an
                    // artifact of search_candidates returning
                    // embedding-only hits we can't use here. (No
                    // embedding was sent → no embedding score → skip.)
                    if !m.alias_match && match_strength == 0.0 {
                        continue;
                    }

                    hits.push((
                        mention.clone(),
                        ResolvedAnchor {
                            entity_id: m.entity_id,
                            canonical_name: m.canonical_name,
                            match_strength,
                        },
                    ));
                }
            }
        }

        // ISS-192 specificity dedup. ISS-165's n-gram scan emits every
        // sub-span of a multi-word entity, so a query like "Caroline's
        // LGBTQ support group" resolves the precise entity *and* its
        // generic fragments ("support", "group", "support group") —
        // each as a full alias hit at strength 0.7. With the Factual
        // plan's `max_anchors` cap and recency-only tiebreak, the
        // precise (often least-recent) entity gets truncated out and
        // the gold-bearing edge never reaches the candidate pool.
        //
        // Fix: drop any hit whose mention span is strictly subsumed by
        // a *longer* mention that also resolved to ≥1 entity. This is
        // span-based (not entity-based), so a fragment that happens to
        // resolve to a distinct, legitimately-shorter entity is only
        // dropped when a longer overlapping mention actually resolved.
        // Inert when no multi-word mention resolves (e.g. all hits are
        // unigrams) — preserves pre-ISS-192 behaviour for those queries.
        let resolved_mentions: Vec<Mention> =
            hits.iter().map(|(mention, _)| mention.clone()).collect();
        hits.retain(|(mention, _)| {
            !resolved_mentions
                .iter()
                .any(|other| mention.subsumed_by(other))
        });

        // Dedupe by entity_id, keeping the highest match_strength. Sort
        // by (match_strength desc, entity_id asc) for determinism.
        hits.sort_by(|(_, a), (_, b)| {
            b.match_strength
                .partial_cmp(&a.match_strength)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.entity_id.cmp(&b.entity_id))
        });
        let mut seen = std::collections::HashSet::new();
        hits.retain(|(_, a)| seen.insert(a.entity_id));

        hits.into_iter().map(|(_, anchor)| anchor).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::store::{GraphWrite, SqliteGraphStore};
    use crate::graph::test_helpers::fresh_conn;
    use crate::graph::{Entity, EntityKind};

    fn write_entity(
        store: &mut SqliteGraphStore,
        canonical_name: &str,
        ns: &str,
    ) -> uuid::Uuid {
        let mut e = Entity::new_random_id(canonical_name.to_string(), EntityKind::Person, Utc::now());
        let id = e.id;
        // The default identity_confidence is 0.0; bump to 1.0 so the
        // search_candidates path treats it as a high-confidence anchor.
        e.identity_confidence = 1.0;
        let _ = ns; // namespace is set on the store, not the entity
        store.insert_entity(&e).expect("insert entity");
        // search_candidates does not match by canonical_name alone — it
        // requires a row in graph_entity_aliases (normalized form). Mirror
        // the production path by upserting a self-alias.
        store
            .upsert_alias(
                &canonical_name.to_lowercase(),
                canonical_name,
                id,
                None,
            )
            .expect("upsert alias");
        id
    }

    #[test]
    fn empty_query_returns_empty_anchors() {
        let mut conn = fresh_conn();
        let store = SqliteGraphStore::new(&mut conn);
        let resolver = GraphEntityResolver::new(&store);
        assert!(resolver.resolve("").is_empty());
        assert!(resolver.resolve("   ").is_empty());
    }

    #[test]
    fn alias_match_returns_anchor() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let id = write_entity(&mut store, "Caroline", "default");
        let resolver = GraphEntityResolver::new(&store);
        let hits = resolver.resolve("Caroline");
        assert!(
            !hits.is_empty(),
            "expected at least one anchor, got {hits:?}"
        );
        assert!(hits.iter().any(|h| h.entity_id == id));
        assert!(
            hits[0].match_strength >= 0.5,
            "alias match should score >= 0.5 to survive default min_confidence; got {}",
            hits[0].match_strength
        );
    }

    #[test]
    fn unknown_query_returns_empty() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let _ = write_entity(&mut store, "Caroline", "default");
        let resolver = GraphEntityResolver::new(&store);
        assert!(resolver.resolve("Zinedine").is_empty());
    }

    #[test]
    fn results_sorted_by_match_strength_desc() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let _ = write_entity(&mut store, "Caroline", "default");
        let _ = write_entity(&mut store, "Carolyn", "default");
        let resolver = GraphEntityResolver::new(&store);
        let hits = resolver.resolve("Caroline");
        // First hit (exact alias) should outrank any partial.
        if hits.len() >= 2 {
            assert!(
                hits[0].match_strength >= hits[1].match_strength,
                "results must be sorted by match_strength desc"
            );
        }
    }

    // ---------------------------------------------------------------
    // ISS-165: mention extraction regression tests.
    //
    // These tests cover the core failure mode: before the fix, the
    // resolver passed the entire natural-language query as a single
    // alias key, so multi-word questions never matched any entity.
    // ---------------------------------------------------------------

    #[test]
    fn natural_language_query_resolves_person_entity() {
        // Repro of ISS-165 AC-1 (conv-26 q3 shape).
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let cid = write_entity(&mut store, "Caroline", "default");
        let resolver = GraphEntityResolver::new(&store);
        let hits = resolver.resolve("What did Caroline research?");
        assert!(
            hits.iter().any(|h| h.entity_id == cid),
            "expected to anchor on Caroline; got {hits:?}"
        );
    }

    #[test]
    fn possessive_apostrophe_strips_correctly() {
        // Repro of ISS-165 AC-1 (conv-26 q7 / q71 shape).
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let cid = write_entity(&mut store, "Caroline", "default");
        let resolver = GraphEntityResolver::new(&store);
        let hits = resolver.resolve("What is Caroline's relationship status?");
        assert!(
            hits.iter().any(|h| h.entity_id == cid),
            "expected to anchor on Caroline after stripping 's; got {hits:?}"
        );
    }

    #[test]
    fn multi_word_entity_resolves_via_bigram() {
        // Repro of ISS-165 AC-1 (conv-26 q71 — "Becoming Nicole").
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let bid = write_entity(&mut store, "Becoming Nicole", "default");
        let resolver = GraphEntityResolver::new(&store);
        let hits = resolver.resolve("Did she like Becoming Nicole?");
        assert!(
            hits.iter().any(|h| h.entity_id == bid),
            "expected to anchor on Becoming Nicole via bigram; got {hits:?}"
        );
    }

    #[test]
    fn multiple_entities_in_one_query_all_resolve() {
        // Repro of ISS-165 AC-1 (conv-26 q11 — Caroline + Sweden).
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let cid = write_entity(&mut store, "Caroline", "default");
        let sid = write_entity(&mut store, "Sweden", "default");
        let resolver = GraphEntityResolver::new(&store);
        let hits = resolver.resolve("Where did Caroline move from before settling down in Sweden?");
        assert!(
            hits.iter().any(|h| h.entity_id == cid),
            "expected Caroline anchor; got {hits:?}"
        );
        assert!(
            hits.iter().any(|h| h.entity_id == sid),
            "expected Sweden anchor; got {hits:?}"
        );
    }

    #[test]
    fn single_char_token_filtered() {
        // Defensive: "I" / "a" / "1" must not generate alias-lookup
        // noise. If someone writes an entity literally called "a",
        // this test will start failing — that's the right time to
        // revisit `MIN_UNIGRAM_CHARS`.
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let aid = write_entity(&mut store, "a", "default");
        let resolver = GraphEntityResolver::new(&store);
        let hits = resolver.resolve("I went to a place yesterday");
        assert!(
            !hits.iter().any(|h| h.entity_id == aid),
            "expected NOT to anchor on single-char alias 'a'; got {hits:?}"
        );
    }

    #[test]
    fn unknown_natural_language_query_returns_empty() {
        // Negative: no relevant entities → still empty after the fix.
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let _ = write_entity(&mut store, "Caroline", "default");
        let resolver = GraphEntityResolver::new(&store);
        let hits = resolver.resolve("What is the price of bread in 2026?");
        assert!(
            hits.is_empty(),
            "expected empty result for query naming no known entity; got {hits:?}"
        );
    }

    #[test]
    fn iss192_specificity_dedup_drops_subsumed_fragments() {
        // Mirror of conv-26-q0: a query whose precise entity ("LGBTQ
        // support group") shares tokens with generic fragments
        // ("support", "group", "support group") that *also* have their
        // own alias entities. Pre-ISS-192 all four resolved at equal
        // strength and the precise (least-recent) one could be
        // truncated out by the Factual plan's max_anchors cap. After
        // the fix, the subsumed fragments are dropped because the
        // longer "LGBTQ support group" mention resolved.
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let caroline = write_entity(&mut store, "Caroline", "default");
        let precise = write_entity(&mut store, "LGBTQ support group", "default");
        let frag_pair = write_entity(&mut store, "support group", "default");
        let frag_support = write_entity(&mut store, "support", "default");
        let frag_group = write_entity(&mut store, "group", "default");

        let resolver = GraphEntityResolver::new(&store);
        let hits = resolver.resolve("When did Caroline go to the LGBTQ support group?");
        let ids: std::collections::HashSet<_> = hits.iter().map(|h| h.entity_id).collect();

        // Precise entities survive.
        assert!(
            ids.contains(&caroline),
            "Caroline must survive dedup; got {hits:?}"
        );
        assert!(
            ids.contains(&precise),
            "precise 'LGBTQ support group' must survive dedup; got {hits:?}"
        );
        // Subsumed fragments are dropped (their spans sit strictly
        // inside the longer "LGBTQ support group" mention that resolved).
        assert!(
            !ids.contains(&frag_pair),
            "'support group' fragment should be dropped; got {hits:?}"
        );
        assert!(
            !ids.contains(&frag_support),
            "'support' fragment should be dropped; got {hits:?}"
        );
        assert!(
            !ids.contains(&frag_group),
            "'group' fragment should be dropped; got {hits:?}"
        );
    }

    #[test]
    fn iss192_dedup_inert_when_no_longer_mention_resolves() {
        // Non-regression: a fragment that resolves to a distinct entity
        // is NOT dropped when no longer overlapping mention resolves.
        // Here only "support" has an alias entity; "LGBTQ support group"
        // and "support group" resolve to nothing, so "support" must
        // survive (the fix is span-subsumption-by-a-RESOLVED-longer-
        // mention, not blanket fragment suppression).
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let caroline = write_entity(&mut store, "Caroline", "default");
        let frag_support = write_entity(&mut store, "support", "default");

        let resolver = GraphEntityResolver::new(&store);
        let hits = resolver.resolve("Did Caroline need support?");
        let ids: std::collections::HashSet<_> = hits.iter().map(|h| h.entity_id).collect();
        assert!(ids.contains(&caroline), "Caroline must resolve; got {hits:?}");
        assert!(
            ids.contains(&frag_support),
            "'support' must survive when no longer mention resolves; got {hits:?}"
        );
    }

    // ---------------------------------------------------------------
    // Mention struct unit tests (span subsumption).
    // ---------------------------------------------------------------

    #[test]
    fn mention_subsumed_by_strict_containment() {
        // "support" (token 1, len 1) inside "LGBTQ support group"
        // (token 0, len 3).
        let frag = Mention { text: "support".into(), start: 1, len: 1 };
        let whole = Mention { text: "LGBTQ support group".into(), start: 0, len: 3 };
        assert!(frag.subsumed_by(&whole));
        // Equal spans are NOT subsumed (same length).
        let same = Mention { text: "LGBTQ support group".into(), start: 0, len: 3 };
        assert!(!whole.subsumed_by(&same));
        // Disjoint spans are not subsumed.
        let other = Mention { text: "Caroline".into(), start: 5, len: 1 };
        assert!(!other.subsumed_by(&whole));
        // A longer span is never subsumed by a shorter one.
        assert!(!whole.subsumed_by(&frag));
    }

    // ---------------------------------------------------------------
    // Pure mention-extraction tests (no graph layer).
    // ---------------------------------------------------------------

    #[test]
    fn extract_mentions_simple_sentence() {
        let m = extract_mentions("What did Caroline research?");
        // 1-grams: What, did, Caroline, research
        // 2-grams: "What did", "did Caroline", "Caroline research"
        // 3-grams: "What did Caroline", "did Caroline research"
        // 4-grams: "What did Caroline research"
        let texts: Vec<&str> = m.iter().map(|x| x.text.as_str()).collect();
        assert!(texts.contains(&"Caroline"));
        assert!(texts.contains(&"What did"));
        assert!(texts.contains(&"Caroline research"));
        assert!(texts.contains(&"What did Caroline research"));
    }

    #[test]
    fn extract_mentions_strips_possessive() {
        let m = extract_mentions("Caroline's hat");
        assert!(m.iter().any(|x| x.text == "Caroline"), "got: {m:?}");
        assert!(m.iter().any(|x| x.text == "hat"), "got: {m:?}");
        assert!(
            !m.iter().any(|x| x.text.contains("'s")),
            "possessive should be stripped; got: {m:?}"
        );
    }

    #[test]
    fn extract_mentions_filters_single_chars() {
        let m = extract_mentions("I a 1");
        // Each 1-gram is below MIN_UNIGRAM_CHARS=2, so all dropped.
        // The 2-gram "I a" / "a 1" / "I a 1" survive because the
        // length filter is only on n=1.
        assert!(
            !m.iter().any(|x| x.text == "I"),
            "single-char 1-gram should be filtered; got: {m:?}"
        );
        assert!(
            !m.iter().any(|x| x.text == "a"),
            "single-char 1-gram should be filtered; got: {m:?}"
        );
    }

    #[test]
    fn extract_mentions_empty_input() {
        assert!(extract_mentions("").is_empty());
        assert!(extract_mentions("   ").is_empty());
        assert!(extract_mentions("?!.,").is_empty());
    }

    #[test]
    fn extract_mentions_deterministic_order() {
        // Length-ascending, left-to-right — part of the
        // EntityResolver determinism contract.
        let m1 = extract_mentions("Alice met Bob");
        let m2 = extract_mentions("Alice met Bob");
        assert_eq!(m1, m2, "extract_mentions must be deterministic");
        // First 3 entries should be the 1-grams in input order.
        assert_eq!(m1[0].text, "Alice");
        assert_eq!(m1[1].text, "met");
        assert_eq!(m1[2].text, "Bob");
    }

    #[test]
    fn extract_mentions_respects_max_cap() {
        // Build a 200-token query; should cap at MAX_MENTIONS=64.
        let q: String = (0..200)
            .map(|i| format!("word{i} "))
            .collect();
        let m = extract_mentions(&q);
        assert!(
            m.len() <= MAX_MENTIONS,
            "expected ≤{MAX_MENTIONS} mentions, got {}",
            m.len()
        );
    }

    #[test]
    fn extract_mentions_unicode_alphanumeric() {
        // CJK / non-ASCII tokens should tokenize too (alphanumeric
        // is Unicode-aware).
        let m = extract_mentions("Caroline 去 Sweden");
        assert!(m.iter().any(|x| x.text == "Caroline"));
        // 去 is a single CJK char — passes is_alphanumeric, but is
        // single-char so filtered by MIN_UNIGRAM_CHARS.
        // Sweden is 6 chars, passes filter.
        assert!(m.iter().any(|x| x.text == "Sweden"));
    }
}
