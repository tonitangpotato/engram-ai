//! Capability detection and graceful degradation for the Knowledge Compiler.
//!
//! Detects available capabilities (LLM, embeddings) and provides fallback
//! chains with actionable error messages when features are unavailable.

use std::fmt;

// ═══════════════════════════════════════════════════════════════════════════════
//  DEGRADATION LEVEL
// ═══════════════════════════════════════════════════════════════════════════════

/// What level of functionality is available.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DegradationLevel {
    /// No LLM, no embeddings — basic store/recall only
    Minimal,
    /// Embeddings available, no LLM — semantic search + clustering works
    Embeddings,
    /// Full — LLM + embeddings — all features
    Full,
}

impl fmt::Display for DegradationLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DegradationLevel::Minimal => write!(f, "Minimal"),
            DegradationLevel::Embeddings => write!(f, "Embeddings"),
            DegradationLevel::Full => write!(f, "Full"),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  KC FEATURE
// ═══════════════════════════════════════════════════════════════════════════════

/// Features that require different capability levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KcFeature {
    /// Basic text store/recall
    BasicRecall,
    /// Semantic similarity search
    SemanticSearch,
    /// Topic discovery via clustering
    TopicDiscovery,
    /// Topic compilation (basic, template-based)
    BasicCompilation,
    /// LLM-enhanced topic compilation
    EnhancedCompilation,
    /// LLM conflict analysis
    ConflictAnalysis,
    /// Smart topic naming via LLM
    SmartNaming,
    /// Import/export
    ImportExport,
    /// Decay evaluation
    DecayEvaluation,
}

impl KcFeature {
    /// The minimum degradation level required for this feature.
    fn required_level(self) -> DegradationLevel {
        match self {
            KcFeature::BasicRecall => DegradationLevel::Minimal,
            KcFeature::ImportExport => DegradationLevel::Minimal,
            KcFeature::DecayEvaluation => DegradationLevel::Minimal,
            KcFeature::SemanticSearch => DegradationLevel::Embeddings,
            KcFeature::TopicDiscovery => DegradationLevel::Embeddings,
            KcFeature::BasicCompilation => DegradationLevel::Embeddings,
            KcFeature::EnhancedCompilation => DegradationLevel::Full,
            KcFeature::ConflictAnalysis => DegradationLevel::Full,
            KcFeature::SmartNaming => DegradationLevel::Full,
        }
    }

    /// Human-readable name for this feature.
    fn display_name(self) -> &'static str {
        match self {
            KcFeature::BasicRecall => "Basic Recall",
            KcFeature::SemanticSearch => "Semantic Search",
            KcFeature::TopicDiscovery => "Topic Discovery",
            KcFeature::BasicCompilation => "Basic Compilation",
            KcFeature::EnhancedCompilation => "Enhanced Compilation",
            KcFeature::ConflictAnalysis => "Conflict Analysis",
            KcFeature::SmartNaming => "Smart Naming",
            KcFeature::ImportExport => "Import/Export",
            KcFeature::DecayEvaluation => "Decay Evaluation",
        }
    }

    /// Description of the fallback behaviour when this feature is unavailable.
    fn fallback_hint(self) -> Option<&'static str> {
        match self {
            KcFeature::EnhancedCompilation => {
                Some("Falling back to template-based BasicCompilation.")
            }
            KcFeature::SmartNaming => {
                Some("Falling back to entity-frequency heuristic naming.")
            }
            KcFeature::ConflictAnalysis => {
                Some("Falling back to embedding-similarity-only conflict detection (no LLM contradiction check).")
            }
            KcFeature::SemanticSearch => {
                Some("Falling back to text-match search.")
            }
            _ => None,
        }
    }
}

impl fmt::Display for KcFeature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  GRACEFUL DEGRADATION
// ═══════════════════════════════════════════════════════════════════════════════

/// Capability detection and degradation management.
///
/// Detects what backends (LLM, embeddings) are available and exposes
/// per-feature availability queries with actionable upgrade instructions.
pub struct GracefulDegradation {
    level: DegradationLevel,
}

impl GracefulDegradation {
    /// Detect capability level from configuration.
    ///
    /// - Both LLM and embeddings → [`DegradationLevel::Full`]
    /// - Embeddings only → [`DegradationLevel::Embeddings`]
    /// - Neither → [`DegradationLevel::Minimal`]
    pub fn detect(llm_available: bool, embeddings_available: bool) -> Self {
        let level = match (llm_available, embeddings_available) {
            (true, true) => DegradationLevel::Full,
            (_, true) => DegradationLevel::Embeddings,
            // LLM without embeddings is treated as Minimal since embedding-dependent
            // features (which are prerequisites for LLM features) won't work.
            _ => DegradationLevel::Minimal,
        };
        Self { level }
    }

    /// Get the current capability level.
    pub fn level(&self) -> DegradationLevel {
        self.level
    }

    /// Check if a specific feature is available at the current level.
    pub fn is_available(&self, feature: KcFeature) -> bool {
        self.level >= feature.required_level()
    }

    /// Get a user-friendly message about why a feature is unavailable.
    ///
    /// Returns `None` when the feature **is** available at the current level.
    pub fn unavailability_message(&self, feature: KcFeature) -> Option<String> {
        if self.is_available(feature) {
            return None;
        }

        let required = feature.required_level();
        let mut msg = match self.level {
            DegradationLevel::Minimal => {
                match required {
                    DegradationLevel::Embeddings => format!(
                        "⚠️ {} requires embeddings. Run `engram setup` to download a model (80MB, one-time).",
                        feature.display_name()
                    ),
                    DegradationLevel::Full => format!(
                        "⚠️ {} requires embeddings and an LLM. Run `engram setup` to download an embedding model (80MB, one-time), then set llm.provider in config.",
                        feature.display_name()
                    ),
                    // Minimal features are always available when level is Minimal
                    DegradationLevel::Minimal => unreachable!(),
                }
            }
            DegradationLevel::Embeddings => {
                // Only Full-level features can be unavailable here
                format!(
                    "ℹ️ {} works but without LLM enhancement. Set llm.provider in config for full features.",
                    feature.display_name()
                )
            }
            DegradationLevel::Full => {
                // Everything is available at Full — unreachable since we return None above
                unreachable!()
            }
        };

        // Append fallback hint if one exists
        if let Some(hint) = feature.fallback_hint() {
            msg.push(' ');
            msg.push_str(hint);
        }

        Some(msg)
    }

    /// Get setup instructions to reach a higher capability level.
    ///
    /// Returns `None` when already at [`DegradationLevel::Full`].
    pub fn upgrade_instructions(&self) -> Option<String> {
        match self.level {
            DegradationLevel::Minimal => Some(
                "To enable semantic features:\n\
                 1. Run `engram setup` to download an embedding model (~80MB, one-time).\n\
                 2. (Optional) Set `llm.provider` in your engram config to enable LLM-enhanced \
                    compilation, conflict analysis, and smart naming."
                    .to_string(),
            ),
            DegradationLevel::Embeddings => Some(
                "To enable LLM-enhanced features:\n\
                 • Set `llm.provider` in your engram config (e.g. \"openai\", \"anthropic\", or \"local\").\n\
                 • Ensure the corresponding API key environment variable is set, or start a local Ollama server."
                    .to_string(),
            ),
            DegradationLevel::Full => None,
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
    fn test_detect_full() {
        let gd = GracefulDegradation::detect(true, true);
        assert_eq!(gd.level(), DegradationLevel::Full);
    }

    #[test]
    fn test_detect_embeddings_only() {
        let gd = GracefulDegradation::detect(false, true);
        assert_eq!(gd.level(), DegradationLevel::Embeddings);
    }

    #[test]
    fn test_detect_minimal() {
        let gd = GracefulDegradation::detect(false, false);
        assert_eq!(gd.level(), DegradationLevel::Minimal);

        // LLM without embeddings is also Minimal
        let gd2 = GracefulDegradation::detect(true, false);
        assert_eq!(gd2.level(), DegradationLevel::Minimal);
    }

    #[test]
    fn test_feature_availability_full() {
        let gd = GracefulDegradation::detect(true, true);

        let all_features = [
            KcFeature::BasicRecall,
            KcFeature::SemanticSearch,
            KcFeature::TopicDiscovery,
            KcFeature::BasicCompilation,
            KcFeature::EnhancedCompilation,
            KcFeature::ConflictAnalysis,
            KcFeature::SmartNaming,
            KcFeature::ImportExport,
            KcFeature::DecayEvaluation,
        ];

        for feature in &all_features {
            assert!(
                gd.is_available(*feature),
                "{:?} should be available at Full level",
                feature
            );
        }
    }

    #[test]
    fn test_feature_availability_embeddings() {
        let gd = GracefulDegradation::detect(false, true);

        // Available at Embeddings level
        assert!(gd.is_available(KcFeature::BasicRecall));
        assert!(gd.is_available(KcFeature::SemanticSearch));
        assert!(gd.is_available(KcFeature::TopicDiscovery));
        assert!(gd.is_available(KcFeature::BasicCompilation));
        assert!(gd.is_available(KcFeature::ImportExport));
        assert!(gd.is_available(KcFeature::DecayEvaluation));

        // NOT available at Embeddings level (require Full)
        assert!(!gd.is_available(KcFeature::EnhancedCompilation));
        assert!(!gd.is_available(KcFeature::ConflictAnalysis));
        assert!(!gd.is_available(KcFeature::SmartNaming));
    }

    #[test]
    fn test_feature_availability_minimal() {
        let gd = GracefulDegradation::detect(false, false);

        // Available at Minimal level
        assert!(gd.is_available(KcFeature::BasicRecall));
        assert!(gd.is_available(KcFeature::ImportExport));
        assert!(gd.is_available(KcFeature::DecayEvaluation));

        // NOT available at Minimal level
        assert!(!gd.is_available(KcFeature::SemanticSearch));
        assert!(!gd.is_available(KcFeature::TopicDiscovery));
        assert!(!gd.is_available(KcFeature::BasicCompilation));
        assert!(!gd.is_available(KcFeature::EnhancedCompilation));
        assert!(!gd.is_available(KcFeature::ConflictAnalysis));
        assert!(!gd.is_available(KcFeature::SmartNaming));
    }

    #[test]
    fn test_unavailability_messages() {
        let gd_minimal = GracefulDegradation::detect(false, false);
        let gd_embeddings = GracefulDegradation::detect(false, true);
        let gd_full = GracefulDegradation::detect(true, true);

        // Available features return None
        assert!(gd_full
            .unavailability_message(KcFeature::EnhancedCompilation)
            .is_none());
        assert!(gd_minimal
            .unavailability_message(KcFeature::BasicRecall)
            .is_none());

        // Unavailable features at Minimal return non-empty messages
        let msg = gd_minimal
            .unavailability_message(KcFeature::SemanticSearch)
            .expect("should have a message");
        assert!(!msg.is_empty());
        assert!(msg.contains("⚠️"));
        assert!(msg.contains("Semantic Search"));

        // Unavailable features at Minimal that need Full
        let msg = gd_minimal
            .unavailability_message(KcFeature::EnhancedCompilation)
            .expect("should have a message");
        assert!(!msg.is_empty());
        assert!(msg.contains("⚠️"));

        // Unavailable features at Embeddings return non-empty messages
        let msg = gd_embeddings
            .unavailability_message(KcFeature::EnhancedCompilation)
            .expect("should have a message");
        assert!(!msg.is_empty());
        assert!(msg.contains("ℹ️"));
        assert!(msg.contains("Enhanced Compilation"));

        // Fallback hints are included
        let msg = gd_embeddings
            .unavailability_message(KcFeature::SmartNaming)
            .expect("should have a message");
        assert!(msg.contains("entity-frequency heuristic"));

        let msg = gd_minimal
            .unavailability_message(KcFeature::SemanticSearch)
            .expect("should have a message");
        assert!(msg.contains("text-match search"));

        let msg = gd_embeddings
            .unavailability_message(KcFeature::ConflictAnalysis)
            .expect("should have a message");
        assert!(msg.contains("embedding-similarity-only"));
    }

    #[test]
    fn test_upgrade_instructions() {
        let gd_minimal = GracefulDegradation::detect(false, false);
        let gd_embeddings = GracefulDegradation::detect(false, true);
        let gd_full = GracefulDegradation::detect(true, true);

        // Minimal → provides instructions mentioning setup and llm
        let instructions = gd_minimal
            .upgrade_instructions()
            .expect("should have instructions");
        assert!(!instructions.is_empty());
        assert!(instructions.contains("engram setup"));
        assert!(instructions.contains("llm.provider"));

        // Embeddings → provides instructions mentioning llm config
        let instructions = gd_embeddings
            .upgrade_instructions()
            .expect("should have instructions");
        assert!(!instructions.is_empty());
        assert!(instructions.contains("llm.provider"));

        // Full → no upgrade needed
        assert!(gd_full.upgrade_instructions().is_none());
    }
}
