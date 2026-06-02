//! # MMR reranker (ISS-139, `task:retr-impl-reranker-mmr`)
//!
//! Maximal Marginal Relevance reranker — diversifies the top-K of fused
//! candidates so list-style queries (LoCoMo Mode A: "what foods does X
//! eat?") don't collapse into N paraphrases of one item.
//!
//! ## Where it sits
//!
//! - Stage C produces a `Vec<ScoredResult>` sorted by fused score (desc).
//! - This reranker is invoked **post-fusion, pre-top-K-truncate** at
//!   `retrieval/api.rs` (single chokepoint for all 7 plans, see ISS-139
//!   §"Hook location").
//! - The output is a permutation of the input (no drops).
//!
//! ## Formula (greedy MMR, Carbonell & Goldstein 1998)
//!
//! For each output slot `i`:
//!
//! ```text
//! mmr(c) = λ * rel(c, q) − (1 − λ) * max_{s ∈ selected} sim(c, s)
//! ```
//!
//! - `rel(c, q)` — fused relevance score, normalized to `[0, 1]`. We use
//!   the candidate's existing `ScoredResult::score()` (already in `[0, 1]`
//!   by fusion contract).
//! - `sim(c, s)` — cosine similarity on candidate embeddings, clamped to
//!   `[0, 1]` (negative cosine treated as 0 — diversity from
//!   anti-correlated vectors is not "more diverse" than orthogonal).
//! - `λ ∈ [0, 1]`. `1.0` = pure relevance (== input order, no MMR effect).
//!   `0.0` = pure diversity (don't use). Literature recommends `0.5..0.8`.
//!
//! ## Score preservation (contract property 3)
//!
//! MMR is used **for ordering only**. Output scores equal input scores
//! (the fused score). This:
//!
//! 1. Trivially satisfies the `[0, 1]` / no-NaN invariant — fusion
//!    already guarantees it.
//! 2. Keeps the explain-trace honest: the score the caller sees is the
//!    fusion score, not a synthesized MMR score that would need its own
//!    explainer.
//! 3. Preserves byte-identity at `λ = 1.0` (output order = input order,
//!    output scores = input scores).
//!
//! ## Topics & non-Memory variants
//!
//! `ScoredResult::Topic` has no embedding — diversity between topics is
//! not well-defined in this embedding space. We **leave Topics at their
//! original indices** and rerank only the Memory subsequence around
//! them. This:
//!
//! - Preserves the multiset (contract property 4).
//! - Doesn't punish Topics for not having embeddings.
//! - Keeps Hybrid plan's Topic interleaving (RRF-driven) intact.
//!
//! ## Missing embeddings on Memory candidates
//!
//! Adapters that don't have an embedding in hand set `embedding: None`
//! (factual / episodic / associative / affective — see
//! `retrieval/orchestrator.rs`). For those candidates we **skip the
//! diversity penalty** (treat their `max_sim` to selected as 0). This:
//!
//! - Keeps them eligible for ranking — they aren't dropped or pushed to
//!   the tail (which would silently change recall on plans that never
//!   carry embeddings).
//! - Means: on plans with **no** populated embeddings (factual / episodic
//!   / associative / affective today), MMR degenerates to pure
//!   relevance — i.e. it's a no-op. That's correct: you can't
//!   diversify what you can't measure.
//! - On Hybrid (the plan that *does* carry embeddings via
//!   `HybridSeedRecaller`, ISS-139 Strategy B), MMR fires properly.
//!
//! Future work: opt-in Storage-backed fallback (`get_embedding` per
//! candidate) for plans that want diversity without paying the per-call
//! adapter wiring cost. Tracked in ISS-139 follow-ups.

use super::super::api::{RetrievalError, ScoredResult};
use super::reranker::Reranker;

/// MMR reranker. Construct with λ ∈ [0, 1].
///
/// **Purity**: same `(query, candidates, lambda)` → same output. The
/// `query` argument to `rerank()` is **ignored** — MMR's relevance
/// term reuses the pre-computed fused score on the candidate (the
/// fusion pipeline already encoded query-relevance into it; re-scoring
/// with cosine-to-query here would double-count). Diversity is
/// measured between candidates, not against the query.
///
/// This keeps the constructor minimal — see ISS-139 §"Constructor
/// shape" for the design rationale (rejected: `Storage`-aware fallback
/// path; rejected: capturing query embedding for relevance rescoring).
#[derive(Debug, Clone)]
pub struct MmrReranker {
    /// Diversity / relevance trade-off. `1.0` = pure relevance.
    lambda: f32,
}

impl MmrReranker {
    /// Construct a new MMR reranker.
    ///
    /// # Panics
    ///
    /// Panics if `lambda` is NaN or outside `[0, 1]`. MMR with `λ`
    /// outside that range is meaningless — fail-fast at construction
    /// rather than silently producing garbage rankings.
    pub fn new(lambda: f32) -> Self {
        assert!(
            !lambda.is_nan() && (0.0..=1.0).contains(&lambda),
            "MmrReranker: lambda must be in [0, 1], got {lambda}"
        );
        Self { lambda }
    }

    /// Current λ value. Test-only accessor.
    #[cfg(test)]
    pub(crate) fn lambda(&self) -> f32 {
        self.lambda
    }
}

/// Cosine similarity on two equal-length vectors, clamped to `[0, 1]`.
///
/// Returns `0.0` when either vector is empty, lengths mismatch, or
/// either norm is zero. Negative cosines (anti-correlated vectors) are
/// clamped to `0.0` — see module docs.
fn cosine_clamped(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    let cos = dot / (na.sqrt() * nb.sqrt());
    cos.clamp(0.0, 1.0)
}

impl Reranker for MmrReranker {
    fn rerank(
        &self,
        _query: &str,
        candidates: &[ScoredResult],
    ) -> Result<Vec<ScoredResult>, RetrievalError> {
        // Fast paths.
        if candidates.is_empty() {
            return Ok(Vec::new());
        }
        if self.lambda >= 1.0 {
            // Pure relevance: input is already score-sorted, return as-is.
            // Byte-identical to NullReranker at λ=1.0.
            return Ok(candidates.to_vec());
        }
        if candidates.len() == 1 {
            return Ok(candidates.to_vec());
        }

        // Split indices into Memory (MMR-eligible) and Topic (passthrough).
        // We record the *original positions* of every Topic so we can
        // reinsert them at the same indices in the output.
        let mut topic_slots: Vec<(usize, ScoredResult)> = Vec::new();
        let mut memory_indices: Vec<usize> = Vec::with_capacity(candidates.len());
        for (i, c) in candidates.iter().enumerate() {
            match c {
                ScoredResult::Memory { .. } => memory_indices.push(i),
                ScoredResult::Topic { .. } => topic_slots.push((i, c.clone())),
            }
        }

        // Degenerate case: no Memory items → nothing to MMR-rerank.
        if memory_indices.is_empty() {
            return Ok(candidates.to_vec());
        }

        // Greedy MMR over the Memory subsequence.
        //
        // `remaining` is a Vec<usize> of indices into `candidates` that
        // are still unselected. `selected_order` collects the MMR order.
        let mut remaining: Vec<usize> = memory_indices;
        let mut selected_order: Vec<usize> = Vec::with_capacity(remaining.len());
        // Cache for embeddings of *selected* items, to avoid re-borrowing
        // through `candidates[...]` inside the hot loop. None = no
        // embedding for that selected item (diversity penalty skipped
        // against it).
        let mut selected_embeddings: Vec<Option<&[f32]>> = Vec::new();

        // Seed: pick the highest-relevance Memory first. (Stable: ties
        // resolved by lowest original index, which matches input order.)
        let seed_pos = remaining
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| {
                let sa = candidates[**a].score();
                let sb = candidates[**b].score();
                sa.partial_cmp(&sb).unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(p, _)| p)
            .expect("remaining non-empty checked above");
        let seed_idx = remaining.swap_remove(seed_pos);
        selected_order.push(seed_idx);
        selected_embeddings.push(memory_embedding(&candidates[seed_idx]));

        // Greedy fill.
        while !remaining.is_empty() {
            let mut best_pos: usize = 0;
            let mut best_score: f32 = f32::NEG_INFINITY;
            for (pos, &idx) in remaining.iter().enumerate() {
                let rel = candidates[idx].score() as f32;
                let cand_emb = memory_embedding(&candidates[idx]);
                let max_sim = match cand_emb {
                    // Candidate has no embedding → can't measure
                    // diversity from it, skip penalty (treat as 0).
                    None => 0.0,
                    Some(ce) => selected_embeddings
                        .iter()
                        .filter_map(|s| s.map(|se| cosine_clamped(ce, se)))
                        .fold(0.0f32, f32::max),
                };
                let mmr = self.lambda * rel - (1.0 - self.lambda) * max_sim;
                // Tie-break: prefer lower original index (stable wrt
                // input ordering, matches fused-score order on ties).
                let is_better =
                    mmr > best_score || (mmr == best_score && idx < remaining[best_pos]);
                if is_better {
                    best_score = mmr;
                    best_pos = pos;
                }
            }
            let chosen = remaining.swap_remove(best_pos);
            selected_order.push(chosen);
            selected_embeddings.push(memory_embedding(&candidates[chosen]));
        }

        // Rebuild output: Topics at their original indices, Memory items
        // in MMR order filling the remaining slots in left-to-right order.
        let mut out: Vec<Option<ScoredResult>> = vec![None; candidates.len()];
        for (idx, item) in &topic_slots {
            out[*idx] = Some(item.clone());
        }
        let mut mem_iter = selected_order.into_iter();
        for slot in out.iter_mut() {
            if slot.is_none() {
                let mem_idx = mem_iter.next().expect("count matches");
                *slot = Some(candidates[mem_idx].clone());
            }
        }
        Ok(out.into_iter().map(|s| s.expect("filled")).collect())
    }
}

/// Extract the embedding slice from a `ScoredResult::Memory`, or `None`
/// if absent / not a Memory variant.
fn memory_embedding(r: &ScoredResult) -> Option<&[f32]> {
    match r {
        ScoredResult::Memory { embedding, .. } => embedding.as_deref(),
        ScoredResult::Topic { .. } => None,
    }
}

/// ISS-188 — backfill missing candidate embeddings before MMR.
///
/// MMR's diversity term needs per-candidate embeddings, but the
/// Factual/Episodic plans build `ScoredResult::Memory` candidates with
/// `embedding == None`. Without vectors, MMR gives them a 0 diversity
/// penalty and degenerates to a no-op on exactly the plans that serve
/// list-questions (ISS-187).
///
/// This walks `ranked`, collects the ids of Memory candidates still
/// missing an embedding, asks `fetch` for them in **one** batch, and
/// backfills the `embedding` field in place. Candidates whose id the
/// fetcher doesn't return (deleted / superseded / no stored vector)
/// keep `None` and MMR treats them as maximally diverse — the same
/// behaviour as before, no candidate is dropped.
///
/// `fetch` takes the id slice and returns an id → vector map. It is
/// injected (rather than taking `&Storage`) so this stays a pure,
/// unit-testable function decoupled from the SQL layer.
pub fn populate_missing_embeddings<F>(ranked: &mut [ScoredResult], fetch: F)
where
    F: FnOnce(&[&str]) -> std::collections::HashMap<String, Vec<f32>>,
{
    let missing_ids: Vec<String> = ranked
        .iter()
        .filter_map(|r| match r {
            ScoredResult::Memory {
                record, embedding, ..
            } if embedding.is_none() => Some(record.id.clone()),
            _ => None,
        })
        .collect();
    if missing_ids.is_empty() {
        return;
    }
    let id_refs: Vec<&str> = missing_ids.iter().map(String::as_str).collect();
    let embs = fetch(&id_refs);
    if embs.is_empty() {
        return;
    }
    for r in ranked.iter_mut() {
        if let ScoredResult::Memory {
            record, embedding, ..
        } = r
        {
            if embedding.is_none() {
                if let Some(v) = embs.get(&record.id) {
                    *embedding = Some(v.clone());
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::KnowledgeTopic;
    use crate::retrieval::api::SubScores;
    use crate::retrieval::fusion::reranker::{assert_reranker_contract, ContractCheck};
    use crate::types::{MemoryLayer, MemoryRecord, MemoryType};
    use uuid::Uuid;

    // -- fixture builders ---------------------------------------------------

    fn mk_record(id: &str) -> MemoryRecord {
        MemoryRecord {
            id: id.to_string(),
            content: format!("memory-{id}"),
            memory_type: MemoryType::Factual,
            layer: MemoryLayer::Working,
            created_at: chrono::Utc::now(),
            occurred_at: None,
            access_times: vec![],
            working_strength: 0.0,
            core_strength: 0.0,
            importance: 0.0,
            pinned: false,
            consolidation_count: 0,
            last_consolidated: None,
            source: String::new(),
            contradicts: None,
            contradicted_by: None,
            superseded_by: None,
            metadata: None,
        }
    }

    fn mk_memory(id: &str, score: f64, emb: Option<Vec<f32>>) -> ScoredResult {
        ScoredResult::Memory {
            record: mk_record(id),
            score,
            sub_scores: SubScores::default(),
            embedding: emb,
            reserved: false,
        }
    }

    fn mk_topic(score: f64) -> ScoredResult {
        ScoredResult::Topic {
            topic: KnowledgeTopic::new(
                Uuid::new_v4(),
                "topic-x".to_string(),
                String::new(),
                "default".to_string(),
                0.0,
            ),
            score,
            source_memories: Vec::new(),
            contributing_entities: Vec::new(),
        }
    }

    fn ids(rs: &[ScoredResult]) -> Vec<String> {
        rs.iter()
            .map(|r| match r {
                ScoredResult::Memory { record, .. } => format!("m:{}", record.id),
                ScoredResult::Topic { topic, .. } => format!("t:{}", topic.title),
            })
            .collect()
    }

    /// Three near-duplicate "apple" candidates + one distinct "car"
    /// candidate. With λ=1.0 the apples cluster wins. With λ low enough
    /// the car should be pulled up.
    fn list_fixture() -> Vec<ScoredResult> {
        // 3D unit-ish embeddings: apples cluster around [1,0,0], car around [0,1,0].
        let apple_a = vec![1.0, 0.0, 0.0];
        let apple_b = vec![0.98, 0.02, 0.0];
        let apple_c = vec![0.95, 0.05, 0.0];
        let car = vec![0.0, 1.0, 0.0];
        vec![
            mk_memory("apple-a", 0.95, Some(apple_a)),
            mk_memory("apple-b", 0.90, Some(apple_b)),
            mk_memory("apple-c", 0.85, Some(apple_c)),
            mk_memory("car", 0.80, Some(car)),
        ]
    }

    // -- λ=1.0 byte-identical regression -----------------------------------

    #[test]
    fn lambda_one_is_byte_identical_to_input_order() {
        let rr = MmrReranker::new(1.0);
        let input = list_fixture();
        let out = rr.rerank("q", &input).unwrap();
        assert_eq!(ids(&out), ids(&input));
        // Scores preserved bit-exact.
        for (a, b) in input.iter().zip(out.iter()) {
            assert_eq!(a.score(), b.score());
        }
    }

    // -- λ low enough → car gets pulled up --------------------------------

    #[test]
    fn low_lambda_diversifies_apple_cluster() {
        // λ=0.3: diversity dominates. Apple-A wins seed (highest rel).
        // Next: car (sim to apple ≈ 0) wins over apple-b (sim ≈ 1).
        let rr = MmrReranker::new(0.3);
        let out = rr.rerank("q", &list_fixture()).unwrap();
        let order = ids(&out);
        assert_eq!(order[0], "m:apple-a", "seed = top-rel");
        assert_eq!(order[1], "m:car", "diversity pick = distinct cluster");
    }

    // -- intermediate λ stays relevance-biased ---------------------------

    #[test]
    fn lambda_zero_seven_keeps_seed_top() {
        // λ=0.7 is the production-target default; seed is always top.
        let rr = MmrReranker::new(0.7);
        let out = rr.rerank("q", &list_fixture()).unwrap();
        let order = ids(&out);
        assert_eq!(order[0], "m:apple-a");
        // With λ=0.7: rel(apple-b)=0.9, sim≈0.9998 → mmr ≈ 0.63 - 0.3*1 = 0.33
        // rel(car)=0.80, sim≈0 → mmr = 0.7*0.8 - 0 = 0.56
        // So car should still win slot 2.
        assert_eq!(order[1], "m:car");
    }

    // -- multiset preservation: missing-embedding candidates kept --------

    #[test]
    fn candidates_with_no_embedding_are_not_dropped() {
        let rr = MmrReranker::new(0.5);
        let input = vec![
            mk_memory("with-emb", 0.9, Some(vec![1.0, 0.0])),
            mk_memory("no-emb-1", 0.85, None),
            mk_memory("no-emb-2", 0.80, None),
        ];
        let out = rr.rerank("q", &input).unwrap();
        assert_eq!(out.len(), input.len());
        let mut got: Vec<String> = ids(&out);
        let mut want: Vec<String> = ids(&input);
        got.sort();
        want.sort();
        assert_eq!(got, want);
    }

    // -- topics stay at their original positions --------------------------

    #[test]
    fn topics_preserve_their_original_indices() {
        let rr = MmrReranker::new(0.3);
        let input = vec![
            mk_memory("apple-a", 0.95, Some(vec![1.0, 0.0, 0.0])),
            mk_topic(0.92),
            mk_memory("apple-b", 0.90, Some(vec![0.98, 0.02, 0.0])),
            mk_memory("car", 0.80, Some(vec![0.0, 1.0, 0.0])),
        ];
        let out = rr.rerank("q", &input).unwrap();
        // Topic was at index 1 in input → must be at index 1 in output.
        match &out[1] {
            ScoredResult::Topic { .. } => {}
            other => panic!("expected Topic at index 1, got {other:?}"),
        }
        // Memory slots are 0, 2, 3 — should contain MMR-reordered memories.
        let mem_ids: Vec<String> = [0usize, 2, 3]
            .iter()
            .map(|i| match &out[*i] {
                ScoredResult::Memory { record, .. } => record.id.clone(),
                _ => panic!("expected Memory"),
            })
            .collect();
        assert_eq!(mem_ids[0], "apple-a", "seed");
        assert_eq!(mem_ids[1], "car", "diversity pick");
        assert_eq!(mem_ids[2], "apple-b");
    }

    // -- empty input -------------------------------------------------------

    #[test]
    fn empty_input_returns_empty_output() {
        let rr = MmrReranker::new(0.5);
        let out = rr.rerank("q", &[]).unwrap();
        assert!(out.is_empty());
    }

    // -- contract assertions at multiple λ values ------------------------

    #[test]
    fn satisfies_contract_at_lambda_one_zero() {
        let rr = MmrReranker::new(1.0);
        let input = list_fixture();
        assert_reranker_contract(&rr, "q", &input, &ContractCheck::default())
            .expect("contract @ λ=1.0");
    }

    #[test]
    fn satisfies_contract_at_lambda_zero_nine() {
        let rr = MmrReranker::new(0.9);
        let input = list_fixture();
        assert_reranker_contract(&rr, "q", &input, &ContractCheck::default())
            .expect("contract @ λ=0.9");
    }

    #[test]
    fn satisfies_contract_at_lambda_zero_seven() {
        let rr = MmrReranker::new(0.7);
        let input = list_fixture();
        assert_reranker_contract(&rr, "q", &input, &ContractCheck::default())
            .expect("contract @ λ=0.7");
    }

    #[test]
    fn satisfies_contract_at_lambda_zero_five() {
        let rr = MmrReranker::new(0.5);
        let input = list_fixture();
        assert_reranker_contract(&rr, "q", &input, &ContractCheck::default())
            .expect("contract @ λ=0.5");
    }

    #[test]
    fn satisfies_contract_at_lambda_zero_zero() {
        let rr = MmrReranker::new(0.0);
        let input = list_fixture();
        assert_reranker_contract(&rr, "q", &input, &ContractCheck::default())
            .expect("contract @ λ=0.0");
    }

    // -- contract on mixed Memory + Topic --------------------------------

    #[test]
    fn satisfies_contract_with_topics_mixed_in() {
        let rr = MmrReranker::new(0.5);
        let input = vec![
            mk_memory("m1", 0.95, Some(vec![1.0, 0.0])),
            mk_topic(0.90),
            mk_memory("m2", 0.85, Some(vec![0.0, 1.0])),
            mk_topic(0.80),
            mk_memory("m3", 0.75, None),
        ];
        assert_reranker_contract(&rr, "q", &input, &ContractCheck::default())
            .expect("contract @ mixed");
    }

    // -- λ validation -----------------------------------------------------

    #[test]
    #[should_panic(expected = "lambda must be in [0, 1]")]
    fn rejects_lambda_above_one() {
        let _ = MmrReranker::new(1.5);
    }

    #[test]
    #[should_panic(expected = "lambda must be in [0, 1]")]
    fn rejects_lambda_negative() {
        let _ = MmrReranker::new(-0.1);
    }

    #[test]
    #[should_panic(expected = "lambda must be in [0, 1]")]
    fn rejects_lambda_nan() {
        let _ = MmrReranker::new(f32::NAN);
    }

    // -- cosine_clamped sanity --------------------------------------------

    #[test]
    fn cosine_identical_vectors_is_one() {
        let v = vec![0.5, 0.5, 0.5];
        assert!((cosine_clamped(&v, &v) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_orthogonal_is_zero() {
        assert_eq!(cosine_clamped(&[1.0, 0.0], &[0.0, 1.0]), 0.0);
    }

    #[test]
    fn cosine_anti_correlated_clamped_to_zero() {
        // [1,0] · [-1,0] = -1 → clamp to 0.
        assert_eq!(cosine_clamped(&[1.0, 0.0], &[-1.0, 0.0]), 0.0);
    }

    #[test]
    fn cosine_mismatched_length_is_zero() {
        assert_eq!(cosine_clamped(&[1.0, 0.0], &[1.0]), 0.0);
    }

    #[test]
    fn cosine_empty_is_zero() {
        assert_eq!(cosine_clamped(&[], &[]), 0.0);
    }

    #[test]
    fn cosine_zero_norm_is_zero() {
        assert_eq!(cosine_clamped(&[0.0, 0.0], &[1.0, 0.0]), 0.0);
    }

    // -- lambda() accessor ------------------------------------------------

    #[test]
    fn lambda_accessor_roundtrips() {
        let rr = MmrReranker::new(0.7);
        assert_eq!(rr.lambda(), 0.7);
    }

    // -- ISS-188: populate_missing_embeddings ------------------------------

    #[test]
    fn populate_backfills_missing_memory_embeddings() {
        // Two Factual candidates with no embedding (the production
        // state on Factual/Episodic plans). The fetcher returns a
        // vector for each → both fields get populated.
        let mut ranked = vec![mk_memory("m1", 0.9, None), mk_memory("m2", 0.8, None)];
        populate_missing_embeddings(&mut ranked, |ids| {
            assert_eq!(ids.len(), 2, "both missing ids requested in one batch");
            let mut m = std::collections::HashMap::new();
            m.insert("m1".to_string(), vec![1.0, 0.0, 0.0]);
            m.insert("m2".to_string(), vec![0.0, 1.0, 0.0]);
            m
        });
        assert_eq!(
            memory_embedding(&ranked[0]),
            Some([1.0, 0.0, 0.0].as_slice())
        );
        assert_eq!(
            memory_embedding(&ranked[1]),
            Some([0.0, 1.0, 0.0].as_slice())
        );
    }

    #[test]
    fn populate_leaves_unreturned_ids_as_none() {
        // The fetcher only knows m1 (m2 deleted / no stored vector).
        // m2 stays None → MMR will treat it as maximally diverse, no
        // candidate is dropped.
        let mut ranked = vec![mk_memory("m1", 0.9, None), mk_memory("m2", 0.8, None)];
        populate_missing_embeddings(&mut ranked, |_ids| {
            let mut m = std::collections::HashMap::new();
            m.insert("m1".to_string(), vec![1.0, 0.0, 0.0]);
            m
        });
        assert_eq!(
            memory_embedding(&ranked[0]),
            Some([1.0, 0.0, 0.0].as_slice())
        );
        assert_eq!(memory_embedding(&ranked[1]), None);
    }

    #[test]
    fn populate_does_not_overwrite_existing_embeddings() {
        // A candidate that already carries an embedding (Hybrid plan)
        // must not be re-fetched or clobbered.
        let mut ranked = vec![mk_memory("m1", 0.9, Some(vec![9.0, 9.0, 9.0]))];
        populate_missing_embeddings(&mut ranked, |ids| {
            assert!(ids.is_empty(), "no missing ids → fetcher gets empty slice");
            std::collections::HashMap::new()
        });
        assert_eq!(
            memory_embedding(&ranked[0]),
            Some([9.0, 9.0, 9.0].as_slice())
        );
    }

    #[test]
    fn populate_then_low_lambda_diversifies_previously_dead_cluster() {
        // The end-to-end ISS-188 contract: candidates arrive with NO
        // embeddings (MMR would be a no-op), we backfill them with a
        // redundant-cluster layout, and λ<1.0 then reorders to surface
        // the distant candidate into the head — proving the diversity
        // channel comes alive once fed.
        //
        // Layout: three "apple" candidates near [1,0,0] (high score) and
        // one "car" near [0,1,0] (lower score). Pre-backfill MMR cannot
        // distinguish them. Post-backfill, λ=0.7 should pull `car` up
        // past at least one redundant apple.
        let mut ranked = vec![
            mk_memory("apple-a", 0.95, None),
            mk_memory("apple-b", 0.90, None),
            mk_memory("apple-c", 0.85, None),
            mk_memory("car", 0.80, None),
        ];

        // Sanity: with no embeddings, λ=0.7 leaves order unchanged
        // (every candidate has 0 diversity penalty → pure relevance).
        let dead = MmrReranker::new(0.7).rerank("q", &ranked).unwrap();
        assert_eq!(
            ids(&dead),
            vec!["m:apple-a", "m:apple-b", "m:apple-c", "m:car"],
            "no embeddings → MMR degenerates to relevance order"
        );

        // Backfill the redundant-cluster embeddings.
        populate_missing_embeddings(&mut ranked, |_ids| {
            let mut m = std::collections::HashMap::new();
            m.insert("apple-a".to_string(), vec![1.0, 0.0, 0.0]);
            m.insert("apple-b".to_string(), vec![0.98, 0.02, 0.0]);
            m.insert("apple-c".to_string(), vec![0.95, 0.05, 0.0]);
            m.insert("car".to_string(), vec![0.0, 1.0, 0.0]);
            m
        });

        // Now MMR can see the redundancy: `car` should rise above at
        // least one apple it was originally ranked below.
        let alive = MmrReranker::new(0.7).rerank("q", &ranked).unwrap();
        let order = ids(&alive);
        let car_pos = order.iter().position(|s| s == "m:car").unwrap();
        assert!(
            car_pos < 3,
            "post-backfill λ=0.7 should surface the diverse `car` out of \
             the tail; got order {order:?}"
        );
        assert_ne!(
            order,
            vec!["m:apple-a", "m:apple-b", "m:apple-c", "m:car"],
            "backfill must change the order vs the dead-channel baseline"
        );
    }
}
