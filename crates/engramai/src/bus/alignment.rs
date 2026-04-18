//! Drive Alignment Scorer — Score how well memories align with SOUL drives.
//!
//! Two scoring strategies:
//! - **Embedding-based** (preferred): Cosine similarity between content and drive embeddings.
//!   Naturally handles multilingual content (Chinese SOUL + English content = still works).
//! - **Keyword-based** (fallback): Simple keyword matching. Fast but monolingual.

use crate::bus::mod_io::Drive;
use crate::embeddings::EmbeddingProvider;

/// Default importance multiplier for drive-aligned memories.
pub const ALIGNMENT_BOOST: f64 = 1.5;

/// Minimum cosine similarity to consider content "aligned" with a drive.
/// With nomic-embed-text, cross-language baseline is ~0.1-0.3, so we need 
/// a high enough threshold to filter noise while catching real alignment.
const EMBEDDING_ALIGNMENT_THRESHOLD: f32 = 0.3;

/// Pre-computed drive embeddings for fast alignment scoring.
#[derive(Clone)]
pub struct DriveEmbeddings {
    /// (drive_index, embedding_vector) pairs
    pub(crate) entries: Vec<(usize, Vec<f32>)>,
}

impl DriveEmbeddings {
    /// Number of drives with embeddings.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no drives have embeddings.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Pre-compute embeddings for all drives.
    /// Returns None if embedding provider is unavailable.
    pub fn compute(drives: &[Drive], provider: &EmbeddingProvider) -> Option<Self> {
        if drives.is_empty() || !provider.is_available() {
            return None;
        }

        let mut entries = Vec::new();
        for (i, drive) in drives.iter().enumerate() {
            // Embed the drive description + name for rich semantic representation
            let text = format!("{}: {}", drive.name, drive.description);
            match provider.embed(&text) {
                Ok(vec) => entries.push((i, vec)),
                Err(e) => {
                    log::debug!("Failed to embed drive '{}': {}", drive.name, e);
                    // Continue with other drives
                }
            }
        }

        if entries.is_empty() {
            None
        } else {
            Some(Self { entries })
        }
    }

    /// Score alignment using cosine similarity between content embedding and drive embeddings.
    /// Returns 0.0-1.0 alignment score.
    pub fn score(&self, content_embedding: &[f32]) -> f64 {
        if self.entries.is_empty() {
            return 0.0;
        }

        let mut max_similarity: f32 = 0.0;
        let mut total_similarity: f32 = 0.0;
        let mut aligned_count = 0;

        for (_idx, drive_emb) in &self.entries {
            let sim = EmbeddingProvider::cosine_similarity(content_embedding, drive_emb);
            if sim > EMBEDDING_ALIGNMENT_THRESHOLD {
                aligned_count += 1;
                total_similarity += sim;
            }
            if sim > max_similarity {
                max_similarity = sim;
            }
        }

        if aligned_count == 0 {
            return 0.0;
        }

        // Use average of aligned similarities, normalized to 0.0-1.0
        let avg = total_similarity / aligned_count as f32;
        // Map from [threshold..1.0] to [0.0..1.0]
        let normalized = ((avg - EMBEDDING_ALIGNMENT_THRESHOLD) / (1.0 - EMBEDDING_ALIGNMENT_THRESHOLD)).min(1.0);
        normalized as f64
    }

    /// Find which drives align with content, returning (drive_index, similarity).
    pub fn find_aligned(&self, content_embedding: &[f32]) -> Vec<(usize, f32)> {
        self.entries.iter()
            .map(|(idx, drive_emb)| (*idx, EmbeddingProvider::cosine_similarity(content_embedding, drive_emb)))
            .filter(|(_, sim)| *sim > EMBEDDING_ALIGNMENT_THRESHOLD)
            .collect()
    }
}

/// Score alignment using combined embedding + keyword signals.
///
/// Strategy:
/// - If both embedding and keyword signals exist, combine them (max wins)
/// - If only embedding, use embedding score
/// - If only keyword, use keyword score
/// - This naturally handles multilingual: embedding catches cross-language,
///   keywords catch same-language exact matches
pub fn score_alignment_hybrid(
    content: &str,
    drives: &[Drive],
    drive_embeddings: Option<&DriveEmbeddings>,
    content_embedding: Option<&[f32]>,
) -> f64 {
    let keyword_score = score_alignment(content, drives);
    
    let embedding_score = match (drive_embeddings, content_embedding) {
        (Some(de), Some(ce)) => de.score(ce),
        _ => 0.0,
    };

    // Take the max — either signal is sufficient
    keyword_score.max(embedding_score)
}

/// Score how well a memory content aligns with a set of drives.
///
/// Returns a score from 0.0 (no alignment) to 1.0 (strong alignment).
/// The scoring is based on keyword matching between the memory content
/// and the drives' keywords.
///
/// # Arguments
///
/// * `content` - The memory content to score
/// * `drives` - List of drives to check alignment against
pub fn score_alignment(content: &str, drives: &[Drive]) -> f64 {
    if drives.is_empty() {
        return 0.0;
    }
    
    let content_lower = content.to_lowercase();
    let content_words: Vec<&str> = content_lower.split_whitespace().collect();
    
    let mut total_score = 0.0;
    let mut matched_drives = 0;
    
    for drive in drives {
        let mut drive_matches = 0;
        let keywords = if drive.keywords.is_empty() {
            drive.extract_keywords()
        } else {
            drive.keywords.clone()
        };
        
        for keyword in &keywords {
            // Check for exact word match or substring match
            if content_words.iter().any(|w| w.contains(keyword)) {
                drive_matches += 1;
            }
        }
        
        if drive_matches > 0 {
            matched_drives += 1;
            // Score contribution: min(1.0, matches / 3) - need at least 3 matches for full score
            let drive_score = (drive_matches as f64 / 3.0).min(1.0);
            total_score += drive_score;
        }
    }
    
    if matched_drives == 0 {
        return 0.0;
    }
    
    // Average score across matched drives, capped at 1.0
    (total_score / matched_drives as f64).min(1.0)
}

/// Calculate the importance boost for a memory based on drive alignment.
///
/// Returns a multiplier (1.0 = no boost, ALIGNMENT_BOOST for perfect alignment).
///
/// # Arguments
///
/// * `content` - The memory content
/// * `drives` - List of drives from SOUL.md
pub fn calculate_importance_boost(content: &str, drives: &[Drive]) -> f64 {
    let alignment = score_alignment(content, drives);
    
    if alignment <= 0.0 {
        return 1.0; // No boost
    }
    
    // Linear interpolation between 1.0 and ALIGNMENT_BOOST based on alignment
    1.0 + (ALIGNMENT_BOOST - 1.0) * alignment
}

/// Check if content is strongly aligned with any drive.
///
/// Returns true if alignment score is above 0.5.
pub fn is_strongly_aligned(content: &str, drives: &[Drive]) -> bool {
    score_alignment(content, drives) > 0.5
}

/// Find which drives a piece of content aligns with.
///
/// Returns a list of (drive_name, alignment_score) pairs for aligned drives.
pub fn find_aligned_drives(content: &str, drives: &[Drive]) -> Vec<(String, f64)> {
    let content_lower = content.to_lowercase();
    let content_words: Vec<&str> = content_lower.split_whitespace().collect();
    
    let mut aligned = Vec::new();
    
    for drive in drives {
        let keywords = if drive.keywords.is_empty() {
            drive.extract_keywords()
        } else {
            drive.keywords.clone()
        };
        
        let mut matches = 0;
        for keyword in &keywords {
            if content_words.iter().any(|w| w.contains(keyword)) {
                matches += 1;
            }
        }
        
        if matches > 0 {
            let score = (matches as f64 / 3.0).min(1.0);
            aligned.push((drive.name.clone(), score));
        }
    }
    
    // Sort by score descending
    aligned.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    aligned
}

/// Convert a drive alignment score into an [`InteroceptiveSignal`].
///
/// - `valence`: alignment_score mapped to [-1, 1] (0.5 → 0, 1.0 → 1.0, 0.0 → -1.0).
/// - `arousal`: low — alignment is a slow background signal.
pub fn alignment_to_signal(
    content: &str,
    drives: &[Drive],
) -> crate::interoceptive::InteroceptiveSignal {
    use crate::interoceptive::{InteroceptiveSignal, SignalContext, SignalSource};

    let score = score_alignment(content, drives);
    let valence = score * 2.0 - 1.0; // [0,1] → [-1,1]
    let arousal = 0.15; // alignment is a slow background signal

    let snippet = if content.len() > 80 {
        format!("{}...", &content[..80])
    } else {
        content.to_string()
    };

    InteroceptiveSignal::new(SignalSource::Alignment, None, valence, arousal)
        .with_context(SignalContext::DriveAlignment {
            content_snippet: snippet,
            alignment_score: score,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    
    fn sample_drives() -> Vec<Drive> {
        vec![
            Drive {
                name: "curiosity".to_string(),
                description: "Always seek to understand and learn new things".to_string(),
                keywords: vec!["curiosity".to_string(), "understand".to_string(), "learn".to_string(), "new".to_string()],
            },
            Drive {
                name: "helpfulness".to_string(),
                description: "Help users solve problems effectively".to_string(),
                keywords: vec!["helpfulness".to_string(), "help".to_string(), "solve".to_string(), "problems".to_string()],
            },
            Drive {
                name: "honesty".to_string(),
                description: "Be honest and direct in communication".to_string(),
                keywords: vec!["honesty".to_string(), "honest".to_string(), "direct".to_string(), "communication".to_string()],
            },
        ]
    }
    
    #[test]
    fn test_strong_alignment() {
        let drives = sample_drives();
        
        // Content that strongly aligns with "curiosity"
        let content = "I want to learn and understand these new concepts deeply";
        let score = score_alignment(content, &drives);
        assert!(score > 0.5, "Expected strong alignment, got {}", score);
    }
    
    #[test]
    fn test_weak_alignment() {
        let drives = sample_drives();
        
        // Content with minimal alignment
        let content = "The weather is nice today";
        let score = score_alignment(content, &drives);
        assert!(score < 0.3, "Expected weak alignment, got {}", score);
    }
    
    #[test]
    fn test_no_alignment() {
        let drives = sample_drives();
        
        // Content with no alignment
        let content = "xyz abc 123";
        let score = score_alignment(content, &drives);
        assert_eq!(score, 0.0);
    }
    
    #[test]
    fn test_importance_boost() {
        let drives = sample_drives();
        
        // Strongly aligned content gets boost
        let aligned = "I want to learn and understand new concepts";
        let boost = calculate_importance_boost(aligned, &drives);
        assert!(boost > 1.0, "Expected boost > 1.0, got {}", boost);
        assert!(boost <= ALIGNMENT_BOOST);
        
        // Non-aligned content gets no boost
        let unaligned = "xyz abc 123";
        let boost = calculate_importance_boost(unaligned, &drives);
        assert_eq!(boost, 1.0);
    }
    
    #[test]
    fn test_find_aligned_drives() {
        let drives = sample_drives();
        
        let content = "I want to help people understand and solve their problems";
        let aligned = find_aligned_drives(content, &drives);
        
        assert!(aligned.len() >= 2);
        // Should find helpfulness and curiosity
        let drive_names: Vec<_> = aligned.iter().map(|(n, _)| n.as_str()).collect();
        assert!(drive_names.contains(&"helpfulness") || drive_names.contains(&"curiosity"));
    }
    
    #[test]
    fn test_empty_drives() {
        let drives: Vec<Drive> = vec![];
        let content = "any content here";
        assert_eq!(score_alignment(content, &drives), 0.0);
        assert_eq!(calculate_importance_boost(content, &drives), 1.0);
    }
}

#[cfg(test)]
mod embedding_tests {
    use super::*;

    #[test]
    fn test_embedding_alignment_if_available() {
        // Only runs meaningfully with Ollama available
        let provider = EmbeddingProvider::new(crate::embeddings::EmbeddingConfig::ollama("nomic-embed-text", 768));
        
        if !provider.is_available() {
            println!("⚠️ Ollama not available, skipping embedding alignment test");
            return;
        }

        // Create drives with Chinese descriptions (simulating SOUL.md parse)
        let drives = vec![
            crate::bus::mod_io::Drive {
                name: "财务自由".to_string(),
                description: "帮potato实现财务自由，找到市场机会，交易获利".to_string(),
                keywords: vec!["财务自由".into(), "市场机会".into(), "交易获利".into()],
            },
            crate::bus::mod_io::Drive {
                name: "技术深度".to_string(),
                description: "写优秀的代码，深入理解Rust和系统架构".to_string(),
                keywords: vec!["代码".into(), "rust".into(), "架构".into()],
            },
        ];

        // Pre-compute drive embeddings
        let de = DriveEmbeddings::compute(&drives, &provider);
        assert!(de.is_some(), "Should compute drive embeddings");
        let de = de.unwrap();
        assert_eq!(de.len(), 2);

        // Test 1: English "trading profit" should align with Chinese "交易获利" drive
        let english_trading = provider.embed("trading profit market opportunity revenue").unwrap();
        let trading_score = de.score(&english_trading);
        println!("English 'trading profit' → Chinese '财务自由' drive: score={:.3}", trading_score);
        
        // Test 2: English "rust code architecture" should align with Chinese "技术深度" drive  
        let english_coding = provider.embed("rust code architecture system design").unwrap();
        let coding_score = de.score(&english_coding);
        println!("English 'rust code' → Chinese '技术深度' drive: score={:.3}", coding_score);

        // Test 3: Unrelated content should NOT align
        let unrelated = provider.embed("weather forecast sunny tomorrow beach vacation").unwrap();
        let unrelated_score = de.score(&unrelated);
        println!("English 'weather beach' → drives: score={:.3}", unrelated_score);

        // Verify: trading and coding should score higher than unrelated
        assert!(trading_score > unrelated_score, 
            "Trading ({:.3}) should score higher than unrelated ({:.3})", trading_score, unrelated_score);
        assert!(coding_score > unrelated_score,
            "Coding ({:.3}) should score higher than unrelated ({:.3})", coding_score, unrelated_score);

        // Test 4: Chinese content should also work
        let chinese_trading = provider.embed("交易策略今天赚了50美元").unwrap();
        let zh_score = de.score(&chinese_trading);
        println!("Chinese '交易策略赚了50美元' → drives: score={:.3}", zh_score);

        // Test hybrid function: English content + Chinese drives
        let hybrid_en = score_alignment_hybrid(
            "trading profit revenue",
            &drives,
            Some(&de),
            Some(&english_trading),
        );
        println!("Hybrid (English→Chinese drives): {:.3}", hybrid_en);
        // Keyword alone returns 0 for cross-language, embedding provides signal
        assert!(hybrid_en > 0.0, "Hybrid should find cross-language alignment");

        // Test hybrid: Chinese content + Chinese drives (both signals)
        let hybrid_zh = score_alignment_hybrid(
            "交易策略今天赚了50美元 市场机会",
            &drives,
            Some(&de),
            Some(&chinese_trading),
        );
        println!("Hybrid (Chinese→Chinese drives): {:.3}", hybrid_zh);
        // Both keyword AND embedding should contribute
        assert!(hybrid_zh > 0.0, "Chinese-Chinese should have strong alignment");

        // Test: keyword-only fallback still works for same-language
        let keyword_only = score_alignment_hybrid(
            "市场机会 交易获利 财务自由",
            &drives,
            None,
            None,
        );
        println!("Keyword-only (Chinese→Chinese): {:.3}", keyword_only);
        assert!(keyword_only > 0.0, "Same-language keywords should match");

        // Test: keyword-only fails cross-language (this is the bug we're fixing)
        let keyword_cross = score_alignment_hybrid(
            "trading profit revenue",
            &drives,
            None,
            None,
        );
        println!("Keyword-only (English→Chinese): {:.3}", keyword_cross);
        assert_eq!(keyword_cross, 0.0, "Keywords alone can't match cross-language");

        println!("\n🎉 Embedding alignment solves cross-language: English→Chinese works!");
    }

    fn test_drives() -> Vec<Drive> {
        vec![
            Drive {
                name: "curiosity".into(),
                description: "Always seek to understand and learn new things".into(),
                keywords: vec!["curiosity".into(), "understand".into(), "learn".into(), "new".into()],
            },
            Drive {
                name: "helpfulness".into(),
                description: "Help users solve problems effectively".into(),
                keywords: vec!["helpfulness".into(), "help".into(), "solve".into(), "problems".into()],
            },
        ]
    }

    #[test]
    fn test_alignment_to_signal_high_alignment() {
        let drives = test_drives();
        let sig = alignment_to_signal("I want to understand and learn new things", &drives);
        assert!(matches!(sig.source, crate::interoceptive::SignalSource::Alignment));
        assert!(sig.domain.is_none());
        assert!(sig.valence > 0.0, "aligned content → positive valence, got {}", sig.valence);
        assert!((sig.arousal - 0.15).abs() < 0.01);
        assert!(matches!(
            sig.context,
            Some(crate::interoceptive::SignalContext::DriveAlignment { .. })
        ));
    }

    #[test]
    fn test_alignment_to_signal_no_alignment() {
        let drives = test_drives();
        let sig = alignment_to_signal("the weather is nice today", &drives);
        assert!(sig.valence < 0.0, "unaligned content → negative valence, got {}", sig.valence);
    }
}
