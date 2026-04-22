//! Meta-Cognition Loop — self-monitoring for the memory system.
//!
//! Tracks recall accuracy, synthesis quality, and channel effectiveness over time.
//! Generates parameter adjustment suggestions based on observed patterns.
//!
//! TASK-15: Exploratory — metrics collection + suggestion generation.
//! Parameters are NOT auto-applied; the caller decides whether to act on suggestions.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// Maximum events kept in the in-memory rolling window.
const ROLLING_WINDOW_SIZE: usize = 500;

// ── Event Types ───────────────────────────────────────────────

/// A recorded recall event with timing and quality signals.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecallEvent {
    /// Unix timestamp (seconds)
    pub timestamp: i64,
    /// The query string
    pub query: String,
    /// Number of results returned
    pub result_count: usize,
    /// Mean confidence of returned results (0.0–1.0)
    pub mean_confidence: f64,
    /// Max confidence among results
    pub max_confidence: f64,
    /// Wall-clock latency in milliseconds
    pub latency_ms: u64,
    /// Whether embedding was used (vs FTS fallback)
    pub used_embedding: bool,
    /// Optional external feedback score (set later via feedback_event)
    pub feedback_score: Option<f64>,
}

/// A recorded consolidation/synthesis event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SynthesisEvent {
    /// Unix timestamp (seconds)
    pub timestamp: i64,
    /// Number of clusters found
    pub clusters_found: usize,
    /// Number of insights created
    pub insights_created: usize,
    /// Duration in milliseconds
    pub duration_ms: u64,
    /// Errors encountered
    pub error_count: usize,
}

// ── Metrics & Suggestions ─────────────────────────────────────

/// Aggregated metrics from the rolling window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetaCognitionReport {
    /// Total recall events in window
    pub recall_count: usize,
    /// Average result count per recall
    pub avg_result_count: f64,
    /// Average confidence across all recalls
    pub avg_confidence: f64,
    /// Fraction of recalls that returned 0 results
    pub empty_recall_rate: f64,
    /// Average recall latency (ms)
    pub avg_latency_ms: f64,
    /// P95 recall latency (ms)
    pub p95_latency_ms: f64,
    /// Fraction of recalls using embedding (vs FTS fallback)
    pub embedding_utilization: f64,
    /// Average feedback score (only from events that have feedback)
    pub avg_feedback_score: Option<f64>,
    /// Total synthesis events in window
    pub synthesis_count: usize,
    /// Average insights per synthesis run
    pub avg_insights_per_synthesis: f64,
    /// Synthesis error rate
    pub synthesis_error_rate: f64,
}

/// A suggested parameter adjustment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParameterSuggestion {
    /// Which config parameter to adjust
    pub parameter: String,
    /// Current value (as string for display)
    pub current_value: String,
    /// Suggested new value
    pub suggested_value: String,
    /// Why this adjustment is recommended
    pub reason: String,
    /// Confidence in this suggestion (0.0–1.0)
    pub confidence: f64,
}

// ── Tracker ───────────────────────────────────────────────────

/// Self-monitoring tracker for the memory system.
///
/// Maintains a rolling window of events in memory and persists all events
/// to SQLite for long-term analysis. Generates metrics and parameter
/// adjustment suggestions on demand.
pub struct MetaCognitionTracker {
    /// In-memory rolling window of recent recall events
    recall_window: VecDeque<RecallEvent>,
    /// In-memory rolling window of recent synthesis events
    synthesis_window: VecDeque<SynthesisEvent>,
    /// Counter for generating recall event IDs for feedback correlation
    recall_counter: u64,
}

impl MetaCognitionTracker {
    /// Create a new tracker, initializing the SQLite table if needed.
    ///
    /// The table is created in the same database as the memory system.
    pub fn new(conn: &Connection) -> Result<Self, Box<dyn std::error::Error>> {
        Self::ensure_table(conn)?;
        Ok(Self {
            recall_window: VecDeque::with_capacity(ROLLING_WINDOW_SIZE),
            synthesis_window: VecDeque::with_capacity(ROLLING_WINDOW_SIZE),
            recall_counter: 0,
        })
    }

    /// Create the metacognition_events table if it doesn't exist.
    fn ensure_table(conn: &Connection) -> Result<(), Box<dyn std::error::Error>> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS metacognition_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                event_type TEXT NOT NULL,
                timestamp INTEGER NOT NULL,
                query TEXT,
                result_count INTEGER,
                mean_confidence REAL,
                max_confidence REAL,
                latency_ms INTEGER,
                used_embedding INTEGER,
                feedback_score REAL,
                clusters_found INTEGER,
                insights_created INTEGER,
                error_count INTEGER,
                duration_ms INTEGER
            );
            CREATE INDEX IF NOT EXISTS idx_metacog_type_ts
                ON metacognition_events(event_type, timestamp);",
        )?;
        Ok(())
    }

    /// Record a recall event.
    ///
    /// Returns a recall_id that can be used with `feedback_event()` to
    /// retroactively score this recall.
    pub fn record_recall(
        &mut self,
        conn: &Connection,
        event: RecallEvent,
    ) -> Result<u64, Box<dyn std::error::Error>> {
        self.recall_counter += 1;
        let recall_id = self.recall_counter;

        // Persist to SQLite
        conn.execute(
            "INSERT INTO metacognition_events
                (event_type, timestamp, query, result_count, mean_confidence,
                 max_confidence, latency_ms, used_embedding)
             VALUES ('recall', ?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                event.timestamp,
                event.query,
                event.result_count,
                event.mean_confidence,
                event.max_confidence,
                event.latency_ms,
                event.used_embedding as i32,
            ],
        )?;

        // Add to rolling window
        if self.recall_window.len() >= ROLLING_WINDOW_SIZE {
            self.recall_window.pop_front();
        }
        self.recall_window.push_back(event);

        Ok(recall_id)
    }

    /// Record a synthesis/consolidation event.
    pub fn record_synthesis(
        &mut self,
        conn: &Connection,
        event: SynthesisEvent,
    ) -> Result<(), Box<dyn std::error::Error>> {
        conn.execute(
            "INSERT INTO metacognition_events
                (event_type, timestamp, clusters_found, insights_created,
                 duration_ms, error_count)
             VALUES ('synthesis', ?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                event.timestamp,
                event.clusters_found,
                event.insights_created,
                event.duration_ms,
                event.error_count,
            ],
        )?;

        if self.synthesis_window.len() >= ROLLING_WINDOW_SIZE {
            self.synthesis_window.pop_front();
        }
        self.synthesis_window.push_back(event);

        Ok(())
    }

    /// Attach external feedback to a recent recall event.
    ///
    /// `feedback_score`: 0.0 (useless) to 1.0 (perfect recall).
    /// Looks for the most recent recall event in the window and updates it.
    pub fn feedback_event(
        &mut self,
        conn: &Connection,
        feedback_score: f64,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        // Find most recent recall without feedback and attach
        if let Some(event) = self
            .recall_window
            .iter_mut()
            .rev()
            .find(|e| e.feedback_score.is_none())
        {
            event.feedback_score = Some(feedback_score);

            // Also update in SQLite (last unfeedback'd row)
            conn.execute(
                "UPDATE metacognition_events
                 SET feedback_score = ?1
                 WHERE id = (
                     SELECT id FROM metacognition_events
                     WHERE event_type = 'recall' AND feedback_score IS NULL
                     ORDER BY id DESC LIMIT 1
                 )",
                rusqlite::params![feedback_score],
            )?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Generate a metrics report from the rolling window.
    pub fn report(&self) -> MetaCognitionReport {
        let recall_count = self.recall_window.len();

        // Recall metrics
        let (avg_result_count, avg_confidence, empty_recall_rate, avg_latency_ms, p95_latency_ms, embedding_utilization, avg_feedback_score) =
            if recall_count == 0 {
                (0.0, 0.0, 0.0, 0.0, 0.0, 0.0, None)
            } else {
                let n = recall_count as f64;
                let sum_results: usize = self.recall_window.iter().map(|e| e.result_count).sum();
                let sum_conf: f64 = self.recall_window.iter().map(|e| e.mean_confidence).sum();
                let empty_count = self.recall_window.iter().filter(|e| e.result_count == 0).count();
                let sum_latency: u64 = self.recall_window.iter().map(|e| e.latency_ms).sum();
                let emb_count = self.recall_window.iter().filter(|e| e.used_embedding).count();

                // P95 latency
                let mut latencies: Vec<u64> = self.recall_window.iter().map(|e| e.latency_ms).collect();
                latencies.sort_unstable();
                let p95_idx = ((recall_count as f64) * 0.95).ceil() as usize;
                let p95 = latencies.get(p95_idx.min(recall_count - 1)).copied().unwrap_or(0);

                // Feedback average (only events with feedback)
                let feedback_events: Vec<f64> = self
                    .recall_window
                    .iter()
                    .filter_map(|e| e.feedback_score)
                    .collect();
                let avg_fb = if feedback_events.is_empty() {
                    None
                } else {
                    Some(feedback_events.iter().sum::<f64>() / feedback_events.len() as f64)
                };

                (
                    sum_results as f64 / n,
                    sum_conf / n,
                    empty_count as f64 / n,
                    sum_latency as f64 / n,
                    p95 as f64,
                    emb_count as f64 / n,
                    avg_fb,
                )
            };

        // Synthesis metrics
        let synthesis_count = self.synthesis_window.len();
        let (avg_insights, synth_error_rate) = if synthesis_count == 0 {
            (0.0, 0.0)
        } else {
            let sn = synthesis_count as f64;
            let sum_insights: usize = self.synthesis_window.iter().map(|e| e.insights_created).sum();
            let err_count = self.synthesis_window.iter().filter(|e| e.error_count > 0).count();
            (sum_insights as f64 / sn, err_count as f64 / sn)
        };

        MetaCognitionReport {
            recall_count,
            avg_result_count,
            avg_confidence,
            empty_recall_rate,
            avg_latency_ms,
            p95_latency_ms,
            embedding_utilization,
            avg_feedback_score,
            synthesis_count,
            avg_insights_per_synthesis: avg_insights,
            synthesis_error_rate: synth_error_rate,
        }
    }

    /// Analyze patterns and suggest parameter adjustments.
    ///
    /// Examines rolling-window metrics to recommend changes to:
    /// - `actr_decay` — if recall confidence is consistently low/high
    /// - `promote_threshold` — if too many/few memories are being promoted
    /// - `forget_threshold` — if empty recall rate is too high
    /// - `hebbian_threshold` — if recall diversity is low
    ///
    /// Returns suggestions sorted by confidence (highest first).
    pub fn parameter_suggestions(
        &self,
        config: &crate::config::MemoryConfig,
    ) -> Vec<ParameterSuggestion> {
        let report = self.report();
        let mut suggestions = Vec::new();

        // Need minimum data to make suggestions
        if report.recall_count < 10 {
            return suggestions;
        }

        // 1. High empty recall rate → lower forget_threshold to retain more memories
        if report.empty_recall_rate > 0.3 {
            let current = config.forget_threshold;
            let suggested = (current * 0.8).max(0.01);
            suggestions.push(ParameterSuggestion {
                parameter: "forget_threshold".into(),
                current_value: format!("{:.3}", current),
                suggested_value: format!("{:.3}", suggested),
                reason: format!(
                    "Empty recall rate is {:.0}% (>30%). Lowering forget_threshold retains more memories.",
                    report.empty_recall_rate * 100.0
                ),
                confidence: (report.empty_recall_rate - 0.3).min(0.5) + 0.3,
            });
        }

        // 2. Low average confidence → slow down ACT-R decay to keep memories accessible longer
        if report.avg_confidence < 0.3 {
            let current = config.actr_decay;
            let suggested = (current * 0.85).max(0.1);
            suggestions.push(ParameterSuggestion {
                parameter: "actr_decay".into(),
                current_value: format!("{:.3}", current),
                suggested_value: format!("{:.3}", suggested),
                reason: format!(
                    "Average recall confidence is {:.2} (<0.3). Reducing ACT-R decay keeps memories active longer.",
                    report.avg_confidence
                ),
                confidence: 0.5,
            });
        }

        // 3. High average confidence → could tighten promote_threshold (more selective promotion)
        if report.avg_confidence > 0.8 && report.recall_count >= 50 {
            let current = config.promote_threshold;
            let suggested = (current * 1.2).min(0.5);
            if (suggested - current).abs() > 0.01 {
                suggestions.push(ParameterSuggestion {
                    parameter: "promote_threshold".into(),
                    current_value: format!("{:.3}", current),
                    suggested_value: format!("{:.3}", suggested),
                    reason: format!(
                        "Recall confidence averages {:.2} (>0.8). Can raise promote_threshold for more selective promotion.",
                        report.avg_confidence
                    ),
                    confidence: 0.4,
                });
            }
        }

        // 4. Feedback score trending low → decay too fast
        if let Some(avg_fb) = report.avg_feedback_score {
            if avg_fb < 0.4 {
                let current = config.mu1;
                let suggested = (current * 0.8).max(0.01);
                suggestions.push(ParameterSuggestion {
                    parameter: "mu1".into(),
                    current_value: format!("{:.3}", current),
                    suggested_value: format!("{:.3}", suggested),
                    reason: format!(
                        "Average feedback score is {:.2} (<0.4). Reducing working-memory decay rate (mu1) may help.",
                        avg_fb
                    ),
                    confidence: 0.6,
                });
            }
        }

        // 5. High synthesis error rate → consolidation parameters may be too aggressive
        if report.synthesis_count >= 3 && report.synthesis_error_rate > 0.5 {
            suggestions.push(ParameterSuggestion {
                parameter: "consolidation_bonus".into(),
                current_value: format!("{:.3}", config.consolidation_bonus),
                suggested_value: format!("{:.3}", config.consolidation_bonus * 0.8),
                reason: format!(
                    "Synthesis error rate is {:.0}% (>50%). Reducing consolidation_bonus may improve stability.",
                    report.synthesis_error_rate * 100.0
                ),
                confidence: 0.35,
            });
        }

        // 6. Embedding not being used → degrade actr_weight to compensate
        if report.embedding_utilization < 0.5 && report.recall_count >= 20 {
            suggestions.push(ParameterSuggestion {
                parameter: "fts_weight".into(),
                current_value: format!("{:.3}", config.fts_weight),
                suggested_value: format!("{:.3}", (config.fts_weight * 1.3).min(0.5)),
                reason: format!(
                    "Embedding is used in only {:.0}% of recalls. Consider increasing FTS weight as fallback.",
                    report.embedding_utilization * 100.0
                ),
                confidence: 0.4,
            });
        }

        // Sort by confidence descending
        suggestions.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap());
        suggestions
    }

    /// Load historical events from SQLite into the rolling window.
    ///
    /// Call this on startup to pre-populate the window from persisted data.
    pub fn load_history(
        &mut self,
        conn: &Connection,
    ) -> Result<usize, Box<dyn std::error::Error>> {
        let mut count = 0;

        // Load recent recall events
        let mut stmt = conn.prepare(
            "SELECT timestamp, query, result_count, mean_confidence, max_confidence,
                    latency_ms, used_embedding, feedback_score
             FROM metacognition_events
             WHERE event_type = 'recall'
             ORDER BY id DESC LIMIT ?1",
        )?;
        let recall_rows = stmt.query_map(
            rusqlite::params![ROLLING_WINDOW_SIZE],
            |row| {
                Ok(RecallEvent {
                    timestamp: row.get(0)?,
                    query: row.get::<_, String>(1).unwrap_or_default(),
                    result_count: row.get::<_, i64>(2).unwrap_or(0) as usize,
                    mean_confidence: row.get(3).unwrap_or(0.0),
                    max_confidence: row.get(4).unwrap_or(0.0),
                    latency_ms: row.get::<_, i64>(5).unwrap_or(0) as u64,
                    used_embedding: row.get::<_, i32>(6).unwrap_or(0) != 0,
                    feedback_score: row.get(7).ok(),
                })
            },
        )?;
        for row in recall_rows {
            if let Ok(event) = row {
                self.recall_window.push_front(event);
                count += 1;
            }
        }

        // Load recent synthesis events
        let mut stmt = conn.prepare(
            "SELECT timestamp, clusters_found, insights_created, duration_ms, error_count
             FROM metacognition_events
             WHERE event_type = 'synthesis'
             ORDER BY id DESC LIMIT ?1",
        )?;
        let synth_rows = stmt.query_map(
            rusqlite::params![ROLLING_WINDOW_SIZE],
            |row| {
                Ok(SynthesisEvent {
                    timestamp: row.get(0)?,
                    clusters_found: row.get::<_, i64>(1).unwrap_or(0) as usize,
                    insights_created: row.get::<_, i64>(2).unwrap_or(0) as usize,
                    duration_ms: row.get::<_, i64>(3).unwrap_or(0) as u64,
                    error_count: row.get::<_, i64>(4).unwrap_or(0) as usize,
                })
            },
        )?;
        for row in synth_rows {
            if let Ok(event) = row {
                self.synthesis_window.push_front(event);
                count += 1;
            }
        }

        Ok(count)
    }

    /// Get the number of events in the rolling windows.
    pub fn window_sizes(&self) -> (usize, usize) {
        (self.recall_window.len(), self.synthesis_window.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        // Minimal schema — just the metacognition table
        MetaCognitionTracker::ensure_table(&conn).unwrap();
        conn
    }

    #[test]
    fn test_record_and_report_recall() {
        let conn = setup_db();
        let mut tracker = MetaCognitionTracker::new(&conn).unwrap();

        // Record 20 recall events with varying quality
        for i in 0..20 {
            let event = RecallEvent {
                timestamp: 1000 + i,
                query: format!("query_{}", i),
                result_count: if i % 5 == 0 { 0 } else { 3 },
                mean_confidence: 0.5 + (i as f64 * 0.02),
                max_confidence: 0.8,
                latency_ms: 10 + i as u64 * 2,
                used_embedding: i % 2 == 0,
                feedback_score: None,
            };
            tracker.record_recall(&conn, event).unwrap();
        }

        let report = tracker.report();
        assert_eq!(report.recall_count, 20);
        assert!(report.avg_result_count > 0.0);
        assert!(report.avg_confidence > 0.0);
        // 4 out of 20 are empty (i=0,5,10,15)
        assert!((report.empty_recall_rate - 0.2).abs() < 0.01);
        assert_eq!(report.embedding_utilization, 0.5); // every other one
        assert!(report.avg_latency_ms > 0.0);
        assert!(report.p95_latency_ms >= report.avg_latency_ms);
    }

    #[test]
    fn test_record_and_report_synthesis() {
        let conn = setup_db();
        let mut tracker = MetaCognitionTracker::new(&conn).unwrap();

        tracker
            .record_synthesis(
                &conn,
                SynthesisEvent {
                    timestamp: 2000,
                    clusters_found: 5,
                    insights_created: 2,
                    duration_ms: 500,
                    error_count: 0,
                },
            )
            .unwrap();

        tracker
            .record_synthesis(
                &conn,
                SynthesisEvent {
                    timestamp: 3000,
                    clusters_found: 3,
                    insights_created: 1,
                    duration_ms: 300,
                    error_count: 1,
                },
            )
            .unwrap();

        let report = tracker.report();
        assert_eq!(report.synthesis_count, 2);
        assert!((report.avg_insights_per_synthesis - 1.5).abs() < 0.01);
        assert!((report.synthesis_error_rate - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_feedback_event() {
        let conn = setup_db();
        let mut tracker = MetaCognitionTracker::new(&conn).unwrap();

        tracker
            .record_recall(
                &conn,
                RecallEvent {
                    timestamp: 1000,
                    query: "test".into(),
                    result_count: 3,
                    mean_confidence: 0.6,
                    max_confidence: 0.9,
                    latency_ms: 15,
                    used_embedding: true,
                    feedback_score: None,
                },
            )
            .unwrap();

        // Attach feedback
        let attached = tracker.feedback_event(&conn, 0.85).unwrap();
        assert!(attached);

        let report = tracker.report();
        assert_eq!(report.avg_feedback_score, Some(0.85));

        // No more events without feedback → returns false
        let attached2 = tracker.feedback_event(&conn, 0.5).unwrap();
        assert!(!attached2);
    }

    #[test]
    fn test_parameter_suggestions_empty_recalls() {
        let conn = setup_db();
        let mut tracker = MetaCognitionTracker::new(&conn).unwrap();

        let config = crate::config::MemoryConfig::default();

        // Not enough data → no suggestions
        let suggestions = tracker.parameter_suggestions(&config);
        assert!(suggestions.is_empty());

        // Add 15 empty recalls → should trigger forget_threshold suggestion
        for i in 0..15 {
            tracker
                .record_recall(
                    &conn,
                    RecallEvent {
                        timestamp: 1000 + i,
                        query: format!("q{}", i),
                        result_count: 0,
                        mean_confidence: 0.0,
                        max_confidence: 0.0,
                        latency_ms: 5,
                        used_embedding: true,
                        feedback_score: None,
                    },
                )
                .unwrap();
        }

        let suggestions = tracker.parameter_suggestions(&config);
        assert!(!suggestions.is_empty());
        assert!(suggestions.iter().any(|s| s.parameter == "forget_threshold"));
    }

    #[test]
    fn test_parameter_suggestions_low_confidence() {
        let conn = setup_db();
        let mut tracker = MetaCognitionTracker::new(&conn).unwrap();

        let config = crate::config::MemoryConfig::default();

        // 15 recalls with low confidence
        for i in 0..15 {
            tracker
                .record_recall(
                    &conn,
                    RecallEvent {
                        timestamp: 1000 + i,
                        query: format!("q{}", i),
                        result_count: 2,
                        mean_confidence: 0.15,
                        max_confidence: 0.25,
                        latency_ms: 10,
                        used_embedding: true,
                        feedback_score: None,
                    },
                )
                .unwrap();
        }

        let suggestions = tracker.parameter_suggestions(&config);
        assert!(suggestions.iter().any(|s| s.parameter == "actr_decay"));
    }

    #[test]
    fn test_rolling_window_cap() {
        let conn = setup_db();
        let mut tracker = MetaCognitionTracker::new(&conn).unwrap();

        // Overflow the window
        for i in 0..(ROLLING_WINDOW_SIZE + 50) {
            tracker
                .record_recall(
                    &conn,
                    RecallEvent {
                        timestamp: i as i64,
                        query: format!("q{}", i),
                        result_count: 1,
                        mean_confidence: 0.5,
                        max_confidence: 0.7,
                        latency_ms: 10,
                        used_embedding: true,
                        feedback_score: None,
                    },
                )
                .unwrap();
        }

        assert_eq!(tracker.recall_window.len(), ROLLING_WINDOW_SIZE);
        // SQLite has all events
        let db_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM metacognition_events WHERE event_type = 'recall'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(db_count as usize, ROLLING_WINDOW_SIZE + 50);
    }

    #[test]
    fn test_load_history() {
        let conn = setup_db();

        // Insert some events directly into SQLite
        for i in 0..5 {
            conn.execute(
                "INSERT INTO metacognition_events
                    (event_type, timestamp, query, result_count, mean_confidence,
                     max_confidence, latency_ms, used_embedding)
                 VALUES ('recall', ?1, ?2, 2, 0.6, 0.8, 12, 1)",
                rusqlite::params![1000 + i, format!("hist_q{}", i)],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO metacognition_events
                (event_type, timestamp, clusters_found, insights_created, duration_ms, error_count)
             VALUES ('synthesis', 2000, 4, 2, 400, 0)",
            [],
        )
        .unwrap();

        // New tracker should start empty, then load
        let mut tracker = MetaCognitionTracker::new(&conn).unwrap();
        assert_eq!(tracker.window_sizes(), (0, 0));

        let loaded = tracker.load_history(&conn).unwrap();
        assert_eq!(loaded, 6); // 5 recall + 1 synthesis
        assert_eq!(tracker.window_sizes(), (5, 1));

        let report = tracker.report();
        assert_eq!(report.recall_count, 5);
        assert_eq!(report.synthesis_count, 1);
    }

    #[test]
    fn test_empty_report() {
        let conn = setup_db();
        let tracker = MetaCognitionTracker::new(&conn).unwrap();
        let report = tracker.report();
        assert_eq!(report.recall_count, 0);
        assert_eq!(report.avg_confidence, 0.0);
        assert_eq!(report.synthesis_count, 0);
        assert!(report.avg_feedback_score.is_none());
    }

    #[test]
    fn test_suggestions_sorted_by_confidence() {
        let conn = setup_db();
        let mut tracker = MetaCognitionTracker::new(&conn).unwrap();
        let config = crate::config::MemoryConfig::default();

        // Create conditions that trigger multiple suggestions:
        // - all empty recalls (high empty rate + low confidence)
        // - no embedding
        for i in 0..25 {
            tracker
                .record_recall(
                    &conn,
                    RecallEvent {
                        timestamp: 1000 + i,
                        query: format!("q{}", i),
                        result_count: 0,
                        mean_confidence: 0.1,
                        max_confidence: 0.2,
                        latency_ms: 10,
                        used_embedding: false,
                        feedback_score: None,
                    },
                )
                .unwrap();
        }

        let suggestions = tracker.parameter_suggestions(&config);
        assert!(suggestions.len() >= 2);
        // Verify sorted by confidence descending
        for w in suggestions.windows(2) {
            assert!(w[0].confidence >= w[1].confidence);
        }
    }
}
