/**
 * Base Pipeline
 * Abstract base class for all pipeline types
 */

import type { MetricsSummary } from '../types/metrics.js';
import type { FeatureFlags } from '../types/features.js';
import { WebSocketSession, type SessionConfig, type SessionState } from '../ws/session.js';
import { SessionEventEmitter, type SessionEventMap, type SessionEventHandler } from '../ws/events.js';

/**
 * Base pipeline configuration
 */
export interface BasePipelineConfig {
  /** Gateway URL (WebSocket) */
  url: string;
  /** API key for authentication */
  apiKey?: string;
  /** Connection timeout in milliseconds */
  connectionTimeout?: number;
  /** Feature flags */
  features?: FeatureFlags;
  /** Auto-connect on creation (default: false) */
  autoConnect?: boolean;
}

/**
 * Pipeline state
 */
export type PipelineState = SessionState;

/**
 * Abstract base class for pipelines
 */
export abstract class BasePipeline {
  protected session: WebSocketSession;
  protected emitter: SessionEventEmitter;
  protected features?: FeatureFlags;
  protected isInitialized = false;

  constructor(config: BasePipelineConfig & { sessionConfig?: Partial<SessionConfig> }) {
    this.features = config.features;
    this.emitter = new SessionEventEmitter();

    this.session = new WebSocketSession({
      url: config.url,
      apiKey: config.apiKey,
      connectionTimeout: config.connectionTimeout,
      features: config.features,
      autoConfig: true,
      ...config.sessionConfig,
    });

    this.setupEventForwarding();
  }

  /**
   * Forward session events to pipeline emitter
   */
  protected setupEventForwarding(): void {
    // Forward common events
    this.session.on('ready', (e) => this.emitter.emit('ready', e));
    this.session.on('error', (e) => this.emitter.emit('error', e));
    this.session.on('close', (e) => this.emitter.emit('close', e));
    this.session.on('connectionState', (e) => this.emitter.emit('connectionState', e));
    this.session.on('reconnect', (e) => this.emitter.emit('reconnect', e));
    this.session.on('metrics', (e) => this.emitter.emit('metrics', e));
    this.session.on('pong', (e) => this.emitter.emit('pong', e));
  }

  /**
   * Connect to the server
   */
  async connect(): Promise<void> {
    await this.session.connect();
    await this.session.waitForReady();
    this.isInitialized = true;
  }

  /**
   * Disconnect from the server
   */
  async disconnect(): Promise<void> {
    await this.session.disconnect();
    this.isInitialized = false;
  }

  /**
   * Close the pipeline (alias for disconnect)
   */
  async close(): Promise<void> {
    return this.disconnect();
  }

  /**
   * Check if connected
   */
  isConnected(): boolean {
    return this.session.isConnected();
  }

  /**
   * Check if ready
   */
  isReady(): boolean {
    return this.session.isReady();
  }

  /**
   * Get current state
   */
  getState(): PipelineState {
    return this.session.getState();
  }

  /**
   * Get session ID
   */
  getSessionId(): string | null {
    return this.session.getSessionId();
  }

  /**
   * Get current metrics
   */
  getMetrics(): MetricsSummary {
    return this.session.getMetrics();
  }

  /**
   * Update feature flags
   */
  updateFeatures(features: Partial<FeatureFlags>): void {
    this.features = { ...this.features, ...features } as FeatureFlags;
    this.session.updateFeatures(features);
  }

  /**
   * Add event listener
   */
  on<K extends keyof SessionEventMap>(event: K, handler: SessionEventHandler<K>): () => void {
    return this.emitter.on(event, handler);
  }

  /**
   * Add one-time event listener
   */
  once<K extends keyof SessionEventMap>(event: K, handler: SessionEventHandler<K>): () => void {
    return this.emitter.once(event, handler);
  }

  /**
   * Remove event listener
   */
  off<K extends keyof SessionEventMap>(event: K, handler: SessionEventHandler<K>): void {
    this.emitter.off(event, handler);
  }

  /**
   * Stop current operation
   */
  stop(): void {
    this.session.stop();
  }

  /**
   * Interrupt current operation
   */
  interrupt(): void {
    this.session.interrupt();
  }
}
