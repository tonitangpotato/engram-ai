//! Configuration knobs for the v0.3 Knowledge Compiler.
//!
//! See design §5bis for the canonical defaults. Every field here maps 1:1
//! to a value cited in `.gid/features/v03-resolution/design.md`.

use std::time::Duration;

/// Configuration for one Knowledge Compiler run.
///
/// All defaults match the values stated in design §5bis. Keep them in sync
/// with the design when tuning — operators rely on the defaults landing
/// without surprise.
#[derive(Debug, Clone)]
pub struct KnowledgeCompileConfig {
    /// Schedule cadence for the background runner (§5bis.1). The compile
    /// module itself does not schedule — this value is consumed by the
    /// caller (cron/timer/scheduler) and surfaced here so a single
    /// configuration source-of-truth covers both manual and scheduled
    /// invocations.
    pub compiler_interval_hours: u32,

    /// K1 importance floor (§5bis.2). Memories below this importance are
    /// not eligible for compile candidacy. Default: `0.3`.
    pub compile_min_importance: f32,

    /// K1 per-run cap (§5bis.2). Excess memories roll to the next run.
    /// Default: `5000`.
    pub compile_max_candidates_per_run: usize,

    /// K3 supersession threshold (§5bis.4). When a new cluster's
    /// `source_memories` overlap an existing live topic by ≥ this
    /// fraction (Jaccard-style on `|new ∩ old| / |new|`), the old topic
    /// is marked superseded by the new one. Default: `0.5`.
    pub topic_supersede_threshold: f32,

    /// Wall-clock budget for one run (§5bis.5). Completed clusters are
    /// kept; the run is marked `failed` with `error_detail =
    /// "timeout, N clusters completed"`. Default: `1h`.
    pub compile_max_duration: Duration,
}

impl Default for KnowledgeCompileConfig {
    fn default() -> Self {
        Self {
            compiler_interval_hours: 24,
            compile_min_importance: 0.3,
            compile_max_candidates_per_run: 5000,
            topic_supersede_threshold: 0.5,
            compile_max_duration: Duration::from_secs(60 * 60),
        }
    }
}

impl KnowledgeCompileConfig {
    /// Construct a fast-iterating test config. Tightens the wall budget so
    /// timeout paths are observable in tests.
    #[cfg(test)]
    pub fn for_test() -> Self {
        Self {
            compiler_interval_hours: 24,
            compile_min_importance: 0.0,
            compile_max_candidates_per_run: 100,
            topic_supersede_threshold: 0.5,
            compile_max_duration: Duration::from_secs(30),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_design_section_5bis() {
        // These constants are part of the v0.3 contract — failing this
        // test means the design doc and the code disagree.
        let c = KnowledgeCompileConfig::default();
        assert_eq!(c.compiler_interval_hours, 24);
        assert_eq!(c.compile_min_importance, 0.3);
        assert_eq!(c.compile_max_candidates_per_run, 5000);
        assert_eq!(c.topic_supersede_threshold, 0.5);
        assert_eq!(c.compile_max_duration, Duration::from_secs(3600));
    }
}
