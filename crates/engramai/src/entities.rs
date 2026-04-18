//! Entity extraction for memory content.
//!
//! Identifies known projects, people, technologies, and patterns like
//! issue IDs, file paths, URLs, and @-mentions using Aho-Corasick
//! automaton for known entities and regex patterns for structural matches.

use aho_corasick::AhoCorasick;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Classification of an extracted entity.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntityType {
    Project,
    Person,
    Technology,
    Concept,
    File,
    Url,
    Organization,
    Other(String),
}

impl EntityType {
    /// Returns the entity type as a string slice.
    pub fn as_str(&self) -> &str {
        match self {
            EntityType::Project => "project",
            EntityType::Person => "person",
            EntityType::Technology => "technology",
            EntityType::Concept => "concept",
            EntityType::File => "file",
            EntityType::Url => "url",
            EntityType::Organization => "organization",
            EntityType::Other(s) => s.as_str(),
        }
    }
}

/// An entity extracted from memory content.
#[derive(Clone, Debug)]
pub struct ExtractedEntity {
    /// Original matched name
    pub name: String,
    /// Normalized form (lowercase, stripped prefixes, etc.)
    pub normalized: String,
    /// Classification of the entity
    pub entity_type: EntityType,
}

/// Configuration for entity extraction.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EntityConfig {
    /// Known project names to match
    pub known_projects: Vec<String>,
    /// Known people names to match
    pub known_people: Vec<String>,
    /// Known technology names to match
    pub known_technologies: Vec<String>,
    /// Whether entity extraction is enabled
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Weight for entity matches in recall scoring
    #[serde(default = "default_recall_weight")]
    pub recall_weight: f64,
}

fn default_enabled() -> bool {
    true
}

fn default_recall_weight() -> f64 {
    0.15
}

impl Default for EntityConfig {
    fn default() -> Self {
        Self {
            known_projects: Vec::new(),
            known_people: Vec::new(),
            known_technologies: Vec::new(),
            enabled: true,
            recall_weight: 0.15,
        }
    }
}

/// A compiled regex pattern with its associated entity type.
struct EntityPattern {
    regex: Regex,
    entity_type: EntityType,
    name_group: usize,
}

/// Extracts entities from text using Aho-Corasick for known entities
/// and regex patterns for structural matches (issue IDs, file paths, URLs, etc.).
pub struct EntityExtractor {
    patterns: Vec<EntityPattern>,
    known_matcher: AhoCorasick,
    known_index: Vec<(EntityType, String)>,
}

impl EntityExtractor {
    /// Build a new extractor from the given configuration.
    ///
    /// Compiles an Aho-Corasick automaton for known entities and
    /// regex patterns for structural matches.
    pub fn new(config: &EntityConfig) -> Self {
        // Build known entity list for Aho-Corasick
        let mut known_index: Vec<(EntityType, String)> = Vec::new();
        let mut ac_patterns: Vec<String> = Vec::new();

        for name in &config.known_projects {
            ac_patterns.push(name.clone());
            known_index.push((EntityType::Project, name.clone()));
        }
        for name in &config.known_people {
            ac_patterns.push(name.clone());
            known_index.push((EntityType::Person, name.clone()));
        }
        for name in &config.known_technologies {
            ac_patterns.push(name.clone());
            known_index.push((EntityType::Technology, name.clone()));
        }
        
        // Built-in common technology names (always matched unless already in user config)
        let builtin_technologies = [
            "Rust", "Python", "TypeScript", "JavaScript", "Go", "Java", "C++",
            "SQLite", "PostgreSQL", "Redis", "MongoDB",
            "Supabase", "Docker", "Kubernetes", "Terraform",
            "React", "Next.js", "Svelte", "Vue",
            "Tokio", "Actix", "Axum", "Warp",
            "PyTorch", "TensorFlow", "ONNX",
            "WebSocket", "gRPC", "GraphQL", "REST",
            "OAuth", "JWT", "WASM",
        ];
        let existing_lower: HashSet<String> = config.known_technologies
            .iter()
            .map(|s| s.to_lowercase())
            .collect();
        for name in &builtin_technologies {
            if !existing_lower.contains(&name.to_lowercase()) {
                ac_patterns.push(name.to_string());
                known_index.push((EntityType::Technology, name.to_string()));
            }
        }

        // Build Aho-Corasick automaton (case-insensitive, leftmost-longest matching)
        let known_matcher = AhoCorasick::builder()
            .ascii_case_insensitive(true)
            .match_kind(aho_corasick::MatchKind::LeftmostLongest)
            .build(&ac_patterns)
            .expect("failed to build Aho-Corasick automaton");

        // Build regex patterns
        let patterns = vec![
            EntityPattern {
                regex: Regex::new(r"(?i)(ISS-\d+|GOAL-\d+|GUARD-\d+)")
                    .expect("invalid concept regex"),
                entity_type: EntityType::Concept,
                name_group: 1,
            },
            EntityPattern {
                regex: Regex::new(r"(src/\S+\.rs|\S+\.(rs|py|ts|md))")
                    .expect("invalid file regex"),
                entity_type: EntityType::File,
                name_group: 1,
            },
            EntityPattern {
                regex: Regex::new(r"(https?://\S+)")
                    .expect("invalid url regex"),
                entity_type: EntityType::Url,
                name_group: 1,
            },
            EntityPattern {
                // @mentions: require 3+ alpha chars, no pure-digit handles
                regex: Regex::new(r"(@[a-zA-Z]\w{2,})")
                    .expect("invalid person regex"),
                entity_type: EntityType::Person,
                name_group: 1,
            },
            EntityPattern {
                regex: Regex::new(r"([a-z][a-z0-9_]*-rs)")
                    .expect("invalid project-fallback regex"),
                entity_type: EntityType::Project,
                name_group: 1,
            },
        ];

        Self {
            patterns,
            known_matcher,
            known_index,
        }
    }

    /// Extract entities from content text.
    ///
    /// Phase 1: Aho-Corasick scan for known entities.
    /// Phase 2: Regex patterns for structural matches.
    /// Results are deduplicated by (normalized name, entity type).
    pub fn extract(&self, content: &str) -> Vec<ExtractedEntity> {
        let mut seen: HashSet<(String, String)> = HashSet::new();
        let mut results: Vec<ExtractedEntity> = Vec::new();

        // Phase 1: Aho-Corasick scan for known entities (with word boundary check)
        let content_bytes = content.as_bytes();
        for mat in self.known_matcher.find_iter(content) {
            let start = mat.start();
            let end = mat.end();
            
            // Word boundary: char before start and after end must be non-alphanumeric
            // This prevents "Rust" matching inside "RustClaw"
            let before_ok = start == 0 || {
                let b = content_bytes[start - 1];
                !b.is_ascii_alphanumeric() && b != b'_'
            };
            let after_ok = end >= content_bytes.len() || {
                let b = content_bytes[end];
                !b.is_ascii_alphanumeric() && b != b'_'
            };
            if !before_ok || !after_ok {
                continue;
            }
            
            let idx = mat.pattern().as_usize();
            let (ref entity_type, ref canonical_name) = self.known_index[idx];
            let matched_text = &content[start..end];
            let normalized = normalize_entity_name(matched_text, entity_type);
            let key = (normalized.clone(), entity_type.as_str().to_string());

            if seen.insert(key) {
                results.push(ExtractedEntity {
                    name: canonical_name.clone(),
                    normalized,
                    entity_type: entity_type.clone(),
                });
            }
        }

        // Phase 2: Regex patterns
        for pattern in &self.patterns {
            for caps in pattern.regex.captures_iter(content) {
                if let Some(m) = caps.get(pattern.name_group) {
                    let matched = m.as_str().to_string();
                    let normalized = normalize_entity_name(&matched, &pattern.entity_type);
                    let key = (normalized.clone(), pattern.entity_type.as_str().to_string());

                    if seen.insert(key) {
                        results.push(ExtractedEntity {
                            name: matched,
                            normalized,
                            entity_type: pattern.entity_type.clone(),
                        });
                    }
                }
            }
        }

        results
    }
}

/// Normalize an entity name based on its type.
///
/// - Lowercases the name
/// - Strips leading `@` for Person entities
/// - Strips trailing `/` for URL entities
/// - Keeps relative paths for File entities
pub fn normalize_entity_name(name: &str, entity_type: &EntityType) -> String {
    let mut normalized = name.to_lowercase();

    match entity_type {
        EntityType::Person => {
            normalized = normalized.trim_start_matches('@').to_string();
        }
        EntityType::Url => {
            normalized = normalized.trim_end_matches('/').to_string();
        }
        EntityType::File => {
            // Keep relative path as-is (already lowercased)
        }
        _ => {}
    }

    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_entity_type_as_str() {
        assert_eq!(EntityType::Project.as_str(), "project");
        assert_eq!(EntityType::Person.as_str(), "person");
        assert_eq!(EntityType::Technology.as_str(), "technology");
        assert_eq!(EntityType::Concept.as_str(), "concept");
        assert_eq!(EntityType::File.as_str(), "file");
        assert_eq!(EntityType::Url.as_str(), "url");
        assert_eq!(EntityType::Organization.as_str(), "organization");
        assert_eq!(EntityType::Other("custom".to_string()).as_str(), "custom");
    }

    #[test]
    fn test_entity_config_default() {
        let config = EntityConfig::default();
        assert!(config.enabled);
        assert_eq!(config.recall_weight, 0.15);
        assert!(config.known_projects.is_empty());
    }

    #[test]
    fn test_known_entity_extraction() {
        let config = EntityConfig {
            known_projects: vec!["IronClaw".to_string(), "Engram".to_string()],
            known_people: vec!["potato".to_string()],
            known_technologies: vec!["Rust".to_string(), "Supabase".to_string()],
            ..Default::default()
        };
        let extractor = EntityExtractor::new(&config);
        let entities = extractor.extract("potato is building IronClaw in Rust");

        let names: Vec<&str> = entities.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"potato"));
        assert!(names.contains(&"IronClaw"));
        assert!(names.contains(&"Rust"));
    }

    #[test]
    fn test_regex_patterns() {
        let config = EntityConfig::default();
        let extractor = EntityExtractor::new(&config);

        // Issue IDs
        let entities = extractor.extract("Working on ISS-123 and GOAL-45");
        let concepts: Vec<&str> = entities
            .iter()
            .filter(|e| e.entity_type == EntityType::Concept)
            .map(|e| e.name.as_str())
            .collect();
        assert!(concepts.contains(&"ISS-123"));
        assert!(concepts.contains(&"GOAL-45"));

        // File paths
        let entities = extractor.extract("Edited src/entities.rs and README.md");
        let files: Vec<&str> = entities
            .iter()
            .filter(|e| e.entity_type == EntityType::File)
            .map(|e| e.name.as_str())
            .collect();
        assert!(files.contains(&"src/entities.rs"));

        // URLs
        let entities = extractor.extract("See https://example.com/page");
        let urls: Vec<&str> = entities
            .iter()
            .filter(|e| e.entity_type == EntityType::Url)
            .map(|e| e.name.as_str())
            .collect();
        assert!(urls.contains(&"https://example.com/page"));

        // @mentions
        let entities = extractor.extract("Thanks @alice for the review");
        let people: Vec<&str> = entities
            .iter()
            .filter(|e| e.entity_type == EntityType::Person)
            .map(|e| e.normalized.as_str())
            .collect();
        assert!(people.contains(&"alice"));
    }

    #[test]
    fn test_normalize_entity_name() {
        assert_eq!(
            normalize_entity_name("@Alice", &EntityType::Person),
            "alice"
        );
        assert_eq!(
            normalize_entity_name("https://example.com/", &EntityType::Url),
            "https://example.com"
        );
        assert_eq!(
            normalize_entity_name("src/Main.rs", &EntityType::File),
            "src/main.rs"
        );
        assert_eq!(
            normalize_entity_name("IronClaw", &EntityType::Project),
            "ironclaw"
        );
    }

    #[test]
    fn test_dedup() {
        let config = EntityConfig {
            known_people: vec!["potato".to_string()],
            ..Default::default()
        };
        let extractor = EntityExtractor::new(&config);
        let entities = extractor.extract("potato and potato again");

        let potato_count = entities
            .iter()
            .filter(|e| e.normalized == "potato")
            .count();
        assert_eq!(potato_count, 1);
    }

    #[test]
    fn test_case_insensitive_known() {
        let config = EntityConfig {
            known_technologies: vec!["Rust".to_string()],
            ..Default::default()
        };
        let extractor = EntityExtractor::new(&config);
        let entities = extractor.extract("I love RUST and rust is great");

        let rust_count = entities
            .iter()
            .filter(|e| e.normalized == "rust" && e.entity_type == EntityType::Technology)
            .count();
        assert_eq!(rust_count, 1);
    }

    #[test]
    fn test_extract_concept_patterns() {
        let config = EntityConfig::default();
        let extractor = EntityExtractor::new(&config);
        let entities = extractor.extract("Tracking ISS-009, GOAL-1, and GUARD-3 this sprint");

        let concepts: Vec<&str> = entities
            .iter()
            .filter(|e| e.entity_type == EntityType::Concept)
            .map(|e| e.name.as_str())
            .collect();
        assert!(concepts.contains(&"ISS-009"));
        assert!(concepts.contains(&"GOAL-1"));
        assert!(concepts.contains(&"GUARD-3"));
        assert_eq!(concepts.len(), 3);
    }

    #[test]
    fn test_extract_file_paths() {
        let config = EntityConfig::default();
        let extractor = EntityExtractor::new(&config);
        let entities = extractor.extract("Changed src/memory.rs and config.py");

        let files: Vec<&str> = entities
            .iter()
            .filter(|e| e.entity_type == EntityType::File)
            .map(|e| e.name.as_str())
            .collect();
        assert!(files.contains(&"src/memory.rs"));
        assert!(files.contains(&"config.py"));
    }

    #[test]
    fn test_extract_urls() {
        let config = EntityConfig::default();
        let extractor = EntityExtractor::new(&config);

        // Basic URL extraction
        let entities = extractor.extract("Check https://crates.io/crates/engramai for details");
        let urls: Vec<&str> = entities
            .iter()
            .filter(|e| e.entity_type == EntityType::Url)
            .map(|e| e.name.as_str())
            .collect();
        assert!(urls.contains(&"https://crates.io/crates/engramai"));

        // Trailing slash gets normalized (stripped)
        let entities = extractor.extract("Visit https://example.com/docs/");
        let url_entity = entities
            .iter()
            .find(|e| e.entity_type == EntityType::Url)
            .expect("should extract a URL");
        assert_eq!(url_entity.normalized, "https://example.com/docs");
    }

    #[test]
    fn test_extract_at_mentions() {
        let config = EntityConfig::default();
        let extractor = EntityExtractor::new(&config);
        let entities = extractor.extract("Thanks @potatosoupup for the help");

        let person = entities
            .iter()
            .find(|e| e.entity_type == EntityType::Person)
            .expect("should extract a person");
        assert_eq!(person.name, "@potatosoupup");
        assert_eq!(person.normalized, "potatosoupup");
    }

    #[test]
    fn test_extract_crate_names() {
        let config = EntityConfig::default();
        let extractor = EntityExtractor::new(&config);
        let entities = extractor.extract("Check out infomap-rs for graph clustering");

        let projects: Vec<&str> = entities
            .iter()
            .filter(|e| e.entity_type == EntityType::Project)
            .map(|e| e.name.as_str())
            .collect();
        assert!(projects.contains(&"infomap-rs"));
    }

    #[test]
    fn test_empty_content() {
        let config = EntityConfig::default();
        let extractor = EntityExtractor::new(&config);
        let entities = extractor.extract("");

        assert!(entities.is_empty());
    }

    #[test]
    fn test_dedup_same_entity_twice() {
        let config = EntityConfig {
            known_projects: vec!["rustclaw".to_string()],
            ..Default::default()
        };
        let extractor = EntityExtractor::new(&config);
        let entities = extractor.extract("rustclaw is great, I love rustclaw");

        let rustclaw_count = entities
            .iter()
            .filter(|e| e.normalized == "rustclaw")
            .count();
        assert_eq!(rustclaw_count, 1);
    }

    #[test]
    fn test_overlapping_known_and_regex() {
        // "gid-rs" is in known_projects AND matches the *-rs regex fallback
        let config = EntityConfig {
            known_projects: vec!["gid-rs".to_string()],
            ..Default::default()
        };
        let extractor = EntityExtractor::new(&config);
        let entities = extractor.extract("Working on gid-rs today");

        let gid_count = entities
            .iter()
            .filter(|e| e.normalized == "gid-rs" && e.entity_type == EntityType::Project)
            .count();
        assert_eq!(gid_count, 1, "gid-rs should appear only once even if matched by both known and regex");
    }

    #[test]
    fn test_case_insensitive_known_project() {
        let config = EntityConfig {
            known_projects: vec!["rustclaw".to_string()],
            ..Default::default()
        };
        let extractor = EntityExtractor::new(&config);
        let entities = extractor.extract("RustClaw is awesome, also rustclaw rocks");

        let rustclaw_count = entities
            .iter()
            .filter(|e| e.normalized == "rustclaw" && e.entity_type == EntityType::Project)
            .count();
        assert_eq!(rustclaw_count, 1, "RustClaw and rustclaw should both match known project and dedup to one");
    }

    #[test]
    fn test_normalize_entity_name_cases() {
        // Person: strip @ prefix and lowercase
        assert_eq!(normalize_entity_name("@foo", &EntityType::Person), "foo");

        // Url: strip trailing slash
        assert_eq!(
            normalize_entity_name("https://x.com/", &EntityType::Url),
            "https://x.com"
        );

        // File: unchanged except lowercase
        assert_eq!(
            normalize_entity_name("src/lib.rs", &EntityType::File),
            "src/lib.rs"
        );

        // Default (Project, Technology, etc.): just lowercase
        assert_eq!(
            normalize_entity_name("MyProject", &EntityType::Project),
            "myproject"
        );
        assert_eq!(
            normalize_entity_name("TypeScript", &EntityType::Technology),
            "typescript"
        );
        assert_eq!(
            normalize_entity_name("GUARD-5", &EntityType::Concept),
            "guard-5"
        );
    }
    
    #[test]
    fn test_at_mention_rejects_short_and_numeric() {
        let config = EntityConfig::default();
        let extractor = EntityExtractor::new(&config);
        
        // Should NOT match: too short or pure digits
        let entities = extractor.extract("@0 @1 @ab test");
        let persons: Vec<&str> = entities.iter()
            .filter(|e| e.entity_type == EntityType::Person)
            .map(|e| e.normalized.as_str())
            .collect();
        assert!(persons.is_empty(), "Short @mentions should not be extracted: {:?}", persons);
        
        // Should match: 3+ chars starting with alpha
        let entities = extractor.extract("Thanks @alice and @bob123");
        let persons: Vec<&str> = entities.iter()
            .filter(|e| e.entity_type == EntityType::Person)
            .map(|e| e.normalized.as_str())
            .collect();
        assert!(persons.contains(&"alice"), "Valid @mention should be extracted");
        assert!(persons.contains(&"bob123"), "Valid @mention should be extracted");
    }
    
    #[test]
    fn test_builtin_technologies() {
        let config = EntityConfig::default();
        let extractor = EntityExtractor::new(&config);
        
        let entities = extractor.extract("Building with Rust and PostgreSQL, deployed on Docker");
        let techs: Vec<&str> = entities.iter()
            .filter(|e| e.entity_type == EntityType::Technology)
            .map(|e| e.name.as_str())
            .collect();
        assert!(techs.contains(&"Rust"), "Builtin tech 'Rust' should be extracted");
        assert!(techs.contains(&"PostgreSQL"), "Builtin tech 'PostgreSQL' should be extracted");
        assert!(techs.contains(&"Docker"), "Builtin tech 'Docker' should be extracted");
    }
}
