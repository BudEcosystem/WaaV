/**
 * Percentile calculation using a simple reservoir sampling approach
 * For production, consider using TDigest for more memory-efficient streaming percentiles
 */

import type { PercentileStats } from '../types/metrics.js';

/**
 * Maximum number of samples to keep for percentile calculation
 */
const MAX_SAMPLES = 1000;

/**
 * Percentile tracker for a single metric
 */
export class PercentileTracker {
  private samples: number[] = [];
  private sorted = false;
  private _min = Infinity;
  private _max = -Infinity;
  private _sum = 0;
  private _count = 0;
  private _last = 0;

  /**
   * Record a new value
   */
  record(value: number): void {
    this._last = value;
    this._count++;
    this._sum += value;
    this._min = Math.min(this._min, value);
    this._max = Math.max(this._max, value);

    // Reservoir sampling to keep memory bounded
    if (this.samples.length < MAX_SAMPLES) {
      this.samples.push(value);
    } else {
      // Replace a random element with probability MAX_SAMPLES / count
      const index = Math.floor(Math.random() * this._count);
      if (index < MAX_SAMPLES) {
        this.samples[index] = value;
      }
    }
    this.sorted = false;
  }

  /**
   * Get percentile value (0-100)
   */
  percentile(p: number): number {
    if (this.samples.length === 0) return 0;

    if (!this.sorted) {
      this.samples.sort((a, b) => a - b);
      this.sorted = true;
    }

    const index = Math.ceil((p / 100) * this.samples.length) - 1;
    return this.samples[Math.max(0, Math.min(index, this.samples.length - 1))] ?? 0;
  }

  /**
   * Get all percentile stats
   */
  getStats(): PercentileStats {
    return {
      p50: this.percentile(50),
      p95: this.percentile(95),
      p99: this.percentile(99),
      min: this._count > 0 ? this._min : 0,
      max: this._count > 0 ? this._max : 0,
      mean: this._count > 0 ? this._sum / this._count : 0,
      last: this._last,
      count: this._count,
    };
  }

  /**
   * Reset all data
   */
  reset(): void {
    this.samples = [];
    this.sorted = false;
    this._min = Infinity;
    this._max = -Infinity;
    this._sum = 0;
    this._count = 0;
    this._last = 0;
  }

  /**
   * Get number of samples recorded
   */
  get count(): number {
    return this._count;
  }

  /**
   * Get last recorded value
   */
  get last(): number {
    return this._last;
  }

  /**
   * Get mean value
   */
  get mean(): number {
    return this._count > 0 ? this._sum / this._count : 0;
  }

  /**
   * Get min value
   */
  get min(): number {
    return this._count > 0 ? this._min : 0;
  }

  /**
   * Get max value
   */
  get max(): number {
    return this._count > 0 ? this._max : 0;
  }
}

/**
 * Sliding window percentile tracker
 * Only keeps samples from the last N milliseconds
 */
export class SlidingWindowPercentileTracker {
  private samples: Array<{ value: number; timestamp: number }> = [];
  private windowMs: number;
  private _min = Infinity;
  private _max = -Infinity;

  constructor(windowMs = 60000) {
    this.windowMs = windowMs;
  }

  /**
   * Record a new value
   */
  record(value: number): void {
    const now = Date.now();
    this.samples.push({ value, timestamp: now });
    this._min = Math.min(this._min, value);
    this._max = Math.max(this._max, value);
    this.cleanup(now);
  }

  /**
   * Remove old samples outside the window
   */
  private cleanup(now: number): void {
    const cutoff = now - this.windowMs;
    while (this.samples.length > 0 && (this.samples[0]?.timestamp ?? 0) < cutoff) {
      this.samples.shift();
    }

    // Recalculate min/max after cleanup
    if (this.samples.length > 0) {
      this._min = Math.min(...this.samples.map((s) => s.value));
      this._max = Math.max(...this.samples.map((s) => s.value));
    } else {
      this._min = Infinity;
      this._max = -Infinity;
    }
  }

  /**
   * Get percentile value (0-100)
   */
  percentile(p: number): number {
    this.cleanup(Date.now());
    if (this.samples.length === 0) return 0;

    const values = this.samples.map((s) => s.value).sort((a, b) => a - b);
    const index = Math.ceil((p / 100) * values.length) - 1;
    return values[Math.max(0, Math.min(index, values.length - 1))] ?? 0;
  }

  /**
   * Get all percentile stats
   */
  getStats(): PercentileStats {
    this.cleanup(Date.now());
    const values = this.samples.map((s) => s.value);
    const sum = values.reduce((a, b) => a + b, 0);

    return {
      p50: this.percentile(50),
      p95: this.percentile(95),
      p99: this.percentile(99),
      min: values.length > 0 ? this._min : 0,
      max: values.length > 0 ? this._max : 0,
      mean: values.length > 0 ? sum / values.length : 0,
      last: values.length > 0 ? values[values.length - 1] ?? 0 : 0,
      count: values.length,
    };
  }

  /**
   * Reset all data
   */
  reset(): void {
    this.samples = [];
    this._min = Infinity;
    this._max = -Infinity;
  }

  /**
   * Get number of samples in window
   */
  get count(): number {
    this.cleanup(Date.now());
    return this.samples.length;
  }
}
