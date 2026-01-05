/**
 * Performance Metrics Types
 * OpenTelemetry compatible metric definitions
 */

/**
 * Percentile statistics for a metric
 */
export interface PercentileStats {
  /** 50th percentile (median) */
  p50: number;
  /** 95th percentile */
  p95: number;
  /** 99th percentile */
  p99: number;
  /** Minimum value */
  min: number;
  /** Maximum value */
  max: number;
  /** Mean/average value */
  mean: number;
  /** Most recent value */
  last: number;
  /** Total count of samples */
  count: number;
}

/**
 * Single metric data point
 */
export interface MetricPoint {
  /** Metric value */
  value: number;
  /** Timestamp when recorded (ms since epoch) */
  timestamp: number;
  /** Optional labels/tags */
  labels?: Record<string, string>;
}

/**
 * STT-specific metrics
 */
export interface STTMetrics {
  /** Time to First Token in milliseconds */
  ttft: PercentileStats;
  /** Processing time per audio chunk in ms */
  processingTime: PercentileStats;
  /** Number of transcriptions completed */
  transcriptionCount: number;
  /** Total audio duration processed in seconds */
  totalAudioDuration: number;
  /** Character count of all transcriptions */
  totalCharacters: number;
}

/**
 * TTS-specific metrics
 */
export interface TTSMetrics {
  /** Time to First Byte in milliseconds */
  ttfb: PercentileStats;
  /** Total synthesis time in ms */
  synthesisTime: PercentileStats;
  /** Number of speak requests */
  speakCount: number;
  /** Total characters synthesized */
  totalCharacters: number;
  /** Characters per second throughput */
  throughput: PercentileStats;
}

/**
 * WebSocket connection metrics
 */
export interface WebSocketMetrics {
  /** Connection establishment time in ms */
  connectTime: PercentileStats;
  /** Number of reconnection attempts */
  reconnectCount: number;
  /** Messages sent count */
  messagesSent: number;
  /** Messages received count */
  messagesReceived: number;
  /** Bytes sent */
  bytesSent: number;
  /** Bytes received */
  bytesReceived: number;
  /** Current connection state */
  state: 'connecting' | 'connected' | 'disconnected' | 'reconnecting';
}

/**
 * End-to-end latency metrics
 */
export interface E2EMetrics {
  /** Full voice loop latency (STT + processing + TTS) in ms */
  latency: PercentileStats;
  /** Number of complete loops measured */
  loopCount: number;
}

/**
 * Audio processing metrics
 */
export interface AudioMetrics {
  /** Buffer underrun count (playback gaps) */
  bufferUnderruns: number;
  /** Buffer overrun count (dropped samples) */
  bufferOverruns: number;
  /** Audio processing time per chunk in ms */
  processingTime: PercentileStats;
  /** Current buffer level (0-1) */
  bufferLevel: number;
}

/**
 * Memory and resource metrics
 */
export interface ResourceMetrics {
  /** Heap memory usage in MB */
  heapMb: number;
  /** Active WebSocket connections */
  activeConnections: number;
  /** Pending messages in queue */
  pendingMessages: number;
}

/**
 * Complete metrics summary
 */
export interface MetricsSummary {
  /** STT metrics */
  stt: STTMetrics;
  /** TTS metrics */
  tts: TTSMetrics;
  /** WebSocket metrics */
  ws: WebSocketMetrics;
  /** End-to-end metrics */
  e2e: E2EMetrics;
  /** Audio metrics */
  audio: AudioMetrics;
  /** Resource metrics */
  resource: ResourceMetrics;
  /** Timestamp of this summary */
  timestamp: number;
  /** Duration of metrics collection in ms */
  collectionDurationMs: number;
}

/**
 * SLO threshold definition
 */
export interface SLOThreshold {
  /** Metric name */
  metric: string;
  /** Threshold value */
  threshold: number;
  /** Comparison operator */
  operator: 'lt' | 'lte' | 'gt' | 'gte' | 'eq';
  /** Percentile to use for comparison (e.g., 95 for p95) */
  percentile?: number;
  /** Description of this SLO */
  description?: string;
}

/**
 * SLO status
 */
export interface SLOStatus {
  /** SLO definition */
  slo: SLOThreshold;
  /** Whether SLO is currently met */
  met: boolean;
  /** Current value */
  currentValue: number;
  /** Time since last violation in ms (null if never violated) */
  timeSinceViolation: number | null;
  /** Total violation count */
  violationCount: number;
}

/**
 * Default SLO definitions for voice AI
 */
export const DEFAULT_SLOS: SLOThreshold[] = [
  {
    metric: 'bud.stt.ttft_ms',
    threshold: 200,
    operator: 'lt',
    percentile: 95,
    description: 'STT Time to First Token p95 < 200ms',
  },
  {
    metric: 'bud.tts.ttfb_ms',
    threshold: 150,
    operator: 'lt',
    percentile: 95,
    description: 'TTS Time to First Byte p95 < 150ms',
  },
  {
    metric: 'bud.e2e.latency_ms',
    threshold: 1000,
    operator: 'lt',
    percentile: 95,
    description: 'End-to-end latency p95 < 1000ms',
  },
  {
    metric: 'bud.ws.connect_ms',
    threshold: 100,
    operator: 'lt',
    percentile: 95,
    description: 'WebSocket connect time p95 < 100ms',
  },
  {
    metric: 'bud.audio.buffer_underruns',
    threshold: 0,
    operator: 'eq',
    description: 'Zero audio buffer underruns',
  },
];

/**
 * Create empty percentile stats
 */
export function emptyPercentileStats(): PercentileStats {
  return {
    p50: 0,
    p95: 0,
    p99: 0,
    min: 0,
    max: 0,
    mean: 0,
    last: 0,
    count: 0,
  };
}

/**
 * Create empty metrics summary
 */
export function emptyMetricsSummary(): MetricsSummary {
  return {
    stt: {
      ttft: emptyPercentileStats(),
      processingTime: emptyPercentileStats(),
      transcriptionCount: 0,
      totalAudioDuration: 0,
      totalCharacters: 0,
    },
    tts: {
      ttfb: emptyPercentileStats(),
      synthesisTime: emptyPercentileStats(),
      speakCount: 0,
      totalCharacters: 0,
      throughput: emptyPercentileStats(),
    },
    ws: {
      connectTime: emptyPercentileStats(),
      reconnectCount: 0,
      messagesSent: 0,
      messagesReceived: 0,
      bytesSent: 0,
      bytesReceived: 0,
      state: 'disconnected',
    },
    e2e: {
      latency: emptyPercentileStats(),
      loopCount: 0,
    },
    audio: {
      bufferUnderruns: 0,
      bufferOverruns: 0,
      processingTime: emptyPercentileStats(),
      bufferLevel: 0,
    },
    resource: {
      heapMb: 0,
      activeConnections: 0,
      pendingMessages: 0,
    },
    timestamp: Date.now(),
    collectionDurationMs: 0,
  };
}
