//! LLM-based memory extraction.
//!
//! Converts raw text into structured facts using LLMs. Optional feature
//! that preserves backward compatibility — if no extractor is set,
//! memories are stored as-is.

use serde::{Deserialize, Serialize};
use std::error::Error;
use std::time::Duration;

/// A single extracted fact from a conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedFact {
    /// The extracted fact content (self-contained, understandable without context)
    pub content: String,
    /// Memory type classification: "factual", "episodic", "relational", "procedural", "emotional", "opinion", "causal"
    pub memory_type: String,
    /// Importance score (0.0 - 1.0)
    pub importance: f64,
    /// Confidence level: "confident", "likely", "uncertain"
    #[serde(default = "default_confidence")]
    pub confidence: String,
}

fn default_confidence() -> String {
    "likely".to_string()
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

/// The extraction prompt template.
const EXTRACTION_PROMPT: &str = r#"You are a memory extraction system. Extract key facts from the following conversation that are worth remembering long-term.

Rules:
- Extract concrete facts, preferences, decisions, and commitments
- Each fact should be self-contained (understandable without context)
- Skip greetings, filler, acknowledgments
- Classify each fact: factual, episodic, relational, procedural, emotional, opinion, causal
- Rate importance 0.0-1.0 (preferences=0.6, decisions=0.8, commitments=0.9)
- Rate confidence: "confident" (direct statement, clear fact), "likely" (reasonable inference), "uncertain" (vague mention, speculation)
- If nothing worth remembering, return empty array
- Respond in the SAME LANGUAGE as the input

DO NOT extract any of these — return empty array [] if the input contains ONLY these:
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

Respond with ONLY a JSON array (no markdown, no explanation):
[{"content": "...", "memory_type": "...", "importance": 0.X, "confidence": "confident|likely|uncertain"}]

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

/// Static token provider — wraps a fixed string. For backward compatibility.
struct StaticToken(String);

impl TokenProvider for StaticToken {
    fn get_token(&self) -> Result<String, Box<dyn Error + Send + Sync>> {
        Ok(self.0.clone())
    }
}

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
        let mut headers = reqwest::header::HeaderMap::new();
        let token = self.token_provider.get_token()?;
        
        headers.insert("anthropic-version", "2023-06-01".parse().unwrap());
        headers.insert("content-type", "application/json".parse().unwrap());
        
        if self.is_oauth {
            // OAuth mode — mimic Claude Code stealth headers
            headers.insert(
                "anthropic-beta",
                "claude-code-20250219,oauth-2025-04-20".parse().unwrap(),
            );
            headers.insert(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", token).parse().unwrap(),
            );
            headers.insert(
                reqwest::header::USER_AGENT,
                "claude-cli/2.1.39 (external, cli)".parse().unwrap(),
            );
            headers.insert("x-app", "cli".parse().unwrap());
            headers.insert(
                "anthropic-dangerous-direct-browser-access",
                "true".parse().unwrap(),
            );
        } else {
            // API key mode
            headers.insert("x-api-key", token.parse().unwrap());
        }
        
        Ok(headers)
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
/// Handles common LLM quirks:
/// - Markdown-wrapped JSON (```json ... ```)
/// - Extra whitespace
/// - Invalid JSON (returns empty vec with warning)
fn parse_extraction_response(content: &str) -> Result<Vec<ExtractedFact>, Box<dyn Error + Send + Sync>> {
    // Strip markdown code blocks if present
    let json_str = content
        .trim()
        .strip_prefix("```json")
        .or_else(|| content.trim().strip_prefix("```"))
        .map(|s| s.strip_suffix("```").unwrap_or(s))
        .unwrap_or(content)
        .trim();
    
    // Handle empty array case
    if json_str == "[]" {
        return Ok(vec![]);
    }
    
    // Try to find JSON array in the response
    let json_start = json_str.find('[');
    let json_end = json_str.rfind(']');
    
    let json_to_parse = match (json_start, json_end) {
        (Some(start), Some(end)) if start < end => &json_str[start..=end],
        _ => {
            log::warn!("No JSON array found in extraction response: {}", json_str);
            return Ok(vec![]);
        }
    };
    
    match serde_json::from_str::<Vec<ExtractedFact>>(json_to_parse) {
        Ok(facts) => {
            // Transform and filter facts
            let valid_facts: Vec<ExtractedFact> = facts
                .into_iter()
                .map(|mut f| {
                    // Normalize memory_type to lowercase
                    f.memory_type = f.memory_type.to_lowercase();
                    // Clamp importance to valid range
                    f.importance = f.importance.clamp(0.0, 1.0);
                    f
                })
                .filter(|f| {
                    // Only filter out empty content
                    !f.content.is_empty()
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
    fn test_parse_clean_json() {
        let response = r#"[{"content": "User prefers tea over coffee", "memory_type": "relational", "importance": 0.6}]"#;
        let facts = parse_extraction_response(response).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].content, "User prefers tea over coffee");
        assert_eq!(facts[0].memory_type, "relational");
        assert!((facts[0].importance - 0.6).abs() < 0.001);
    }
    
    #[test]
    fn test_parse_markdown_wrapped() {
        let response = r#"```json
[{"content": "Meeting scheduled for Friday", "memory_type": "episodic", "importance": 0.8}]
```"#;
        let facts = parse_extraction_response(response).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].content, "Meeting scheduled for Friday");
    }
    
    #[test]
    fn test_parse_empty_array() {
        let response = "[]";
        let facts = parse_extraction_response(response).unwrap();
        assert!(facts.is_empty());
    }
    
    #[test]
    fn test_parse_invalid_json() {
        let response = "This is not JSON at all";
        let facts = parse_extraction_response(response).unwrap();
        assert!(facts.is_empty()); // Should return empty, not error
    }
    
    #[test]
    fn test_parse_with_surrounding_text() {
        let response = r#"Here are the extracted facts:
[{"content": "Project deadline is next week", "memory_type": "factual", "importance": 0.9}]
Hope this helps!"#;
        let facts = parse_extraction_response(response).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].content, "Project deadline is next week");
    }
    
    #[test]
    fn test_parse_normalizes_memory_type() {
        let response = r#"[{"content": "Test fact", "memory_type": "FACTUAL", "importance": 0.5}]"#;
        let facts = parse_extraction_response(response).unwrap();
        assert_eq!(facts[0].memory_type, "factual"); // Should be lowercase
    }
    
    #[test]
    fn test_parse_clamps_importance() {
        let response = r#"[
            {"content": "Low", "memory_type": "factual", "importance": -0.5},
            {"content": "High", "memory_type": "factual", "importance": 1.5}
        ]"#;
        let facts = parse_extraction_response(response).unwrap();
        assert_eq!(facts.len(), 2);
        assert_eq!(facts[0].importance, 0.0); // Clamped from -0.5
        assert_eq!(facts[1].importance, 1.0); // Clamped from 1.5
    }
    
    #[test]
    fn test_parse_filters_empty_content() {
        let response = r#"[
            {"content": "", "memory_type": "factual", "importance": 0.5},
            {"content": "Valid fact", "memory_type": "factual", "importance": 0.5}
        ]"#;
        let facts = parse_extraction_response(response).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].content, "Valid fact");
    }
    
    #[test]
    fn test_parse_multiple_facts() {
        let response = r#"[
            {"content": "Fact 1", "memory_type": "factual", "importance": 0.3},
            {"content": "Fact 2", "memory_type": "episodic", "importance": 0.7},
            {"content": "Fact 3", "memory_type": "relational", "importance": 0.9}
        ]"#;
        let facts = parse_extraction_response(response).unwrap();
        assert_eq!(facts.len(), 3);
    }
    
    #[test]
    fn test_extraction_prompt_format() {
        // Verify the prompt is well-formed
        assert!(EXTRACTION_PROMPT.contains("JSON array"));
        assert!(EXTRACTION_PROMPT.contains("SAME LANGUAGE"));
        assert!(EXTRACTION_PROMPT.contains("importance"));
    }
    
    #[test]
    #[ignore] // Requires Ollama running locally
    fn test_ollama_extraction() {
        let extractor = OllamaExtractor::new("llama3.2:3b");
        let facts = extractor.extract("I really love pizza, especially pepperoni. My favorite restaurant is Mario's.").unwrap();
        println!("Extracted facts: {:?}", facts);
        // Should extract something about pizza preference
    }
    
    #[test]
    #[ignore] // Requires Anthropic API key
    fn test_anthropic_extraction() {
        let api_key = std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY not set");
        let extractor = AnthropicExtractor::new(&api_key, false);
        let facts = extractor.extract("我昨天和小明一起去吃了火锅，很好吃。小明说他下周要去上海出差。").unwrap();
        println!("Extracted facts: {:?}", facts);
        // Should extract facts in Chinese
    }
}
