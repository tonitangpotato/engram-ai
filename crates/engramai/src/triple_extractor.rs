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
    fn extract_triples(&self, _content: &str) -> Result<Vec<Triple>, Box<dyn Error + Send + Sync>> {
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

/// ISS-203 root-fix prompt: enforces an atomicity contract so the LLM
/// decomposes possessive/prepositional phrases into atomic entities + a
/// relation, instead of emitting the whole phrase as one "entity".
///
/// The defect this fixes is a *type error*: the legacy prompt (above) demos
/// phrase-objects like `"prevention of data races"` and never tells the model
/// that subject/object must be real-world things. The LLM learned to emit
/// `"Caroline's paintings"`, `"support from Caroline"`, `"conversation with
/// Caroline"` as standalone entities. Each then becomes its own node, never
/// linked back to the `Caroline` person node — fragmenting one entity into
/// ~21 disconnected shards and starving entity-anchored retrieval.
///
/// Note: no new `Predicate` variant is needed. `belongs_to` is already an
/// accepted alias of `PartOf` in `Predicate::from_str_lossy`, and
/// `associated_with` aliases `RelatedTo`. So "X's Y → Y belongs_to X" and
/// "support from X → support associated_with X" both round-trip through the
/// existing enum. The fix is teaching the model the contract, not widening
/// the vocabulary.
///
/// Flag-gated via `ENGRAM_TRIPLE_PROMPT_V2` because the extraction layer has
/// regressed before (ISS-162/178 harmful, ISS-161 inert) — must A/B on
/// conv-26 + conv-44 before flipping the default.
const TRIPLE_EXTRACTION_PROMPT_V2: &str = r#"Extract subject-predicate-object triples from the following text.

Allowed predicates: is_a, part_of, belongs_to, uses, depends_on, caused_by, leads_to, implements, contradicts, associated_with, related_to

Allowed entity kinds (optional, for subject_kind and object_kind):
  Person, Organization, Place, Concept, Artifact, Event, Topic

If the entity kind is unclear or doesn't fit the above, OMIT the field — do not
guess. Anything outside this allowlist is dropped (it does NOT fall through to
an "Other" bucket).

ATOMICITY CONTRACT — subject and object must each be an ATOMIC ENTITY: a single
real-world thing (a person, place, organization, artifact, concept, event, or
topic). They must NOT be phrases that bury a relationship inside the name.

When a phrase encodes a relation, DECOMPOSE it into entity + relation + entity:
  - Possessive ("X's Y"): emit Y as the subject and X as the object with
    predicate `belongs_to`. "Caroline's paintings" → paintings belongs_to Caroline.
  - Prepositional ("Y from X", "Y with X", "Y of X"): emit Y and X as separate
    entities linked with `associated_with` (or a more specific predicate if one
    fits). "support from Caroline" → support associated_with Caroline.
  - Never emit a possessive or prepositional phrase as a single subject/object
    (no "Caroline's paintings", "support from Caroline", "conversation with Bob"
    as one entity).

Return ONLY a JSON array (no markdown, no explanation):
[{"subject": "...", "predicate": "...", "object": "...", "confidence": 0.X, "subject_kind": "Person", "object_kind": "Organization"}]

Examples:
Input: "Rust's borrow checker prevents data races at compile time"
Output: [{"subject": "borrow checker", "predicate": "belongs_to", "object": "Rust", "confidence": 0.9, "subject_kind": "Concept", "object_kind": "Artifact"}, {"subject": "borrow checker", "predicate": "leads_to", "object": "data races", "confidence": 0.8, "object_kind": "Concept"}]

Input: "Caroline showed her paintings at the LGBTQ art show"
Output: [{"subject": "paintings", "predicate": "belongs_to", "object": "Caroline", "confidence": 0.9, "subject_kind": "Artifact", "object_kind": "Person"}, {"subject": "paintings", "predicate": "part_of", "object": "LGBTQ art show", "confidence": 0.85, "object_kind": "Event"}]

Input: "The Memory struct uses SQLite for persistence"
Output: [{"subject": "Memory struct", "predicate": "uses", "object": "SQLite", "confidence": 0.9, "object_kind": "Artifact"}, {"subject": "SQLite", "predicate": "implements", "object": "persistence", "confidence": 0.8, "subject_kind": "Artifact", "object_kind": "Concept"}]

If nothing worth extracting, return empty array [].

Text:
"#;

/// Select the active triple-extraction prompt.
///
/// Returns the ISS-203 atomicity-contract prompt when `ENGRAM_TRIPLE_PROMPT_V2`
/// is set to a truthy value (`1`, `true`, `on`, `yes`, case-insensitive),
/// otherwise the legacy prompt. Default is legacy until the conv-26 + conv-44
/// A/B clears the regression gate.
fn select_triple_prompt() -> &'static str {
    match std::env::var("ENGRAM_TRIPLE_PROMPT_V2") {
        Ok(v) => {
            let v = v.trim().to_ascii_lowercase();
            if matches!(v.as_str(), "1" | "true" | "on" | "yes") {
                TRIPLE_EXTRACTION_PROMPT_V2
            } else {
                TRIPLE_EXTRACTION_PROMPT
            }
        }
        Err(_) => TRIPLE_EXTRACTION_PROMPT,
    }
}

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

    // ISS-168: extract the FIRST complete top-level JSON array, not
    // `s[find('[')..rfind(']')+1]`. Haiku occasionally emits a chain-of-thought
    // pattern with two arrays separated by prose, e.g.
    //
    //     [{"subject": "Caroline", ...}]
    //
    //     Wait, "creates" isn't in the allowed predicates. Let me re-evaluate:
    //
    //     []
    //
    // The old span-to-last-`]` slice produced `[{...}]\n\nprose\n\n[]`, which
    // fails JSON parse and drops the entire response (~5% of Haiku calls per
    // ISS-166 validation probe).
    //
    // First-array-wins matches the principle "extract the first self-contained
    // JSON, ignore CoT remnants" and aligns with how most LLM JSON-output
    // parsers handle this. Tradeoff vs. LAST-array-wins: Haiku's "final"
    // array in the observed pattern is `[]` (it talked itself out of a valid
    // triple), so taking the last array would *worsen* recall here, not
    // improve it.
    let json_to_parse = match extract_first_top_level_array(json_str) {
        Some(slice) => slice,
        None => {
            log::warn!(
                "No top-level JSON array found in triple extraction response: {}",
                json_str
            );
            return Ok(vec![]);
        }
    };

    // ISS-167: parse via `serde_json::Value` first, then convert per-element.
    // The strict `Deserialize` derive on `RawTriple` rejected duplicate fields,
    // which Haiku reliably emits when a single noun fits two `EntityKind`s
    // (e.g. `{"object_kind": "Artifact", "object_kind": "Concept"}`). Going
    // through `Value` silently takes the LAST value on duplicate keys
    // (matches JSON.parse + every permissive JSON impl) and lets us recover
    // 100% of these previously-dropped triples.
    //
    // Cost: one extra allocation per triple (the intermediate `Value::Object`).
    // Acceptable — extractor runs at ~1 call per episode, not per query.
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

    let value: serde_json::Value = match serde_json::from_str(json_to_parse) {
        Ok(v) => v,
        Err(e) => {
            log::warn!(
                "Failed to parse triple extraction JSON (not valid JSON): {} - content: {}",
                e,
                json_to_parse
            );
            return Ok(vec![]);
        }
    };

    let array = match value {
        serde_json::Value::Array(a) => a,
        other => {
            log::warn!(
                "Triple extraction response was valid JSON but not an array: {:?}",
                other
            );
            return Ok(vec![]);
        }
    };

    let mut raw_triples: Vec<RawTriple> = Vec::with_capacity(array.len());
    let mut decode_errors: usize = 0;
    for v in array {
        match serde_json::from_value::<RawTriple>(v) {
            Ok(rt) => raw_triples.push(rt),
            Err(e) => {
                decode_errors += 1;
                log::warn!(
                    "Triple extraction: dropping malformed element ({}); continuing with the rest",
                    e
                );
            }
        }
    }
    if decode_errors > 0 {
        log::warn!(
            "Triple extraction: {} element(s) dropped due to decode errors out of {} total",
            decode_errors,
            decode_errors + raw_triples.len()
        );
    }

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

/// Find the first complete top-level JSON array in `s` and return the
/// slice from the opening `[` through the matching `]` (inclusive).
///
/// Scans char-by-char tracking bracket depth and whether we're inside a
/// JSON string literal (where `[` / `]` do not count toward nesting). The
/// scanner respects backslash escapes inside strings (`\"`, `\\`).
///
/// Returns `None` if no balanced top-level array is found.
///
/// ISS-168: this replaces the old `s[find('[')..rfind(']')+1]` policy,
/// which over-extracted into trailing prose / second arrays on Haiku CoT
/// output.
fn extract_first_top_level_array(s: &str) -> Option<&str> {
    let bytes = s.as_bytes();
    // Find the first '[' that isn't inside a string. At the top level we are
    // never inside a string before the first '[' (the prompt asks for a JSON
    // array), so a plain `find('[')` is enough here — but the depth scanner
    // below handles strings correctly once we're inside the array.
    let start = s.find('[')?;
    let mut i = start;

    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut escape = false;

    while i < bytes.len() {
        let c = bytes[i];
        if in_string {
            if escape {
                escape = false;
            } else if c == b'\\' {
                escape = true;
            } else if c == b'"' {
                in_string = false;
            }
        } else {
            match c {
                b'"' => in_string = true,
                b'[' => depth += 1,
                b']' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(&s[start..=i]);
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }

    // Unbalanced — no matching close found.
    None
}

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
        Ok(crate::anthropic_client::build_anthropic_headers(
            &token,
            self.is_oauth,
        ))
    }
}

impl TripleExtractor for AnthropicTripleExtractor {
    fn extract_triples(&self, content: &str) -> Result<Vec<Triple>, Box<dyn Error + Send + Sync>> {
        let prompt = format!("{}{}", select_triple_prompt(), content);

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

        let response = self
            .client
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
        let prompt = format!("{}{}", select_triple_prompt(), content);

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

        let response = self
            .client
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
        let response =
            r#"[{"subject": "Rust", "predicate": "uses", "object": "LLVM", "confidence": 0.9}]"#;
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
        let response =
            r#"[{"subject": "X", "predicate": "foobar", "object": "Y", "confidence": 0.5}]"#;
        let triples = parse_triple_response(response).unwrap();
        assert_eq!(triples[0].predicate, Predicate::RelatedTo);
    }

    #[test]
    fn test_parse_triple_response_clamps_confidence() {
        let response =
            r#"[{"subject": "X", "predicate": "uses", "object": "Y", "confidence": 1.5}]"#;
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
        assert_eq!(
            parse_kind_hint(Some("Artifact")),
            Some(EntityKind::Artifact)
        );
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
        let response =
            r#"[{"subject": "X", "predicate": "uses", "object": "Y", "confidence": 0.8}]"#;
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

    // ISS-167: parser tolerance for duplicate keys ---------------------------
    //
    // Haiku reliably emits duplicate `object_kind` (and occasionally
    // `subject_kind`) keys when a single noun fits two `EntityKind`s
    // simultaneously (e.g. "necklace" = Artifact AND Concept). Before
    // ISS-167 the strict `Deserialize` impl rejected the entire array on
    // first duplicate, dropping 100% of real Haiku output in production.
    // Parser now goes through `serde_json::Value`, which takes the LAST
    // value on duplicate keys (JSON.parse semantics).

    #[test]
    fn parse_triple_response_tolerates_duplicate_object_kind() {
        // Verbatim Haiku payload from /tmp/iss166-probe-validate.log.
        let response = r#"[
            {
                "subject": "organization",
                "predicate": "implements",
                "object": "inclusivity",
                "confidence": 0.85,
                "object_kind": "Organization",
                "object_kind": "Concept"
            }
        ]"#;
        let triples = parse_triple_response(response).expect("must parse");
        assert_eq!(triples.len(), 1, "expected 1 triple, got: {triples:?}");
        // Last value wins → "Concept".
        assert_eq!(triples[0].object_kind_hint, Some(EntityKind::Concept));
        assert_eq!(triples[0].subject, "organization");
        assert_eq!(triples[0].object, "inclusivity");
    }

    #[test]
    fn parse_triple_response_tolerates_duplicate_subject_kind() {
        // Symmetric case: duplicate `subject_kind`.
        let response = r#"[
            {
                "subject": "necklace",
                "predicate": "related_to",
                "object": "family",
                "confidence": 0.8,
                "subject_kind": "Artifact",
                "subject_kind": "Concept",
                "object_kind": "Concept"
            }
        ]"#;
        let triples = parse_triple_response(response).expect("must parse");
        assert_eq!(triples.len(), 1);
        assert_eq!(triples[0].subject_kind_hint, Some(EntityKind::Concept));
        assert_eq!(triples[0].object_kind_hint, Some(EntityKind::Concept));
    }

    #[test]
    fn parse_triple_response_tolerates_duplicate_core_fields() {
        // If the LLM duplicates a core field (subject/predicate/object/
        // confidence), take the last — consistent with the kind-field
        // policy. We don't want a single duplicate to nuke the whole array.
        let response = r#"[
            {
                "subject": "wrong",
                "subject": "right",
                "predicate": "uses",
                "object": "X",
                "confidence": 0.5,
                "confidence": 0.9
            }
        ]"#;
        let triples = parse_triple_response(response).expect("must parse");
        assert_eq!(triples.len(), 1);
        assert_eq!(triples[0].subject, "right");
        assert!((triples[0].confidence - 0.9).abs() < 1e-9);
    }

    #[test]
    fn parse_triple_response_mixed_valid_and_malformed_elements_keeps_valid() {
        // Real Haiku output sometimes mixes well-formed triples with one
        // truly malformed element (e.g. missing `predicate`). Old parser
        // failed the whole array; new parser drops only the bad element.
        let response = r#"[
            {"subject": "A", "predicate": "uses", "object": "B", "confidence": 0.9},
            {"subject": "C", "object": "D", "confidence": 0.8},
            {"subject": "E", "predicate": "leads_to", "object": "F", "confidence": 0.7, "object_kind": "Concept", "object_kind": "Topic"}
        ]"#;
        let triples = parse_triple_response(response).expect("must parse");
        assert_eq!(triples.len(), 2, "valid + tolerated = 2; bad one dropped");
        assert_eq!(triples[0].subject, "A");
        assert_eq!(triples[1].subject, "E");
        assert_eq!(triples[1].object_kind_hint, Some(EntityKind::Topic));
    }

    #[test]
    fn parse_triple_response_not_an_array_returns_empty() {
        // Defensive: if Haiku ever responds with a non-array top-level value
        // that contains no `[...]` substring at all, we log + return empty
        // rather than panic. (Note: the upstream `find('[')..rfind(']')`
        // slice in `parse_triple_response` will *extract* an array embedded
        // inside a wrapping object — that's intentional permissive
        // behaviour, not what this test guards. This guards the truly-no-
        // array case.)
        let response = r#"{"error": "no triples found"}"#;
        let triples = parse_triple_response(response).expect("must not error");
        assert!(triples.is_empty());
    }

    // -----------------------------------------------------------------------
    // ISS-168 — first-array-wins parser tolerates Haiku CoT with multiple
    // top-level arrays separated by prose.
    // -----------------------------------------------------------------------

    #[test]
    fn iss168_two_arrays_with_prose_between_takes_first() {
        // Observed pattern from ISS-166 validation probe (~5% of conv-26
        // Haiku calls): Haiku emits a first array, second-guesses itself in
        // prose, then emits an empty array as its "final answer".
        let response = r#"[{"subject": "Caroline", "predicate": "uses", "object": "red and blue colors", "confidence": 0.9}]

Wait, I need to reconsider - "creates" is not in the allowed predicates.
The allowed predicates don't fit this casual conversation well.

[]"#;
        let triples = parse_triple_response(response).expect("must parse");
        assert_eq!(
            triples.len(),
            1,
            "first array wins; second [] is discarded along with the prose"
        );
        assert_eq!(triples[0].subject, "Caroline");
        assert_eq!(triples[0].predicate, Predicate::Uses);
        assert_eq!(triples[0].object, "red and blue colors");
    }

    #[test]
    fn iss168_replacement_pattern_first_array_wins() {
        // The replacement-pattern CoT: first answer, then "scratch that",
        // then a different answer. First-array-wins policy → first answer.
        // (LAST-array policy would have been the opposite call; this test
        // pins the documented choice.)
        let response = r#"[
            {"subject": "Rust", "predicate": "uses", "object": "LLVM", "confidence": 0.9},
            {"subject": "Rust", "predicate": "implements", "object": "ownership", "confidence": 0.8}
        ]

Wait, scratch that — let me re-extract more conservatively.

[{"subject": "Rust", "predicate": "is_a", "object": "language", "confidence": 0.95}]"#;
        let triples = parse_triple_response(response).expect("must parse");
        assert_eq!(
            triples.len(),
            2,
            "first array (2 triples) wins over second (1)"
        );
        assert_eq!(triples[0].subject, "Rust");
        assert_eq!(triples[0].predicate, Predicate::Uses);
        assert_eq!(triples[1].predicate, Predicate::Implements);
    }

    #[test]
    fn iss168_prose_preamble_and_postamble_still_extracts() {
        // ISS-167 already handled the "prose before + after a single array"
        // case via find('[')..rfind(']'). The new scanner must preserve this
        // behaviour. Regression guard.
        let response = r#"Here's what I extracted from the conversation:

[{"subject": "A", "predicate": "uses", "object": "B", "confidence": 0.7}]

Hope this helps!"#;
        let triples = parse_triple_response(response).expect("must parse");
        assert_eq!(triples.len(), 1);
        assert_eq!(triples[0].subject, "A");
        assert_eq!(triples[0].object, "B");
    }

    #[test]
    fn iss168_nested_brackets_in_string_values_do_not_confuse_scanner() {
        // The scanner must respect JSON string boundaries — a `[` or `]`
        // inside a quoted string value is content, not structure. Without
        // string-awareness the depth counter would underflow or close early
        // on `"object": "the [main] idea"`.
        let response = r#"[{"subject": "Caroline", "predicate": "related_to", "object": "the [main] idea", "confidence": 0.6}]"#;
        let triples = parse_triple_response(response).expect("must parse");
        assert_eq!(triples.len(), 1);
        assert_eq!(triples[0].object, "the [main] idea");
    }

    #[test]
    fn iss168_escaped_quote_in_string_preserves_string_boundary() {
        // Escape-handling guard: a `\"` inside a string must not be
        // mistaken for the string's closing quote.
        let response = r#"[{"subject": "X", "predicate": "uses", "object": "the \"big\" idea", "confidence": 0.5}]"#;
        let triples = parse_triple_response(response).expect("must parse");
        assert_eq!(triples.len(), 1);
        assert_eq!(triples[0].object, r#"the "big" idea"#);
    }

    // --- ISS-203: atomicity-contract prompt (V2) ---

    /// The fix relies on `belongs_to` and `associated_with` already being
    /// accepted aliases, so no new `Predicate` variant is needed. Guard that
    /// assumption: if these mappings ever change, the V2 prompt silently
    /// degrades possessive/prepositional edges and this test must catch it.
    #[test]
    fn iss203_possessive_predicates_roundtrip_through_existing_enum() {
        assert_eq!(Predicate::from_str_lossy("belongs_to"), Predicate::PartOf);
        assert_eq!(
            Predicate::from_str_lossy("associated_with"),
            Predicate::RelatedTo
        );
    }

    /// The V2 prompt must carry the atomicity contract and demonstrate
    /// decomposing a possessive phrase into entity + relation + entity.
    #[test]
    fn iss203_v2_prompt_enforces_atomicity_and_decomposition() {
        let p = TRIPLE_EXTRACTION_PROMPT_V2;
        assert!(
            p.contains("ATOMICITY CONTRACT"),
            "V2 must state the atomicity contract"
        );
        assert!(
            p.contains("belongs_to"),
            "V2 must allow the possessive predicate"
        );
        // The canonical possessive decomposition example.
        assert!(
            p.contains("paintings belongs_to Caroline")
                || p.contains(
                    "\"paintings\", \"predicate\": \"belongs_to\", \"object\": \"Caroline\""
                ),
            "V2 must demo X's Y -> Y belongs_to X decomposition"
        );
        // Prepositional decomposition guidance.
        assert!(
            p.contains("support associated_with Caroline"),
            "V2 must demo 'support from X' -> associated_with decomposition"
        );
    }

    /// The bad phrase-object example that taught the LLM the wrong behavior
    /// must NOT survive into V2.
    #[test]
    fn iss203_v2_prompt_drops_phrase_object_example() {
        assert!(
            !TRIPLE_EXTRACTION_PROMPT_V2.contains("prevention of data races"),
            "V2 must not demo a buried-relation phrase as an object"
        );
        // The legacy prompt still carries it (we only gate, never mutate it).
        assert!(
            TRIPLE_EXTRACTION_PROMPT.contains("prevention of data races"),
            "legacy prompt is left untouched by the gate"
        );
    }

    /// `select_triple_prompt()` defaults to legacy and switches to V2 only on
    /// a truthy env value. Serialized via a mutex because it mutates a process-
    /// global env var.
    #[test]
    fn iss203_select_triple_prompt_gating() {
        use std::sync::Mutex;
        static ENV_LOCK: Mutex<()> = Mutex::new(());
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let key = "ENGRAM_TRIPLE_PROMPT_V2";
        let restore = std::env::var(key).ok();

        std::env::remove_var(key);
        assert_eq!(select_triple_prompt(), TRIPLE_EXTRACTION_PROMPT);

        for truthy in ["1", "true", "on", "yes", "TRUE", "On"] {
            std::env::set_var(key, truthy);
            assert_eq!(
                select_triple_prompt(),
                TRIPLE_EXTRACTION_PROMPT_V2,
                "value {:?} should select V2",
                truthy
            );
        }

        for falsy in ["0", "false", "off", "", "garbage"] {
            std::env::set_var(key, falsy);
            assert_eq!(
                select_triple_prompt(),
                TRIPLE_EXTRACTION_PROMPT,
                "value {:?} should keep legacy",
                falsy
            );
        }

        match restore {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
    }
}
