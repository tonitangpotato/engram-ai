//! Embedding providers for semantic memory retrieval.
//!
//! This module provides vector embedding support using:
//! - Ollama (default): Local LLM server, no API key needed
//! - OpenAI: Cloud-based, requires OPENAI_API_KEY
//!
//! Embeddings enable semantic similarity search alongside ACT-R activation scoring.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Configuration for the embedding provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    /// Provider name: "ollama" or "openai"
    pub provider: String,
    /// Model name for embeddings
    /// - Ollama: "nomic-embed-text" (768 dims)
    /// - OpenAI: "text-embedding-3-small" (1536 dims) or "text-embedding-ada-002"
    pub model: String,
    /// Ollama host URL (default: "http://localhost:11434")
    /// For OpenAI, this is the API base URL ("https://api.openai.com/v1")
    pub host: String,
    /// Embedding vector dimensions
    /// - nomic-embed-text: 768
    /// - text-embedding-3-small: 1536
    /// - text-embedding-ada-002: 1536
    pub dimensions: usize,
    /// Connection timeout in seconds
    pub timeout_secs: u64,
    /// OpenAI API key (optional, can also use OPENAI_API_KEY env var)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            provider: "ollama".into(),
            model: "nomic-embed-text".into(),
            host: "http://localhost:11434".into(),
            dimensions: 768,
            timeout_secs: 30,
            api_key: None,
        }
    }
}

impl EmbeddingConfig {
    /// Get the model identifier in protocol format: `{provider}/{model}`.
    ///
    /// This is the canonical model string used in `memory_embeddings` table
    /// per the Engram Embedding Protocol v2.
    pub fn model_id(&self) -> String {
        format!("{}/{}", self.provider, self.model)
    }
    
    /// Create config for OpenAI embeddings.
    ///
    /// Uses text-embedding-3-small by default (1536 dimensions).
    /// API key is read from OPENAI_API_KEY env var if not provided.
    pub fn openai(api_key: Option<String>) -> Self {
        Self {
            provider: "openai".into(),
            model: "text-embedding-3-small".into(),
            host: "https://api.openai.com/v1".into(),
            dimensions: 1536,
            timeout_secs: 30,
            api_key,
        }
    }
    
    /// Create config for OpenAI ada-002 embeddings.
    pub fn openai_ada(api_key: Option<String>) -> Self {
        Self {
            provider: "openai".into(),
            model: "text-embedding-ada-002".into(),
            host: "https://api.openai.com/v1".into(),
            dimensions: 1536,
            timeout_secs: 30,
            api_key,
        }
    }
    
    /// Create config for Ollama with custom model.
    pub fn ollama(model: &str, dimensions: usize) -> Self {
        Self {
            provider: "ollama".into(),
            model: model.into(),
            host: "http://localhost:11434".into(),
            dimensions,
            timeout_secs: 30,
            api_key: None,
        }
    }
}

/// Ollama API request for embeddings.
#[derive(Serialize)]
struct OllamaEmbedRequest<'a> {
    model: &'a str,
    input: &'a str,
}

/// Ollama batch request.
#[derive(Serialize)]
struct OllamaBatchRequest<'a> {
    model: &'a str,
    input: Vec<&'a str>,
}

/// Ollama API response for embeddings.
#[derive(Deserialize)]
struct OllamaEmbedResponse {
    embeddings: Vec<Vec<f32>>,
}

/// OpenAI API request for embeddings.
#[derive(Serialize)]
struct OpenAIEmbedRequest<'a> {
    model: &'a str,
    input: Vec<&'a str>,
}

/// OpenAI API response for embeddings.
#[derive(Deserialize)]
struct OpenAIEmbedResponse {
    data: Vec<OpenAIEmbedding>,
}

#[derive(Deserialize)]
struct OpenAIEmbedding {
    embedding: Vec<f32>,
    index: usize,
}

/// Embedding provider supporting Ollama and OpenAI.
///
/// Uses reqwest::blocking since Memory methods are synchronous.
pub struct EmbeddingProvider {
    config: EmbeddingConfig,
    client: reqwest::blocking::Client,
}

impl EmbeddingProvider {
    /// Create a new embedding provider with the given configuration.
    pub fn new(config: EmbeddingConfig) -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()
            .expect("Failed to create HTTP client");
        
        Self { config, client }
    }
    
    /// Get the configuration.
    pub fn config(&self) -> &EmbeddingConfig {
        &self.config
    }
    
    /// Generate embedding for text.
    ///
    /// Routes to Ollama or OpenAI based on provider config.
    pub fn embed(&self, text: &str) -> Result<Vec<f32>, EmbeddingError> {
        match self.config.provider.as_str() {
            "openai" => self.embed_openai(&[text]).map(|mut v| v.pop().unwrap_or_default()),
            _ => self.embed_ollama(text),
        }
    }
    
    /// Generate embedding using Ollama.
    fn embed_ollama(&self, text: &str) -> Result<Vec<f32>, EmbeddingError> {
        let url = format!("{}/api/embed", self.config.host);
        
        let request = OllamaEmbedRequest {
            model: &self.config.model,
            input: text,
        };
        
        let response = self.client
            .post(&url)
            .json(&request)
            .send()
            .map_err(|e| {
                if e.is_connect() {
                    EmbeddingError::OllamaNotAvailable(self.config.host.clone())
                } else if e.is_timeout() {
                    EmbeddingError::Timeout
                } else {
                    EmbeddingError::Request(e.to_string())
                }
            })?;
        
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            
            if status.as_u16() == 404 || body.contains("not found") {
                return Err(EmbeddingError::ModelNotFound(self.config.model.clone()));
            }
            
            return Err(EmbeddingError::Request(format!(
                "Ollama returned {}: {}",
                status, body
            )));
        }
        
        let embed_response: OllamaEmbedResponse = response.json()
            .map_err(|e| EmbeddingError::Parse(e.to_string()))?;
        
        embed_response
            .embeddings
            .into_iter()
            .next()
            .ok_or(EmbeddingError::EmptyResponse)
    }
    
    /// Generate embeddings using OpenAI API.
    fn embed_openai(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        // Get API key from config or environment
        let api_key = self.config.api_key.clone()
            .or_else(|| std::env::var("OPENAI_API_KEY").ok())
            .ok_or_else(|| EmbeddingError::Request(
                "OpenAI API key not found. Set OPENAI_API_KEY env var or provide api_key in config.".into()
            ))?;
        
        let url = format!("{}/embeddings", self.config.host);
        
        let request = OpenAIEmbedRequest {
            model: &self.config.model,
            input: texts.to_vec(),
        };
        
        let response = self.client
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .map_err(|e| {
                if e.is_connect() {
                    EmbeddingError::Request(format!("Cannot connect to OpenAI: {}", e))
                } else if e.is_timeout() {
                    EmbeddingError::Timeout
                } else {
                    EmbeddingError::Request(e.to_string())
                }
            })?;
        
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            
            if status.as_u16() == 401 {
                return Err(EmbeddingError::Request("Invalid OpenAI API key".into()));
            }
            if status.as_u16() == 404 || body.contains("model_not_found") {
                return Err(EmbeddingError::ModelNotFound(self.config.model.clone()));
            }
            
            return Err(EmbeddingError::Request(format!(
                "OpenAI returned {}: {}",
                status, body
            )));
        }
        
        let embed_response: OpenAIEmbedResponse = response.json()
            .map_err(|e| EmbeddingError::Parse(e.to_string()))?;
        
        if embed_response.data.is_empty() {
            return Err(EmbeddingError::EmptyResponse);
        }
        
        // Sort by index to maintain order
        let mut data = embed_response.data;
        data.sort_by_key(|d| d.index);
        
        Ok(data.into_iter().map(|d| d.embedding).collect())
    }
    
    /// Batch embed multiple texts.
    ///
    /// More efficient than calling embed() multiple times.
    pub fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        match self.config.provider.as_str() {
            "openai" => self.embed_openai(texts),
            _ => self.embed_batch_ollama(texts),
        }
    }
    
    /// Batch embed using Ollama.
    fn embed_batch_ollama(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        let url = format!("{}/api/embed", self.config.host);
        
        let request = OllamaBatchRequest {
            model: &self.config.model,
            input: texts.to_vec(),
        };
        
        let response = self.client
            .post(&url)
            .json(&request)
            .send()
            .map_err(|e| {
                if e.is_connect() {
                    EmbeddingError::OllamaNotAvailable(self.config.host.clone())
                } else if e.is_timeout() {
                    EmbeddingError::Timeout
                } else {
                    EmbeddingError::Request(e.to_string())
                }
            })?;
        
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            
            if status.as_u16() == 404 || body.contains("not found") {
                return Err(EmbeddingError::ModelNotFound(self.config.model.clone()));
            }
            
            return Err(EmbeddingError::Request(format!(
                "Ollama returned {}: {}",
                status, body
            )));
        }
        
        let embed_response: OllamaEmbedResponse = response.json()
            .map_err(|e| EmbeddingError::Parse(e.to_string()))?;
        
        Ok(embed_response.embeddings)
    }
    
    /// Check if the embedding provider is available.
    ///
    /// For Ollama: checks if the server is running.
    /// For OpenAI: checks if the API key is available and valid.
    pub fn is_available(&self) -> bool {
        match self.config.provider.as_str() {
            "openai" => self.is_openai_available(),
            _ => self.is_ollama_available(),
        }
    }
    
    /// Check if Ollama is running.
    fn is_ollama_available(&self) -> bool {
        let url = format!("{}/api/tags", self.config.host);
        
        match self.client.get(&url).send() {
            Ok(response) => response.status().is_success(),
            Err(_) => false,
        }
    }
    
    /// Check if OpenAI API is available.
    fn is_openai_available(&self) -> bool {
        // Check if API key exists
        let api_key = self.config.api_key.clone()
            .or_else(|| std::env::var("OPENAI_API_KEY").ok());
        
        if api_key.is_none() {
            return false;
        }
        
        // Optionally do a lightweight API check (list models)
        // For now, just check if key exists
        true
    }
    
    /// Check if the configured model is available.
    pub fn is_model_available(&self) -> Result<bool, EmbeddingError> {
        let url = format!("{}/api/show", self.config.host);
        
        #[derive(Serialize)]
        struct ShowRequest<'a> {
            name: &'a str,
        }
        
        let response = self.client
            .post(&url)
            .json(&ShowRequest { name: &self.config.model })
            .send()
            .map_err(|e| {
                if e.is_connect() {
                    EmbeddingError::OllamaNotAvailable(self.config.host.clone())
                } else {
                    EmbeddingError::Request(e.to_string())
                }
            })?;
        
        Ok(response.status().is_success())
    }
    
    /// Get embedding dimensions from Ollama model info.
    pub fn get_dimensions(&self) -> Result<usize, EmbeddingError> {
        let url = format!("{}/api/show", self.config.host);
        
        #[derive(Serialize)]
        struct ShowRequest<'a> {
            name: &'a str,
        }
        
        #[derive(Deserialize)]
        struct ShowResponse {
            details: Option<ModelDetails>,
        }
        
        #[derive(Deserialize)]
        struct ModelDetails {
            embedding_length: Option<usize>,
        }
        
        let response = self.client
            .post(&url)
            .json(&ShowRequest { name: &self.config.model })
            .send()
            .map_err(|e| {
                if e.is_connect() {
                    EmbeddingError::OllamaNotAvailable(self.config.host.clone())
                } else {
                    EmbeddingError::Request(e.to_string())
                }
            })?;
        
        if !response.status().is_success() {
            return Err(EmbeddingError::ModelNotFound(self.config.model.clone()));
        }
        
        let show_response: ShowResponse = response.json()
            .map_err(|e| EmbeddingError::Parse(e.to_string()))?;
        
        Ok(show_response
            .details
            .and_then(|d| d.embedding_length)
            .unwrap_or(self.config.dimensions))
    }
    
    /// Compute cosine similarity between two vectors.
    ///
    /// Returns a value between -1.0 and 1.0, where 1.0 means identical.
    pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
        if a.len() != b.len() || a.is_empty() {
            return 0.0;
        }
        
        let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        
        if norm_a == 0.0 || norm_b == 0.0 {
            return 0.0;
        }
        
        dot / (norm_a * norm_b)
    }
}

/// Errors that can occur during embedding operations.
#[derive(Debug, thiserror::Error)]
pub enum EmbeddingError {
    #[error("Ollama not available at {0}")]
    OllamaNotAvailable(String),
    
    #[error("Model '{0}' not found in Ollama")]
    ModelNotFound(String),
    
    #[error("Request failed: {0}")]
    Request(String),
    
    #[error("Failed to parse response: {0}")]
    Parse(String),
    
    #[error("Empty embedding response")]
    EmptyResponse,
    
    #[error("Request timed out")]
    Timeout,
    
    #[error("Storage error: {0}")]
    Storage(String),
}

impl From<rusqlite::Error> for EmbeddingError {
    fn from(e: rusqlite::Error) -> Self {
        EmbeddingError::Storage(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_cosine_similarity_identical() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![1.0, 2.0, 3.0];
        let sim = EmbeddingProvider::cosine_similarity(&a, &b);
        assert!((sim - 1.0).abs() < 1e-6);
    }
    
    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        let sim = EmbeddingProvider::cosine_similarity(&a, &b);
        assert!(sim.abs() < 1e-6);
    }
    
    #[test]
    fn test_cosine_similarity_opposite() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![-1.0, -2.0, -3.0];
        let sim = EmbeddingProvider::cosine_similarity(&a, &b);
        assert!((sim + 1.0).abs() < 1e-6);
    }
    
    #[test]
    fn test_cosine_similarity_empty() {
        let a: Vec<f32> = vec![];
        let b: Vec<f32> = vec![];
        let sim = EmbeddingProvider::cosine_similarity(&a, &b);
        assert_eq!(sim, 0.0);
    }
    
    #[test]
    fn test_cosine_similarity_different_lengths() {
        let a = vec![1.0, 2.0];
        let b = vec![1.0, 2.0, 3.0];
        let sim = EmbeddingProvider::cosine_similarity(&a, &b);
        assert_eq!(sim, 0.0);
    }
    
    #[test]
    fn test_config_default() {
        let config = EmbeddingConfig::default();
        assert_eq!(config.provider, "ollama");
        assert_eq!(config.model, "nomic-embed-text");
        assert_eq!(config.host, "http://localhost:11434");
        assert_eq!(config.dimensions, 768);
    }
    
    #[test]
    #[ignore] // Requires Ollama running
    fn test_embed_real() {
        let provider = EmbeddingProvider::new(EmbeddingConfig::default());
        
        if !provider.is_available() {
            println!("Ollama not available, skipping test");
            return;
        }
        
        let result = provider.embed("Hello, world!");
        assert!(result.is_ok());
        
        let embedding = result.unwrap();
        assert!(!embedding.is_empty());
        println!("Embedding dimensions: {}", embedding.len());
    }
    
    #[test]
    #[ignore] // Requires Ollama running
    fn test_semantic_similarity() {
        let provider = EmbeddingProvider::new(EmbeddingConfig::default());
        
        if !provider.is_available() {
            println!("Ollama not available, skipping test");
            return;
        }
        
        let dog = provider.embed("A dog is playing in the park").unwrap();
        let puppy = provider.embed("A puppy is running in the garden").unwrap();
        let car = provider.embed("The car is parked in the garage").unwrap();
        
        let sim_dog_puppy = EmbeddingProvider::cosine_similarity(&dog, &puppy);
        let sim_dog_car = EmbeddingProvider::cosine_similarity(&dog, &car);
        
        println!("dog-puppy similarity: {:.4}", sim_dog_puppy);
        println!("dog-car similarity: {:.4}", sim_dog_car);
        
        // Dog and puppy should be more similar than dog and car
        assert!(sim_dog_puppy > sim_dog_car);
    }
}
