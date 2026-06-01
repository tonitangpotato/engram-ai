//! Production adapters for the [`Summarizer`] and [`Embedder`] traits.
//!
//! The traits in [`super::summarizer`] are deliberately scoped to the
//! Knowledge Compiler's needs (one method each). This module wires those
//! traits onto the existing engramai infrastructure so `Memory::compile_knowledge`
//! can run K3 against real services without re-implementing HTTP plumbing:
//!
//! - [`EmbeddingProviderAdapter`] wraps an existing
//!   [`crate::embeddings::EmbeddingProvider`] (the same one used for
//!   memory embeddings — design §5bis.4 step 3 mandates "same model, same
//!   dim" so retrieval can pool topics and memories in one index).
//! - [`AnthropicSummarizer`] talks to the Anthropic API for K3 summary
//!   generation, reusing [`crate::anthropic_client`] for headers/auth and
//!   the same OAuth/API-key pattern as [`crate::extractor::AnthropicExtractor`].
//!
//! Tests still use [`super::summarizer::FirstSentenceSummarizer`] and
//! [`super::summarizer::IdentityEmbedder`] for hermetic, deterministic
//! pipelines; the production code path picks the adapters here.

use std::error::Error;
use std::time::Duration;

use crate::anthropic_client::{build_anthropic_headers, StaticToken, DEFAULT_ANTHROPIC_API_URL};
use crate::embeddings::EmbeddingProvider;
use crate::extractor::TokenProvider;

use super::summarizer::{EmbedError, Embedder, SummarizeError, Summarizer, Summary};

// ════════════════════════════════════════════════════════════════════════
//  Embedder adapter
// ════════════════════════════════════════════════════════════════════════

/// Wraps a borrowed [`EmbeddingProvider`] as the [`Embedder`] trait the
/// Knowledge Compiler expects.
///
/// Borrows the provider rather than owning it because `Memory` already
/// holds the canonical instance — duplicating the HTTP client would waste
/// connections and could pick up stale config if the operator hot-swaps.
///
/// All `EmbeddingError` variants are mapped as **transient** *except* the
/// ones that indicate a misconfiguration the run can never recover from
/// on retry (model not found, parse error, empty response). Per design
/// §5bis.4 step 3 + §5bis.5, transient errors retry with backoff and
/// permanent errors fail the cluster (recorded as `graph_extraction_failures`,
/// the run continues with the next cluster).
pub struct EmbeddingProviderAdapter<'a> {
    provider: &'a EmbeddingProvider,
}

impl<'a> EmbeddingProviderAdapter<'a> {
    pub fn new(provider: &'a EmbeddingProvider) -> Self {
        Self { provider }
    }
}

impl<'a> Embedder for EmbeddingProviderAdapter<'a> {
    fn embed(&self, text: &str) -> Result<Vec<f32>, EmbedError> {
        let expected_dim = self.dim();
        let vec = self.provider.embed(text).map_err(|e| {
            use crate::embeddings::EmbeddingError as E;
            match e {
                // Network/connection level — retry the whole cluster's K3
                // step. The next attempt may succeed if Ollama recovers
                // or the network blip clears.
                E::OllamaNotAvailable(_) | E::Timeout | E::Request(_) => {
                    EmbedError::Transient(Box::new(e))
                }
                // Configuration-level: retrying won't help.
                E::ModelNotFound(_) | E::Parse(_) | E::EmptyResponse | E::Storage(_) => {
                    EmbedError::Permanent(Box::new(e))
                }
            }
        })?;
        if vec.len() != expected_dim {
            return Err(EmbedError::DimMismatch {
                expected: expected_dim,
                got: vec.len(),
            });
        }
        Ok(vec)
    }

    fn dim(&self) -> usize {
        self.provider.config().dimensions
    }
}

// ════════════════════════════════════════════════════════════════════════
//  Summarizer adapter — Anthropic
// ════════════════════════════════════════════════════════════════════════

/// Configuration for [`AnthropicSummarizer`].
///
/// Defaults match `AnthropicExtractorConfig` (Haiku, 30s timeout) — Haiku
/// is the right cost/speed tradeoff for K3 since each cluster is summarized
/// independently and we may run many per compile.
#[derive(Debug, Clone)]
pub struct AnthropicSummarizerConfig {
    pub model: String,
    pub api_url: String,
    /// Max tokens for the summary response. K3 produces a short title +
    /// a multi-sentence summary, so 1024 is comfortable.
    pub max_tokens: usize,
    pub timeout_secs: u64,
}

impl Default for AnthropicSummarizerConfig {
    fn default() -> Self {
        Self {
            model: "claude-haiku-4-5-20251001".to_string(),
            api_url: DEFAULT_ANTHROPIC_API_URL.to_string(),
            max_tokens: 1024,
            timeout_secs: 30,
        }
    }
}

/// Production [`Summarizer`] backed by Anthropic Claude.
///
/// Built on the same auth pattern as [`crate::extractor::AnthropicExtractor`]:
/// supports both static API keys and dynamic OAuth tokens (Claude Max) via
/// the [`TokenProvider`] trait. Uses `reqwest::blocking` to match the rest
/// of the engramai sync API surface (the Knowledge Compiler runs on a
/// background thread, not in async context).
///
/// ## Prompt design
///
/// The prompt asks the model for **strict JSON output** with two fields:
/// `title` and `summary`. We parse and validate; an empty/missing field
/// becomes [`SummarizeError::EmptyOutput`]. Markdown fences (```json…```)
/// are stripped before parsing because Claude sometimes wraps JSON in them
/// despite explicit instructions not to.
///
/// ## Error classification
///
/// - HTTP 5xx, timeouts, connect errors → [`SummarizeError::Transient`].
/// - HTTP 4xx (auth, malformed prompt, model-not-found), JSON parse
///   failures, missing fields → [`SummarizeError::Permanent`].
/// - Empty `title` or `summary` → [`SummarizeError::EmptyOutput`].
pub struct AnthropicSummarizer {
    config: AnthropicSummarizerConfig,
    token_provider: Box<dyn TokenProvider>,
    is_oauth: bool,
    client: reqwest::blocking::Client,
}

impl AnthropicSummarizer {
    /// Create with a static auth token (API key or fixed OAuth bearer).
    pub fn new(auth_token: &str, is_oauth: bool) -> Self {
        Self::with_config(auth_token, is_oauth, AnthropicSummarizerConfig::default())
    }

    /// Create with a static token and custom config.
    pub fn with_config(
        auth_token: &str,
        is_oauth: bool,
        config: AnthropicSummarizerConfig,
    ) -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()
            .expect("failed to create HTTP client");
        Self {
            config,
            token_provider: Box::new(StaticToken(auth_token.to_string())),
            is_oauth,
            client,
        }
    }

    /// Create with a dynamic [`TokenProvider`] — tokens are refreshed per
    /// request, so OAuth flows that auto-refresh stay valid across long
    /// compile runs.
    pub fn with_token_provider(
        provider: Box<dyn TokenProvider>,
        is_oauth: bool,
        config: AnthropicSummarizerConfig,
    ) -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()
            .expect("failed to create HTTP client");
        Self {
            config,
            token_provider: provider,
            is_oauth,
            client,
        }
    }

    fn build_headers(&self) -> Result<reqwest::header::HeaderMap, Box<dyn Error + Send + Sync>> {
        let token = self.token_provider.get_token()?;
        Ok(build_anthropic_headers(&token, self.is_oauth))
    }
}

/// K3 system + user prompt. Kept verbatim here (not in a config file) so
/// changes are reviewable in git history alongside the parsing logic that
/// depends on them — the prompt and parser are a tight pair.
const K3_PROMPT_PREFIX: &str = r#"You are summarizing a cluster of memories into a single knowledge topic.

Produce a short, descriptive title (max 80 chars) that names the cluster, and a multi-sentence summary (3-6 sentences) that distills the cluster's content. The summary should be self-contained — a reader who never sees the underlying memories should still understand what the topic is about.

Output STRICT JSON only — no markdown fences, no commentary, no preamble:

{"title": "...", "summary": "..."}

Memories in this cluster:
"#;

const K3_ENTITIES_PREFIX: &str = "\nEntities spanning these memories: ";

impl Summarizer for AnthropicSummarizer {
    fn summarize(
        &self,
        memory_contents: &[&str],
        contributing_entity_names: &[&str],
    ) -> Result<Summary, SummarizeError> {
        if memory_contents.is_empty() {
            return Err(SummarizeError::EmptyOutput);
        }

        // Build the user content: prompt prefix + numbered memories +
        // (optional) entity names. Numbering helps the model attribute
        // claims back to specific memories in the summary.
        let mut user_content = String::with_capacity(
            K3_PROMPT_PREFIX.len() + memory_contents.iter().map(|s| s.len()).sum::<usize>() + 64,
        );
        user_content.push_str(K3_PROMPT_PREFIX);
        for (i, m) in memory_contents.iter().enumerate() {
            user_content.push_str(&format!("\n[{}] {}\n", i + 1, m));
        }
        if !contributing_entity_names.is_empty() {
            user_content.push_str(K3_ENTITIES_PREFIX);
            user_content.push_str(&contributing_entity_names.join(", "));
            user_content.push('\n');
        }

        let body = serde_json::json!({
            "model": self.config.model,
            "max_tokens": self.config.max_tokens,
            "messages": [
                { "role": "user", "content": user_content }
            ]
        });

        let url = format!("{}/v1/messages", self.config.api_url);
        let headers = self.build_headers().map_err(SummarizeError::Permanent)?;

        let response = self
            .client
            .post(&url)
            .headers(headers)
            .json(&body)
            .send()
            .map_err(|e| {
                // reqwest::Error: connect/timeout/network = transient.
                if e.is_timeout() || e.is_connect() || e.is_request() {
                    SummarizeError::Transient(Box::new(e))
                } else {
                    SummarizeError::Permanent(Box::new(e))
                }
            })?;

        let status = response.status();
        if !status.is_success() {
            let body_text = response.text().unwrap_or_default();
            let err: Box<dyn Error + Send + Sync> =
                format!("Anthropic API {}: {}", status, body_text).into();
            // 5xx → Transient; 4xx (auth, bad request, model-not-found) → Permanent.
            // 429 (rate limit) is transient — backoff + retry is the right move.
            return Err(if status.is_server_error() || status.as_u16() == 429 {
                SummarizeError::Transient(err)
            } else {
                SummarizeError::Permanent(err)
            });
        }

        let response_json: serde_json::Value = response
            .json()
            .map_err(|e| SummarizeError::Permanent(Box::new(e) as Box<dyn Error + Send + Sync>))?;

        let content_text = response_json
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|item| item.get("text"))
            .and_then(|t| t.as_str())
            .ok_or_else(|| {
                SummarizeError::Permanent(
                    "Invalid Anthropic response: missing content[0].text".into(),
                )
            })?;

        parse_summary_response(content_text)
    }
}

/// Parse the model's JSON response into a [`Summary`].
///
/// Strips optional ```json ... ``` fences before parsing. Rejects empty
/// title or summary as [`SummarizeError::EmptyOutput`].
fn parse_summary_response(raw: &str) -> Result<Summary, SummarizeError> {
    let trimmed = raw.trim();
    let json_str = strip_code_fences(trimmed);

    #[derive(serde::Deserialize)]
    struct Parsed {
        title: Option<String>,
        summary: Option<String>,
    }

    let parsed: Parsed = serde_json::from_str(json_str).map_err(|e| {
        SummarizeError::Permanent(
            format!("failed to parse summarizer JSON: {e}; raw: {raw}").into(),
        )
    })?;

    let title = parsed.title.unwrap_or_default().trim().to_string();
    let summary = parsed.summary.unwrap_or_default().trim().to_string();

    if title.is_empty() || summary.is_empty() {
        return Err(SummarizeError::EmptyOutput);
    }

    Ok(Summary { title, summary })
}

/// Strip ```json … ``` or ``` … ``` fences if present. Returns the inner
/// payload (or `s` unchanged if no fence is detected).
fn strip_code_fences(s: &str) -> &str {
    let t = s.trim();
    let stripped = t
        .strip_prefix("```json")
        .or_else(|| t.strip_prefix("```"))
        .unwrap_or(t);
    stripped.strip_suffix("```").unwrap_or(stripped).trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_summary_response_strict_json() {
        let raw = r#"{"title": "Coffee in Paris", "summary": "Alice and Bob met in Paris and had coffee."}"#;
        let s = parse_summary_response(raw).unwrap();
        assert_eq!(s.title, "Coffee in Paris");
        assert!(s.summary.starts_with("Alice"));
    }

    #[test]
    fn parse_summary_response_strips_json_fences() {
        let raw = "```json\n{\"title\":\"X\",\"summary\":\"Y is interesting.\"}\n```";
        let s = parse_summary_response(raw).unwrap();
        assert_eq!(s.title, "X");
        assert_eq!(s.summary, "Y is interesting.");
    }

    #[test]
    fn parse_summary_response_strips_plain_fences() {
        let raw = "```\n{\"title\":\"X\",\"summary\":\"Y\"}\n```";
        let s = parse_summary_response(raw).unwrap();
        assert_eq!(s.title, "X");
    }

    #[test]
    fn parse_summary_response_empty_title_errors() {
        let raw = r#"{"title": "", "summary": "non-empty"}"#;
        match parse_summary_response(raw) {
            Err(SummarizeError::EmptyOutput) => {}
            other => panic!("expected EmptyOutput, got {other:?}"),
        }
    }

    #[test]
    fn parse_summary_response_missing_summary_errors() {
        let raw = r#"{"title": "X"}"#;
        match parse_summary_response(raw) {
            Err(SummarizeError::EmptyOutput) => {}
            other => panic!("expected EmptyOutput, got {other:?}"),
        }
    }

    #[test]
    fn parse_summary_response_malformed_json_is_permanent() {
        let raw = "not json at all";
        match parse_summary_response(raw) {
            Err(SummarizeError::Permanent(_)) => {}
            other => panic!("expected Permanent, got {other:?}"),
        }
    }
}
