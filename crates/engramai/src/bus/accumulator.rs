//! Emotional Accumulator — Track emotional valence trends per domain.
//!
//! Monitors emotional patterns over time and flags domains that need SOUL updates.

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

/// Convert a Unix float (seconds since epoch) to `DateTime<Utc>`.
fn f64_to_datetime(ts: f64) -> DateTime<Utc> {
    let secs = ts.floor() as i64;
    let nanos = ((ts - secs as f64) * 1_000_000_000.0).max(0.0) as u32;
    Utc.timestamp_opt(secs, nanos)
        .single()
        .unwrap_or_else(Utc::now)
}

/// Get the current time as a Unix float (seconds since epoch).
fn now_f64() -> f64 {
    let now = Utc::now();
    now.timestamp() as f64 + now.timestamp_subsec_nanos() as f64 / 1_000_000_000.0
}

/// Threshold for negative valence to trigger SOUL update suggestion.
pub const NEGATIVE_THRESHOLD: f64 = -0.5;
/// Minimum event count before suggesting SOUL updates.
pub const MIN_EVENTS_FOR_SUGGESTION: i32 = 10;

/// Emotional trend for a domain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmotionalTrend {
    /// Domain name (e.g., "coding", "communication", "research")
    pub domain: String,
    /// Running average valence (-1.0 to 1.0)
    pub valence: f64,
    /// Number of emotional events recorded
    pub count: i32,
    /// Last update timestamp
    pub last_updated: DateTime<Utc>,
}

impl EmotionalTrend {
    /// Check if this trend suggests a need for SOUL update.
    pub fn needs_soul_update(&self) -> bool {
        self.count >= MIN_EVENTS_FOR_SUGGESTION && self.valence < NEGATIVE_THRESHOLD
    }
    
    /// Describe the trend in human-readable terms.
    pub fn describe(&self) -> String {
        let sentiment = if self.valence > 0.3 {
            "positive"
        } else if self.valence < -0.3 {
            "negative"
        } else {
            "neutral"
        };
        
        format!(
            "{}: {} trend ({:.2} avg over {} events)",
            self.domain, sentiment, self.valence, self.count
        )
    }
}

/// Emotional accumulator that tracks valence trends per domain.
pub struct EmotionalAccumulator<'a> {
    conn: &'a Connection,
}

impl<'a> EmotionalAccumulator<'a> {
    /// Create a new accumulator using an existing database connection.
    pub fn new(conn: &'a Connection) -> Result<Self, rusqlite::Error> {
        Self::ensure_table(conn)?;
        Ok(Self { conn })
    }
    
    /// Ensure the emotional_trends table exists.
    fn ensure_table(conn: &Connection) -> Result<(), rusqlite::Error> {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS emotional_trends (
                domain TEXT PRIMARY KEY,
                valence REAL NOT NULL DEFAULT 0.0,
                count INTEGER NOT NULL DEFAULT 0,
                last_updated REAL NOT NULL
            );
            "#,
        )?;
        Ok(())
    }
    
    /// Record an emotional event for a domain.
    ///
    /// Updates the running average valence for the domain.
    /// Valence should be in range -1.0 (very negative) to 1.0 (very positive).
    pub fn record_emotion(&self, domain: &str, valence: f64) -> Result<(), rusqlite::Error> {
        // Clamp valence to valid range
        let valence = valence.max(-1.0).min(1.0);
        
        // Try to get existing trend
        let existing: Option<(f64, i32)> = self.conn
            .query_row(
                "SELECT valence, count FROM emotional_trends WHERE domain = ?",
                params![domain],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        
        match existing {
            Some((old_valence, count)) => {
                // Update running average: new_avg = (old_avg * count + new_value) / (count + 1)
                let new_count = count + 1;
                let new_valence = (old_valence * count as f64 + valence) / new_count as f64;
                
                self.conn.execute(
                    "UPDATE emotional_trends SET valence = ?, count = ?, last_updated = ? WHERE domain = ?",
                    params![new_valence, new_count, now_f64(), domain],
                )?;
            }
            None => {
                // Insert new trend
                self.conn.execute(
                    "INSERT INTO emotional_trends (domain, valence, count, last_updated) VALUES (?, ?, 1, ?)",
                    params![domain, valence, now_f64()],
                )?;
            }
        }
        
        Ok(())
    }
    
    /// Get the emotional trend for a specific domain.
    pub fn get_trend(&self, domain: &str) -> Result<Option<EmotionalTrend>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT domain, valence, count, last_updated FROM emotional_trends WHERE domain = ?",
                params![domain],
                |row| {
                    let last_updated_f64: f64 = row.get(3)?;
                    Ok(EmotionalTrend {
                        domain: row.get(0)?,
                        valence: row.get(1)?,
                        count: row.get(2)?,
                        last_updated: f64_to_datetime(last_updated_f64),
                    })
                },
            )
            .optional()
    }
    
    /// Get all emotional trends.
    pub fn get_all_trends(&self) -> Result<Vec<EmotionalTrend>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT domain, valence, count, last_updated FROM emotional_trends ORDER BY count DESC"
        )?;
        
        let rows = stmt.query_map([], |row| {
            let last_updated_f64: f64 = row.get(3)?;
            Ok(EmotionalTrend {
                domain: row.get(0)?,
                valence: row.get(1)?,
                count: row.get(2)?,
                last_updated: f64_to_datetime(last_updated_f64),
            })
        })?;
        
        rows.collect()
    }
    
    /// Get all trends that suggest SOUL updates.
    pub fn get_trends_needing_update(&self) -> Result<Vec<EmotionalTrend>, rusqlite::Error> {
        let all = self.get_all_trends()?;
        Ok(all.into_iter().filter(|t| t.needs_soul_update()).collect())
    }
    
    /// Reset a domain's trend (after SOUL has been updated).
    pub fn reset_trend(&self, domain: &str) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "DELETE FROM emotional_trends WHERE domain = ?",
            params![domain],
        )?;
        Ok(())
    }
    
    /// Decay all trends by a factor (used during consolidation).
    /// This moves trends toward neutral over time.
    pub fn decay_trends(&self, factor: f64) -> Result<usize, rusqlite::Error> {
        let affected = self.conn.execute(
            "UPDATE emotional_trends SET valence = valence * ?, last_updated = ?",
            params![factor, now_f64()],
        )?;
        Ok(affected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_record_and_get_emotion() {
        let conn = Connection::open_in_memory().unwrap();
        let acc = EmotionalAccumulator::new(&conn).unwrap();
        
        // Record some emotions
        acc.record_emotion("coding", 0.8).unwrap();
        acc.record_emotion("coding", 0.6).unwrap();
        acc.record_emotion("coding", 0.4).unwrap();
        
        let trend = acc.get_trend("coding").unwrap().unwrap();
        assert_eq!(trend.count, 3);
        // Average of 0.8, 0.6, 0.4 = 0.6
        assert!((trend.valence - 0.6).abs() < 0.01);
    }
    
    #[test]
    fn test_negative_trend_flags_update() {
        let conn = Connection::open_in_memory().unwrap();
        let acc = EmotionalAccumulator::new(&conn).unwrap();
        
        // Record many negative emotions
        for _ in 0..12 {
            acc.record_emotion("debugging", -0.7).unwrap();
        }
        
        let trend = acc.get_trend("debugging").unwrap().unwrap();
        assert!(trend.needs_soul_update());
        assert!(trend.valence < NEGATIVE_THRESHOLD);
    }
    
    #[test]
    fn test_get_all_trends() {
        let conn = Connection::open_in_memory().unwrap();
        let acc = EmotionalAccumulator::new(&conn).unwrap();
        
        acc.record_emotion("coding", 0.5).unwrap();
        acc.record_emotion("writing", -0.3).unwrap();
        acc.record_emotion("research", 0.8).unwrap();
        
        let trends = acc.get_all_trends().unwrap();
        assert_eq!(trends.len(), 3);
    }
    
    #[test]
    fn test_reset_trend() {
        let conn = Connection::open_in_memory().unwrap();
        let acc = EmotionalAccumulator::new(&conn).unwrap();
        
        acc.record_emotion("test", 0.5).unwrap();
        assert!(acc.get_trend("test").unwrap().is_some());
        
        acc.reset_trend("test").unwrap();
        assert!(acc.get_trend("test").unwrap().is_none());
    }
    
    #[test]
    fn test_valence_clamping() {
        let conn = Connection::open_in_memory().unwrap();
        let acc = EmotionalAccumulator::new(&conn).unwrap();
        
        // Values outside range should be clamped
        acc.record_emotion("extreme", 5.0).unwrap();
        let trend = acc.get_trend("extreme").unwrap().unwrap();
        assert_eq!(trend.valence, 1.0);
        
        acc.record_emotion("extreme", -10.0).unwrap();
        let trend = acc.get_trend("extreme").unwrap().unwrap();
        // Average of 1.0 and -1.0 = 0.0
        assert!((trend.valence).abs() < 0.01);
    }
}
