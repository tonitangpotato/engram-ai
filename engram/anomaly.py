"""
Anomaly Detection — Simplified Predictive Coding

Neuroscience basis: The brain constantly generates predictions about incoming
stimuli (predictive coding / free energy principle — Friston, 2005). When
reality deviates significantly from expectation, a "prediction error" signal
fires, attracting attention and boosting encoding of the surprising event.

This module tracks rolling baselines for various metrics (memory access rates,
consolidation strength changes, retrieval patterns) and flags when values
deviate more than 2σ from the rolling mean. Anomalous events can trigger:
- Higher importance encoding (surprising = memorable)
- Alerts to the agent ("something unusual happened")
- Adaptive parameter tuning

This is a simplified version of the hierarchical predictive coding framework,
reduced to univariate Gaussian tracking per metric.

References:
- Rao & Ballard (1999) — Predictive coding in visual cortex
- Friston (2005) — Free energy principle
- Den Ouden et al. (2012) — How prediction errors shape perception and learning
"""

import math
from collections import defaultdict, deque


class BaselineTracker:
    """
    Track rolling averages for anomaly detection.

    Maintains a sliding window of observations per metric.
    Flags values that deviate > 2σ from the rolling mean.

    This is the agent's "surprise detector" — when something breaks
    pattern, it's worth paying extra attention to.
    """

    def __init__(self, window_size: int = 100):
        """
        Initialize tracker.

        Args:
            window_size: Number of observations to keep per metric.
                Larger = more stable baselines but slower to adapt.
                100 is a good default for daily-resolution metrics.
        """
        self.window_size = window_size
        self._data: dict[str, deque] = defaultdict(lambda: deque(maxlen=window_size))

    def update(self, metric: str, value: float):
        """
        Add a new data point for a metric.

        Args:
            metric: Name of the metric (e.g., "access_rate", "consolidation_delta")
            value: Observed value
        """
        self._data[metric].append(value)

    def get_baseline(self, metric: str) -> dict:
        """
        Return current baseline statistics for a metric.

        Args:
            metric: Name of the metric

        Returns:
            Dict with {mean, std, n}. Returns {mean: 0, std: 0, n: 0}
            if no data exists.
        """
        values = self._data.get(metric)
        if not values or len(values) == 0:
            return {"mean": 0.0, "std": 0.0, "n": 0}

        n = len(values)
        mean = sum(values) / n

        if n < 2:
            return {"mean": mean, "std": 0.0, "n": n}

        variance = sum((v - mean) ** 2 for v in values) / (n - 1)
        std = math.sqrt(variance)

        return {"mean": mean, "std": std, "n": n}

    def is_anomaly(self, metric: str, value: float,
                   sigma_threshold: float = 2.0,
                   min_samples: int = 5) -> bool:
        """
        Check if a value deviates > threshold σ from rolling mean.

        Requires at least min_samples observations before flagging
        anomalies (avoids false positives during warmup).

        The 2σ threshold corresponds to ~5% false positive rate under
        Gaussian assumptions — a reasonable sensitivity for memory systems.

        Args:
            metric: Name of the metric
            value: Value to check
            sigma_threshold: Number of standard deviations for anomaly (default 2.0)
            min_samples: Minimum observations before anomaly detection activates

        Returns:
            True if the value is anomalous
        """
        baseline = self.get_baseline(metric)

        if baseline["n"] < min_samples:
            return False

        if baseline["std"] == 0:
            # No variance — any deviation is anomalous
            return value != baseline["mean"]

        z_score = abs(value - baseline["mean"]) / baseline["std"]
        return z_score > sigma_threshold

    def z_score(self, metric: str, value: float) -> float:
        """
        Compute z-score for a value against the baseline.

        Args:
            metric: Name of the metric
            value: Value to score

        Returns:
            z-score (0.0 if insufficient data)
        """
        baseline = self.get_baseline(metric)
        if baseline["n"] < 2 or baseline["std"] == 0:
            return 0.0
        return (value - baseline["mean"]) / baseline["std"]

    def metrics(self) -> list[str]:
        """Return list of tracked metric names."""
        return list(self._data.keys())


if __name__ == "__main__":
    """Demo: anomaly detection on simulated memory access patterns."""
    import random

    tracker = BaselineTracker(window_size=50)

    print("=== Anomaly Detection Demo ===\n")
    print("  Simulating daily memory access counts (normal ~20, anomaly ~50):\n")

    random.seed(42)
    for day in range(60):
        # Normal: ~20 accesses/day with noise
        if day == 30:
            # Sudden spike on day 30
            value = 55.0
        elif day == 45:
            # Sudden drop on day 45
            value = 2.0
        else:
            value = random.gauss(20, 4)

        is_anom = tracker.is_anomaly("daily_accesses", value)
        z = tracker.z_score("daily_accesses", value)
        tracker.update("daily_accesses", value)

        if is_anom or day in (0, 10, 29, 30, 31, 44, 45, 46, 59):
            flag = " ⚠️  ANOMALY!" if is_anom else ""
            print(f"  Day {day:2d}: accesses={value:5.1f}  z={z:+5.2f}{flag}")

    print()
    baseline = tracker.get_baseline("daily_accesses")
    print(f"  Final baseline: mean={baseline['mean']:.1f}, std={baseline['std']:.1f}, "
          f"n={baseline['n']}")
