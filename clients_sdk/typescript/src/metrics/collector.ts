/**
 * Metrics Collector
 * Collects and aggregates performance metrics for the SDK
 */

import type {
  MetricsSummary,
  STTMetrics,
  TTSMetrics,
  WebSocketMetrics,
  E2EMetrics,
  AudioMetrics,
  ResourceMetrics,
} from '../types/metrics.js';
import { emptyMetricsSummary, emptyPercentileStats } from '../types/metrics.js';
import { PercentileTracker } from './percentile.js';

/**
 * Metric names for recording
 */
export type MetricName =
  | 'stt.ttft'
  | 'stt.processing'
  | 'tts.ttfb'
  | 'tts.synthesis'
  | 'tts.throughput'
  | 'ws.connect'
  | 'e2e.latency'
  | 'audio.processing';

/**
 * MetricsCollector aggregates performance metrics
 */
export class MetricsCollector {
  private trackers: Map<MetricName, PercentileTracker> = new Map();
  private counters: Map<string, number> = new Map();
  private gauges: Map<string, number> = new Map();
  private startTime: number;
  private wsState: 'connecting' | 'connected' | 'disconnected' | 'reconnecting' = 'disconnected';

  constructor() {
    this.startTime = Date.now();
    this.initTrackers();
  }

  private initTrackers(): void {
    const metrics: MetricName[] = [
      'stt.ttft',
      'stt.processing',
      'tts.ttfb',
      'tts.synthesis',
      'tts.throughput',
      'ws.connect',
      'e2e.latency',
      'audio.processing',
    ];

    for (const metric of metrics) {
      this.trackers.set(metric, new PercentileTracker());
    }
  }

  /**
   * Record a timing metric
   */
  record(metric: MetricName, value: number): void {
    const tracker = this.trackers.get(metric);
    if (tracker) {
      tracker.record(value);
    }
  }

  /**
   * Increment a counter
   */
  increment(counter: string, value = 1): void {
    const current = this.counters.get(counter) ?? 0;
    this.counters.set(counter, current + value);
  }

  /**
   * Set a gauge value
   */
  setGauge(gauge: string, value: number): void {
    this.gauges.set(gauge, value);
  }

  /**
   * Get a gauge value
   */
  getGauge(gauge: string): number {
    return this.gauges.get(gauge) ?? 0;
  }

  /**
   * Get a counter value
   */
  getCounter(counter: string): number {
    return this.counters.get(counter) ?? 0;
  }

  /**
   * Set WebSocket connection state
   */
  setWSState(state: 'connecting' | 'connected' | 'disconnected' | 'reconnecting'): void {
    this.wsState = state;
  }

  /**
   * Get STT metrics
   */
  getSTTMetrics(): STTMetrics {
    return {
      ttft: this.trackers.get('stt.ttft')?.getStats() ?? emptyPercentileStats(),
      processingTime: this.trackers.get('stt.processing')?.getStats() ?? emptyPercentileStats(),
      transcriptionCount: this.getCounter('stt.transcriptions'),
      totalAudioDuration: this.getGauge('stt.audioDuration'),
      totalCharacters: this.getCounter('stt.characters'),
    };
  }

  /**
   * Get TTS metrics
   */
  getTTSMetrics(): TTSMetrics {
    return {
      ttfb: this.trackers.get('tts.ttfb')?.getStats() ?? emptyPercentileStats(),
      synthesisTime: this.trackers.get('tts.synthesis')?.getStats() ?? emptyPercentileStats(),
      speakCount: this.getCounter('tts.speaks'),
      totalCharacters: this.getCounter('tts.characters'),
      throughput: this.trackers.get('tts.throughput')?.getStats() ?? emptyPercentileStats(),
    };
  }

  /**
   * Get WebSocket metrics
   */
  getWSMetrics(): WebSocketMetrics {
    return {
      connectTime: this.trackers.get('ws.connect')?.getStats() ?? emptyPercentileStats(),
      reconnectCount: this.getCounter('ws.reconnects'),
      messagesSent: this.getCounter('ws.sent'),
      messagesReceived: this.getCounter('ws.received'),
      bytesSent: this.getCounter('ws.bytesSent'),
      bytesReceived: this.getCounter('ws.bytesReceived'),
      state: this.wsState,
    };
  }

  /**
   * Get E2E metrics
   */
  getE2EMetrics(): E2EMetrics {
    return {
      latency: this.trackers.get('e2e.latency')?.getStats() ?? emptyPercentileStats(),
      loopCount: this.getCounter('e2e.loops'),
    };
  }

  /**
   * Get audio metrics
   */
  getAudioMetrics(): AudioMetrics {
    return {
      bufferUnderruns: this.getCounter('audio.underruns'),
      bufferOverruns: this.getCounter('audio.overruns'),
      processingTime: this.trackers.get('audio.processing')?.getStats() ?? emptyPercentileStats(),
      bufferLevel: this.getGauge('audio.bufferLevel'),
    };
  }

  /**
   * Get resource metrics
   */
  getResourceMetrics(): ResourceMetrics {
    // Attempt to get heap size if available (Node.js)
    let heapMb = 0;
    if (typeof process !== 'undefined' && process.memoryUsage) {
      heapMb = process.memoryUsage().heapUsed / (1024 * 1024);
    } else if (typeof performance !== 'undefined' && 'memory' in performance) {
      const memory = (performance as unknown as { memory: { usedJSHeapSize: number } }).memory;
      heapMb = memory.usedJSHeapSize / (1024 * 1024);
    }

    return {
      heapMb,
      activeConnections: this.getGauge('connections.active'),
      pendingMessages: this.getGauge('messages.pending'),
    };
  }

  /**
   * Get complete metrics summary
   */
  getMetrics(): MetricsSummary {
    return {
      stt: this.getSTTMetrics(),
      tts: this.getTTSMetrics(),
      ws: this.getWSMetrics(),
      e2e: this.getE2EMetrics(),
      audio: this.getAudioMetrics(),
      resource: this.getResourceMetrics(),
      timestamp: Date.now(),
      collectionDurationMs: Date.now() - this.startTime,
    };
  }

  /**
   * Reset all metrics
   */
  reset(): void {
    for (const tracker of this.trackers.values()) {
      tracker.reset();
    }
    this.counters.clear();
    this.gauges.clear();
    this.startTime = Date.now();
    this.wsState = 'disconnected';
  }
}

/**
 * Global metrics collector instance
 */
let globalCollector: MetricsCollector | null = null;

/**
 * Get or create the global metrics collector
 */
export function getMetricsCollector(): MetricsCollector {
  if (!globalCollector) {
    globalCollector = new MetricsCollector();
  }
  return globalCollector;
}

/**
 * Reset the global metrics collector
 */
export function resetMetricsCollector(): void {
  globalCollector?.reset();
}
