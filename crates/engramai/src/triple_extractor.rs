//! LLM-based triple extraction from memory content.
//!
//! Extracts subject-predicate-object triples using LLMs (Anthropic or Ollama)
//! to enrich Hebbian link quality with semantic relationships.

use std::error::Error;
use std::time::Duration;

use crate::extractor::TokenProvider;
use crate::graph::EntityKind;
use crate::triple::{Predicate, Triple, TripleSource};

/// Trait for extracting triples from memory content.
pub trait TripleExtractor: Send + Sync {
    /// Extract triples from the given content string.
    fn extract_triples(&self, content: &str) -> Result<Vec<Triple>, Box<dyn Error + Send + Sync>>;
}

/// A no-op triple extractor that always returns an empty list.
///
/// Useful when the resolution pipeline must be wired (so entities are
/// extracted into the v0.3 graph layer) but no LLM is available for
/// semantic triple/edge extraction. Entity-only retrieval still works;
/// only semantic edge queries degrade.
///
/// Filed under ISS-046 (engram store CLI graph plumbing): the CLI installs
/// this when `--graph-db` is set without `--triple-extractor`, so users
/// can populate v0.3 graphs without configuring an LLM.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopTripleExtractor;

impl NoopTripleExtractor {
    /// Construct a new no-op extractor.
    pub fn new() -> Self {
        Self
    }
}

impl TripleExtractor for NoopTripleExtractor {
    fn extract_triples(
        &self,
        _content: &str,
    ) -> Result<Vec<Triple>, Box<dyn Error + Send + Sync>> {
        Ok(Vec::new())
    }
}

/// Few-shot prompt for triple extraction.
const TRIPLE_EXTRACTION_PROMPT: &str = r#"Extract subject-predicate-object triples from the following text.

Allowed predicates: is_a, part_of, uses, depends_on, caused_by, leads_to, implements, contradicts, related_to

Allowed entity kinds (optional, for subject_kind and object_kind):
  Person, Organization, Place, Concept, Artifact, Event, Topic

If the entity kind is unclear or doesn't fit the above, OMIT the field — do not
guess. Anything outside this allowlist is dropped (it does NOT fall through to
an "Other" bucket).

Return ONLY a JSON array (no markdown, no explanation):
[{"subject": "...", "predicate": "...", "object": "...", "confidence": 0.X, "subject_kind": "Person", "object_kind": "Organization"}]

Examples:
Input: "Rust's borrow checker prevents data races at compile time"
Output: [{"subject": "borrow checker", "predicate": "part_of", "object": "Rust", "confidence": 0.9, "object_kind": "Artifact"}, {"subject": "borrow checker", "predicate": "leads_to", "object": "prevention of data races", "confidence": 0.8}]

Input: "The Memory struct uses SQLite for persistence"
Output: [{"subject": "Memory struct", "predicate": "uses", "object": "SQLite", "confidence": 0.9, "object_kind": "Artifact"}, {"subject": "SQLite", "predicate": "implements", "object": "persistence", "confidence": 0.8, "subject_kind": "Artifact", "object_kind": "Concept"}]

If nothing worth extracting, return empty array [].

Text:
"#;

/// Parse a triple extraction response from an LLM.
fn parse_triple_response(content: &str) -> Result<Vec<Triple>, Box<dyn Error + Send + Sync>> {
    // Strip markdown code blocks if present
    let json_str = content
        .trim()
        .strip_prefix("```json")
        .or_else(|| content.trim().strip_prefix("```"))
        .map(|s| s.strip_suffix("```").unwrap_or(s))
        .unwrap_or(content)
        .trim();

    if json_str == "[]" {
        return Ok(vec![]);
    }

    let json_start = json_str.find('[');
    let json_end = json_str.rfind(']');

    let json_to_parse = match (json_start, json_end) {
        (Some(start), Some(end)) if start < end => &json_str[start..=end],
        _ => {
            log::warn!("No JSON array found in triple extraction response: {}", json_str);
            return Ok(vec![]);
        }
    };

    #[derive(serde::Deserialize)]
    struct RawTriple {
        subject: String,
        predicate: String,
        object: String,
        confidence: f64,
        #[serde(default)]
        subject_kind: Option<String>,
        #[serde(default)]
        object_kind: Option<String>,
    }

    match serde_json::from_str::<Vec<RawTriple>>(json_to_parse) {
        Ok(raw_triples) => {
            let triples = raw_triples
                .into_iter()
                .filter(|t| !t.subject.is_empty() && !t.object.is_empty())
                .map(|t| {
                    let subject_kind_hint = parse_kind_hint(t.subject_kind.as_deref());
                    let object_kind_hint = parse_kind_hint(t.object_kind.as_deref());
                    let mut triple = Triple::new(
                        t.subject,
                        Predicate::from_str_lossy(&t.predicate),
                        t.object,
                        t.confidence,
                    );
                    triple.source = TripleSource::Llm;
                    triple.subject_kind_hint = subject_kind_hint;
                    triple.object_kind_hint = object_kind_hint;
                    triple
                })
                .collect();
            Ok(triples)
        }
        Err(e) => {
            log::warn!("Failed to parse triple extraction JSON: {} - content: {}", e, json_to_parse);
            Ok(vec![])
        }
    }
}

/// Map the LLM's `subject_kind` / `object_kind` string (from `RawTriple`) to
/// `EntityKind`. Returns `None` for empty / unknown / out-of-allowlist strings;
/// callers fall back to `KindSource::Default`.
///
/// The allowlist mirrors the canonical `EntityKind` variants exactly (see
/// `graph/entity.rs`). `Other(_)` is intentionally excluded — the LLM must
/// not be able to mint arbitrary kinds via this path. Pressure to add a new
/// kind should turn into a real variant via code review, not a smuggled
/// `Other` string.
///
/// Out-of-allowlist hits are logged at debug level for observability — if the
/// LLM keeps suggesting `"Animal"`, that's a signal we should add the variant,
/// not patch the prompt.
pub(crate) fn parse_kind_hint(s: Option<&str>) -> Option<EntityKind> {
    let s = s?.trim();
    if s.is_empty() {
        return None;
    }
    match s {
        "Person" => Some(EntityKind::Person),
        "Organization" => Some(EntityKind::Organization),
        "Place" => Some(EntityKind::Place),
        "Concept" => Some(EntityKind::Concept),
        "Artifact" => Some(EntityKind::Artifact),
        "Event" => Some(EntityKind::Event),
        "Topic" => Some(EntityKind::Topic),
        other => {
            log::debug!(
                "triple_extractor: dropping out-of-allowlist kind hint: {:?}",
                other
            );
            None
        }
    }
}

use crate::anthropic_client::StaticToken;

/// Extracts triples using the Anthropic Claude API.
pub struct AnthropicTripleExtractor {
    _api_key: String,
    model: String,
    is_oauth: bool,
    client: reqwest::blocking::Client,
    token_provider: Box<dyn TokenProvider>,
}

impl AnthropicTripleExtractor {
    /// Create a new extractor with a static API key.
    pub fn new(api_key: &str, is_oauth: bool) -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("failed to create HTTP client");

        Self {
            _api_key: api_key.to_string(),
            model: "claude-haiku-4-5-20251001".to_string(),
            is_oauth,
            client,
            token_provider: Box::new(StaticToken(api_key.to_string())),
        }
    }

    /// Create with a custom model.
    pub fn with_model(api_key: &str, is_oauth: bool, model: &str) -> Self {
        let mut ext = Self::new(api_key, is_oauth);
        ext.model = model.to_string();
        ext
    }

    /// Create with a dynamic token provider.
    pub fn with_token_provider(provider: Box<dyn TokenProvider>, is_oauth: bool) -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("failed to create HTTP client");

        Self {
            _api_key: String::new(),
            model: "claude-haiku-4-5-20251001".to_string(),
            is_oauth,
            client,
            token_provider: provider,
        }
    }

    fn build_headers(&self) -> Result<reqwest::header::HeaderMap, Box<dyn Error + Send + Sync>> {
        let token = self.token_provider.get_token()?;
        Ok(crate::anthropic_client::build_anthropic_headers(&token, self.is_oauth))
    }
}

impl TripleExtractor for AnthropicTripleExtractor {
    fn extract_triples(&self, content: &str) -> Result<Vec<Triple>, Box<dyn Error + Send + Sync>> {
        let prompt = format!("{}{}", TRIPLE_EXTRACTION_PROMPT, content);

        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": 1024,
            "messages": [
                {
                    "role": "user",
                    "content": prompt
                }
            ]
        });

        let response = self.client
            .post("https://api.anthropic.com/v1/messages")
            .headers(self.build_headers()?)
            .json(&body)
            .send()?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            return Err(format!("Anthropic API error {}: {}", status, body).into());
        }

        let response_json: serde_json::Value = response.json()?;

        let content_text = response_json
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|item| item.get("text"))
            .and_then(|t| t.as_str())
            .ok_or("Invalid response structure from Anthropic API")?;

        parse_triple_response(content_text)
    }
}

/// Extracts triples using a local Ollama model.
pub struct OllamaTripleExtractor {
    model: String,
    url: String,
    client: reqwest::blocking::Client,
}

impl OllamaTripleExtractor {
    /// Create a new extractor with the specified model.
    pub fn new(model: &str) -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .expect("failed to create HTTP client");

        Self {
            model: model.to_string(),
            url: "http://localhost:11434".to_string(),
            client,
        }
    }

    /// Create with a custom host URL.
    pub fn with_host(model: &str, url: &str) -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .expect("failed to create HTTP client");

        Self {
            model: model.to_string(),
            url: url.to_string(),
            client,
        }
    }
}

impl TripleExtractor for OllamaTripleExtractor {
    fn extract_triples(&self, content: &str) -> Result<Vec<Triple>, Box<dyn Error + Send + Sync>> {
        let prompt = format!("{}{}", TRIPLE_EXTRACTION_PROMPT, content);

        let body = serde_json::json!({
            "model": self.model,
            "messages": [
                {
                    "role": "user",
                    "content": prompt
                }
            ],
            "stream": false
        });

        let url = format!("{}/api/chat", self.url);

        let response = self.client
            .post(&url)
            .header("content-type", "application/json")
            .json(&body)
            .send()?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            return Err(format!("Ollama API error {}: {}", status, body).into());
        }

        let response_json: serde_json::Value = response.json()?;

        let content_text = response_json
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .ok_or("Invalid response structure from Ollama API")?;

        parse_triple_response(content_text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_triple_response_clean() {
        let response = r#"[{"subject": "Rust", "predicate": "uses", "object": "LLVM", "confidence": 0.9}]"#;
        let triples = parse_triple_response(response).unwrap();
        assert_eq!(triples.len(), 1);
        assert_eq!(triples[0].subject, "Rust");
        assert_eq!(triples[0].predicate, Predicate::Uses);
        assert_eq!(triples[0].object, "LLVM");
        assert!((triples[0].confidence - 0.9).abs() < f64::EPSILON);
    }

    #[test]
    fn test_parse_triple_response_markdown() {
        let response = "```json\n[{\"subject\": \"A\", \"predicate\": \"is_a\", \"object\": \"B\", \"confidence\": 0.8}]\n```";
        let triples = parse_triple_response(response).unwrap();
        assert_eq!(triples.len(), 1);
        assert_eq!(triples[0].predicate, Predicate::IsA);
    }

    #[test]
    fn test_parse_triple_response_empty() {
        let triples = parse_triple_response("[]").unwrap();
        assert!(triples.is_empty());
    }

    #[test]
    fn test_parse_triple_response_invalid() {
        let triples = parse_triple_response("not json").unwrap();
        assert!(triples.is_empty());
    }

    #[test]
    fn test_parse_triple_response_unknown_predicate() {
        let response = r#"[{"subject": "X", "predicate": "foobar", "object": "Y", "confidence": 0.5}]"#;
        let triples = parse_triple_response(response).unwrap();
        assert_eq!(triples[0].predicate, Predicate::RelatedTo);
    }

    #[test]
    fn test_parse_triple_response_clamps_confidence() {
        let response = r#"[{"subject": "X", "predicate": "uses", "object": "Y", "confidence": 1.5}]"#;
        let triples = parse_triple_response(response).unwrap();
        assert!((triples[0].confidence - 1.0).abs() < f64::EPSILON);
    }

    // -----------------------------------------------------------------------
    // ISS-072 GOAL-2 (A-clean) — kind hint propagation tests.
    // -----------------------------------------------------------------------

    #[test]
    fn parse_kind_hint_recognizes_all_canonical_variants() {
        // Design test #4 — every canonical EntityKind variant maps from its
        // PascalCase string. Other(_) is intentionally NOT mappable.
        assert_eq!(parse_kind_hint(Some("Person")), Some(EntityKind::Person));
        assert_eq!(
            parse_kind_hint(Some("Organization")),
            Some(EntityKind::Organization)
        );
        assert_eq!(parse_kind_hint(Some("Place")), Some(EntityKind::Place));
        assert_eq!(parse_kind_hint(Some("Concept")), Some(EntityKind::Concept));
        assert_eq!(parse_kind_hint(Some("Artifact")), Some(EntityKind::Artifact));
        assert_eq!(parse_kind_hint(Some("Event")), Some(EntityKind::Event));
        assert_eq!(parse_kind_hint(Some("Topic")), Some(EntityKind::Topic));
    }

    #[test]
    fn parse_kind_hint_drops_out_of_allowlist_strings() {
        // Design test #5 — out-of-allowlist hits return None (drop), they do
        // NOT fall through to EntityKind::Other. The LLM must not mint kinds.
        assert_eq!(parse_kind_hint(Some("Animal")), None);
        assert_eq!(parse_kind_hint(Some("Location")), None); // common alias
        assert_eq!(parse_kind_hint(Some("person")), None); // case-sensitive
        assert_eq!(parse_kind_hint(Some("")), None);
        assert_eq!(parse_kind_hint(Some("   ")), None);
        assert_eq!(parse_kind_hint(None), None);
    }

    #[test]
    fn parse_triple_response_propagates_kind_hints_onto_triple() {
        // Design test #7 — kind hints in the JSON show up on the parsed
        // Triple via subject_kind_hint / object_kind_hint.
        let response = r#"[{"subject": "Caroline", "predicate": "works_at", "object": "Acme", "confidence": 0.9, "subject_kind": "Person", "object_kind": "Organization"}]"#;
        let triples = parse_triple_response(response).unwrap();
        assert_eq!(triples.len(), 1);
        assert_eq!(triples[0].subject_kind_hint, Some(EntityKind::Person));
        assert_eq!(triples[0].object_kind_hint, Some(EntityKind::Organization));
    }

    #[test]
    fn parse_triple_response_omitted_kind_yields_none_hint() {
        // Design test #8 — old fixtures (no subject_kind/object_kind) still
        // parse via #[serde(default)], yielding None hints. Wire-compatible.
        let response = r#"[{"subject": "X", "predicate": "uses", "object": "Y", "confidence": 0.8}]"#;
        let triples = parse_triple_response(response).unwrap();
        assert_eq!(triples.len(), 1);
        assert_eq!(triples[0].subject_kind_hint, None);
        assert_eq!(triples[0].object_kind_hint, None);
    }

    #[test]
    fn parse_triple_response_partial_hint_only_one_side() {
        // Design test #9 — LLM may give a hint for one endpoint but not the
        // other; both sides handled independently.
        let response = r#"[{"subject": "Rust", "predicate": "is_a", "object": "language", "confidence": 0.9, "subject_kind": "Artifact"}]"#;
        let triples = parse_triple_response(response).unwrap();
        assert_eq!(triples[0].subject_kind_hint, Some(EntityKind::Artifact));
        assert_eq!(triples[0].object_kind_hint, None);
    }

    #[test]
    fn parse_triple_response_unknown_kind_drops_silently() {
        // Out-of-allowlist kind doesn't break parsing — just drops the hint.
        let response = r#"[{"subject": "Rex", "predicate": "is_a", "object": "dog", "confidence": 0.9, "subject_kind": "Animal"}]"#;
        let triples = parse_triple_response(response).unwrap();
        assert_eq!(triples.len(), 1);
        assert_eq!(triples[0].subject_kind_hint, None); // "Animal" dropped
    }
}
