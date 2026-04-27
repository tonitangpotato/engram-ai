//! Resolution pipeline configuration and statistics.
//!
//! - [`ResolutionConfig`]: tunable knobs for the worker pool, queue, and
//!   downstream stages. Defaults match the design (§5.1 / §5.2).
//! - [`ResolutionStats`]: per-call counter snapshot exposed via §6.4
//!   `Memory::ingest_with_stats` (public benchmarks-handoff contract).
//!
//! See `.gid/features/v03-resolution/design.md` §5.1 (worker pool defaults),
//! §5.2 (queue capacity), §6.4 (stats), §7 (telemetry).
//!
//! # Boundary rules
//!
//! - This module is **pure data** — no IO, no globals. Workers and queue
//!   own instances and read them by reference.
//! - Counters in [`ResolutionStats`] are saturating-add semantics:
//!   overflow saturates at `u64::MAX`. Long-running processes that
//!   produce > 2^64 jobs are not a concern; the saturation prevents UB
//!   if a future caller sums hot-path counters into a single `Stats`.
//! - Adding a new field to [`ResolutionConfig`] or [`ResolutionStats`]
//!   is **non-breaking** because both are `#[non_exhaustive]`. Renaming
//!   or removing fields is breaking.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Default worker pool size. `1` is intentionally conservative (single-writer
/// serializes SQLite writes; multi-writer is opt-in). Design §5.1.
pub const DEFAULT_WORKER_COUNT: usize = 1;

/// Hard cap on configured worker count. Above this the pool returns an
/// error at start time; the design (§5.1) names a "small cap (8)" so users
/// don't accidentally configure dozens of workers and starve the SQLite
/// connection pool.
pub const MAX_WORKER_COUNT: usize = 8;

/// Default bounded queue capacity (§5.2). Sized so 10k jobs at ~1KB each
/// is ~10MB of in-process memory — comfortable for the v0.3 single-process
/// MVP.
pub const DEFAULT_QUEUE_CAP: usize = 10_000;

/// Default shutdown drain deadline. On shutdown the pool stops accepting
/// new enqueues, then waits up to this long for in-flight jobs to commit
/// before forcibly stopping workers.
pub const DEFAULT_SHUTDOWN_DRAIN: Duration = Duration::from_secs(30);

/// Default poll interval used by the in-memory queue worker loop when the
/// queue is empty. Real production deployments should swap in a
/// notify-driven channel (crossbeam / tokio-mpsc) so this constant only
/// drives the test/MVP path. See [`ResolutionConfig::worker_idle_poll`].
pub const DEFAULT_WORKER_IDLE_POLL: Duration = Duration::from_millis(10);

/// Tunable parameters for the resolution pipeline.
///
/// Constructed once at `Memory::open` time (or test setup) and shared by
/// reference between the worker pool, queue producer, and stage drivers.
/// Cloning is cheap (all `Copy` or small `Duration`s).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ResolutionConfig {
    /// Number of resolution workers in the pool. Range: `1..=MAX_WORKER_COUNT`.
    /// `1` (default) serializes all writes; `>1` enables session-affinity
    /// dispatch (`hash(session_id) % N`) for inter-session parallelism.
    pub worker_count: usize,

    /// Bounded queue capacity for droppable (`Initial`) jobs. `ReExtract`
    /// jobs bypass this cap (§5.2 non-droppable). Default: 10,000.
    pub queue_cap: usize,

    /// Maximum wall time the pool waits for in-flight jobs to commit on
    /// shutdown before stopping workers. After this expires, queued jobs
    /// remain in `graph_pipeline_runs` with `status = queued` and are
    /// recovered on restart (§5.1.1 crash recovery).
    pub shutdown_drain: Duration,

    /// Poll interval for the worker loop when the queue is empty. Only
    /// affects the in-memory `BoundedJobQueue` MVP backend; a notify-driven
    /// queue ignores this.
    pub worker_idle_poll: Duration,
}

impl Default for ResolutionConfig {
    fn default() -> Self {
        Self {
            worker_count: DEFAULT_WORKER_COUNT,
            queue_cap: DEFAULT_QUEUE_CAP,
            shutdown_drain: DEFAULT_SHUTDOWN_DRAIN,
            worker_idle_poll: DEFAULT_WORKER_IDLE_POLL,
        }
    }
}

/// Why a [`ResolutionConfig`] was rejected at validation time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigError {
    /// `worker_count == 0`.
    ZeroWorkers,
    /// `worker_count > MAX_WORKER_COUNT`.
    TooManyWorkers { requested: usize, cap: usize },
    /// `queue_cap == 0` — pathological, would reject every `Initial` job.
    ZeroQueueCap,
    /// `shutdown_drain` is zero — would skip the drain phase entirely. We
    /// require at least 1ms so a graceful shutdown is observable.
    ZeroShutdownDrain,
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ZeroWorkers => f.write_str("worker_count must be >= 1"),
            Self::TooManyWorkers { requested, cap } => {
                write!(f, "worker_count {} exceeds cap {}", requested, cap)
            }
            Self::ZeroQueueCap => f.write_str("queue_cap must be >= 1"),
            Self::ZeroShutdownDrain => f.write_str("shutdown_drain must be > 0"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl ResolutionConfig {
    /// Validate the config. Called once at pool construction; failures
    /// abort startup rather than getting silently clamped (GUARD-2 — never
    /// silent on a misconfiguration).
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.worker_count == 0 {
            return Err(ConfigError::ZeroWorkers);
        }
        if self.worker_count > MAX_WORKER_COUNT {
            return Err(ConfigError::TooManyWorkers {
                requested: self.worker_count,
                cap: MAX_WORKER_COUNT,
            });
        }
        if self.queue_cap == 0 {
            return Err(ConfigError::ZeroQueueCap);
        }
        if self.shutdown_drain.is_zero() {
            return Err(ConfigError::ZeroShutdownDrain);
        }
        Ok(())
    }
}

/// Per-call resolution statistics. Returned from `Memory::ingest_with_stats`
/// (§6.4) so benchmarks and operators can observe the pipeline cost of a
/// single ingest without scraping internal metrics.
///
/// Counters are scoped to **one** call (one `store_raw` + downstream
/// pipeline). For aggregate process-wide metrics see the telemetry sink
/// (§7).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ResolutionStats {
    /// Number of mentions extracted by §3.2.
    pub entities_extracted: u64,
    /// Number of triples extracted by §3.3.
    pub edges_extracted: u64,

    /// Number of `MergeInto` decisions (existing entity reused).
    pub entities_merged: u64,
    /// Number of `CreateNew` decisions (new canonical entity minted).
    pub entities_created: u64,
    /// Number of `DeferToLlm` decisions (entity decision punted to a
    /// downstream LLM round). For v0.3 MVP this is observed but not
    /// actioned (the deferred decision becomes `CreateNew` on persist).
    pub entities_deferred: u64,

    /// Number of edges added (decision = ADD).
    pub edges_added: u64,
    /// Number of edges marked superseded (decision = UPDATE / supersede).
    pub edges_updated: u64,
    /// Number of edges left alone (decision = NONE / preserve).
    pub edges_preserved: u64,

    /// Stage failures recorded on this call's `PipelineContext`.
    pub stage_failures: u64,

    /// Time spent in the entity-extract stage. Sums multi-extractor
    /// invocations; on retry, both attempts contribute.
    pub entity_extract_duration: Duration,
    /// Time spent in the edge-extract stage (LLM call dominates).
    pub edge_extract_duration: Duration,
    /// Time spent in the resolve stage (candidate retrieval + fusion +
    /// decision).
    pub resolve_duration: Duration,
    /// Time spent in the persist stage (one SQLite transaction).
    pub persist_duration: Duration,
}

impl ResolutionStats {
    /// Saturating-add accumulator. Used by the worker to fold per-job
    /// stats into a per-call rollup before returning.
    pub fn add(&mut self, other: &ResolutionStats) {
        self.entities_extracted = self.entities_extracted.saturating_add(other.entities_extracted);
        self.edges_extracted = self.edges_extracted.saturating_add(other.edges_extracted);
        self.entities_merged = self.entities_merged.saturating_add(other.entities_merged);
        self.entities_created = self.entities_created.saturating_add(other.entities_created);
        self.entities_deferred = self.entities_deferred.saturating_add(other.entities_deferred);
        self.edges_added = self.edges_added.saturating_add(other.edges_added);
        self.edges_updated = self.edges_updated.saturating_add(other.edges_updated);
        self.edges_preserved = self.edges_preserved.saturating_add(other.edges_preserved);
        self.stage_failures = self.stage_failures.saturating_add(other.stage_failures);
        self.entity_extract_duration =
            self.entity_extract_duration.saturating_add(other.entity_extract_duration);
        self.edge_extract_duration =
            self.edge_extract_duration.saturating_add(other.edge_extract_duration);
        self.resolve_duration = self.resolve_duration.saturating_add(other.resolve_duration);
        self.persist_duration = self.persist_duration.saturating_add(other.persist_duration);
    }

    /// Total wall time across all stages. Convenience for benchmarks.
    pub fn total_duration(&self) -> Duration {
        self.entity_extract_duration
            .saturating_add(self.edge_extract_duration)
            .saturating_add(self.resolve_duration)
            .saturating_add(self.persist_duration)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_validates() {
        let c = ResolutionConfig::default();
        assert_eq!(c.worker_count, 1);
        assert_eq!(c.queue_cap, 10_000);
        c.validate().unwrap();
    }

    #[test]
    fn zero_workers_rejected() {
        let mut c = ResolutionConfig::default();
        c.worker_count = 0;
        assert_eq!(c.validate(), Err(ConfigError::ZeroWorkers));
    }

    #[test]
    fn too_many_workers_rejected() {
        let mut c = ResolutionConfig::default();
        c.worker_count = MAX_WORKER_COUNT + 1;
        match c.validate() {
            Err(ConfigError::TooManyWorkers { requested, cap }) => {
                assert_eq!(requested, MAX_WORKER_COUNT + 1);
                assert_eq!(cap, MAX_WORKER_COUNT);
            }
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn zero_queue_cap_rejected() {
        let mut c = ResolutionConfig::default();
        c.queue_cap = 0;
        assert_eq!(c.validate(), Err(ConfigError::ZeroQueueCap));
    }

    #[test]
    fn zero_shutdown_drain_rejected() {
        let mut c = ResolutionConfig::default();
        c.shutdown_drain = Duration::ZERO;
        assert_eq!(c.validate(), Err(ConfigError::ZeroShutdownDrain));
    }

    #[test]
    fn config_at_cap_validates() {
        let mut c = ResolutionConfig::default();
        c.worker_count = MAX_WORKER_COUNT;
        c.validate().unwrap();
    }

    #[test]
    fn stats_default_is_zero() {
        let s = ResolutionStats::default();
        assert_eq!(s.entities_extracted, 0);
        assert_eq!(s.total_duration(), Duration::ZERO);
    }

    #[test]
    fn stats_add_accumulates() {
        let mut a = ResolutionStats {
            entities_extracted: 3,
            edges_added: 2,
            entity_extract_duration: Duration::from_millis(10),
            ..Default::default()
        };
        let b = ResolutionStats {
            entities_extracted: 4,
            edges_added: 1,
            entity_extract_duration: Duration::from_millis(15),
            ..Default::default()
        };
        a.add(&b);
        assert_eq!(a.entities_extracted, 7);
        assert_eq!(a.edges_added, 3);
        assert_eq!(a.entity_extract_duration, Duration::from_millis(25));
    }

    #[test]
    fn stats_add_saturates_on_overflow() {
        let mut a = ResolutionStats {
            entities_extracted: u64::MAX - 1,
            ..Default::default()
        };
        let b = ResolutionStats {
            entities_extracted: 5,
            ..Default::default()
        };
        a.add(&b);
        assert_eq!(a.entities_extracted, u64::MAX);
    }

    #[test]
    fn total_duration_sums_all_stages() {
        let s = ResolutionStats {
            entity_extract_duration: Duration::from_millis(1),
            edge_extract_duration: Duration::from_millis(2),
            resolve_duration: Duration::from_millis(4),
            persist_duration: Duration::from_millis(8),
            ..Default::default()
        };
        assert_eq!(s.total_duration(), Duration::from_millis(15));
    }

    #[test]
    fn config_serializes_roundtrip() {
        let c = ResolutionConfig::default();
        let j = serde_json::to_string(&c).unwrap();
        let c2: ResolutionConfig = serde_json::from_str(&j).unwrap();
        assert_eq!(c, c2);
    }
}
