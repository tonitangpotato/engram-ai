//! LLM-based memory extraction.
//!
//! Converts raw text into structured facts using LLMs. Optional feature
//! that preserves backward compatibility — if no extractor is set,
//! memories are stored as-is.

use serde::{Deserialize, Serialize};
use std::error::Error;
use std::time::Duration;

/// A single extracted fact from a conversation (dimensional format).
///
/// 11 semantic dimensions: core_fact (required) + 10 optional dimensions.
/// Type classification is inferred from dimension presence via `infer_type_weights()`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedFact {
    /// Core fact — the essential information (required). Maps to MemoryRecord.content.
    pub core_fact: String,
    /// Participants — who was involved
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub participants: Option<String>,
    /// Temporal — when it happened
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temporal: Option<String>,
    /// Location / source
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// Background / surrounding situation
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// Cause / motivation
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub causation: Option<String>,
    /// Result / impact
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    /// How it was done / steps
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// Connections to other known things
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relations: Option<String>,
    /// Emotional expression if present (e.g., frustrated, excited)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sentiment: Option<String>,
    /// Opinion / preference / position (e.g., prefers X over Y)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stance: Option<String>,
    /// Importance score (0.0–1.0)
    pub importance: f64,
    /// Tags
    #[serde(default)]
    pub tags: Vec<String>,
    /// Confidence: "confident" / "likely" / "uncertain"
    #[serde(default = "default_confidence")]
    pub confidence: String,
    /// Emotional valence: -1.0 to 1.0. Drives interoceptive emotion system.
    #[serde(default)]
    pub valence: f64,
    /// Domain: "coding" / "trading" / "research" / "communication" / "general"
    #[serde(default = "default_domain")]
    pub domain: String,
}

impl Default for ExtractedFact {
    fn default() -> Self {
        Self {
            core_fact: String::new(),
            participants: None,
            temporal: None,
            location: None,
            context: None,
            causation: None,
            outcome: None,
            method: None,
            relations: None,
            sentiment: None,
            stance: None,
            importance: 0.5,
            tags: Vec::new(),
            confidence: default_confidence(),
            valence: 0.0,
            domain: default_domain(),
        }
    }
}

/// Legacy format for backward-compatible parsing of old-style extraction responses.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)] // memory_type is read by serde deserialization
struct LegacyExtractedFact {
    pub content: String,
    #[serde(default)]
    pub memory_type: String,
    #[serde(default = "default_importance")]
    pub importance: f64,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default = "default_confidence")]
    pub confidence: String,
    #[serde(default)]
    pub valence: f64,
    #[serde(default = "default_domain")]
    pub domain: String,
}

fn default_importance() -> f64 {
    0.5
}

/// Wrapper for dimensional structured output: `{"memories": [...]}`.
#[derive(Debug, Deserialize)]
struct DimensionalResponse {
    memories: Vec<ExtractedFact>,
}

fn default_confidence() -> String {
    "likely".to_string()
}

fn default_domain() -> String {
    "general".to_string()
}

/// Trait for memory extraction — converts raw text into structured facts.
///
/// Implement this trait to use different LLM backends for extraction.
pub trait MemoryExtractor: Send + Sync {
    /// Extract key facts from raw conversation text.
    ///
    /// Returns empty vec if nothing worth remembering.
    /// Returns an error if the extraction fails (network, parsing, etc.).
    fn extract(&self, text: &str) -> Result<Vec<ExtractedFact>, Box<dyn Error + Send + Sync>>;
}

/// The extraction prompt template (dimensional format).
///
/// Uses structured output with 11 semantic dimensions. LLM fills only dimensions
/// that are explicitly present in the text — no inference or fabrication.
const EXTRACTION_PROMPT: &str = r#"You are a memory extraction system. Extract key facts from the following conversation that are worth remembering long-term.

Rules:
- Extract concrete facts, preferences, decisions, and commitments
- Each fact should have a self-contained core_fact (understandable without context)
- Fill dimensional fields ONLY if the information is explicitly present — do NOT infer or fabricate
- Skip greetings, filler, acknowledgments
- Rate importance 0.0-1.0 (preferences=0.6, decisions=0.8, commitments=0.9)
- Rate confidence: "confident" (direct statement), "likely" (reasonable inference), "uncertain" (vague mention)
- If nothing worth remembering, return {"memories": []}
- Respond in the SAME LANGUAGE as the input

DO NOT extract any of these — return {"memories": []} if the input contains ONLY these:
- System instructions or agent identity setup ("You are X agent", "你是 XX", "Read SOUL.md", "Follow AGENTS.md")
- Tool/function schema definitions (JSON with "type", "properties", "required" describing tool parameters)
- Agent role/persona descriptions ("You are an AI assistant running on...", framework version info)
- Template operational reports with no decisions or events ("所有系统正常", "无新 commit", "Disk: XXG free")
- Raw config file contents (YAML/JSON configuration being loaded, not discussed)
- Heartbeat check results that are pure status repetition with no new information
- Memory recall results being echoed back (content starting with "Recalled Memories" or lists of previously stored memories)
- Trivial Q&A: single punctuation/emoji questions ("？", "ok", "👍") with filler responses ("嗯？怎么了", "收到", "好的")
- Already-known identity facts: username, timezone, Telegram ID — these are in config files, not memories
- Pure acknowledgments with no new information: "好的", "收到", "了解", "ok got it"
- Repetitive status pings: "还在跑吗" → "还在跑" (no new state change)

STILL extract from these (they contain real information):
- Conversations about system instructions (e.g., "let's update SOUL.md to add X") — the discussion IS worth remembering
- Heartbeat reports that discover actual issues (test failures, disk critical, new commits)
- Status reports with decisions or action items
- Any user preferences, requests, commitments, or decisions
- Short messages that contain actual decisions: "ok 那就用方案B" — extract the decision, not the "ok"

Respond with ONLY valid JSON (no markdown, no explanation):
{"memories": [
  {
    "core_fact": "What happened — the essential fact (REQUIRED)",
    "participants": "Who was involved (omit if not mentioned)",
    "temporal": "When it happened (omit if not mentioned)",
    "location": "Where / in what context (omit if not mentioned)",
    "context": "Background / surrounding situation (omit if not relevant)",
    "causation": "Why it happened / motivation (omit if not mentioned)",
    "outcome": "What resulted / impact (omit if not mentioned)",
    "method": "How it was done / steps (omit if not mentioned)",
    "relations": "Connections to other known things (omit if none)",
    "sentiment": "Emotional expression if present, e.g. frustrated, excited (omit if neutral)",
    "stance": "Opinion / preference / position if present (omit if none)",
    "importance": 0.0,
    "tags": ["tag1"],
    "confidence": "confident",
    "valence": 0.0,
    "domain": "general"
  }
]}

Field notes:
- core_fact (REQUIRED): The essential information
- importance (REQUIRED): 0.0-1.0 based on long-term relevance
- tags (REQUIRED): relevant keywords
- confidence (REQUIRED): confident | likely | uncertain
- valence (REQUIRED): -1.0 (very negative) to 1.0 (very positive). 0.0 = neutral. Consider speaker's emotional state.
- domain (REQUIRED): coding | trading | research | communication | general
- All other fields: include ONLY if explicitly present in the text

Conversation:
"#;

/// Configuration for Anthropic-based extraction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicExtractorConfig {
    /// Model to use (default: "claude-haiku-4-5-20251001")
    pub model: String,
    /// API base URL (default: "https://api.anthropic.com")
    pub api_url: String,
    /// Maximum tokens for response (default: 1024)
    pub max_tokens: usize,
    /// Request timeout in seconds (default: 30)
    pub timeout_secs: u64,
}

impl Default for AnthropicExtractorConfig {
    fn default() -> Self {
        Self {
            model: "claude-haiku-4-5-20251001".to_string(),
            api_url: "https://api.anthropic.com".to_string(),
            max_tokens: 1024,
            timeout_secs: 30,
        }
    }
}

/// Extracts facts using Anthropic Claude API.
///
/// Token provider trait for dynamic auth token resolution.
///
/// Implement this to provide tokens that auto-refresh (e.g., OAuth managed tokens).
/// The extractor calls `get_token()` before each request, so expired tokens
/// get refreshed transparently.
pub trait TokenProvider: Send + Sync {
    /// Get a valid auth token. May refresh if expired.
    fn get_token(&self) -> Result<String, Box<dyn Error + Send + Sync>>;
}

use crate::anthropic_client::StaticToken;

/// Supports both OAuth tokens (Claude Max) and API keys.
/// Haiku is recommended for cost/speed balance.
///
/// Auth tokens can be:
/// - Static (fixed string, backward compatible)
/// - Dynamic (via `TokenProvider` trait, auto-refreshes on each request)
pub struct AnthropicExtractor {
    config: AnthropicExtractorConfig,
    token_provider: Box<dyn TokenProvider>,
    is_oauth: bool,
    client: reqwest::blocking::Client,
}

impl AnthropicExtractor {
    /// Create a new AnthropicExtractor with a static token.
    ///
    /// # Arguments
    ///
    /// * `auth_token` - API key or OAuth token (fixed string)
    /// * `is_oauth` - True if using OAuth token (Claude Max), false for API key
    pub fn new(auth_token: &str, is_oauth: bool) -> Self {
        Self::with_config(auth_token, is_oauth, AnthropicExtractorConfig::default())
    }
    
    /// Create with a static token and custom config.
    pub fn with_config(auth_token: &str, is_oauth: bool, config: AnthropicExtractorConfig) -> Self {
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

    /// Create with a dynamic token provider (auto-refreshes on each request).
    ///
    /// Use this for OAuth managed tokens that may expire and need refresh.
    /// The provider's `get_token()` is called before each extraction request.
    pub fn with_token_provider(
        provider: Box<dyn TokenProvider>,
        is_oauth: bool,
        config: AnthropicExtractorConfig,
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
    
    /// Build the request headers based on auth type.
    fn build_headers(&self) -> Result<reqwest::header::HeaderMap, Box<dyn Error + Send + Sync>> {
        let token = self.token_provider.get_token()?;
        Ok(crate::anthropic_client::build_anthropic_headers(&token, self.is_oauth))
    }
}

impl MemoryExtractor for AnthropicExtractor {
    fn extract(&self, text: &str) -> Result<Vec<ExtractedFact>, Box<dyn Error + Send + Sync>> {
        let prompt = format!("{}{}", EXTRACTION_PROMPT, text);
        
        let body = serde_json::json!({
            "model": self.config.model,
            "max_tokens": self.config.max_tokens,
            "messages": [
                {
                    "role": "user",
                    "content": prompt
                }
            ]
        });
        
        let url = format!("{}/v1/messages", self.config.api_url);
        
        let response = self.client
            .post(&url)
            .headers(self.build_headers()?)
            .json(&body)
            .send()?;
        
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            return Err(format!("Anthropic API error {}: {}", status, body).into());
        }
        
        let response_json: serde_json::Value = response.json()?;
        
        // Extract the text content from the response
        let content_text = response_json
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|item| item.get("text"))
            .and_then(|t| t.as_str())
            .ok_or("Invalid response structure from Anthropic API")?;
        
        parse_extraction_response(content_text)
    }
}

/// Configuration for Ollama-based extraction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaExtractorConfig {
    /// Ollama host URL (default: "http://localhost:11434")
    pub host: String,
    /// Model to use (default: "llama3.2:3b")
    pub model: String,
    /// Request timeout in seconds (default: 60)
    pub timeout_secs: u64,
}

impl Default for OllamaExtractorConfig {
    fn default() -> Self {
        Self {
            host: "http://localhost:11434".to_string(),
            model: "llama3.2:3b".to_string(),
            timeout_secs: 60,
        }
    }
}

/// Extracts facts using a local Ollama chat model.
///
/// Useful for local/private extraction without API costs.
pub struct OllamaExtractor {
    config: OllamaExtractorConfig,
    client: reqwest::blocking::Client,
}

impl OllamaExtractor {
    /// Create a new OllamaExtractor with the specified model.
    pub fn new(model: &str) -> Self {
        Self::with_config(OllamaExtractorConfig {
            model: model.to_string(),
            ..Default::default()
        })
    }
    
    /// Create a new OllamaExtractor with custom host and model.
    pub fn with_host(model: &str, host: &str) -> Self {
        Self::with_config(OllamaExtractorConfig {
            host: host.to_string(),
            model: model.to_string(),
            ..Default::default()
        })
    }
    
    /// Create a new OllamaExtractor with full config.
    pub fn with_config(config: OllamaExtractorConfig) -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()
            .expect("failed to create HTTP client");
        
        Self { config, client }
    }
}

impl MemoryExtractor for OllamaExtractor {
    fn extract(&self, text: &str) -> Result<Vec<ExtractedFact>, Box<dyn Error + Send + Sync>> {
        let prompt = format!("{}{}", EXTRACTION_PROMPT, text);
        
        let body = serde_json::json!({
            "model": self.config.model,
            "messages": [
                {
                    "role": "user",
                    "content": prompt
                }
            ],
            "stream": false
        });
        
        let url = format!("{}/api/chat", self.config.host);
        
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
        
        // Extract the message content from Ollama response
        let content_text = response_json
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .ok_or("Invalid response structure from Ollama API")?;
        
        parse_extraction_response(content_text)
    }
}

/// Parse LLM extraction response into ExtractedFacts.
///
/// Dual-path parser:
/// - Path 1: New dimensional format `{"memories": [{core_fact, ...}]}`
/// - Path 2: Legacy format `[{content, memory_type, importance, tags}]`
///
/// Handles common LLM quirks: markdown-wrapped JSON, extra whitespace.
fn parse_extraction_response(content: &str) -> Result<Vec<ExtractedFact>, Box<dyn Error + Send + Sync>> {
    // Strip markdown code blocks if present
    let json_str = content
        .trim()
        .strip_prefix("```json")
        .or_else(|| content.trim().strip_prefix("```"))
        .map(|s| s.strip_suffix("```").unwrap_or(s))
        .unwrap_or(content)
        .trim();

    // Path 1: Try new dimensional format {"memories": [...]}
    if let Ok(dimensional) = serde_json::from_str::<DimensionalResponse>(json_str) {
        let valid: Vec<ExtractedFact> = dimensional.memories
            .into_iter()
            .map(|mut f| {
                f.importance = f.importance.clamp(0.0, 1.0);
                f.valence = f.valence.clamp(-1.0, 1.0);
                f
            })
            .filter(|f| !f.core_fact.is_empty())
            .collect();
        return Ok(valid);
    }

    // Also try: the LLM might return just the array without the wrapper
    // (i.e., `[{core_fact: ...}]` instead of `{"memories": [...]}`)
    if let Some(start) = json_str.find('[') {
        if let Some(end) = json_str.rfind(']') {
            if start < end {
                let arr_str = &json_str[start..=end];
                // Try parsing as array of dimensional facts
                if let Ok(facts) = serde_json::from_str::<Vec<ExtractedFact>>(arr_str) {
                    let valid: Vec<ExtractedFact> = facts
                        .into_iter()
                        .map(|mut f| {
                            f.importance = f.importance.clamp(0.0, 1.0);
                            f.valence = f.valence.clamp(-1.0, 1.0);
                            f
                        })
                        .filter(|f| !f.core_fact.is_empty())
                        .collect();
                    if !valid.is_empty() {
                        return Ok(valid);
                    }
                }
            }
        }
    }

    // Path 2: Legacy format [{content, memory_type, importance, ...}]
    // Handle empty array case
    if json_str.trim() == "[]" || json_str.contains(r#""memories": []"#) || json_str.contains(r#""memories":[]"#) {
        return Ok(vec![]);
    }

    let json_start = json_str.find('[');
    let json_end = json_str.rfind(']');

    let json_to_parse = match (json_start, json_end) {
        (Some(start), Some(end)) if start < end => &json_str[start..=end],
        _ => {
            log::warn!("No JSON array found in extraction response: {}", json_str);
            return Ok(vec![]);
        }
    };

    match serde_json::from_str::<Vec<LegacyExtractedFact>>(json_to_parse) {
        Ok(facts) => {
            let valid_facts: Vec<ExtractedFact> = facts
                .into_iter()
                .filter(|f| !f.content.is_empty())
                .map(|f| ExtractedFact {
                    core_fact: f.content,
                    importance: f.importance.clamp(0.0, 1.0),
                    tags: f.tags,
                    confidence: f.confidence,
                    valence: f.valence.clamp(-1.0, 1.0),
                    domain: f.domain,
                    // All dimensions empty — legacy format has none
                    ..Default::default()
                })
                .collect();
            Ok(valid_facts)
        }
        Err(e) => {
            log::warn!("Failed to parse extraction JSON: {} - content: {}", e, json_to_parse);
            Ok(vec![])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_parse_new_dimensional_format() {
        let response = r#"{"memories": [{"core_fact": "User prefers tea over coffee", "stance": "prefers tea", "importance": 0.6, "tags": ["preference"], "confidence": "confident", "valence": 0.1, "domain": "general"}]}"#;
        let facts = parse_extraction_response(response).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].core_fact, "User prefers tea over coffee");
        assert_eq!(facts[0].stance.as_deref(), Some("prefers tea"));
        assert!((facts[0].importance - 0.6).abs() < 0.001);
    }
    
    #[test]
    fn test_parse_new_format_array_without_wrapper() {
        let response = r#"[{"core_fact": "Meeting at 3pm", "temporal": "3pm today", "importance": 0.7, "tags": ["meeting"], "confidence": "confident", "valence": 0.0, "domain": "communication"}]"#;
        let facts = parse_extraction_response(response).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].core_fact, "Meeting at 3pm");
        assert_eq!(facts[0].temporal.as_deref(), Some("3pm today"));
    }
    
    #[test]
    fn test_parse_markdown_wrapped_new_format() {
        let response = r#"```json
{"memories": [{"core_fact": "Meeting scheduled for Friday", "temporal": "Friday", "importance": 0.8, "tags": [], "confidence": "confident", "valence": 0.0, "domain": "communication"}]}
```"#;
        let facts = parse_extraction_response(response).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].core_fact, "Meeting scheduled for Friday");
    }
    
    #[test]
    fn test_parse_legacy_format() {
        let response = r#"[{"content": "User prefers tea over coffee", "memory_type": "relational", "importance": 0.6}]"#;
        let facts = parse_extraction_response(response).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].core_fact, "User prefers tea over coffee");
        // Legacy format: no dimensional fields
        assert!(facts[0].participants.is_none());
        assert!(facts[0].temporal.is_none());
    }
    
    #[test]
    fn test_parse_legacy_with_surrounding_text() {
        let response = r#"Here are the extracted facts:
[{"content": "Project deadline is next week", "memory_type": "factual", "importance": 0.9}]
Hope this helps!"#;
        let facts = parse_extraction_response(response).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].core_fact, "Project deadline is next week");
    }
    
    #[test]
    fn test_parse_empty_array() {
        let response = "[]";
        let facts = parse_extraction_response(response).unwrap();
        assert!(facts.is_empty());
    }
    
    #[test]
    fn test_parse_empty_memories() {
        let response = r#"{"memories": []}"#;
        let facts = parse_extraction_response(response).unwrap();
        assert!(facts.is_empty());
    }
    
    #[test]
    fn test_parse_invalid_json() {
        let response = "This is not JSON at all";
        let facts = parse_extraction_response(response).unwrap();
        assert!(facts.is_empty());
    }
    
    #[test]
    fn test_parse_clamps_importance() {
        let response = r#"{"memories": [
            {"core_fact": "Low", "importance": -0.5, "tags": [], "confidence": "likely", "valence": 0.0, "domain": "general"},
            {"core_fact": "High", "importance": 1.5, "tags": [], "confidence": "likely", "valence": 0.0, "domain": "general"}
        ]}"#;
        let facts = parse_extraction_response(response).unwrap();
        assert_eq!(facts.len(), 2);
        assert_eq!(facts[0].importance, 0.0);
        assert_eq!(facts[1].importance, 1.0);
    }
    
    #[test]
    fn test_parse_filters_empty_core_fact() {
        let response = r#"{"memories": [
            {"core_fact": "", "importance": 0.5, "tags": [], "confidence": "likely", "valence": 0.0, "domain": "general"},
            {"core_fact": "Valid fact", "importance": 0.5, "tags": [], "confidence": "likely", "valence": 0.0, "domain": "general"}
        ]}"#;
        let facts = parse_extraction_response(response).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].core_fact, "Valid fact");
    }

    #[test]
    fn test_parse_legacy_filters_empty() {
        let response = r#"[
            {"content": "", "memory_type": "factual", "importance": 0.5},
            {"content": "Valid fact", "memory_type": "factual", "importance": 0.5}
        ]"#;
        let facts = parse_extraction_response(response).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].core_fact, "Valid fact");
    }
    
    #[test]
    fn test_parse_multiple_dimensional_facts() {
        let response = r#"{"memories": [
            {"core_fact": "Fact 1", "importance": 0.3, "tags": [], "confidence": "confident", "valence": 0.0, "domain": "general"},
            {"core_fact": "Fact 2", "temporal": "yesterday", "importance": 0.7, "tags": [], "confidence": "likely", "valence": 0.0, "domain": "coding"},
            {"core_fact": "Fact 3", "participants": "potato", "importance": 0.9, "tags": [], "confidence": "confident", "valence": 0.3, "domain": "communication"}
        ]}"#;
        let facts = parse_extraction_response(response).unwrap();
        assert_eq!(facts.len(), 3);
        assert!(facts[1].temporal.is_some());
        assert!(facts[2].participants.is_some());
    }

    #[test]
    fn test_parse_all_dimensions() {
        let response = r#"{"memories": [{
            "core_fact": "potato rewrote in Rust",
            "participants": "potato",
            "temporal": "yesterday",
            "location": "home office",
            "context": "Python was too slow",
            "causation": "performance bottleneck",
            "outcome": "rewrite completed",
            "method": "spent evening coding",
            "relations": "related to engramai project",
            "sentiment": "excited",
            "stance": "prefers Rust over Python for perf",
            "importance": 0.8,
            "tags": ["rust", "python"],
            "confidence": "confident",
            "valence": 0.6,
            "domain": "coding"
        }]}"#;
        let facts = parse_extraction_response(response).unwrap();
        assert_eq!(facts.len(), 1);
        let f = &facts[0];
        assert_eq!(f.core_fact, "potato rewrote in Rust");
        assert_eq!(f.participants.as_deref(), Some("potato"));
        assert_eq!(f.temporal.as_deref(), Some("yesterday"));
        assert_eq!(f.location.as_deref(), Some("home office"));
        assert_eq!(f.context.as_deref(), Some("Python was too slow"));
        assert_eq!(f.causation.as_deref(), Some("performance bottleneck"));
        assert_eq!(f.outcome.as_deref(), Some("rewrite completed"));
        assert_eq!(f.method.as_deref(), Some("spent evening coding"));
        assert_eq!(f.relations.as_deref(), Some("related to engramai project"));
        assert_eq!(f.sentiment.as_deref(), Some("excited"));
        assert_eq!(f.stance.as_deref(), Some("prefers Rust over Python for perf"));
        assert_eq!(f.valence, 0.6);
        assert_eq!(f.domain, "coding");
    }
    
    #[test]
    fn test_extraction_prompt_format() {
        assert!(EXTRACTION_PROMPT.contains("core_fact"));
        assert!(EXTRACTION_PROMPT.contains("SAME LANGUAGE"));
        assert!(EXTRACTION_PROMPT.contains("importance"));
        assert!(EXTRACTION_PROMPT.contains("dimensional"));
    }

    #[test]
    fn test_default_extracted_fact() {
        let fact = ExtractedFact::default();
        assert!(fact.core_fact.is_empty());
        assert!(fact.participants.is_none());
        assert!(fact.temporal.is_none());
        assert_eq!(fact.confidence, "likely");
        assert_eq!(fact.domain, "general");
        assert_eq!(fact.valence, 0.0);
    }
    
    #[test]
    #[ignore] // Requires Ollama running locally
    fn test_ollama_extraction() {
        let extractor = OllamaExtractor::new("llama3.2:3b");
        let facts = extractor.extract("I really love pizza, especially pepperoni. My favorite restaurant is Mario's.").unwrap();
        println!("Extracted facts: {:?}", facts);
    }
    
    #[test]
    #[ignore] // Requires Anthropic API key
    fn test_anthropic_extraction() {
        let api_key = std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY not set");
        let extractor = AnthropicExtractor::new(&api_key, false);
        let facts = extractor.extract("我昨天和小明一起去吃了火锅，很好吃。小明说他下周要去上海出差。").unwrap();
        println!("Extracted facts: {:?}", facts);
    }
}
