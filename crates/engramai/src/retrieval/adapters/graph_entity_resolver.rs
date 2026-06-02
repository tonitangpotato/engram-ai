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
fn extract_mentions(query: &str) -> Vec<String> {
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
    let mut out: Vec<String> = Vec::new();
    for n in 1..=MAX_NGRAM_N {
        if n > n_tokens {
            break;
        }
        for window in cleaned.windows(n) {
            if out.len() >= MAX_MENTIONS {
                return out;
            }
            // 1-gram length filter — see MIN_UNIGRAM_CHARS doc.
            if n == 1 && window[0].chars().count() < MIN_UNIGRAM_CHARS {
                continue;
            }
            out.push(window.join(" "));
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

        // ISS-205: recency must break ties ACROSS the merged candidate
        // pool, not per isolated lookup. Each `search_candidates` call
        // resolves a single mention to (in v0) at most one alias hit, so
        // the per-call `recency_score` is computed over a one-element set
        // and is *always* 0.0 (min_last_seen == max_last_seen). That made
        // every alias hit tie at exactly 0.7, and the final order fell to
        // the deterministic `entity_id ASC` secondary sort — an arbitrary
        // UUID ordering with no relationship to anchor quality. For
        // conv-26 q0 ("When did Caroline go to the LGBTQ support group?")
        // this pushed the SUBJECT entity `Caroline` (which owns all the
        // dated `OccurredOn` edges) to index 5, below the Factual plan's
        // `max_anchors = 5` truncation, while five object-phrase fragments
        // ("Go", "group", "support", …) that own zero edges survived.
        //
        // Fix: collect raw candidates with their projected `last_seen`,
        // then recompute recency over the full deduped pool so the most
        // recently-touched entity (the live subject of the conversation)
        // outranks stale phrase fragments. This is the cross-pool tiebreak
        // the original doc-comment promised but the per-call computation
        // could never deliver.
        struct RawHit {
            entity_id: uuid::Uuid,
            canonical_name: String,
            alias_boost: f32,
            last_seen: f64,
        }

        let mut raw: Vec<RawHit> = Vec::new();
        for ns in namespaces.into_iter().take(MAX_NAMESPACES_SCANNED) {
            for mention in &mentions {
                let candidate_query = CandidateQuery {
                    mention_text: mention.clone(),
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
                    // We resolve by exact alias match only (no mention
                    // embedding is sent — this is a name-resolution step,
                    // not vector retrieval; see `HybridSeedRecaller` for
                    // the embedding path). Candidates that did NOT alias-
                    // match are embedding-only artifacts we can't score
                    // here, so skip them.
                    if !m.alias_match {
                        continue;
                    }
                    raw.push(RawHit {
                        entity_id: m.entity_id,
                        canonical_name: m.canonical_name,
                        alias_boost: 0.7,
                        last_seen: m.last_seen,
                    });
                }
            }
        }

        // Dedupe by entity_id first, keeping the freshest `last_seen`
        // (and, defensively, the highest alias_boost — currently uniform).
        // We dedupe BEFORE computing the recency span so a high-degree
        // entity reached via several mentions doesn't distort the scale.
        raw.sort_by(|a, b| {
            a.entity_id.cmp(&b.entity_id).then_with(|| {
                b.last_seen
                    .partial_cmp(&a.last_seen)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
        });
        raw.dedup_by_key(|h| h.entity_id);

        if raw.is_empty() {
            return Vec::new();
        }

        // Pool-wide recency scale. Single-candidate pools have a zero span
        // → every candidate gets recency 0.0, which collapses to the
        // alias-only score (0.7) — correct, there's nothing to break.
        let (min_ls, max_ls) = raw
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), h| {
                (lo.min(h.last_seen), hi.max(h.last_seen))
            });
        let span = max_ls - min_ls;

        let mut hits: Vec<ResolvedAnchor> = raw
            .into_iter()
            .map(|h| {
                // Linear recency in [0.0, 1.0] over the deduped pool span.
                let recency_score = if span > 0.0 {
                    ((h.last_seen - min_ls) / span) as f32
                } else {
                    0.0
                };
                // Final strength in [0.0, 1.0]: alias 70% + recency 30%.
                // Alias-only hits stay >= 0.5 (so the default
                // `min_confidence` filter in Factual keeps them); recency
                // breaks ties between equally alias-matched candidates by
                // favouring the most recently-touched entity.
                let match_strength = h.alias_boost + 0.3 * recency_score;
                ResolvedAnchor {
                    entity_id: h.entity_id,
                    canonical_name: h.canonical_name,
                    match_strength,
                }
            })
            .collect();

        // Final ordering: (match_strength desc, entity_id asc) for
        // determinism. Already deduped above.
        hits.sort_by(|a, b| {
            b.match_strength
                .partial_cmp(&a.match_strength)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.entity_id.cmp(&b.entity_id))
        });

        hits
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::store::{GraphWrite, SqliteGraphStore};
    use crate::graph::test_helpers::fresh_conn;
    use crate::graph::{Entity, EntityKind};

    fn write_entity(store: &mut SqliteGraphStore, canonical_name: &str, ns: &str) -> uuid::Uuid {
        let mut e =
            Entity::new_random_id(canonical_name.to_string(), EntityKind::Person, Utc::now());
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
            .upsert_alias(&canonical_name.to_lowercase(), canonical_name, id, None)
            .expect("upsert alias");
        id
    }

    /// Like [`write_entity`] but with an explicit `last_seen`, so tests can
    /// exercise the ISS-205 pool-wide recency tiebreak. `secs` is unix
    /// seconds; larger = more recent.
    fn write_entity_at(
        store: &mut SqliteGraphStore,
        canonical_name: &str,
        kind: EntityKind,
        secs: i64,
    ) -> uuid::Uuid {
        use chrono::TimeZone;
        let now = Utc.timestamp_opt(secs, 0).single().expect("valid ts");
        let mut e = Entity::new_random_id(canonical_name.to_string(), kind, now);
        let id = e.id;
        e.identity_confidence = 1.0;
        e.last_seen = now;
        store.insert_entity(&e).expect("insert entity");
        store
            .upsert_alias(&canonical_name.to_lowercase(), canonical_name, id, None)
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

    // ---------------------------------------------------------------
    // ISS-205: pool-wide recency tiebreak. Before the fix, every alias
    // hit tied at exactly 0.7 (per-call recency was always 0 over a
    // one-element set) and the final order fell to an arbitrary
    // `entity_id ASC` UUID sort. A high-degree SUBJECT entity could be
    // pushed below the Factual plan's max_anchors=5 truncation by
    // object-phrase fragments. The fix recomputes recency over the merged
    // pool so the most recently-touched entity ranks first.
    // ---------------------------------------------------------------

    #[test]
    fn iss205_recency_breaks_ties_subject_ranks_first() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        // The query subject "Caroline" is the most recently touched.
        // Five generic phrase fragments are older. All are exact alias
        // hits, so all share the 0.7 alias_boost — only recency can
        // separate them.
        let caroline = write_entity_at(&mut store, "Caroline", EntityKind::Person, 1_780_412_301);
        let _ = write_entity_at(&mut store, "support", EntityKind::Concept, 1_780_412_290);
        let _ = write_entity_at(&mut store, "group", EntityKind::Organization, 1_780_411_885);
        let _ = write_entity_at(
            &mut store,
            "support group",
            EntityKind::Organization,
            1_780_411_490,
        );
        let _ = write_entity_at(
            &mut store,
            "LGBTQ support group",
            EntityKind::Organization,
            1_780_411_483,
        );
        let _ = write_entity_at(&mut store, "Go", EntityKind::Concept, 1_780_411_990);

        let resolver = GraphEntityResolver::new(&store);
        let hits = resolver.resolve("When did Caroline go to the LGBTQ support group?");

        // Caroline (newest) must rank FIRST, ahead of the phrase fragments,
        // so it survives the downstream max_anchors=5 truncation.
        assert!(!hits.is_empty(), "expected anchors; got none");
        assert_eq!(
            hits[0].entity_id, caroline,
            "subject 'Caroline' (most recent) must rank first; got {hits:?}"
        );
        // Newest gets full recency → 0.7 + 0.3*1.0 = 1.0.
        assert!(
            (hits[0].match_strength - 1.0).abs() < 1e-4,
            "newest entity should score ~1.0; got {}",
            hits[0].match_strength
        );
        // Strictly descending strengths prove recency actually separated
        // the otherwise-tied alias hits (not an arbitrary UUID sort).
        for w in hits.windows(2) {
            assert!(
                w[0].match_strength >= w[1].match_strength,
                "strengths must be non-increasing; got {hits:?}"
            );
        }
        assert!(
            hits[0].match_strength > hits[1].match_strength,
            "recency must produce a strict gap at the top; got {hits:?}"
        );
    }

    #[test]
    fn iss205_single_alias_hit_collapses_to_alias_only_score() {
        // Degenerate pool (one candidate): span is 0, recency contributes
        // nothing, score collapses to the alias-only 0.7 — no divide-by-zero,
        // no spurious boost.
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let _ = write_entity_at(&mut store, "Caroline", EntityKind::Person, 1_780_412_301);
        let resolver = GraphEntityResolver::new(&store);
        let hits = resolver.resolve("Caroline");
        assert_eq!(hits.len(), 1, "expected exactly one anchor; got {hits:?}");
        assert!(
            (hits[0].match_strength - 0.7).abs() < 1e-4,
            "single hit should score exactly 0.7 (alias only); got {}",
            hits[0].match_strength
        );
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
        assert!(m.contains(&"Caroline".to_string()));
        assert!(m.contains(&"What did".to_string()));
        assert!(m.contains(&"Caroline research".to_string()));
        assert!(m.contains(&"What did Caroline research".to_string()));
    }

    #[test]
    fn extract_mentions_strips_possessive() {
        let m = extract_mentions("Caroline's hat");
        assert!(m.contains(&"Caroline".to_string()), "got: {m:?}");
        assert!(m.contains(&"hat".to_string()), "got: {m:?}");
        assert!(
            !m.iter().any(|s| s.contains("'s")),
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
            !m.contains(&"I".to_string()),
            "single-char 1-gram should be filtered; got: {m:?}"
        );
        assert!(
            !m.contains(&"a".to_string()),
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
        assert_eq!(m1[0], "Alice");
        assert_eq!(m1[1], "met");
        assert_eq!(m1[2], "Bob");
    }

    #[test]
    fn extract_mentions_respects_max_cap() {
        // Build a 200-token query; should cap at MAX_MENTIONS=64.
        let q: String = (0..200).map(|i| format!("word{i} ")).collect();
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
        assert!(m.contains(&"Caroline".to_string()));
        // 去 is a single CJK char — passes is_alphanumeric, but is
        // single-char so filtered by MIN_UNIGRAM_CHARS.
        // Sweden is 6 chars, passes filter.
        assert!(m.contains(&"Sweden".to_string()));
    }
}
