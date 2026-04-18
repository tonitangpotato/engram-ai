//! InteroceptiveHub — The integration layer.
//!
//! Receives [`InteroceptiveSignal`]s from all monitoring subsystems,
//! maintains per-domain [`DomainState`] aggregates, computes global arousal,
//! and caches [`SomaticMarker`]s for rapid situation recognition.
//!
//! Design constraints (from INTEROCEPTIVE-LAYER.md):
//! - O(1) per signal processing (EWMA, not full-window recompute)
//! - signal_buffer capped at 1000 entries (FIFO eviction)
//! - somatic_cache LRU eviction when exceeding max size

use std::collections::{HashMap, VecDeque};

use chrono::Utc;

use crate::interoceptive::types::{
    DomainState, InteroceptiveSignal, InteroceptiveState, SomaticMarker,
};

/// Default maximum number of signals to retain in the buffer.
const DEFAULT_BUFFER_CAPACITY: usize = 1000;

/// Default maximum number of somatic markers to cache.
const DEFAULT_MARKER_CACHE_SIZE: usize = 256;

/// Default EWMA alpha (recency weight). 0.3 gives ~70% weight to history.
const DEFAULT_ALPHA: f64 = 0.3;

/// The central integration hub for interoceptive signals.
///
/// Analogous to the anterior insula in Craig's model — receives raw
/// interoceptive signals and builds an integrated "feeling state."
pub struct InteroceptiveHub {
    /// Per-domain aggregated states.
    domain_states: HashMap<String, DomainState>,

    /// Sliding window of recent signals (FIFO, capped at `buffer_capacity`).
    signal_buffer: VecDeque<InteroceptiveSignal>,

    /// Maximum signals to keep in buffer.
    buffer_capacity: usize,

    /// Somatic marker cache: situation_hash → marker.
    somatic_cache: HashMap<u64, SomaticMarker>,

    /// Maximum markers to cache before LRU eviction.
    marker_cache_size: usize,

    /// EWMA smoothing factor for domain state updates.
    alpha: f64,
}

impl Default for InteroceptiveHub {
    fn default() -> Self {
        Self::new()
    }
}

impl InteroceptiveHub {
    /// Create a new hub with default settings.
    pub fn new() -> Self {
        Self {
            domain_states: HashMap::new(),
            signal_buffer: VecDeque::with_capacity(DEFAULT_BUFFER_CAPACITY),
            buffer_capacity: DEFAULT_BUFFER_CAPACITY,
            somatic_cache: HashMap::new(),
            marker_cache_size: DEFAULT_MARKER_CACHE_SIZE,
            alpha: DEFAULT_ALPHA,
        }
    }

    /// Create a hub with custom capacity settings.
    pub fn with_capacity(
        buffer_capacity: usize,
        marker_cache_size: usize,
        alpha: f64,
    ) -> Self {
        Self {
            domain_states: HashMap::new(),
            signal_buffer: VecDeque::with_capacity(buffer_capacity.min(4096)),
            buffer_capacity: buffer_capacity.max(1),
            somatic_cache: HashMap::new(),
            marker_cache_size: marker_cache_size.max(1),
            alpha: alpha.clamp(0.01, 0.99),
        }
    }

    /// Process a single incoming signal.
    ///
    /// 1. Buffers the signal (FIFO eviction if full).
    /// 2. Updates the relevant domain state via EWMA.
    /// 3. Returns `true` if the signal was notable (negative + urgent).
    pub fn process_signal(&mut self, signal: InteroceptiveSignal) -> bool {
        let notable = signal.is_negative() && signal.is_urgent();

        // Update domain state.
        let domain_key = signal
            .domain
            .clone()
            .unwrap_or_else(|| "_global".to_string());

        let ds = self
            .domain_states
            .entry(domain_key)
            .or_insert_with_key(|k| DomainState::new(k));
        ds.update(&signal, self.alpha);

        // Buffer the signal.
        if self.signal_buffer.len() >= self.buffer_capacity {
            self.signal_buffer.pop_front();
        }
        self.signal_buffer.push_back(signal);

        notable
    }

    /// Process a batch of signals.
    pub fn process_batch(&mut self, signals: Vec<InteroceptiveSignal>) -> usize {
        let mut notable_count = 0;
        for signal in signals {
            if self.process_signal(signal) {
                notable_count += 1;
            }
        }
        notable_count
    }

    /// Look up or create a somatic marker for a situation.
    ///
    /// If the situation has been seen before, returns the cached marker
    /// (Damasio's "gut feeling"). If new, creates a fresh marker.
    pub fn somatic_lookup(&mut self, situation_hash: u64, current_valence: f64) -> &SomaticMarker {
        if self.somatic_cache.contains_key(&situation_hash) {
            let marker = self.somatic_cache.get_mut(&situation_hash).unwrap();
            marker.update(current_valence);
        } else {
            // Evict LRU if at capacity.
            if self.somatic_cache.len() >= self.marker_cache_size {
                self.evict_lru_marker();
            }
            self.somatic_cache
                .insert(situation_hash, SomaticMarker::new(situation_hash, current_valence));
        }
        self.somatic_cache.get(&situation_hash).unwrap()
    }

    /// Evict the least recently accessed somatic marker.
    fn evict_lru_marker(&mut self) {
        if let Some((&lru_hash, _)) = self
            .somatic_cache
            .iter()
            .min_by_key(|(_, m)| m.last_accessed)
        {
            self.somatic_cache.remove(&lru_hash);
        }
    }

    /// Take a snapshot of the current interoceptive state.
    ///
    /// This is the primary output consumed by the system prompt builder —
    /// Craig's "conscious interoceptive image."
    pub fn current_state(&self) -> InteroceptiveState {
        let global_arousal = self.compute_global_arousal();

        // Collect recently accessed markers.
        let active_markers: Vec<SomaticMarker> = self
            .somatic_cache
            .values()
            .filter(|m| {
                let age = Utc::now() - m.last_accessed;
                age.num_minutes() < 30
            })
            .cloned()
            .collect();

        InteroceptiveState {
            domain_states: self.domain_states.clone(),
            global_arousal,
            buffer_size: self.signal_buffer.len(),
            active_markers,
            timestamp: Utc::now(),
        }
    }

    /// Compute global arousal as weighted average across domains.
    ///
    /// Domains with more recent signals and higher signal counts
    /// contribute more to the global arousal level.
    fn compute_global_arousal(&self) -> f64 {
        if self.domain_states.is_empty() {
            return 0.0;
        }

        // Use the last N signals to compute arousal.
        let recent_window = 50.min(self.signal_buffer.len());
        if recent_window == 0 {
            return 0.0;
        }

        let sum: f64 = self
            .signal_buffer
            .iter()
            .rev()
            .take(recent_window)
            .map(|s| s.arousal)
            .sum();

        (sum / recent_window as f64).clamp(0.0, 1.0)
    }

    /// Get the domain state for a specific domain, if it exists.
    pub fn domain_state(&self, domain: &str) -> Option<&DomainState> {
        self.domain_states.get(domain)
    }

    /// Get all domain states.
    pub fn all_domain_states(&self) -> &HashMap<String, DomainState> {
        &self.domain_states
    }

    /// Number of signals currently in the buffer.
    pub fn buffer_len(&self) -> usize {
        self.signal_buffer.len()
    }

    /// Number of domains being tracked.
    pub fn domain_count(&self) -> usize {
        self.domain_states.len()
    }

    /// Number of somatic markers cached.
    pub fn marker_count(&self) -> usize {
        self.somatic_cache.len()
    }

    /// Clear all state (for testing or reset).
    pub fn clear(&mut self) {
        self.domain_states.clear();
        self.signal_buffer.clear();
        self.somatic_cache.clear();
    }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interoceptive::types::SignalSource;

    #[test]
    fn hub_processes_signal_and_updates_domain() {
        let mut hub = InteroceptiveHub::new();
        let sig = InteroceptiveSignal::new(
            SignalSource::Accumulator,
            Some("coding".into()),
            0.7,
            0.3,
        );
        let notable = hub.process_signal(sig);
        assert!(!notable); // positive + low arousal → not notable

        assert_eq!(hub.domain_count(), 1);
        assert_eq!(hub.buffer_len(), 1);

        let ds = hub.domain_state("coding").unwrap();
        assert!(ds.valence_trend > 0.0);
    }

    #[test]
    fn hub_notable_signal() {
        let mut hub = InteroceptiveHub::new();
        let sig = InteroceptiveSignal::new(
            SignalSource::Anomaly,
            Some("trading".into()),
            -0.8,
            0.9,
        );
        assert!(hub.process_signal(sig)); // negative + urgent → notable
    }

    #[test]
    fn hub_buffer_fifo_eviction() {
        let mut hub = InteroceptiveHub::with_capacity(5, 10, 0.3);

        for i in 0..8 {
            let sig = InteroceptiveSignal::new(
                SignalSource::Accumulator,
                Some("test".into()),
                i as f64 * 0.1,
                0.1,
            );
            hub.process_signal(sig);
        }

        assert_eq!(hub.buffer_len(), 5); // capped at 5
    }

    #[test]
    fn hub_global_arousal_computation() {
        let mut hub = InteroceptiveHub::new();

        // Add high-arousal signals.
        for _ in 0..10 {
            let sig = InteroceptiveSignal::new(
                SignalSource::Anomaly,
                Some("test".into()),
                -0.5,
                0.8,
            );
            hub.process_signal(sig);
        }

        let state = hub.current_state();
        assert!(state.global_arousal > 0.5, "got {}", state.global_arousal);
    }

    #[test]
    fn hub_somatic_marker_creation_and_update() {
        let mut hub = InteroceptiveHub::new();

        // First encounter.
        let marker = hub.somatic_lookup(42, 0.5);
        assert_eq!(marker.encounter_count, 1);
        assert_eq!(marker.valence, 0.5);

        // Second encounter.
        let marker = hub.somatic_lookup(42, -0.5);
        assert_eq!(marker.encounter_count, 2);
        assert!((marker.valence - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn hub_somatic_lru_eviction() {
        let mut hub = InteroceptiveHub::with_capacity(100, 3, 0.3);

        // Fill cache.
        hub.somatic_lookup(1, 0.1);
        hub.somatic_lookup(2, 0.2);
        hub.somatic_lookup(3, 0.3);
        assert_eq!(hub.marker_count(), 3);

        // Adding a 4th should evict the LRU.
        hub.somatic_lookup(4, 0.4);
        assert_eq!(hub.marker_count(), 3);
    }

    #[test]
    fn hub_current_state_snapshot() {
        let mut hub = InteroceptiveHub::new();

        let sig = InteroceptiveSignal::new(
            SignalSource::Feedback,
            Some("coding".into()),
            0.6,
            0.2,
        );
        hub.process_signal(sig);

        let state = hub.current_state();
        assert_eq!(state.domain_states.len(), 1);
        assert!(state.domain_states.contains_key("coding"));
        assert_eq!(state.buffer_size, 1);
    }

    #[test]
    fn hub_global_domain_signal() {
        let mut hub = InteroceptiveHub::new();

        // Signal with no domain → goes to "_global".
        let sig = InteroceptiveSignal::new(SignalSource::Confidence, None, 0.4, 0.1);
        hub.process_signal(sig);

        assert!(hub.domain_state("_global").is_some());
    }

    #[test]
    fn hub_process_batch() {
        let mut hub = InteroceptiveHub::new();

        let signals = vec![
            InteroceptiveSignal::new(SignalSource::Accumulator, Some("a".into()), 0.5, 0.2),
            InteroceptiveSignal::new(SignalSource::Anomaly, Some("b".into()), -0.8, 0.9),
            InteroceptiveSignal::new(SignalSource::Feedback, Some("a".into()), 0.3, 0.1),
        ];

        let notable = hub.process_batch(signals);
        assert_eq!(notable, 1); // only the anomaly signal is notable
        assert_eq!(hub.buffer_len(), 3);
        assert_eq!(hub.domain_count(), 2); // "a" and "b"
    }

    #[test]
    fn hub_clear() {
        let mut hub = InteroceptiveHub::new();
        hub.process_signal(InteroceptiveSignal::new(
            SignalSource::Accumulator,
            Some("x".into()),
            0.5,
            0.2,
        ));
        hub.somatic_lookup(99, 0.1);

        hub.clear();
        assert_eq!(hub.buffer_len(), 0);
        assert_eq!(hub.domain_count(), 0);
        assert_eq!(hub.marker_count(), 0);
    }
}
