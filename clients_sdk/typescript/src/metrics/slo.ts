/**
 * SLO (Service Level Objective) Tracker
 * Monitors metrics against defined thresholds
 */

import type { SLOThreshold, SLOStatus, MetricsSummary, PercentileStats } from '../types/metrics.js';
import { DEFAULT_SLOS } from '../types/metrics.js';

/**
 * SLO Tracker monitors metrics against defined thresholds
 */
export class SLOTracker {
  private slos: SLOThreshold[];
  private violations: Map<string, number[]> = new Map();
  private lastCheck: Map<string, { met: boolean; value: number; timestamp: number }> = new Map();

  constructor(slos: SLOThreshold[] = DEFAULT_SLOS) {
    this.slos = slos;
  }

  /**
   * Add an SLO definition
   */
  addSLO(slo: SLOThreshold): void {
    this.slos.push(slo);
  }

  /**
   * Remove an SLO by metric name
   */
  removeSLO(metric: string): void {
    this.slos = this.slos.filter((s) => s.metric !== metric);
    this.violations.delete(metric);
    this.lastCheck.delete(metric);
  }

  /**
   * Check all SLOs against current metrics
   */
  check(metrics: MetricsSummary): SLOStatus[] {
    const results: SLOStatus[] = [];

    for (const slo of this.slos) {
      const status = this.checkSLO(slo, metrics);
      results.push(status);

      // Track violation history
      if (!status.met) {
        const violations = this.violations.get(slo.metric) ?? [];
        violations.push(Date.now());
        // Keep only last 100 violations
        if (violations.length > 100) {
          violations.shift();
        }
        this.violations.set(slo.metric, violations);
      }

      // Update last check
      this.lastCheck.set(slo.metric, {
        met: status.met,
        value: status.currentValue,
        timestamp: Date.now(),
      });
    }

    return results;
  }

  /**
   * Check a single SLO
   */
  private checkSLO(slo: SLOThreshold, metrics: MetricsSummary): SLOStatus {
    const value = this.extractMetricValue(slo, metrics);
    const met = this.compare(value, slo.threshold, slo.operator);

    const violations = this.violations.get(slo.metric) ?? [];
    const lastViolation = violations.length > 0 ? violations[violations.length - 1] : null;

    return {
      slo,
      met,
      currentValue: value,
      timeSinceViolation: lastViolation ? Date.now() - lastViolation : null,
      violationCount: violations.length,
    };
  }

  /**
   * Extract metric value based on SLO definition
   */
  private extractMetricValue(slo: SLOThreshold, metrics: MetricsSummary): number {
    const parts = slo.metric.split('.');

    // Handle bud.* prefix
    if (parts[0] === 'bud') {
      parts.shift();
    }

    const category = parts[0];
    const metricName = parts[1];

    let stats: PercentileStats | undefined;

    switch (category) {
      case 'stt':
        if (metricName === 'ttft_ms' || metricName === 'ttft') {
          stats = metrics.stt.ttft;
        } else if (metricName === 'processing') {
          stats = metrics.stt.processingTime;
        }
        break;
      case 'tts':
        if (metricName === 'ttfb_ms' || metricName === 'ttfb') {
          stats = metrics.tts.ttfb;
        } else if (metricName === 'synthesis') {
          stats = metrics.tts.synthesisTime;
        }
        break;
      case 'ws':
        if (metricName === 'connect_ms' || metricName === 'connect') {
          stats = metrics.ws.connectTime;
        } else if (metricName === 'reconnects') {
          return metrics.ws.reconnectCount;
        }
        break;
      case 'e2e':
        if (metricName === 'latency_ms' || metricName === 'latency') {
          stats = metrics.e2e.latency;
        }
        break;
      case 'audio':
        if (metricName === 'buffer_underruns') {
          return metrics.audio.bufferUnderruns;
        } else if (metricName === 'processing') {
          stats = metrics.audio.processingTime;
        }
        break;
      case 'memory':
        if (metricName === 'heap_mb') {
          return metrics.resource.heapMb;
        }
        break;
    }

    if (stats) {
      // Use percentile if specified, otherwise use mean
      if (slo.percentile) {
        if (slo.percentile === 50) return stats.p50;
        if (slo.percentile === 95) return stats.p95;
        if (slo.percentile === 99) return stats.p99;
      }
      return stats.mean;
    }

    return 0;
  }

  /**
   * Compare value against threshold
   */
  private compare(value: number, threshold: number, operator: SLOThreshold['operator']): boolean {
    switch (operator) {
      case 'lt':
        return value < threshold;
      case 'lte':
        return value <= threshold;
      case 'gt':
        return value > threshold;
      case 'gte':
        return value >= threshold;
      case 'eq':
        return value === threshold;
      default:
        return false;
    }
  }

  /**
   * Get overall SLO health (percentage of SLOs met)
   */
  getHealth(metrics: MetricsSummary): { percentage: number; met: number; total: number } {
    const statuses = this.check(metrics);
    const met = statuses.filter((s) => s.met).length;
    return {
      percentage: statuses.length > 0 ? (met / statuses.length) * 100 : 100,
      met,
      total: statuses.length,
    };
  }

  /**
   * Get SLOs currently in violation
   */
  getViolations(metrics: MetricsSummary): SLOStatus[] {
    return this.check(metrics).filter((s) => !s.met);
  }

  /**
   * Reset violation history
   */
  reset(): void {
    this.violations.clear();
    this.lastCheck.clear();
  }

  /**
   * Get all configured SLOs
   */
  getSLOs(): SLOThreshold[] {
    return [...this.slos];
  }
}
