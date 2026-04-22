//! Shared Anthropic API client utilities.
//!
//! Centralizes header construction, static token provider, and API URL constant
//! used by `extractor.rs`, `triple_extractor.rs`, and `HaikuIntentClassifier`.

use reqwest::header::HeaderMap;
use crate::extractor::TokenProvider;

/// Default Anthropic API URL.
pub const DEFAULT_ANTHROPIC_API_URL: &str = "https://api.anthropic.com";

/// Static token provider — wraps a fixed string. For backward compatibility.
pub struct StaticToken(pub String);

impl TokenProvider for StaticToken {
    fn get_token(&self) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        Ok(self.0.clone())
    }
}

/// Build request headers for Anthropic API calls.
///
/// Handles both OAuth (Claude Max) and API key authentication modes.
pub fn build_anthropic_headers(token: &str, is_oauth: bool) -> HeaderMap {
    let mut headers = HeaderMap::new();

    headers.insert("anthropic-version", "2023-06-01".parse().unwrap());
    headers.insert("content-type", "application/json".parse().unwrap());

    if is_oauth {
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

    headers
}
