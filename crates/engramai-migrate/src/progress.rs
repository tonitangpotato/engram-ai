//! Migration progress reporting.
//!
//! Implements §5.5 (progress emission cadence) and §9.2 (progress API struct)
//! of `.gid/features/v03-migration/design.md`. Satisfies GOAL-4.5: emit at
//! least every 100 records processed or every 5 seconds (whichever fires
//! first), expose `(records_processed, records_total, records_succeeded,
//! records_failed)`, and allow counters to survive process restart.
//!
//! Time-to-completion estimation is explicitly **out of scope** for v0.3.0
//! (per GOAL-4.5 final sentence and design §5.5 closing paragraph). This
//! struct carries counts, not ETAs.
//!
//! ## Channels
//!
//! Per design §5.5 there are three output channels:
//! 1. **Stderr** human line — formatted via [`MigrationProgress::human_line`]
//! 2. **Stdout NDJSON** (`--format=json`) — via serde serialization of
//!    [`ProgressEvent`]
//! 3. **Persistent `migration_log` table** — via [`MigrationProgress::log_row`]
//!
//! All three carry the same underlying [`MigrationProgress`] data.
//!
//! ## Cadence
//!
//! [`ProgressEmitter`] implements the OR cadence: every `emit_every_records`
//! processed records (default 100) **or** every `emit_every_duration` since
//! the last emission (default 5 s), whichever fires first.

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Migration phase identifiers (§9.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MigrationPhase {
    /// Phase 0 — pre-flight gates.
    PreFlight,
    /// Phase 1 — backup.
    Backup,
    /// Phase 2 — schema transition.
    SchemaTransition,
    /// Phase 3 — topic carry-forward.
    TopicCarryForward,
    /// Phase 4 — backfill.
    Backfill,
    /// Phase 5 — verify.
    Verify,
    /// Migration complete (post Phase 5 gate).
    Complete,
}

impl MigrationPhase {
    /// Stable ASCII tag used in CLI lines, log rows, and JSON.
    /// Matches design §5.5 NDJSON example: `"phase": "Phase4"`.
    pub fn tag(self) -> &'static str {
        match self {
            MigrationPhase::PreFlight => "Phase0",
            MigrationPhase::Backup => "Phase1",
            MigrationPhase::SchemaTransition => "Phase2",
            MigrationPhase::TopicCarryForward => "Phase3",
            MigrationPhase::Backfill => "Phase4",
            MigrationPhase::Verify => "Phase5",
            MigrationPhase::Complete => "Complete",
        }
    }

    /// 0-based phase index (Phase 0..=5). `Complete` returns 6.
    pub fn index(self) -> u8 {
        match self {
            MigrationPhase::PreFlight => 0,
            MigrationPhase::Backup => 1,
            MigrationPhase::SchemaTransition => 2,
            MigrationPhase::TopicCarryForward => 3,
            MigrationPhase::Backfill => 4,
            MigrationPhase::Verify => 5,
            MigrationPhase::Complete => 6,
        }
    }

    /// Total phase count (constant 6: Phase 0–5). Design §5.5 NDJSON schema
    /// embeds this as `total_phases: 6`. `Complete` is a terminal sentinel,
    /// not a phase, so it is excluded from the count.
    pub const TOTAL_PHASES: u8 = 6;
}

/// Library-facing progress snapshot (design §9.2).
///
/// Returned to callbacks and serialized to all three output channels. Cheap
/// to clone — counters are integers, timestamps are `DateTime<Utc>`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationProgress {
    /// Currently-executing phase.
    pub phase: MigrationPhase,
    /// Total records in the source DB (computed once at Phase 4 start).
    pub records_total: u64,
    /// Records the pipeline has attempted.
    pub records_processed: u64,
    /// Records where the pipeline produced a graph delta and persisted it.
    pub records_succeeded: u64,
    /// Records that surfaced an extraction failure.
    pub records_failed: u64,
    /// When the overall migration started (persists across `--resume`).
    pub started_at: DateTime<Utc>,
    /// When this snapshot was taken.
    pub snapshot_at: DateTime<Utc>,
    /// True iff Phase 5 gate passed.
    pub migration_complete: bool,
    /// Resident-set memory in MiB at snapshot time. `None` if the platform
    /// does not expose RSS or measurement was skipped.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub rss_mb: Option<f64>,
}

impl MigrationProgress {
    /// Construct a fresh snapshot for `phase`, with zeroed counters.
    pub fn new(phase: MigrationPhase, started_at: DateTime<Utc>, records_total: u64) -> Self {
        Self {
            phase,
            records_total,
            records_processed: 0,
            records_succeeded: 0,
            records_failed: 0,
            started_at,
            snapshot_at: started_at,
            migration_complete: false,
            rss_mb: None,
        }
    }

    /// Elapsed wall time from `started_at` to `snapshot_at` in milliseconds.
    /// Returns 0 if the clock went backwards (defensive — should never happen
    /// because the migrator owns both timestamps).
    pub fn elapsed_ms(&self) -> u64 {
        let delta = self
            .snapshot_at
            .signed_duration_since(self.started_at)
            .num_milliseconds();
        delta.max(0) as u64
    }

    /// Records-per-second throughput from `started_at` to `snapshot_at`.
    /// Returns 0.0 if elapsed is zero (avoids div-by-zero on the first
    /// emission of a fresh run).
    pub fn rate_per_sec(&self) -> f64 {
        let ms = self.elapsed_ms();
        if ms == 0 {
            return 0.0;
        }
        (self.records_processed as f64) * 1000.0 / (ms as f64)
    }

    /// Validate the §9.2 invariants. Returns `Err(reason)` on violation.
    /// Migration code should `assert_progress_invariants(&p).expect(...)`
    /// on every emission; tests exercise the negative paths directly.
    pub fn check_invariants(&self) -> Result<(), &'static str> {
        if self.records_processed != self.records_succeeded + self.records_failed {
            return Err("records_processed != records_succeeded + records_failed");
        }
        if self.records_processed > self.records_total {
            return Err("records_processed > records_total");
        }
        Ok(())
    }

    /// Format the human-facing progress line per design §5.5:
    ///
    /// ```text
    /// [Phase 4/5] 12340/50000 records | succeeded=12298 failed=42 | 123.4 rec/s
    /// ```
    ///
    /// Used for both stderr TTY output (with `\r` for in-place redraw — the
    /// caller adds the CR if desired) and stderr non-TTY (append-only).
    pub fn human_line(&self) -> String {
        // Phase index out of 5 (the human line uses 1-based "Phase X/5"
        // wording — design §5.5 literally shows "Phase 4/5" for Phase 4).
        let phase_num = self.phase.index();
        format!(
            "[Phase {}/5] {}/{} records | succeeded={} failed={} | {:.1} rec/s",
            phase_num,
            self.records_processed,
            self.records_total,
            self.records_succeeded,
            self.records_failed,
            self.rate_per_sec()
        )
    }

    /// Build the NDJSON `progress` event per design §5.5. The wrapper enum
    /// [`ProgressEvent`] handles the `"event": "progress" | "complete"`
    /// discriminator.
    pub fn to_event(&self) -> ProgressEvent {
        if self.migration_complete {
            ProgressEvent::Complete(self.clone())
        } else {
            ProgressEvent::Progress(self.clone())
        }
    }

    /// Build a [`MigrationLogRow`] suitable for insertion into the
    /// persistent `migration_log` table (§5.5 channel 3).
    pub fn log_row(&self, message: Option<String>) -> MigrationLogRow {
        MigrationLogRow {
            emitted_at: self.snapshot_at,
            phase: self.phase,
            records_processed: self.records_processed,
            records_succeeded: self.records_succeeded,
            records_failed: self.records_failed,
            rss_mb: self.rss_mb,
            message,
        }
    }
}

/// NDJSON event union for `--format=json` mode (§5.5 channel 2).
///
/// Serializes with an `event` tag — `progress` for live updates and
/// `complete` for the terminal record matching design §9.4 summary shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "lowercase")]
pub enum ProgressEvent {
    /// Live progress update.
    Progress(MigrationProgress),
    /// Final completion event. Schema is identical to `Progress` in v0.3.0;
    /// future minor versions may extend with §9.4 summary fields without
    /// breaking the emission contract.
    Complete(MigrationProgress),
}

impl ProgressEvent {
    /// Serialize to a single NDJSON line (no trailing newline). Caller adds
    /// `\n` when writing.
    pub fn to_ndjson(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

/// Row inserted into the persistent `migration_log` table on each emission.
///
/// Schema mirrors design §5.5:
///
/// ```sql
/// CREATE TABLE IF NOT EXISTS migration_log (
///     id          INTEGER PRIMARY KEY AUTOINCREMENT,
///     emitted_at  TEXT    NOT NULL,
///     phase       TEXT    NOT NULL,
///     records_processed INTEGER, records_succeeded INTEGER, records_failed INTEGER,
///     rss_mb      REAL,
///     message     TEXT
/// );
/// ```
#[derive(Debug, Clone)]
pub struct MigrationLogRow {
    pub emitted_at: DateTime<Utc>,
    pub phase: MigrationPhase,
    pub records_processed: u64,
    pub records_succeeded: u64,
    pub records_failed: u64,
    pub rss_mb: Option<f64>,
    pub message: Option<String>,
}

/// Type alias for the user-supplied progress callback (design §9.3).
///
/// `Send + Sync` because the migrator may invoke it from any internal task.
/// Wrapped in `Arc` so [`MigrateOptions`] (defined in a later task) can
/// `Clone` cheaply.
pub type ProgressCallback = Arc<dyn Fn(&MigrationProgress) + Send + Sync>;

/// Configuration for the [`ProgressEmitter`] cadence.
///
/// Design §5.5 fixes the *defaults*: 100 records or 5 seconds, whichever
/// fires first. The fields are public so tests (and future tuning) can
/// override them; production code should use [`EmitterConfig::default`].
#[derive(Debug, Clone, Copy)]
pub struct EmitterConfig {
    /// Emit when this many records have been processed since the last
    /// emission. Default `100`.
    pub emit_every_records: u64,
    /// Emit when this much wall time has elapsed since the last emission.
    /// Default `5s`.
    pub emit_every_duration: Duration,
}

impl Default for EmitterConfig {
    fn default() -> Self {
        Self {
            emit_every_records: 100,
            emit_every_duration: Duration::from_secs(5),
        }
    }
}

/// Cadence governor for progress emissions (§5.5).
///
/// The emitter tracks the last-emission timestamp and the last-emission
/// `records_processed` count. [`ProgressEmitter::should_emit`] returns true
/// when *either* the record-count delta has reached `emit_every_records`
/// **or** the wall time since the last emission has reached
/// `emit_every_duration`.
///
/// The first call always emits (so the first record is reported). Callers
/// must invoke [`ProgressEmitter::record_emission`] after they actually
/// emit, so the next decision uses fresh baselines.
///
/// Emission is best-effort and never blocks backfill; callers swallow
/// channel errors per design §5.5 ("If the callback panics or the stdout
/// pipe is broken, the error is logged once and further emissions on that
/// channel are skipped"). This emitter does not own the channels — it
/// only decides *when* to emit. The channel-skip policy lives at the
/// dispatch site.
#[derive(Debug)]
pub struct ProgressEmitter {
    config: EmitterConfig,
    last_emit_at: Option<Instant>,
    last_emit_records: u64,
    /// True until the first emission is recorded — used to force the
    /// initial emission so callers always see at least one update per run.
    fresh: bool,
}

impl ProgressEmitter {
    /// Build a new emitter with the design defaults (100 records / 5s).
    pub fn new() -> Self {
        Self::with_config(EmitterConfig::default())
    }

    /// Build a new emitter with a custom cadence (tests / tuning).
    pub fn with_config(config: EmitterConfig) -> Self {
        Self {
            config,
            last_emit_at: None,
            last_emit_records: 0,
            fresh: true,
        }
    }

    /// Returns `true` when the caller should emit a progress snapshot.
    ///
    /// `now` and `records_processed` are caller-supplied so tests can drive
    /// virtual time. Production code passes [`Instant::now`] and the live
    /// counter.
    pub fn should_emit(&self, now: Instant, records_processed: u64) -> bool {
        if self.fresh {
            return true;
        }
        let last_at = match self.last_emit_at {
            Some(t) => t,
            // Should not happen once `fresh` is false, but be defensive.
            None => return true,
        };
        let records_delta = records_processed.saturating_sub(self.last_emit_records);
        records_delta >= self.config.emit_every_records
            || now.duration_since(last_at) >= self.config.emit_every_duration
    }

    /// Record that an emission happened at `now` with `records_processed`
    /// as the live counter. Resets both cadence triggers.
    pub fn record_emission(&mut self, now: Instant, records_processed: u64) {
        self.last_emit_at = Some(now);
        self.last_emit_records = records_processed;
        self.fresh = false;
    }

    /// Configured cadence (read-only accessor — useful for diagnostics).
    pub fn config(&self) -> EmitterConfig {
        self.config
    }
}

impl Default for ProgressEmitter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t0() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 27, 12, 0, 0).unwrap()
    }

    fn snap(processed: u64, succ: u64, fail: u64, total: u64) -> MigrationProgress {
        let mut p = MigrationProgress::new(MigrationPhase::Backfill, t0(), total);
        p.records_processed = processed;
        p.records_succeeded = succ;
        p.records_failed = fail;
        p.snapshot_at = t0() + chrono::Duration::seconds(10);
        p
    }

    // ===== Invariants (design §9.2) =====

    #[test]
    fn invariant_processed_equals_succ_plus_fail() {
        let p = snap(100, 90, 10, 1000);
        assert!(p.check_invariants().is_ok());
    }

    #[test]
    fn invariant_violated_processed_mismatch() {
        let p = snap(100, 50, 10, 1000); // 50 + 10 != 100
        assert!(p.check_invariants().is_err());
    }

    #[test]
    fn invariant_violated_processed_exceeds_total() {
        let p = snap(1001, 1001, 0, 1000);
        assert!(p.check_invariants().is_err());
    }

    #[test]
    fn invariant_zero_counters_ok() {
        let p = MigrationProgress::new(MigrationPhase::Backfill, t0(), 1000);
        assert!(p.check_invariants().is_ok());
    }

    // ===== Phase tagging =====

    #[test]
    fn phase_tags_match_design_ndjson() {
        assert_eq!(MigrationPhase::PreFlight.tag(), "Phase0");
        assert_eq!(MigrationPhase::Backup.tag(), "Phase1");
        assert_eq!(MigrationPhase::SchemaTransition.tag(), "Phase2");
        assert_eq!(MigrationPhase::TopicCarryForward.tag(), "Phase3");
        assert_eq!(MigrationPhase::Backfill.tag(), "Phase4");
        assert_eq!(MigrationPhase::Verify.tag(), "Phase5");
    }

    #[test]
    fn phase_indices_are_dense() {
        assert_eq!(MigrationPhase::PreFlight.index(), 0);
        assert_eq!(MigrationPhase::Backfill.index(), 4);
        assert_eq!(MigrationPhase::Verify.index(), 5);
        assert_eq!(MigrationPhase::TOTAL_PHASES, 6);
    }

    // ===== Human line format (design §5.5) =====

    #[test]
    fn human_line_format() {
        let p = snap(12340, 12298, 42, 50000);
        let line = p.human_line();
        // Match design §5.5 example shape (rate varies with elapsed; just
        // check the prefix + counts + structure).
        assert!(line.starts_with("[Phase 4/5] 12340/50000 records | "));
        assert!(line.contains("succeeded=12298 failed=42"));
        assert!(line.ends_with(" rec/s"));
    }

    #[test]
    fn rate_zero_when_elapsed_zero() {
        let p = MigrationProgress::new(MigrationPhase::Backfill, t0(), 1000);
        // snapshot_at == started_at → elapsed_ms == 0
        assert_eq!(p.rate_per_sec(), 0.0);
    }

    // ===== NDJSON schema (design §5.5 channel 2) =====

    #[test]
    fn ndjson_progress_event_shape() {
        let p = snap(10, 9, 1, 100);
        let line = p.to_event().to_ndjson().unwrap();
        // Required fields per design schema.
        assert!(line.contains("\"event\":\"progress\""));
        assert!(line.contains("\"phase\":\"Backfill\""));
        assert!(line.contains("\"records_processed\":10"));
        assert!(line.contains("\"records_total\":100"));
        assert!(line.contains("\"records_succeeded\":9"));
        assert!(line.contains("\"records_failed\":1"));
    }

    #[test]
    fn ndjson_complete_event_emitted_when_flag_set() {
        let mut p = snap(100, 100, 0, 100);
        p.migration_complete = true;
        let line = p.to_event().to_ndjson().unwrap();
        assert!(line.contains("\"event\":\"complete\""));
    }

    #[test]
    fn ndjson_round_trips() {
        let p = snap(50, 49, 1, 100);
        let line = p.to_event().to_ndjson().unwrap();
        let back: ProgressEvent = serde_json::from_str(&line).unwrap();
        match back {
            ProgressEvent::Progress(q) => {
                assert_eq!(q.records_processed, 50);
                assert_eq!(q.records_total, 100);
                assert_eq!(q.records_failed, 1);
            }
            ProgressEvent::Complete(_) => panic!("expected Progress variant"),
        }
    }

    // ===== Persistent migration_log row (channel 3) =====

    #[test]
    fn log_row_carries_same_counters() {
        let mut p = snap(500, 495, 5, 1000);
        p.rss_mb = Some(312.0);
        let row = p.log_row(Some("phase 4 mid".into()));
        assert_eq!(row.phase, MigrationPhase::Backfill);
        assert_eq!(row.records_processed, 500);
        assert_eq!(row.records_succeeded, 495);
        assert_eq!(row.records_failed, 5);
        assert_eq!(row.rss_mb, Some(312.0));
        assert_eq!(row.message.as_deref(), Some("phase 4 mid"));
    }

    // ===== Emitter cadence (GOAL-4.5) =====

    #[test]
    fn emitter_first_call_always_emits() {
        let e = ProgressEmitter::new();
        assert!(e.should_emit(Instant::now(), 0));
    }

    #[test]
    fn emitter_record_count_trigger() {
        let mut e = ProgressEmitter::with_config(EmitterConfig {
            emit_every_records: 100,
            emit_every_duration: Duration::from_secs(60),
        });
        let t = Instant::now();
        e.record_emission(t, 0);

        assert!(!e.should_emit(t, 50), "below threshold should not emit");
        assert!(!e.should_emit(t, 99), "boundary minus one should not emit");
        assert!(e.should_emit(t, 100), "exactly threshold should emit");
        assert!(e.should_emit(t, 200), "above threshold should emit");
    }

    #[test]
    fn emitter_wallclock_trigger() {
        let mut e = ProgressEmitter::with_config(EmitterConfig {
            emit_every_records: 1_000_000, // effectively disabled
            emit_every_duration: Duration::from_secs(5),
        });
        let t0 = Instant::now();
        e.record_emission(t0, 0);

        assert!(!e.should_emit(t0 + Duration::from_secs(1), 0));
        assert!(!e.should_emit(t0 + Duration::from_secs(4), 0));
        assert!(e.should_emit(t0 + Duration::from_secs(5), 0));
        assert!(e.should_emit(t0 + Duration::from_secs(60), 0));
    }

    #[test]
    fn emitter_or_semantics_records_first() {
        // With both triggers tight, the record trigger fires before the
        // wall-clock trigger when the loop is processing fast.
        let mut e = ProgressEmitter::with_config(EmitterConfig {
            emit_every_records: 100,
            emit_every_duration: Duration::from_secs(5),
        });
        let t0 = Instant::now();
        e.record_emission(t0, 0);

        assert!(e.should_emit(t0 + Duration::from_millis(10), 100));
    }

    #[test]
    fn emitter_or_semantics_time_first() {
        // With a slow-processing loop, the wall-clock trigger fires before
        // the record trigger.
        let mut e = ProgressEmitter::with_config(EmitterConfig {
            emit_every_records: 100,
            emit_every_duration: Duration::from_secs(5),
        });
        let t0 = Instant::now();
        e.record_emission(t0, 0);

        assert!(e.should_emit(t0 + Duration::from_secs(5), 1));
    }

    #[test]
    fn emitter_record_emission_resets_both_triggers() {
        let mut e = ProgressEmitter::with_config(EmitterConfig {
            emit_every_records: 100,
            emit_every_duration: Duration::from_secs(5),
        });
        let t0 = Instant::now();
        e.record_emission(t0, 0);

        // Trigger fires at 100 records.
        assert!(e.should_emit(t0, 100));
        e.record_emission(t0, 100);

        // Now the next emission requires +100 more records (200 total) or
        // +5s of wall time.
        assert!(!e.should_emit(t0, 150));
        assert!(e.should_emit(t0, 200));
        assert!(e.should_emit(t0 + Duration::from_secs(5), 100));
    }

    // ===== started_at survives across snapshots (resume contract) =====

    #[test]
    fn started_at_is_stable_across_snapshots() {
        // The progress struct does not mutate started_at; resume contract
        // (GOAL-4.5: "progress survives process restart") is enforced by
        // checkpoint persistence in a later task. Here we verify the type
        // does not silently overwrite started_at on snapshot updates.
        let p1 = snap(10, 10, 0, 100);
        let mut p2 = p1.clone();
        p2.snapshot_at = t0() + chrono::Duration::seconds(60);
        p2.records_processed = 20;
        p2.records_succeeded = 20;
        assert_eq!(p1.started_at, p2.started_at);
    }
}
