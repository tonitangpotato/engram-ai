//! KC configuration management with zero-config defaults and 4-layer resolution.
//!
//! Resolution order (highest priority first):
//! 1. CLI flags — applied by caller after `load()`
//! 2. Environment variables (`ENGRAM_*`)
//! 3. Project-local `engram.toml`
//! 4. User config (`~/.config/engram/config.toml`)
//! 5. Compiled-in defaults

use std::path::{Path, PathBuf};

use super::types::*;

/// Current config file version for future migration support.
pub const CURRENT_CONFIG_VERSION: u32 = 1;

// ─── Default Implementations ─────────────────────────────────────────────────

impl Default for KcConfig {
    fn default() -> Self {
        Self {
            min_cluster_size: 3,
            quality_threshold: 0.6,
            recompile_strategy: RecompileStrategy::Eager,
            decay: DecayConfig::default(),
            llm: LlmConfig::default(),
            import: ImportConfig::default(),
            intake: IntakeConfig::default(),
            lifecycle: LifecycleConfig::default(),
        }
    }
}

impl Default for DecayConfig {
    fn default() -> Self {
        Self {
            check_interval_hours: 24,
            stale_threshold_days: 30,
            archive_threshold_days: 90,
            min_access_count: 1,
        }
    }
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: String::new(), // empty = LLM disabled, graceful degradation
            model: String::from("gpt-4o-mini"),
            api_key_env: String::from("OPENAI_API_KEY"),
            max_retries: 3,
            timeout_secs: 30,
            temperature: 0.7,
        }
    }
}

impl Default for KcEmbeddingConfig {
    fn default() -> Self {
        Self {
            provider: String::from("local"),
            model: String::from("all-MiniLM-L6-v2"),
            dimensions: 384,
            batch_size: 32,
        }
    }
}

impl Default for ImportConfig {
    fn default() -> Self {
        Self {
            default_policy: ImportPolicy::Skip,
            split_strategy: SplitStrategy::Smart,
            duplicate_strategy: DuplicateStrategy::Skip,
            max_document_size_bytes: 10 * 1024 * 1024, // 10MB
        }
    }
}

impl Default for IntakeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_compile: false,
            buffer_size: 100,
            deduplicate: true,
        }
    }
}

impl Default for LifecycleConfig {
    fn default() -> Self {
        Self {
            merge_overlap_threshold: 0.6,
            max_topic_points: 15,
            link_min_strength: 0.3,
        }
    }
}

// ─── Config Loading (4-layer resolution) ─────────────────────────────────────

impl KcConfig {
    /// Load config with 4-layer resolution:
    /// CLI flags > env vars > project-local `engram.toml` > user config > defaults
    ///
    /// CLI flags (layer 1) are *not* applied here — the caller merges them
    /// after `load()` returns.
    pub fn load() -> Self {
        let mut config = Self::default();

        // Layer 4: user config (~/.config/engram/config.toml)
        if let Some(path) = Self::user_config_path() {
            if path.exists() {
                let _ = config.merge_from_file(&path);
            }
        }

        // Layer 3: project-local engram.toml
        let local = Path::new("engram.toml");
        if local.exists() {
            let _ = config.merge_from_file(local);
        }

        // Layer 2: environment variables
        config.merge_from_env();

        // Layer 1: CLI flags — applied by caller
        config
    }

    /// User config directory path.
    fn user_config_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("engram").join("config.toml"))
    }

    /// Merge from a TOML file. Only overrides fields that are present in the file.
    pub fn merge_from_file(&mut self, path: &Path) -> Result<(), KcError> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            KcError::InvalidConfig(format!("Cannot read {}: {}", path.display(), e))
        })?;
        self.merge_from_toml(&content)
    }

    /// Merge from TOML string content.
    ///
    /// Uses a lightweight `key = value` parser — sufficient for flat and
    /// single-level dotted keys. Full TOML table support will arrive when the
    /// `toml` crate is added as a dependency.
    pub fn merge_from_toml(&mut self, content: &str) -> Result<(), KcError> {
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                let key = key.trim();
                let value = value.trim().trim_matches('"');
                self.set_field(key, value);
            }
        }
        Ok(())
    }

    /// Merge from environment variables.
    pub fn merge_from_env(&mut self) {
        if let Ok(v) = std::env::var("ENGRAM_LLM_PROVIDER") {
            self.llm.provider = v;
        }
        if let Ok(v) = std::env::var("ENGRAM_LLM_MODEL") {
            self.llm.model = v;
        }
        if let Ok(v) = std::env::var("ENGRAM_LLM_API_KEY_ENV") {
            self.llm.api_key_env = v;
        }
        if let Ok(v) = std::env::var("ENGRAM_MIN_CLUSTER_SIZE") {
            if let Ok(n) = v.parse() {
                self.min_cluster_size = n;
            }
        }
        if let Ok(v) = std::env::var("ENGRAM_QUALITY_THRESHOLD") {
            if let Ok(n) = v.parse() {
                self.quality_threshold = n;
            }
        }
        if let Ok(v) = std::env::var("ENGRAM_LLM_TEMPERATURE") {
            if let Ok(n) = v.parse() {
                self.llm.temperature = n;
            }
        }
    }

    /// Set a config field by dotted key path.
    fn set_field(&mut self, key: &str, value: &str) {
        match key {
            "min_cluster_size" => {
                if let Ok(v) = value.parse() {
                    self.min_cluster_size = v;
                }
            }
            "quality_threshold" => {
                if let Ok(v) = value.parse() {
                    self.quality_threshold = v;
                }
            }
            "llm.provider" | "provider" => {
                self.llm.provider = value.to_string();
            }
            "llm.model" | "model" => {
                self.llm.model = value.to_string();
            }
            "llm.api_key_env" => {
                self.llm.api_key_env = value.to_string();
            }
            "llm.max_retries" => {
                if let Ok(v) = value.parse() {
                    self.llm.max_retries = v;
                }
            }
            "llm.timeout_secs" => {
                if let Ok(v) = value.parse() {
                    self.llm.timeout_secs = v;
                }
            }
            "llm.temperature" => {
                if let Ok(v) = value.parse() {
                    self.llm.temperature = v;
                }
            }
            "decay.check_interval_hours" => {
                if let Ok(v) = value.parse() {
                    self.decay.check_interval_hours = v;
                }
            }
            "decay.stale_threshold_days" => {
                if let Ok(v) = value.parse() {
                    self.decay.stale_threshold_days = v;
                }
            }
            "decay.archive_threshold_days" => {
                if let Ok(v) = value.parse() {
                    self.decay.archive_threshold_days = v;
                }
            }
            "decay.min_access_count" => {
                if let Ok(v) = value.parse() {
                    self.decay.min_access_count = v;
                }
            }
            "import.max_document_size_bytes" => {
                if let Ok(v) = value.parse() {
                    self.import.max_document_size_bytes = v;
                }
            }
            "intake.enabled" => {
                self.intake.enabled = value == "true";
            }
            "intake.auto_compile" => {
                self.intake.auto_compile = value == "true";
            }
            "intake.buffer_size" => {
                if let Ok(v) = value.parse() {
                    self.intake.buffer_size = v;
                }
            }
            "intake.deduplicate" => {
                self.intake.deduplicate = value == "true";
            }
            _ => {} // Unknown keys silently ignored
        }
    }

    /// Check if LLM is configured and available.
    pub fn has_llm(&self) -> bool {
        !self.llm.provider.is_empty() && self.llm.provider != "none"
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let cfg = KcConfig::default();
        assert_eq!(cfg.min_cluster_size, 3);
        assert!((cfg.quality_threshold - 0.6).abs() < f64::EPSILON);
        assert!(matches!(cfg.recompile_strategy, RecompileStrategy::Eager));
        assert_eq!(cfg.decay.stale_threshold_days, 30);
        assert_eq!(cfg.decay.archive_threshold_days, 90);
        assert!(cfg.llm.provider.is_empty());
        assert_eq!(cfg.llm.model, "gpt-4o-mini");
        assert_eq!(cfg.import.max_document_size_bytes, 10 * 1024 * 1024);
        assert!(cfg.intake.enabled);
        assert!(!cfg.intake.auto_compile);
    }

    #[test]
    fn test_merge_from_env() {
        // Save any existing values to avoid parallel-test env pollution
        let saved: Vec<(&str, Option<String>)> = vec![
            "ENGRAM_LLM_PROVIDER",
            "ENGRAM_MIN_CLUSTER_SIZE",
            "ENGRAM_QUALITY_THRESHOLD",
            "ENGRAM_LLM_TEMPERATURE",
        ]
        .into_iter()
        .map(|k| (k, std::env::var(k).ok()))
        .collect();

        // Set env vars for this test
        std::env::set_var("ENGRAM_LLM_PROVIDER", "openai");
        std::env::set_var("ENGRAM_MIN_CLUSTER_SIZE", "5");
        std::env::set_var("ENGRAM_QUALITY_THRESHOLD", "0.8");
        std::env::set_var("ENGRAM_LLM_TEMPERATURE", "0.3");

        let mut cfg = KcConfig::default();
        cfg.merge_from_env();

        // Restore original state before assertions (so cleanup runs even if assert fails)
        for (k, v) in &saved {
            match v {
                Some(val) => std::env::set_var(k, val),
                None => std::env::remove_var(k),
            }
        }

        assert_eq!(cfg.llm.provider, "openai");
        assert_eq!(cfg.min_cluster_size, 5);
        assert!((cfg.quality_threshold - 0.8).abs() < f64::EPSILON);
        assert!((cfg.llm.temperature - 0.3).abs() < f32::EPSILON);
    }

    #[test]
    fn test_merge_from_toml() {
        let toml = r#"
# KC configuration
min_cluster_size = 7
quality_threshold = 0.9

[llm]
llm.provider = "anthropic"
llm.model = "claude-3-haiku"
llm.temperature = 0.5

[intake]
intake.enabled = false
intake.buffer_size = 200
"#;

        let mut cfg = KcConfig::default();
        cfg.merge_from_toml(toml).unwrap();

        assert_eq!(cfg.min_cluster_size, 7);
        assert!((cfg.quality_threshold - 0.9).abs() < f64::EPSILON);
        assert_eq!(cfg.llm.provider, "anthropic");
        assert_eq!(cfg.llm.model, "claude-3-haiku");
        assert!((cfg.llm.temperature - 0.5).abs() < f32::EPSILON);
        assert!(!cfg.intake.enabled);
        assert_eq!(cfg.intake.buffer_size, 200);
    }

    #[test]
    fn test_has_llm() {
        let mut cfg = KcConfig::default();

        // Default: empty provider → no LLM
        assert!(!cfg.has_llm());

        // Explicit "none" → no LLM
        cfg.llm.provider = "none".to_string();
        assert!(!cfg.has_llm());

        // Valid provider → LLM available
        cfg.llm.provider = "openai".to_string();
        assert!(cfg.has_llm());
    }

    #[test]
    fn test_merge_toml_empty_and_comments() {
        let toml = "# only comments\n\n# nothing here\n";
        let mut cfg = KcConfig::default();
        cfg.merge_from_toml(toml).unwrap();
        // Should keep all defaults unchanged
        assert_eq!(cfg.min_cluster_size, 3);
        assert!(cfg.llm.provider.is_empty());
    }

    #[test]
    fn test_merge_toml_unknown_keys_ignored() {
        let toml = "unknown_key = 42\nmin_cluster_size = 10\n";
        let mut cfg = KcConfig::default();
        cfg.merge_from_toml(toml).unwrap();
        assert_eq!(cfg.min_cluster_size, 10);
    }

    #[test]
    fn test_merge_toml_decay_fields() {
        let toml = r#"
decay.check_interval_hours = 12
decay.stale_threshold_days = 60
decay.archive_threshold_days = 180
decay.min_access_count = 5
"#;
        let mut cfg = KcConfig::default();
        cfg.merge_from_toml(toml).unwrap();
        assert_eq!(cfg.decay.check_interval_hours, 12);
        assert_eq!(cfg.decay.stale_threshold_days, 60);
        assert_eq!(cfg.decay.archive_threshold_days, 180);
        assert_eq!(cfg.decay.min_access_count, 5);
    }

    #[test]
    fn test_merge_toml_import_and_intake() {
        let toml = r#"
import.max_document_size_bytes = 5242880
intake.enabled = false
intake.auto_compile = true
intake.buffer_size = 50
intake.deduplicate = false
"#;
        let mut cfg = KcConfig::default();
        cfg.merge_from_toml(toml).unwrap();
        assert_eq!(cfg.import.max_document_size_bytes, 5_242_880);
        assert!(!cfg.intake.enabled);
        assert!(cfg.intake.auto_compile);
        assert_eq!(cfg.intake.buffer_size, 50);
        assert!(!cfg.intake.deduplicate);
    }

    #[test]
    fn test_merge_toml_invalid_number_keeps_default() {
        let toml = "min_cluster_size = not_a_number\n";
        let mut cfg = KcConfig::default();
        cfg.merge_from_toml(toml).unwrap();
        // Invalid parse → field unchanged
        assert_eq!(cfg.min_cluster_size, 3);
    }

    #[test]
    fn test_lifecycle_defaults() {
        let cfg = KcConfig::default();
        assert!((cfg.lifecycle.merge_overlap_threshold - 0.6).abs() < f64::EPSILON);
        assert_eq!(cfg.lifecycle.max_topic_points, 15);
        assert!((cfg.lifecycle.link_min_strength - 0.3).abs() < f64::EPSILON);
    }

    #[test]
    fn test_merge_from_env_invalid_number() {
        let saved = std::env::var("ENGRAM_MIN_CLUSTER_SIZE").ok();
        std::env::set_var("ENGRAM_MIN_CLUSTER_SIZE", "abc");
        let mut cfg = KcConfig::default();
        cfg.merge_from_env();
        // Restore before assertions
        match &saved {
            Some(val) => std::env::set_var("ENGRAM_MIN_CLUSTER_SIZE", val),
            None => std::env::remove_var("ENGRAM_MIN_CLUSTER_SIZE"),
        }
        // Invalid → keeps default
        assert_eq!(cfg.min_cluster_size, 3);
    }
}
