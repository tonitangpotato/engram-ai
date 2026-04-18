//! Anomaly/Baseline Tracker — sliding window statistics for anomaly detection.
//!
//! Maintains per-metric baselines using a sliding window of recent values.
//! Uses z-score based anomaly detection: values outside sigma_threshold
//! standard deviations from the mean are flagged as anomalies.

use std::collections::{HashMap, VecDeque};
use serde::{Deserialize, Serialize};

/// Default window size for baseline tracking.
const DEFAULT_WINDOW_SIZE: usize = 100;

/// Baseline statistics for a single metric.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Baseline {
    /// Arithmetic mean of values in the window
    pub mean: f64,
    /// Standard deviation of values in the window
    pub std: f64,
    /// Number of samples in the window
    pub n: usize,
}

impl Default for Baseline {
    fn default() -> Self {
        Self {
            mean: 0.0,
            std: 0.0,
            n: 0,
        }
    }
}

/// Sliding window statistics tracker for anomaly detection.
///
/// Tracks multiple metrics, each with its own sliding window of values.
/// Computes mean and standard deviation for z-score based anomaly detection.
#[derive(Debug, Clone)]
pub struct BaselineTracker {
    /// Maximum number of values to keep per metric
    window_size: usize,
    /// Metric name -> sliding window of values
    data: HashMap<String, VecDeque<f64>>,
}

impl Default for BaselineTracker {
    fn default() -> Self {
        Self::new(DEFAULT_WINDOW_SIZE)
    }
}

impl BaselineTracker {
    /// Create a new baseline tracker with specified window size.
    pub fn new(window_size: usize) -> Self {
        Self {
            window_size: window_size.max(1), // At least 1
            data: HashMap::new(),
        }
    }
    
    /// Update a metric with a new value.
    ///
    /// Adds the value to the sliding window, evicting the oldest
    /// value if the window is full.
    pub fn update(&mut self, metric: &str, value: f64) {
        let window = self.data
            .entry(metric.to_string())
            .or_insert_with(VecDeque::new);
        
        // Evict oldest if at capacity
        if window.len() >= self.window_size {
            window.pop_front();
        }
        
        window.push_back(value);
    }
    
    /// Update a metric with multiple values at once.
    pub fn update_batch(&mut self, metric: &str, values: &[f64]) {
        for value in values {
            self.update(metric, *value);
        }
    }
    
    /// Get baseline statistics for a metric.
    ///
    /// Returns mean=0, std=0, n=0 if the metric has no data.
    pub fn get_baseline(&self, metric: &str) -> Baseline {
        match self.data.get(metric) {
            None => Baseline::default(),
            Some(window) if window.is_empty() => Baseline::default(),
            Some(window) => {
                let n = window.len();
                let mean = window.iter().sum::<f64>() / n as f64;
                
                // Calculate standard deviation
                let variance = if n > 1 {
                    window.iter()
                        .map(|x| (x - mean).powi(2))
                        .sum::<f64>() / (n - 1) as f64 // Sample variance
                } else {
                    0.0
                };
                
                Baseline {
                    mean,
                    std: variance.sqrt(),
                    n,
                }
            }
        }
    }
    
    /// Calculate z-score for a value given the metric's baseline.
    ///
    /// Returns 0.0 if there's insufficient data or std=0.
    pub fn z_score(&self, metric: &str, value: f64) -> f64 {
        let baseline = self.get_baseline(metric);
        
        if baseline.n == 0 || baseline.std == 0.0 {
            return 0.0;
        }
        
        (value - baseline.mean) / baseline.std
    }
    
    /// Check if a value is an anomaly for the given metric.
    ///
    /// # Arguments
    ///
    /// * `metric` - The metric name
    /// * `value` - The value to check
    /// * `sigma_threshold` - Number of standard deviations for anomaly (e.g., 2.0)
    /// * `min_samples` - Minimum number of samples required to detect anomalies
    ///
    /// Returns false if there's insufficient data to make a determination.
    pub fn is_anomaly(
        &self,
        metric: &str,
        value: f64,
        sigma_threshold: f64,
        min_samples: usize,
    ) -> bool {
        let baseline = self.get_baseline(metric);
        
        // Not enough data to determine anomaly
        if baseline.n < min_samples {
            return false;
        }
        
        // If std is 0, only flag if value differs from mean
        if baseline.std == 0.0 {
            return (value - baseline.mean).abs() > f64::EPSILON;
        }
        
        let z = (value - baseline.mean).abs() / baseline.std;
        z > sigma_threshold
    }
    
    /// Check if a value is anomalously high.
    pub fn is_high_anomaly(
        &self,
        metric: &str,
        value: f64,
        sigma_threshold: f64,
        min_samples: usize,
    ) -> bool {
        let baseline = self.get_baseline(metric);
        
        if baseline.n < min_samples || baseline.std == 0.0 {
            return false;
        }
        
        let z = (value - baseline.mean) / baseline.std;
        z > sigma_threshold
    }
    
    /// Check if a value is anomalously low.
    pub fn is_low_anomaly(
        &self,
        metric: &str,
        value: f64,
        sigma_threshold: f64,
        min_samples: usize,
    ) -> bool {
        let baseline = self.get_baseline(metric);
        
        if baseline.n < min_samples || baseline.std == 0.0 {
            return false;
        }
        
        let z = (value - baseline.mean) / baseline.std;
        z < -sigma_threshold
    }
    
    /// Get all tracked metric names.
    pub fn metrics(&self) -> Vec<&str> {
        self.data.keys().map(|s| s.as_str()).collect()
    }
    
    /// Get the number of samples for a metric.
    pub fn sample_count(&self, metric: &str) -> usize {
        self.data.get(metric).map(|w| w.len()).unwrap_or(0)
    }
    
    /// Clear all data for a metric.
    pub fn clear_metric(&mut self, metric: &str) {
        self.data.remove(metric);
    }
    
    /// Clear all tracked data.
    pub fn clear(&mut self) {
        self.data.clear();
    }
    
    /// Get the window size.
    pub fn window_size(&self) -> usize {
        self.window_size
    }
    
    /// Get recent values for a metric (returns a copy).
    pub fn get_values(&self, metric: &str) -> Vec<f64> {
        self.data.get(metric)
            .map(|w| w.iter().copied().collect())
            .unwrap_or_default()
    }
    
    /// Get the most recent value for a metric.
    pub fn last_value(&self, metric: &str) -> Option<f64> {
        self.data.get(metric).and_then(|w| w.back().copied())
    }
    
    /// Calculate percentile for a metric.
    ///
    /// # Arguments
    ///
    /// * `metric` - The metric name
    /// * `percentile` - The percentile (0.0 to 1.0, e.g., 0.95 for 95th percentile)
    pub fn percentile(&self, metric: &str, percentile: f64) -> Option<f64> {
        let window = self.data.get(metric)?;
        
        if window.is_empty() {
            return None;
        }
        
        let mut sorted: Vec<f64> = window.iter().copied().collect();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        
        let index = ((percentile * (sorted.len() - 1) as f64).round() as usize)
            .min(sorted.len() - 1);
        
        Some(sorted[index])
    }

    /// Convert the current state of a metric into an [`InteroceptiveSignal`].
    ///
    /// - `valence`: negated normalized z-score (deviation = negative feeling).
    /// - `arousal`: absolute z-score divided by threshold, clamped to [0, 1].
    ///
    /// Returns `None` if the metric has no data.
    pub fn to_signal(
        &self,
        metric: &str,
        value: f64,
        sigma_threshold: f64,
    ) -> Option<crate::interoceptive::InteroceptiveSignal> {
        use crate::interoceptive::{InteroceptiveSignal, SignalContext, SignalSource};

        let baseline = self.get_baseline(metric);
        if baseline.n == 0 || baseline.std == 0.0 {
            return None;
        }

        let z = (value - baseline.mean) / baseline.std;
        let valence = -(z / sigma_threshold).clamp(-1.0, 1.0);
        let arousal = (z.abs() / sigma_threshold).clamp(0.0, 1.0);

        Some(
            InteroceptiveSignal::new(SignalSource::Anomaly, Some(metric.to_string()), valence, arousal)
                .with_context(SignalContext::AnomalyDetected {
                    metric: metric.to_string(),
                    z_score: z,
                    baseline_mean: baseline.mean,
                }),
        )
    }
}

/// Anomaly detection result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnomalyResult {
    /// The metric name
    pub metric: String,
    /// The observed value
    pub value: f64,
    /// The z-score
    pub z_score: f64,
    /// Whether this is an anomaly
    pub is_anomaly: bool,
    /// Direction: "high", "low", or "normal"
    pub direction: String,
    /// Baseline statistics at time of detection
    pub baseline: Baseline,
}

impl BaselineTracker {
    /// Analyze a value and return detailed anomaly result.
    pub fn analyze(
        &self,
        metric: &str,
        value: f64,
        sigma_threshold: f64,
        min_samples: usize,
    ) -> AnomalyResult {
        let baseline = self.get_baseline(metric);
        let z = self.z_score(metric, value);
        let is_anomaly = self.is_anomaly(metric, value, sigma_threshold, min_samples);
        
        let direction = if !is_anomaly || baseline.n < min_samples {
            "normal"
        } else if z > 0.0 {
            "high"
        } else {
            "low"
        }.to_string();
        
        AnomalyResult {
            metric: metric.to_string(),
            value,
            z_score: z,
            is_anomaly,
            direction,
            baseline,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_baseline_calculation() {
        let mut tracker = BaselineTracker::new(100);
        
        // Add some values
        for i in 1..=10 {
            tracker.update("test", i as f64);
        }
        
        let baseline = tracker.get_baseline("test");
        
        assert_eq!(baseline.n, 10);
        assert!((baseline.mean - 5.5).abs() < 0.01); // Mean of 1..10 is 5.5
        assert!(baseline.std > 0.0);
    }
    
    #[test]
    fn test_z_score() {
        let mut tracker = BaselineTracker::new(100);
        
        // Add values with known mean and std
        for _ in 0..100 {
            tracker.update("test", 100.0);
        }
        
        // Value equal to mean should have z=0
        let z = tracker.z_score("test", 100.0);
        assert!(z.abs() < 0.01);
        
        // Test with more varied data
        let mut tracker2 = BaselineTracker::new(100);
        let values: Vec<f64> = (1..=100).map(|x| x as f64).collect();
        for v in &values {
            tracker2.update("test2", *v);
        }
        
        let baseline = tracker2.get_baseline("test2");
        let z_high = tracker2.z_score("test2", baseline.mean + 2.0 * baseline.std);
        assert!((z_high - 2.0).abs() < 0.1);
    }
    
    #[test]
    fn test_anomaly_detection() {
        let mut tracker = BaselineTracker::new(100);
        
        // Add normal values
        for i in 0..50 {
            tracker.update("latency", 100.0 + (i % 10) as f64);
        }
        
        let baseline = tracker.get_baseline("latency");
        
        // Normal value should not be anomaly
        assert!(!tracker.is_anomaly("latency", baseline.mean, 2.0, 10));
        
        // Very high value should be anomaly
        let extreme = baseline.mean + 5.0 * baseline.std;
        assert!(tracker.is_anomaly("latency", extreme, 2.0, 10));
    }
    
    #[test]
    fn test_min_samples() {
        let mut tracker = BaselineTracker::new(100);
        
        // Only 3 samples
        tracker.update_batch("test", &[1.0, 2.0, 3.0]);
        
        // Shouldn't flag as anomaly because min_samples not met
        assert!(!tracker.is_anomaly("test", 100.0, 2.0, 10));
        
        // Should flag if min_samples is lower
        assert!(tracker.is_anomaly("test", 100.0, 2.0, 2));
    }
    
    #[test]
    fn test_window_eviction() {
        let mut tracker = BaselineTracker::new(5);
        
        // Add more than window size
        for i in 1..=10 {
            tracker.update("test", i as f64);
        }
        
        // Should only have last 5 values
        assert_eq!(tracker.sample_count("test"), 5);
        
        let baseline = tracker.get_baseline("test");
        // Mean should be (6+7+8+9+10)/5 = 8.0
        assert!((baseline.mean - 8.0).abs() < 0.01);
    }
    
    #[test]
    fn test_percentile() {
        let mut tracker = BaselineTracker::new(100);
        
        for i in 1..=100 {
            tracker.update("test", i as f64);
        }
        
        // 50th percentile should be around 50 (for 1-100, it's actually at index 49.5 ~ 50)
        let p50 = tracker.percentile("test", 0.5).unwrap();
        assert!(p50 >= 49.0 && p50 <= 51.0, "p50 was {}", p50);
        
        // 100th percentile should be 100
        let p100 = tracker.percentile("test", 1.0).unwrap();
        assert!((p100 - 100.0).abs() < 0.01);
        
        // 0th percentile should be 1
        let p0 = tracker.percentile("test", 0.0).unwrap();
        assert!((p0 - 1.0).abs() < 0.01);
    }
    
    #[test]
    fn test_high_low_anomaly() {
        let mut tracker = BaselineTracker::new(100);
        
        for i in 1..=50 {
            tracker.update("test", 50.0 + (i % 5) as f64);
        }
        
        let baseline = tracker.get_baseline("test");
        let very_high = baseline.mean + 5.0 * baseline.std;
        let very_low = baseline.mean - 5.0 * baseline.std;
        
        assert!(tracker.is_high_anomaly("test", very_high, 2.0, 10));
        assert!(!tracker.is_low_anomaly("test", very_high, 2.0, 10));
        
        assert!(tracker.is_low_anomaly("test", very_low, 2.0, 10));
        assert!(!tracker.is_high_anomaly("test", very_low, 2.0, 10));
    }
    
    #[test]
    fn test_analyze() {
        let mut tracker = BaselineTracker::new(100);
        
        for _ in 0..50 {
            tracker.update("cpu", 50.0);
        }
        
        // Normal value
        let result = tracker.analyze("cpu", 50.0, 2.0, 10);
        assert!(!result.is_anomaly);
        assert_eq!(result.direction, "normal");
        
        // Anomalous high value (when std > 0)
        tracker.update("cpu", 55.0);
        let result = tracker.analyze("cpu", 200.0, 2.0, 10);
        assert!(result.is_anomaly);
        assert_eq!(result.direction, "high");
    }

    #[test]
    fn test_to_signal_normal_value() {
        let mut tracker = BaselineTracker::new(100);
        for i in 0..50 {
            tracker.update("latency", 100.0 + (i % 5) as f64);
        }

        let baseline = tracker.get_baseline("latency");
        let sig = tracker.to_signal("latency", baseline.mean, 2.0).unwrap();

        assert!(matches!(sig.source, crate::interoceptive::SignalSource::Anomaly));
        assert_eq!(sig.domain.as_deref(), Some("latency"));
        // Normal value → z ≈ 0 → valence ≈ 0, arousal ≈ 0
        assert!(sig.valence.abs() < 0.1, "valence was {}", sig.valence);
        assert!(sig.arousal < 0.1, "arousal was {}", sig.arousal);
        // Should have AnomalyDetected context
        assert!(matches!(
            sig.context,
            Some(crate::interoceptive::SignalContext::AnomalyDetected { .. })
        ));
    }

    #[test]
    fn test_to_signal_anomalous_value() {
        let mut tracker = BaselineTracker::new(100);
        for i in 0..50 {
            tracker.update("cpu", 50.0 + (i % 3) as f64);
        }

        let baseline = tracker.get_baseline("cpu");
        let extreme = baseline.mean + 3.0 * baseline.std;
        let sig = tracker.to_signal("cpu", extreme, 2.0).unwrap();

        // High z-score → negative valence (deviation = bad), high arousal
        assert!(sig.valence < -0.5, "valence was {}", sig.valence);
        assert!(sig.arousal > 0.5, "arousal was {}", sig.arousal);
    }

    #[test]
    fn test_to_signal_no_data_returns_none() {
        let tracker = BaselineTracker::new(100);
        assert!(tracker.to_signal("unknown", 42.0, 2.0).is_none());
    }

    #[test]
    fn test_to_signal_zero_std_returns_none() {
        let mut tracker = BaselineTracker::new(100);
        // All identical values → std = 0
        for _ in 0..10 {
            tracker.update("flat", 42.0);
        }
        assert!(tracker.to_signal("flat", 42.0, 2.0).is_none());
    }
}
