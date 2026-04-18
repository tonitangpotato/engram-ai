//! LLM provider abstraction with multi-provider support and model routing.
//!
//! Provides a synchronous [`LlmProvider`] trait and concrete implementations
//! for OpenAI, Anthropic, local (Ollama), and a no-op fallback. The
//! [`ModelRouter`] allows per-[`LlmTask`] provider overrides.

use std::collections::HashMap;
use std::time::Instant;

use serde_json::json;

use super::types::*;

// ═══════════════════════════════════════════════════════════════════════════════
//  TRAIT
// ═══════════════════════════════════════════════════════════════════════════════

/// Synchronous LLM provider interface.
pub trait LlmProvider: Send + Sync {
    /// Send a completion request and return the response.
    fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError>;

    /// Static metadata describing this provider.
    fn metadata(&self) -> ProviderMetadata;

    /// Lightweight connectivity check.
    fn health_check(&self) -> Result<(), LlmError>;
}

// ═══════════════════════════════════════════════════════════════════════════════
//  TOKEN ESTIMATION
// ═══════════════════════════════════════════════════════════════════════════════

/// Rough token estimate for a string.
///
/// Heuristic: ~4 chars per token for Latin scripts, ~2 chars per token for
/// CJK characters (code-points above U+2E80).
pub fn estimate_tokens(text: &str, _model: &str) -> u32 {
    let mut cjk_chars: usize = 0;
    let mut other_chars: usize = 0;
    for c in text.chars() {
        if (c as u32) > 0x2E80 {
            cjk_chars += 1;
        } else {
            other_chars += 1;
        }
    }
    ((other_chars as f64 / 4.0) + (cjk_chars as f64 / 2.0)) as u32
}

// ═══════════════════════════════════════════════════════════════════════════════
//  SYSTEM PROMPT HELPER
// ═══════════════════════════════════════════════════════════════════════════════

/// Derive a system prompt from the task type.
fn system_prompt_for_task(task: &LlmTask) -> &'static str {
    match task {
        LlmTask::Compile => {
            "You are a knowledge compiler. Synthesize the provided memories into a coherent topic page."
        }
        LlmTask::Enhance => {
            "You are a knowledge editor. Improve the clarity and completeness of the provided text."
        }
        LlmTask::Summarize => {
            "You are a summarizer. Produce a concise summary of the provided content."
        }
        LlmTask::DetectConflict => {
            "You are a conflict detector. Identify contradictions or inconsistencies in the provided content."
        }
        LlmTask::GenerateTitle => {
            "You are a title generator. Produce a short, descriptive title for the provided content."
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  NOOP PROVIDER
// ═══════════════════════════════════════════════════════════════════════════════

/// No-op provider returned when no LLM backend is configured.
///
/// Every method returns [`LlmError::ProviderUnavailable`].
pub struct NoopProvider;

impl LlmProvider for NoopProvider {
    fn complete(&self, _request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        Err(LlmError::ProviderUnavailable(
            "No LLM provider configured".to_string(),
        ))
    }

    fn metadata(&self) -> ProviderMetadata {
        ProviderMetadata {
            name: "noop".to_string(),
            model: "none".to_string(),
            max_context_tokens: 0,
            supports_streaming: false,
        }
    }

    fn health_check(&self) -> Result<(), LlmError> {
        Err(LlmError::ProviderUnavailable(
            "No LLM provider configured".to_string(),
        ))
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  OPENAI PROVIDER
// ═══════════════════════════════════════════════════════════════════════════════

/// OpenAI-compatible chat completions provider.
pub struct OpenAiProvider {
    client: reqwest::blocking::Client,
    api_key: String,
    model: String,
    endpoint: String,
}

impl OpenAiProvider {
    /// Create a new provider from [`LlmConfig`].
    ///
    /// Reads the API key from the environment variable named in
    /// `config.api_key_env`.
    pub fn new(config: &LlmConfig) -> Result<Self, LlmError> {
        let api_key = std::env::var(&config.api_key_env).map_err(|_| {
            LlmError::ProviderUnavailable(format!("Env var {} not set", config.api_key_env))
        })?;
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(config.timeout_secs))
            .build()
            .map_err(|e| LlmError::ProviderUnavailable(e.to_string()))?;
        Ok(Self {
            client,
            api_key,
            model: config.model.clone(),
            endpoint: "https://api.openai.com/v1".to_string(),
        })
    }

    /// Override the base endpoint (e.g. for Azure or a local proxy).
    pub fn with_endpoint(mut self, endpoint: String) -> Self {
        self.endpoint = endpoint;
        self
    }
}

impl LlmProvider for OpenAiProvider {
    fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        let system = system_prompt_for_task(&request.task);
        let temperature = request.temperature.unwrap_or(0.7);
        let max_tokens = request.max_tokens.unwrap_or(1024);

        let body = json!({
            "model": self.model,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": &request.prompt },
            ],
            "max_tokens": max_tokens,
            "temperature": temperature,
        });

        let start = Instant::now();
        let resp = self
            .client
            .post(format!("{}/chat/completions", self.endpoint))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .map_err(|e| {
                if e.is_timeout() {
                    LlmError::Timeout
                } else {
                    LlmError::ProviderUnavailable(e.to_string())
                }
            })?;
        let duration_ms = start.elapsed().as_millis() as u64;

        let status = resp.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let retry_after = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(60);
            return Err(LlmError::RateLimited {
                retry_after_secs: retry_after,
            });
        }

        let json: serde_json::Value = resp
            .json()
            .map_err(|e| LlmError::InvalidResponse(e.to_string()))?;

        if !status.is_success() {
            let msg = json["error"]["message"]
                .as_str()
                .unwrap_or("unknown error");
            return Err(LlmError::ProviderUnavailable(format!(
                "HTTP {}: {}",
                status, msg
            )));
        }

        let content = json["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string();

        let usage = TokenUsage {
            input_tokens: json["usage"]["prompt_tokens"].as_u64().unwrap_or(0) as u32,
            output_tokens: json["usage"]["completion_tokens"].as_u64().unwrap_or(0) as u32,
        };

        let model = json["model"]
            .as_str()
            .unwrap_or(&self.model)
            .to_string();

        Ok(LlmResponse {
            content,
            usage,
            model,
            duration_ms,
        })
    }

    fn metadata(&self) -> ProviderMetadata {
        ProviderMetadata {
            name: "openai".to_string(),
            model: self.model.clone(),
            max_context_tokens: 128_000, // GPT-4o class
            supports_streaming: true,
        }
    }

    fn health_check(&self) -> Result<(), LlmError> {
        // Lightweight: hit the models endpoint to confirm auth.
        let resp = self
            .client
            .get(format!("{}/models", self.endpoint))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .map_err(|e| LlmError::ProviderUnavailable(e.to_string()))?;

        if resp.status().is_success() {
            Ok(())
        } else {
            Err(LlmError::ProviderUnavailable(format!(
                "Health check failed: HTTP {}",
                resp.status()
            )))
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  ANTHROPIC PROVIDER
// ═══════════════════════════════════════════════════════════════════════════════

/// Anthropic Messages API provider.
pub struct AnthropicProvider {
    client: reqwest::blocking::Client,
    api_key: String,
    model: String,
}

impl AnthropicProvider {
    /// Create from [`LlmConfig`]. Reads the API key from the environment.
    pub fn new(config: &LlmConfig) -> Result<Self, LlmError> {
        let api_key = std::env::var(&config.api_key_env).map_err(|_| {
            LlmError::ProviderUnavailable(format!("Env var {} not set", config.api_key_env))
        })?;
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(config.timeout_secs))
            .build()
            .map_err(|e| LlmError::ProviderUnavailable(e.to_string()))?;
        Ok(Self {
            client,
            api_key,
            model: config.model.clone(),
        })
    }
}

impl LlmProvider for AnthropicProvider {
    fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        let system = system_prompt_for_task(&request.task);
        let temperature = request.temperature.unwrap_or(0.7);
        let max_tokens = request.max_tokens.unwrap_or(1024);

        let body = json!({
            "model": self.model,
            "max_tokens": max_tokens,
            "system": system,
            "messages": [
                { "role": "user", "content": &request.prompt },
            ],
            "temperature": temperature,
        });

        let start = Instant::now();
        let resp = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .map_err(|e| {
                if e.is_timeout() {
                    LlmError::Timeout
                } else {
                    LlmError::ProviderUnavailable(e.to_string())
                }
            })?;
        let duration_ms = start.elapsed().as_millis() as u64;

        let status = resp.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let retry_after = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(60);
            return Err(LlmError::RateLimited {
                retry_after_secs: retry_after,
            });
        }

        let json: serde_json::Value = resp
            .json()
            .map_err(|e| LlmError::InvalidResponse(e.to_string()))?;

        if !status.is_success() {
            let msg = json["error"]["message"]
                .as_str()
                .unwrap_or("unknown error");
            return Err(LlmError::ProviderUnavailable(format!(
                "HTTP {}: {}",
                status, msg
            )));
        }

        let content = json["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .to_string();

        let usage = TokenUsage {
            input_tokens: json["usage"]["input_tokens"].as_u64().unwrap_or(0) as u32,
            output_tokens: json["usage"]["output_tokens"].as_u64().unwrap_or(0) as u32,
        };

        let model = json["model"]
            .as_str()
            .unwrap_or(&self.model)
            .to_string();

        Ok(LlmResponse {
            content,
            usage,
            model,
            duration_ms,
        })
    }

    fn metadata(&self) -> ProviderMetadata {
        ProviderMetadata {
            name: "anthropic".to_string(),
            model: self.model.clone(),
            max_context_tokens: 200_000, // Claude 3 class
            supports_streaming: true,
        }
    }

    fn health_check(&self) -> Result<(), LlmError> {
        // Anthropic doesn't have a lightweight health endpoint; send a minimal
        // request and check for auth errors.
        let body = json!({
            "model": self.model,
            "max_tokens": 1,
            "messages": [{ "role": "user", "content": "ping" }],
        });

        let resp = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .map_err(|e| LlmError::ProviderUnavailable(e.to_string()))?;

        if resp.status().is_success() || resp.status() == reqwest::StatusCode::OK {
            Ok(())
        } else if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            Err(LlmError::ProviderUnavailable(
                "Invalid API key".to_string(),
            ))
        } else {
            Err(LlmError::ProviderUnavailable(format!(
                "Health check failed: HTTP {}",
                resp.status()
            )))
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  LOCAL PROVIDER (Ollama)
// ═══════════════════════════════════════════════════════════════════════════════

/// Local LLM provider using the Ollama HTTP API.
pub struct LocalProvider {
    client: reqwest::blocking::Client,
    endpoint: String,
    model: String,
}

impl LocalProvider {
    /// Create from [`LlmConfig`]. No API key required.
    pub fn new(config: &LlmConfig) -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(config.timeout_secs))
            .build()
            .unwrap_or_default();
        Self {
            client,
            endpoint: "http://localhost:11434".to_string(),
            model: config.model.clone(),
        }
    }

    /// Override the Ollama endpoint.
    pub fn with_endpoint(mut self, endpoint: String) -> Self {
        self.endpoint = endpoint;
        self
    }
}

impl LlmProvider for LocalProvider {
    fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        let system = system_prompt_for_task(&request.task);
        let full_prompt = format!("{}\n\n{}", system, request.prompt);

        let body = json!({
            "model": self.model,
            "prompt": full_prompt,
            "stream": false,
        });

        let start = Instant::now();
        let resp = self
            .client
            .post(format!("{}/api/generate", self.endpoint))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .map_err(|e| {
                if e.is_timeout() {
                    LlmError::Timeout
                } else {
                    LlmError::ProviderUnavailable(e.to_string())
                }
            })?;
        let duration_ms = start.elapsed().as_millis() as u64;

        if !resp.status().is_success() {
            return Err(LlmError::ProviderUnavailable(format!(
                "HTTP {}",
                resp.status()
            )));
        }

        let json: serde_json::Value = resp
            .json()
            .map_err(|e| LlmError::InvalidResponse(e.to_string()))?;

        let content = json["response"].as_str().unwrap_or("").to_string();

        // Ollama provides eval/prompt token counts when available.
        let usage = TokenUsage {
            input_tokens: json["prompt_eval_count"].as_u64().unwrap_or(0) as u32,
            output_tokens: json["eval_count"].as_u64().unwrap_or(0) as u32,
        };

        Ok(LlmResponse {
            content,
            usage,
            model: self.model.clone(),
            duration_ms,
        })
    }

    fn metadata(&self) -> ProviderMetadata {
        ProviderMetadata {
            name: "local".to_string(),
            model: self.model.clone(),
            max_context_tokens: 8_192, // conservative default
            supports_streaming: true,
        }
    }

    fn health_check(&self) -> Result<(), LlmError> {
        let resp = self
            .client
            .get(format!("{}/api/tags", self.endpoint))
            .send()
            .map_err(|e| LlmError::ProviderUnavailable(e.to_string()))?;

        if resp.status().is_success() {
            Ok(())
        } else {
            Err(LlmError::ProviderUnavailable(format!(
                "Health check failed: HTTP {}",
                resp.status()
            )))
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  MODEL ROUTER
// ═══════════════════════════════════════════════════════════════════════════════

/// Routes LLM requests to different providers based on the [`LlmTask`].
///
/// A default provider handles all tasks unless an explicit per-task override
/// is registered via [`with_task_override`](ModelRouter::with_task_override).
pub struct ModelRouter {
    default: Box<dyn LlmProvider>,
    task_overrides: HashMap<String, Box<dyn LlmProvider>>,
}

impl ModelRouter {
    /// Create a router with a default provider.
    pub fn new(default: Box<dyn LlmProvider>) -> Self {
        Self {
            default,
            task_overrides: HashMap::new(),
        }
    }

    /// Register a provider override for a specific task type.
    pub fn with_task_override(mut self, task: LlmTask, provider: Box<dyn LlmProvider>) -> Self {
        let task_key = format!("{:?}", task);
        self.task_overrides.insert(task_key, provider);
        self
    }

    /// Complete a request, selecting the provider based on the task.
    pub fn complete_for_task(
        &self,
        task: LlmTask,
        request: &LlmRequest,
    ) -> Result<LlmResponse, LlmError> {
        let task_key = format!("{:?}", task);
        let provider = self
            .task_overrides
            .get(&task_key)
            .map(|p| p.as_ref())
            .unwrap_or(self.default.as_ref());
        provider.complete(request)
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  FACTORY
// ═══════════════════════════════════════════════════════════════════════════════

/// Create the appropriate [`LlmProvider`] from config.
///
/// Falls back to [`NoopProvider`] when the provider string is empty, `"none"`,
/// unknown, or when construction fails (e.g. missing API key).
pub fn create_provider(config: &LlmConfig) -> Box<dyn LlmProvider> {
    if config.provider.is_empty() || config.provider == "none" {
        return Box::new(NoopProvider);
    }
    match config.provider.as_str() {
        "openai" => match OpenAiProvider::new(config) {
            Ok(p) => Box::new(p),
            Err(e) => {
                log::warn!(
                    "Failed to create OpenAI provider: {}, falling back to noop",
                    e
                );
                Box::new(NoopProvider)
            }
        },
        "anthropic" => match AnthropicProvider::new(config) {
            Ok(p) => Box::new(p),
            Err(e) => {
                log::warn!(
                    "Failed to create Anthropic provider: {}, falling back to noop",
                    e
                );
                Box::new(NoopProvider)
            }
        },
        "local" => Box::new(LocalProvider::new(config)),
        other => {
            log::warn!("Unknown LLM provider '{}', falling back to noop", other);
            Box::new(NoopProvider)
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  TESTS
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_noop_provider() {
        let provider = NoopProvider;
        let req = LlmRequest {
            task: LlmTask::Summarize,
            prompt: "test".to_string(),
            max_tokens: None,
            temperature: None,
        };
        let result = provider.complete(&req);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            LlmError::ProviderUnavailable(_)
        ));

        let meta = provider.metadata();
        assert_eq!(meta.name, "noop");
        assert_eq!(meta.model, "none");

        assert!(provider.health_check().is_err());
    }

    #[test]
    fn test_create_provider_none() {
        // Empty provider string → NoopProvider
        let config = LlmConfig {
            provider: String::new(),
            model: "test".to_string(),
            api_key_env: "NONEXISTENT_KEY".to_string(),
            max_retries: 1,
            timeout_secs: 5,
            temperature: 0.5,
        };
        let provider = create_provider(&config);
        assert_eq!(provider.metadata().name, "noop");

        // Explicit "none" → NoopProvider
        let config_none = LlmConfig {
            provider: "none".to_string(),
            ..config
        };
        let provider_none = create_provider(&config_none);
        assert_eq!(provider_none.metadata().name, "noop");

        // Unknown provider → NoopProvider
        let config_unknown = LlmConfig {
            provider: "mystery-llm".to_string(),
            model: "test".to_string(),
            api_key_env: "NONEXISTENT_KEY".to_string(),
            max_retries: 1,
            timeout_secs: 5,
            temperature: 0.5,
        };
        let provider_unknown = create_provider(&config_unknown);
        assert_eq!(provider_unknown.metadata().name, "noop");
    }

    #[test]
    fn test_token_estimation() {
        // Pure English: ~4 chars per token
        let english = "Hello, world! This is a test string.";
        let tokens = estimate_tokens(english, "gpt-4");
        assert!(tokens > 0);
        // 35 chars / 4 ≈ 8
        assert_eq!(tokens, (english.chars().count() as f64 / 4.0) as u32);

        // Empty string
        assert_eq!(estimate_tokens("", "gpt-4"), 0);

        // Mixed CJK + English
        let mixed = "Hello 你好世界";
        let tokens_mixed = estimate_tokens(mixed, "gpt-4");
        // "Hello " = 6 non-CJK bytes → 6/4 = 1.5
        // "你好世界" = 4 CJK chars → 4/2 = 2.0
        // total ≈ 3
        assert!(tokens_mixed > 0);

        // Pure CJK
        let cjk = "你好世界测试";
        let tokens_cjk = estimate_tokens(cjk, "gpt-4");
        // 6 CJK chars → 6/2 = 3
        assert_eq!(tokens_cjk, 3);
    }

    #[test]
    fn test_model_router() {
        let router = ModelRouter::new(Box::new(NoopProvider));
        let req = LlmRequest {
            task: LlmTask::Compile,
            prompt: "test".to_string(),
            max_tokens: None,
            temperature: None,
        };

        // Default provider is noop → should fail
        let result = router.complete_for_task(LlmTask::Compile, &req);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            LlmError::ProviderUnavailable(_)
        ));

        // With override — still noop but tests the routing path
        let router = router.with_task_override(LlmTask::Summarize, Box::new(NoopProvider));
        let result = router.complete_for_task(LlmTask::Summarize, &req);
        assert!(result.is_err());
    }

    #[test]
    fn test_create_provider_missing_api_key() {
        // OpenAI with missing env var → falls back to noop
        let config = LlmConfig {
            provider: "openai".to_string(),
            model: "gpt-4o-mini".to_string(),
            api_key_env: "ENGRAM_TEST_NONEXISTENT_KEY_12345".to_string(),
            max_retries: 1,
            timeout_secs: 5,
            temperature: 0.5,
        };
        let provider = create_provider(&config);
        assert_eq!(provider.metadata().name, "noop");

        // Anthropic with missing env var → falls back to noop
        let config_anthropic = LlmConfig {
            provider: "anthropic".to_string(),
            ..config
        };
        let provider_anthropic = create_provider(&config_anthropic);
        assert_eq!(provider_anthropic.metadata().name, "noop");
    }

    #[test]
    fn test_local_provider_metadata() {
        let config = LlmConfig {
            provider: "local".to_string(),
            model: "llama3".to_string(),
            api_key_env: String::new(),
            max_retries: 1,
            timeout_secs: 30,
            temperature: 0.7,
        };
        let provider = create_provider(&config);
        let meta = provider.metadata();
        assert_eq!(meta.name, "local");
        assert_eq!(meta.model, "llama3");
    }

    #[test]
    fn test_system_prompt_selection() {
        // Ensure each task produces a distinct, non-empty system prompt.
        let tasks = [
            LlmTask::Compile,
            LlmTask::Enhance,
            LlmTask::Summarize,
            LlmTask::DetectConflict,
            LlmTask::GenerateTitle,
        ];
        let prompts: Vec<&str> = tasks.iter().map(system_prompt_for_task).collect();
        for p in &prompts {
            assert!(!p.is_empty());
        }
        // All distinct
        let unique: std::collections::HashSet<&&str> = prompts.iter().collect();
        assert_eq!(unique.len(), tasks.len());
    }
}
