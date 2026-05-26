//! # Cross-encoder reranker (ISS-159 weapon A)
//!
//! Cross-encoder reranker that scores `(query, doc)` pairs with an ONNX
//! transformer (default: `cross-encoder/ms-marco-MiniLM-L-6-v2` via the
//! Xenova HF mirror). Lifts the AC-5a single-fact bucket on conv-26 by
//! replacing the fusion ordering on the **head** of the candidate pool
//! with a true cross-attention relevance signal.
//!
//! ## Where it sits
//!
//! Same Stage C.5 hook as [`MmrReranker`](super::mmr::MmrReranker), but
//! composed **before** MMR when both are wired (cross-encoder reorders
//! by quality, then MMR diversifies that quality-sorted list — running
//! MMR on raw fusion picks "diverse mediocre" instead of "diverse top").
//!
//! ## Algorithm
//!
//! 1. Split input into `head = candidates[..k_in.min(len)]` and
//!    `tail = candidates[k_in.min(len)..]`.
//! 2. For each head candidate: tokenize `(query, text)`, run ONNX
//!    inference, take the single-logit output, apply sigmoid to project
//!    into `[0, 1]` (the reranker contract).
//! 3. Sort `head` by sigmoid score descending. Concatenate with
//!    untouched `tail`. Return.
//!
//! ## Text extraction
//!
//! - `ScoredResult::Memory` — `record.content`.
//! - `ScoredResult::Topic`  — `"{title}\n{summary}"` (both — title alone
//!   is too short for the encoder; summary alone loses the topic label).
//!
//! ## Score preservation (contract property 3)
//!
//! Unlike MMR (which preserves fusion scores), the cross-encoder
//! **replaces** the head scores with sigmoid(logit). This is correct:
//!
//! - The whole point is that cross-attention is a better signal than
//!   fusion for the top of the list.
//! - The explain-trace can still recover the fusion score from
//!   `sub_scores`; only the top-level `score` changes.
//! - `sigmoid(x) ∈ (0, 1)` strictly — no NaN, no clamp needed.
//!
//! Tail scores are untouched (they keep their fusion score). This means
//! head scores and tail scores aren't on the same scale anymore — but
//! ordering across the boundary is preserved by construction: every head
//! item came in with score >= every tail item, and after rerank we put
//! all head items (in whatever order) before any tail item.
//!
//! ## Purity (contract property 1)
//!
//! ONNX inference with a fixed-seed model and fixed input is bitwise
//! deterministic on the same hardware. We use `Mutex<Session>` for
//! interior mutability (`Session::run` requires `&mut`; the trait gives
//! us `&self`). Lock contention is a non-issue in production — rerank is
//! single-threaded per query.
//!
//! ## Latency (contract property 2)
//!
//! Spike on M1 (commit `9b743b7`): 1.5ms per pair, 76ms for a 50-pair
//! batch. Budget at K_fusion=50 is ~80ms — well inside the §7.2 stage
//! cap. No internal yielding needed.
//!
//! ## Feature gate
//!
//! Behind `cross_encoder` feature flag — pulls ~50MB native ORT runtime
//! and a tokenizer crate. Default builds skip both.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use ndarray::Array2;
use ort::session::{builder::GraphOptimizationLevel, Session};
use ort::value::TensorRef;
use tokenizers::Tokenizer;

use super::super::api::{RetrievalError, ScoredResult};
use super::reranker::Reranker;

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Configuration for [`CrossEncoderReranker`].
///
/// Default model is `ms-marco-MiniLM-L-6-v2` (22MB ONNX, ~1.5ms/pair on
/// M1). Override `model_path` / `tokenizer_path` to swap in a different
/// cross-encoder (e.g. `bge-reranker-base` as ISS-159 fallback).
#[derive(Debug, Clone)]
pub struct CrossEncoderConfig {
    /// Path to the ONNX model file (e.g.
    /// `~/.cache/engram/models/ms-marco-MiniLM-L-6-v2/model.onnx`).
    pub model_path: PathBuf,
    /// Path to the `tokenizer.json` next to the model.
    pub tokenizer_path: PathBuf,
    /// Number of head candidates to rerank. Tail candidates beyond this
    /// are passed through with their fusion scores. Default 50 (matches
    /// ISS-159 D3 K_fusion choice).
    pub k_in: usize,
    /// Threads for ONNX intra-op parallelism. Default 4 (matches spike).
    pub intra_threads: usize,
}

impl CrossEncoderConfig {
    /// Construct a config pointing at the default Xenova MiniLM model
    /// under `~/.cache/engram/models/ms-marco-MiniLM-L-6-v2/`.
    ///
    /// The caller is responsible for ensuring the model + tokenizer
    /// files exist; [`CrossEncoderReranker::new`] surfaces a clear
    /// error if they don't.
    pub fn default_minilm() -> Self {
        let base = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join(".cache/engram/models/ms-marco-MiniLM-L-6-v2");
        Self {
            model_path: base.join("model.onnx"),
            tokenizer_path: base.join("tokenizer.json"),
            k_in: 50,
            intra_threads: 4,
        }
    }
}

// ---------------------------------------------------------------------------
// Reranker
// ---------------------------------------------------------------------------

/// Cross-encoder reranker — ONNX-backed, single-logit, sigmoid-normalized.
pub struct CrossEncoderReranker {
    session: Mutex<Session>,
    tokenizer: Tokenizer,
    k_in: usize,
}

impl std::fmt::Debug for CrossEncoderReranker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CrossEncoderReranker")
            .field("k_in", &self.k_in)
            .finish_non_exhaustive()
    }
}

impl CrossEncoderReranker {
    /// Load the model + tokenizer from disk and build a session.
    ///
    /// Returns [`RetrievalError::ConfigError`] if the model or tokenizer
    /// file is missing or unreadable, or [`RetrievalError::Internal`] if
    /// the ONNX runtime fails to initialize the session.
    pub fn new(cfg: &CrossEncoderConfig) -> Result<Self, RetrievalError> {
        if !cfg.model_path.exists() {
            return Err(RetrievalError::ConfigError(format!(
                "cross_encoder model not found: {}",
                cfg.model_path.display()
            )));
        }
        if !cfg.tokenizer_path.exists() {
            return Err(RetrievalError::ConfigError(format!(
                "cross_encoder tokenizer not found: {}",
                cfg.tokenizer_path.display()
            )));
        }

        let tokenizer = Tokenizer::from_file(&cfg.tokenizer_path)
            .map_err(|e| RetrievalError::ConfigError(format!("tokenizer load: {e}")))?;

        let session = build_session(&cfg.model_path, cfg.intra_threads)?;

        Ok(Self {
            session: Mutex::new(session),
            tokenizer,
            k_in: cfg.k_in,
        })
    }

    /// Score a single `(query, doc)` pair. Returns sigmoid(logit) ∈ (0, 1).
    fn score_pair(&self, query: &str, doc: &str) -> Result<f64, RetrievalError> {
        let enc = self
            .tokenizer
            .encode((query, doc), true)
            .map_err(|e| RetrievalError::Internal(format!("tokenize: {e}")))?;

        let ids: Vec<i64> = enc.get_ids().iter().map(|&x| x as i64).collect();
        let mask: Vec<i64> = enc
            .get_attention_mask()
            .iter()
            .map(|&x| x as i64)
            .collect();
        let type_ids: Vec<i64> = enc.get_type_ids().iter().map(|&x| x as i64).collect();
        let seq_len = ids.len();

        let ids_arr = Array2::from_shape_vec((1, seq_len), ids)
            .map_err(|e| RetrievalError::Internal(format!("ids shape: {e}")))?;
        let mask_arr = Array2::from_shape_vec((1, seq_len), mask)
            .map_err(|e| RetrievalError::Internal(format!("mask shape: {e}")))?;
        let type_arr = Array2::from_shape_vec((1, seq_len), type_ids)
            .map_err(|e| RetrievalError::Internal(format!("type shape: {e}")))?;

        let mut session = self
            .session
            .lock()
            .map_err(|_| RetrievalError::Internal("session mutex poisoned".into()))?;

        let outputs = session
            .run(ort::inputs![
                "input_ids" => TensorRef::from_array_view(&ids_arr)
                    .map_err(|e| RetrievalError::Internal(format!("ids tensor: {e}")))?,
                "attention_mask" => TensorRef::from_array_view(&mask_arr)
                    .map_err(|e| RetrievalError::Internal(format!("mask tensor: {e}")))?,
                "token_type_ids" => TensorRef::from_array_view(&type_arr)
                    .map_err(|e| RetrievalError::Internal(format!("type tensor: {e}")))?,
            ])
            .map_err(|e| RetrievalError::Internal(format!("session.run: {e}")))?;

        let (_shape, data) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| RetrievalError::Internal(format!("extract logits: {e}")))?;

        let logit = *data
            .first()
            .ok_or_else(|| RetrievalError::Internal("empty logits".into()))?;

        Ok(sigmoid(logit as f64))
    }
}

/// Standard sigmoid: `1 / (1 + exp(-x))`. Always in `(0, 1)`, never NaN
/// for finite x. Satisfies contract property 3 by construction.
fn sigmoid(x: f64) -> f64 {
    1.0 / (1.0 + (-x).exp())
}

/// Extract text for cross-encoder scoring. Memory uses `content`; Topic
/// uses `"{title}\n{summary}"` (both — title alone is too short for the
/// encoder, summary alone loses the topic label).
fn extract_text(r: &ScoredResult) -> String {
    match r {
        ScoredResult::Memory { record, .. } => record.content.clone(),
        ScoredResult::Topic { topic, .. } => {
            format!("{}\n{}", topic.title, topic.summary)
        }
    }
}

/// Replace the score on a [`ScoredResult`], preserving variant + payload.
fn with_score(mut r: ScoredResult, new_score: f64) -> ScoredResult {
    match &mut r {
        ScoredResult::Memory { score, .. } | ScoredResult::Topic { score, .. } => {
            *score = new_score;
        }
    }
    r
}

fn build_session(model_path: &Path, intra_threads: usize) -> Result<Session, RetrievalError> {
    Session::builder()
        .map_err(|e| RetrievalError::Internal(format!("Session::builder: {e}")))?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|e| RetrievalError::Internal(format!("opt_level: {e}")))?
        .with_intra_threads(intra_threads)
        .map_err(|e| RetrievalError::Internal(format!("intra_threads: {e}")))?
        .commit_from_file(model_path)
        .map_err(|e| {
            RetrievalError::ConfigError(format!(
                "commit_from_file({}): {e}",
                model_path.display()
            ))
        })
}

impl Reranker for CrossEncoderReranker {
    fn rerank(
        &self,
        query: &str,
        candidates: &[ScoredResult],
    ) -> Result<Vec<ScoredResult>, RetrievalError> {
        if candidates.is_empty() {
            return Ok(Vec::new());
        }

        let head_n = self.k_in.min(candidates.len());

        // Score the head; leave tail untouched.
        let mut head: Vec<(ScoredResult, f64)> = Vec::with_capacity(head_n);
        for c in &candidates[..head_n] {
            let text = extract_text(c);
            let s = self.score_pair(query, &text)?;
            head.push((c.clone(), s));
        }

        // Sort head by new score, descending. Stable sort to keep
        // input-order tie-breaks for purity.
        head.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let mut out: Vec<ScoredResult> =
            Vec::with_capacity(candidates.len());
        for (item, score) in head {
            out.push(with_score(item, score));
        }
        // Tail: pass through unchanged.
        for c in &candidates[head_n..] {
            out.push(c.clone());
        }

        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::reranker::{assert_reranker_contract, ContractCheck};
    use crate::graph::topic::KnowledgeTopic;
    use crate::retrieval::api::SubScores;
    use crate::types::{MemoryLayer, MemoryRecord, MemoryType};
    use std::path::PathBuf;
    use uuid::Uuid;

    // -- fixture builders ---------------------------------------------------

    fn mk_record(id: &str, content: &str) -> MemoryRecord {
        MemoryRecord {
            id: id.to_string(),
            content: content.to_string(),
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

    fn mk_memory(id: &str, content: &str, score: f64) -> ScoredResult {
        ScoredResult::Memory {
            record: mk_record(id, content),
            score,
            sub_scores: SubScores::default(),
            embedding: None,
        }
    }

    fn default_cfg(k_in: usize) -> CrossEncoderConfig {
        let mut cfg = CrossEncoderConfig::default_minilm();
        cfg.k_in = k_in;
        cfg
    }

    /// Skip a test cleanly when the model isn't on disk — keeps CI green
    /// on machines that haven't downloaded the 87MB ONNX. Local dev (and
    /// the bench harness) always has it.
    fn require_model() -> Option<CrossEncoderConfig> {
        let cfg = default_cfg(50);
        if !cfg.model_path.exists() || !cfg.tokenizer_path.exists() {
            eprintln!(
                "[cross_encoder tests] skipping — model not found at {}",
                cfg.model_path.display()
            );
            return None;
        }
        Some(cfg)
    }

    // -- tests --------------------------------------------------------------

    #[test]
    fn passes_contract() {
        let Some(cfg) = require_model() else { return };
        let r = CrossEncoderReranker::new(&cfg).expect("load");
        let candidates = vec![
            mk_memory("a", "Paris is the capital of France.", 0.9),
            mk_memory("b", "The mitochondrion is the powerhouse of the cell.", 0.7),
            mk_memory("c", "France has many cities including Lyon and Marseille.", 0.5),
        ];
        let check = ContractCheck {
            latency_budget: std::time::Duration::from_secs(2),
            determinism_repeats: 3,
        };
        assert_reranker_contract(&r, "What is the capital of France?", &candidates, &check)
            .expect("contract");
    }

    #[test]
    fn reorders_relevant_above_irrelevant() {
        let Some(cfg) = require_model() else { return };
        let r = CrossEncoderReranker::new(&cfg).expect("load");
        // Fusion got it backwards on purpose — the irrelevant one is
        // first, the relevant one is last. Cross-encoder should fix it.
        let candidates = vec![
            mk_memory("irrelevant", "The mitochondrion is the powerhouse of the cell.", 0.95),
            mk_memory("partial", "France is a country in Western Europe.", 0.80),
            mk_memory("relevant", "Paris is the capital and most populous city of France.", 0.50),
        ];
        let out = r
            .rerank("What is the capital of France?", &candidates)
            .expect("rerank");

        assert_eq!(out.len(), 3, "no drops");
        let first_id = match &out[0] {
            ScoredResult::Memory { record, .. } => record.id.as_str(),
            _ => panic!("expected Memory"),
        };
        let last_id = match &out[2] {
            ScoredResult::Memory { record, .. } => record.id.as_str(),
            _ => panic!("expected Memory"),
        };
        assert_eq!(first_id, "relevant", "Paris should rank first");
        assert_eq!(last_id, "irrelevant", "mitochondrion should rank last");
    }

    #[test]
    fn preserves_tail_beyond_k_in() {
        let Some(_) = require_model() else { return };
        let cfg = default_cfg(3);
        let r = CrossEncoderReranker::new(&cfg).expect("load");

        // 10 candidates, k_in=3. Tail = 7 items, must come out in input
        // order with input scores untouched.
        let mut candidates = Vec::new();
        for i in 0..10 {
            // Mix relevant + irrelevant so head reorder is meaningful.
            let content = if i % 2 == 0 {
                format!("Paris fact number {i}: it is the capital of France.")
            } else {
                format!("Random fact {i}: cells contain mitochondria.")
            };
            // Strictly decreasing input scores so head/tail boundary is
            // unambiguous.
            let score = 1.0 - (i as f64) * 0.05;
            candidates.push(mk_memory(&format!("id{i}"), &content, score));
        }

        let out = r
            .rerank("What is the capital of France?", &candidates)
            .expect("rerank");

        assert_eq!(out.len(), 10, "no drops");

        // Tail (out[3..]) must equal input[3..] item-for-item, score-for-score.
        for i in 3..10 {
            let out_id = match &out[i] {
                ScoredResult::Memory { record, .. } => record.id.as_str(),
                _ => panic!("expected Memory"),
            };
            let in_id = match &candidates[i] {
                ScoredResult::Memory { record, .. } => record.id.as_str(),
                _ => panic!("expected Memory"),
            };
            assert_eq!(out_id, in_id, "tail item {i} reordered");

            let out_score = out[i].score();
            let in_score = candidates[i].score();
            assert!(
                (out_score - in_score).abs() < 1e-9,
                "tail item {i} score changed: {in_score} -> {out_score}"
            );
        }

        // Head scores must all be in (0, 1) — sigmoid output.
        for i in 0..3 {
            let s = out[i].score();
            assert!(s > 0.0 && s < 1.0, "head[{i}] score {s} not in (0,1)");
        }
    }

    #[test]
    fn missing_model_returns_error() {
        let cfg = CrossEncoderConfig {
            model_path: PathBuf::from("/nonexistent/model.onnx"),
            tokenizer_path: PathBuf::from("/nonexistent/tokenizer.json"),
            k_in: 50,
            intra_threads: 4,
        };
        let err = CrossEncoderReranker::new(&cfg).expect_err("must error");
        match err {
            RetrievalError::ConfigError(msg) => {
                assert!(msg.contains("model not found"), "got: {msg}");
            }
            other => panic!("expected ConfigError, got {other:?}"),
        }
    }

    #[test]
    fn empty_input_returns_empty() {
        let Some(cfg) = require_model() else { return };
        let r = CrossEncoderReranker::new(&cfg).expect("load");
        let out = r.rerank("anything", &[]).expect("rerank");
        assert!(out.is_empty());
    }

    #[test]
    fn topic_uses_title_plus_summary() {
        // Pure unit test — no model needed. Just verifies the text
        // extractor uses both fields.
        let topic = KnowledgeTopic::new(
            Uuid::new_v4(),
            "TitleX".to_string(),
            "SummaryY".to_string(),
            "default".to_string(),
            0.0,
        );
        let r = ScoredResult::Topic {
            topic,
            score: 0.5,
            source_memories: vec![],
            contributing_entities: vec![],
        };
        let text = extract_text(&r);
        assert!(text.contains("TitleX"));
        assert!(text.contains("SummaryY"));
    }
}
