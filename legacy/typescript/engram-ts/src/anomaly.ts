/**
 * Anomaly Detection — Simplified Predictive Coding
 * (Friston, 2005 — Free energy principle)
 */

export class BaselineTracker {
  windowSize: number;
  private data: Map<string, number[]>;

  constructor(windowSize: number = 100) {
    this.windowSize = windowSize;
    this.data = new Map();
  }

  update(metric: string, value: number): void {
    if (!this.data.has(metric)) {
      this.data.set(metric, []);
    }
    const arr = this.data.get(metric)!;
    arr.push(value);
    if (arr.length > this.windowSize) {
      arr.shift();
    }
  }

  getBaseline(metric: string): { mean: number; std: number; n: number } {
    const values = this.data.get(metric);
    if (!values || values.length === 0) {
      return { mean: 0, std: 0, n: 0 };
    }

    const n = values.length;
    const mean = values.reduce((s, v) => s + v, 0) / n;

    if (n < 2) return { mean, std: 0, n };

    const variance = values.reduce((s, v) => s + (v - mean) ** 2, 0) / (n - 1);
    return { mean, std: Math.sqrt(variance), n };
  }

  isAnomaly(
    metric: string,
    value: number,
    sigmaThreshold: number = 2.0,
    minSamples: number = 5,
  ): boolean {
    const baseline = this.getBaseline(metric);
    if (baseline.n < minSamples) return false;
    if (baseline.std === 0) return value !== baseline.mean;

    const zScore = Math.abs(value - baseline.mean) / baseline.std;
    return zScore > sigmaThreshold;
  }

  zScore(metric: string, value: number): number {
    const baseline = this.getBaseline(metric);
    if (baseline.n < 2 || baseline.std === 0) return 0;
    return (value - baseline.mean) / baseline.std;
  }

  metrics(): string[] {
    return Array.from(this.data.keys());
  }
}
