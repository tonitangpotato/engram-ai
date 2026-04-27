//! Pluggable LLM and embedder abstractions for K3 synthesis.
//!
//! The Knowledge Compiler calls out to two external services per cluster:
//! a **summarizer** (LLM that produces a short title + multi-sentence
//! summary, design §5bis.4 step 2) and an **embedder** (produces a vector
//! over the summary, design §5bis.4 step 3). Wiring them as traits keeps
//! the synthesis pipeline unit-testable: production wires real LLM /
//! embedding clients; tests use deterministic stubs.
//!
//! ## Why not reuse [`crate::embeddings::EmbeddingClient`] directly?
//!
//! - That client is concrete and assumes Ollama/OpenAI HTTP endpoints. The
//!   Knowledge Compiler does not need that coupling — it just needs "give
//!   me a vector for this string". The trait is a one-method shim that
//!   `EmbeddingClient` already satisfies via a thin adapter (callers in
//!   production code construct the adapter; out of scope for this task).
//! - Tests need to inject deterministic vectors without an HTTP server.
//!
//! Same reasoning applies to the summarizer — there is no global
//! `LlmClient` trait in this crate yet (`anthropic_client.rs` is HTTP
//! plumbing, not a `summarize`-shaped abstraction). Defining one *here*,
//! scoped to the compiler's needs, is the right granularity.

use std::error::Error;
use std::fmt;

// ════════════════════════════════════════════════════════════════════════
//  Summarizer
// ════════════════════════════════════════════════════════════════════════

/// Output of one summarization call: short title + multi-sentence summary.
///
/// Both fields are required (design §5bis.4 step 2). An empty title is
/// considered a summarizer failure — the contract is "title that names
/// the cluster", and an unnamed topic violates the operator's view of
/// `list_topics`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub title: String,
    pub summary: String,
}

/// Abstraction over an LLM that produces topic summaries.
///
/// Implementations should be deterministic given the same inputs (helps
/// reproducibility / debugging) but determinism is not strictly required —
/// the design only mandates retry-with-backoff (§5bis.4 step 2). The
/// production Anthropic-backed implementation lives outside this module
/// (a future `crate::knowledge_compile::providers::anthropic` or similar).
pub trait Summarizer {
    /// Produce a `Summary` from a list of memory contents and the names of
    /// entities that span them.
    ///
    /// `contributing_entity_names` is best-effort: pass the empty slice if
    /// the producer doesn't care to surface entity context to the LLM.
    fn summarize(
        &self,
        memory_contents: &[&str],
        contributing_entity_names: &[&str],
    ) -> Result<Summary, SummarizeError>;
}

/// Failure category for [`Summarizer::summarize`].
#[derive(Debug)]
pub enum SummarizeError {
    /// LLM call failed transiently. Caller may retry per §5bis.4 step 2.
    Transient(Box<dyn Error + Send + Sync>),
    /// LLM call failed permanently (auth, model unsupported, malformed
    /// prompt, etc.). Per §5bis.4 step 2 + §5bis.5: the cluster is
    /// recorded as a `graph_extraction_failures` row and skipped.
    Permanent(Box<dyn Error + Send + Sync>),
    /// Empty title or summary returned. Treated as permanent.
    EmptyOutput,
}

impl fmt::Display for SummarizeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transient(e) => write!(f, "summarizer transient error: {e}"),
            Self::Permanent(e) => write!(f, "summarizer permanent error: {e}"),
            Self::EmptyOutput => write!(f, "summarizer returned empty title or summary"),
        }
    }
}
impl Error for SummarizeError {}

/// Deterministic stub: takes the first sentence of the first memory as the
/// title, joins the first sentences of all memories as the summary.
///
/// Useful for: unit tests, the "no LLM configured" graceful path, and any
/// deployment that wants topic naming without paying for an LLM call. Not
/// a substitute for a real LLM in production — the summaries it produces
/// are not coherent across long memory clusters — but the public surface
/// of the Knowledge Compiler is unchanged.
pub struct FirstSentenceSummarizer;

impl Summarizer for FirstSentenceSummarizer {
    fn summarize(
        &self,
        memory_contents: &[&str],
        _contributing_entity_names: &[&str],
    ) -> Result<Summary, SummarizeError> {
        if memory_contents.is_empty() {
            return Err(SummarizeError::EmptyOutput);
        }
        let first_sentence = |s: &str| -> String {
            s.split(['.', '?', '!'])
                .next()
                .unwrap_or(s)
                .trim()
                .to_string()
        };
        let title = first_sentence(memory_contents[0]);
        if title.is_empty() {
            return Err(SummarizeError::EmptyOutput);
        }
        let body = memory_contents
            .iter()
            .map(|c| first_sentence(c))
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(". ");
        if body.is_empty() {
            return Err(SummarizeError::EmptyOutput);
        }
        Ok(Summary { title, summary: body })
    }
}

// ════════════════════════════════════════════════════════════════════════
//  Embedder
// ════════════════════════════════════════════════════════════════════════

/// Abstraction over a text embedder used by K3 to embed the produced summary.
///
/// Per design §5bis.4 step 3: "Embed the summary using the same embedder
/// as memory embeddings (same dimensionality, same model version) so
/// retrieval's vector search can pool topics and memories in a single
/// index." The dimensionality contract lives at the call site — the
/// caller validates `vec.len() == graph_layer.embedding_dim`.
pub trait Embedder {
    /// Embed `text` into a fixed-dim vector.
    fn embed(&self, text: &str) -> Result<Vec<f32>, EmbedError>;

    /// Reported embedding dimensionality (used by callers to validate
    /// before persisting). Must match the v03-graph-layer expected dim
    /// configured on the `SqliteGraphStore`.
    fn dim(&self) -> usize;
}

#[derive(Debug)]
pub enum EmbedError {
    Transient(Box<dyn Error + Send + Sync>),
    Permanent(Box<dyn Error + Send + Sync>),
    /// Returned vector did not match the embedder's reported dimension.
    DimMismatch { expected: usize, got: usize },
}

impl fmt::Display for EmbedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transient(e) => write!(f, "embedder transient error: {e}"),
            Self::Permanent(e) => write!(f, "embedder permanent error: {e}"),
            Self::DimMismatch { expected, got } => {
                write!(f, "embedder dim mismatch: expected {expected}, got {got}")
            }
        }
    }
}
impl Error for EmbedError {}

/// Deterministic stub embedder: hashes input bytes into a fixed-dim
/// pseudo-random vector. **Test-only** — the vectors are not semantically
/// meaningful, but they are stable for a given input (idempotence tests
/// can rely on `embed(s) == embed(s)`).
///
/// Used for unit tests of the synthesis pipeline that need to assert
/// "embedding was written" without standing up a real embedding service.
pub struct IdentityEmbedder {
    dim: usize,
}

impl IdentityEmbedder {
    pub fn new(dim: usize) -> Self {
        assert!(dim > 0, "embedder dim must be positive");
        Self { dim }
    }
}

impl Embedder for IdentityEmbedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>, EmbedError> {
        // Deterministic: SipHash-style mixing into `dim` floats.
        // Not cryptographic — purely for stability under the same input.
        let mut out = Vec::with_capacity(self.dim);
        let bytes = text.as_bytes();
        for i in 0..self.dim {
            let mut h: u64 = 0x9e3779b97f4a7c15u64.wrapping_mul(i as u64 + 1);
            for (j, b) in bytes.iter().enumerate() {
                h = h
                    .wrapping_add((*b as u64).wrapping_mul((j as u64).wrapping_add(1)))
                    .rotate_left(7)
                    ^ 0x517cc1b727220a95u64;
            }
            // Map u64 → f32 in [-1, 1]
            let f = ((h as i64 as f32) / (i64::MAX as f32)).clamp(-1.0, 1.0);
            out.push(f);
        }
        Ok(out)
    }

    fn dim(&self) -> usize {
        self.dim
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_sentence_summarizer_picks_first_sentence() {
        let s = FirstSentenceSummarizer;
        let out = s
            .summarize(
                &["Alice met Bob in Paris. They had coffee.", "Bob is a pianist."],
                &[],
            )
            .unwrap();
        assert_eq!(out.title, "Alice met Bob in Paris");
        assert!(out.summary.contains("Alice met Bob in Paris"));
        assert!(out.summary.contains("Bob is a pianist"));
    }

    #[test]
    fn first_sentence_summarizer_empty_input_errors() {
        let s = FirstSentenceSummarizer;
        match s.summarize(&[], &[]) {
            Err(SummarizeError::EmptyOutput) => {}
            other => panic!("expected EmptyOutput, got {other:?}"),
        }
    }

    #[test]
    fn first_sentence_summarizer_blank_first_memory_errors() {
        let s = FirstSentenceSummarizer;
        match s.summarize(&["   ", "...."], &[]) {
            Err(SummarizeError::EmptyOutput) => {}
            other => panic!("expected EmptyOutput, got {other:?}"),
        }
    }

    #[test]
    fn identity_embedder_is_deterministic() {
        let e = IdentityEmbedder::new(16);
        let a = e.embed("hello world").unwrap();
        let b = e.embed("hello world").unwrap();
        assert_eq!(a, b);
        assert_eq!(a.len(), 16);
    }

    #[test]
    fn identity_embedder_different_inputs_differ() {
        let e = IdentityEmbedder::new(16);
        let a = e.embed("hello").unwrap();
        let b = e.embed("world").unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn identity_embedder_reports_dim() {
        let e = IdentityEmbedder::new(384);
        assert_eq!(e.dim(), 384);
        assert_eq!(e.embed("anything").unwrap().len(), 384);
    }
}
