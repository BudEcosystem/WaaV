/**
 * BudWidget Web Component
 */

import { parseConfigFromAttributes, mergeConfig } from './config';
import { StateMachine } from './state';
import { WidgetWebSocket } from './websocket';
import { AudioRecorder } from './audio/recorder';
import { AudioPlayer } from './audio/player';
import { widgetStyles } from './ui/styles';
import { getIcon } from './ui/icons';
import type { WidgetConfig, WidgetState, TranscriptResult, WidgetMetrics, WidgetEventMap } from './types';

export class BudWidget extends HTMLElement {
  private config: WidgetConfig;
  private state: StateMachine;
  private ws: WidgetWebSocket | null = null;
  private recorder: AudioRecorder | null = null;
  private player: AudioPlayer | null = null;
  private shadow: ShadowRoot;
  private button: HTMLButtonElement | null = null;
  private panel: HTMLDivElement | null = null;
  private transcriptEl: HTMLDivElement | null = null;
  private metricsEl: HTMLDivElement | null = null;
  private isPanelOpen = false;
  private currentTranscript = '';
  private interimTranscript = '';
  private streamId: string | null = null;

  constructor() {
    super();

    this.shadow = this.attachShadow({ mode: 'open' });
    this.state = new StateMachine();
    this.config = mergeConfig({});

    // Subscribe to state changes
    this.state.subscribe((state, previousState) => {
      this.updateUI();
      this.dispatchEvent(
        new CustomEvent('stateChange', {
          detail: { state, previousState },
          bubbles: true,
          composed: true,
        })
      );
    });
  }

  static get observedAttributes(): string[] {
    return [
      'data-gateway-url',
      'data-api-key',
      'data-auth-token',
      'data-theme',
      'data-position',
      'data-mode',
      'data-show-metrics',
      'data-stt-provider',
      'data-tts-provider',
      'data-tts-voice',
    ];
  }

  connectedCallback(): void {
    // Parse config from attributes
    this.config = mergeConfig(parseConfigFromAttributes(this));

    // Set data attributes for CSS
    this.dataset.theme = this.config.theme;
    this.dataset.position = this.config.position;

    // Render initial UI
    this.render();
  }

  disconnectedCallback(): void {
    this.disconnect();
  }

  attributeChangedCallback(name: string, oldValue: string, newValue: string): void {
    if (oldValue !== newValue) {
      this.config = mergeConfig(parseConfigFromAttributes(this));
      this.dataset.theme = this.config.theme;
      this.dataset.position = this.config.position;
    }
  }

  private render(): void {
    this.shadow.innerHTML = `
      <style>${widgetStyles}</style>
      ${this.config.customCss ? `<style>${this.config.customCss}</style>` : ''}
      <div class="bud-widget">
        <div class="bud-panel" id="panel">
          <div class="bud-panel-header">
            <span class="bud-panel-title">Voice Assistant</span>
            <span class="bud-panel-status" id="status">Disconnected</span>
          </div>
          <div class="bud-panel-content">
            <div class="bud-transcript" id="transcript">
              <div class="bud-transcript-empty">Click the microphone to start</div>
            </div>
            ${this.config.showMetrics ? `
              <div class="bud-metrics" id="metrics">
                <div class="bud-metric">
                  <span class="bud-metric-label">STT TTFT</span>
                  <span class="bud-metric-value" id="metric-stt-ttft">-</span>
                </div>
                <div class="bud-metric">
                  <span class="bud-metric-label">TTS TTFB</span>
                  <span class="bud-metric-value" id="metric-tts-ttfb">-</span>
                </div>
                <div class="bud-metric">
                  <span class="bud-metric-label">Messages Sent</span>
                  <span class="bud-metric-value" id="metric-sent">0</span>
                </div>
                <div class="bud-metric">
                  <span class="bud-metric-label">Messages Received</span>
                  <span class="bud-metric-value" id="metric-received">0</span>
                </div>
              </div>
            ` : ''}
          </div>
        </div>
        <button class="bud-button" id="main-button" type="button" aria-label="Voice assistant">
          ${getIcon('microphone')}
        </button>
      </div>
    `;

    // Get element references
    this.button = this.shadow.getElementById('main-button') as HTMLButtonElement;
    this.panel = this.shadow.getElementById('panel') as HTMLDivElement;
    this.transcriptEl = this.shadow.getElementById('transcript') as HTMLDivElement;
    this.metricsEl = this.shadow.getElementById('metrics') as HTMLDivElement;

    // Add event listeners
    this.button.addEventListener('click', () => this.handleButtonClick());
    this.button.addEventListener('mousedown', () => this.handleButtonDown());
    this.button.addEventListener('mouseup', () => this.handleButtonUp());
    this.button.addEventListener('mouseleave', () => this.handleButtonUp());

    // Touch events for push-to-talk on mobile
    this.button.addEventListener('touchstart', (e) => {
      e.preventDefault();
      this.handleButtonDown();
    });
    this.button.addEventListener('touchend', (e) => {
      e.preventDefault();
      this.handleButtonUp();
    });
  }

  private handleButtonClick(): void {
    const state = this.state.state;

    if (state === 'idle') {
      // Toggle panel
      this.togglePanel();

      // If panel is now open, connect
      if (this.isPanelOpen) {
        this.connect();
      }
    } else if (state === 'connected' || state === 'listening' || state === 'speaking') {
      // Toggle listening
      if (state === 'listening') {
        this.stopListening();
      } else if (state === 'connected') {
        this.startListening();
      }
    }
  }

  private handleButtonDown(): void {
    if (this.config.mode === 'push-to-talk' && this.state.is('connected')) {
      this.startListening();
    }
  }

  private handleButtonUp(): void {
    if (this.config.mode === 'push-to-talk' && this.state.is('listening')) {
      this.stopListening();
    }
  }

  private togglePanel(): void {
    this.isPanelOpen = !this.isPanelOpen;
    if (this.panel) {
      this.panel.classList.toggle('open', this.isPanelOpen);
    }
  }

  async connect(): Promise<void> {
    if (this.state.isAny('connecting', 'connected', 'listening', 'speaking')) {
      return;
    }

    this.state.transition('connecting');

    try {
      // Initialize WebSocket
      this.ws = new WidgetWebSocket(this.config, {
        onReady: (streamId) => {
          this.streamId = streamId;
          this.state.transition('connected');
          this.updateStatus('Connected', 'connected');
          this.dispatchEvent(
            new CustomEvent('ready', {
              detail: { streamId },
              bubbles: true,
              composed: true,
            })
          );

          // Auto-start listening in VAD mode
          if (this.config.mode === 'vad') {
            this.startListening();
          }
        },
        onTranscript: (result) => {
          this.handleTranscript(result);
        },
        onAudio: (audio, format, sampleRate, isFinal) => {
          this.handleAudio(audio, format, sampleRate, isFinal);
        },
        onPlaybackComplete: () => {
          if (this.state.is('speaking')) {
            this.state.transition('listening');
          }
        },
        onError: (error) => {
          this.handleError(error);
        },
        onClose: () => {
          if (!this.state.is('idle')) {
            this.state.transition('idle');
            this.updateStatus('Disconnected', '');
          }
        },
      });

      await this.ws.connect();

      // Initialize audio player
      this.player = new AudioPlayer({
        sampleRate: this.config.tts?.sampleRate || 24000,
      });
      await this.player.initialize();
    } catch (error) {
      this.handleError(error as Error);
    }
  }

  async disconnect(): Promise<void> {
    this.stopListening();

    if (this.ws) {
      this.ws.disconnect();
      this.ws = null;
    }

    if (this.player) {
      this.player.close();
      this.player = null;
    }

    this.state.reset();
    this.updateStatus('Disconnected', '');
  }

  private async startListening(): Promise<void> {
    if (!this.state.is('connected')) return;

    try {
      this.recorder = new AudioRecorder({
        sampleRate: this.config.stt?.sampleRate || 16000,
        echoCancellation: this.config.features?.echoCancellation,
        noiseSuppression: this.config.features?.noiseCancellation,
      });

      this.recorder.onData((data) => {
        if (this.ws && this.state.is('listening')) {
          this.ws.sendAudio(data);
        }
      });

      this.recorder.onSilence(() => {
        // In VAD mode, don't stop on silence
        // The server will handle end of speech detection
      });

      await this.recorder.start();
      this.state.transition('listening');
    } catch (error) {
      this.handleError(error as Error);
    }
  }

  private stopListening(): void {
    if (this.recorder) {
      this.recorder.stop();
      this.recorder = null;
    }

    if (this.state.is('listening')) {
      this.state.transition('connected');
    }
  }

  private handleTranscript(result: TranscriptResult): void {
    if (result.isFinal) {
      this.currentTranscript += result.text + ' ';
      this.interimTranscript = '';
    } else {
      this.interimTranscript = result.text;
    }

    this.updateTranscriptDisplay();

    this.dispatchEvent(
      new CustomEvent('transcript', {
        detail: result,
        bubbles: true,
        composed: true,
      })
    );
  }

  private handleAudio(audio: ArrayBuffer, format: string, sampleRate: number, isFinal: boolean): void {
    if (this.player) {
      // Stop recording while speaking to prevent feedback
      if (this.state.is('listening')) {
        this.stopListening();
      }

      if (!this.state.is('speaking')) {
        this.state.transition('speaking');
      }

      this.player.play(audio);
    }

    this.dispatchEvent(
      new CustomEvent('audio', {
        detail: { audio, format, sampleRate, isFinal },
        bubbles: true,
        composed: true,
      })
    );
  }

  private handleError(error: Error): void {
    console.error('BudWidget error:', error);
    this.state.transition('error');
    this.updateStatus('Error: ' + error.message, 'error');

    this.dispatchEvent(
      new CustomEvent('error', {
        detail: error,
        bubbles: true,
        composed: true,
      })
    );

    // Auto-recover after a short delay
    setTimeout(() => {
      if (this.state.is('error')) {
        this.state.transition('idle');
        this.updateStatus('Disconnected', '');
      }
    }, 3000);
  }

  private updateUI(): void {
    if (!this.button) return;

    const state = this.state.state;

    // Update button class
    this.button.className = `bud-button ${state}`;

    // Update button icon
    switch (state) {
      case 'idle':
        this.button.innerHTML = getIcon('microphone');
        break;
      case 'connecting':
        this.button.innerHTML = getIcon('loading');
        break;
      case 'connected':
        this.button.innerHTML = getIcon('microphone');
        break;
      case 'listening':
        this.button.innerHTML = getIcon('microphone');
        break;
      case 'speaking':
        this.button.innerHTML = getIcon('speaker');
        break;
      case 'error':
        this.button.innerHTML = getIcon('error');
        break;
    }

    // Update metrics if enabled
    if (this.config.showMetrics && this.ws) {
      this.updateMetricsDisplay();
    }
  }

  private updateStatus(text: string, className: string): void {
    const statusEl = this.shadow.getElementById('status');
    if (statusEl) {
      statusEl.textContent = text;
      statusEl.className = `bud-panel-status ${className}`;
    }
  }

  private updateTranscriptDisplay(): void {
    if (!this.transcriptEl) return;

    const fullTranscript = this.currentTranscript.trim();
    const interim = this.interimTranscript.trim();

    if (!fullTranscript && !interim) {
      this.transcriptEl.innerHTML = '<div class="bud-transcript-empty">Listening...</div>';
    } else {
      this.transcriptEl.innerHTML = `
        <div class="bud-transcript-text">${fullTranscript}</div>
        ${interim ? `<div class="bud-transcript-text interim">${interim}</div>` : ''}
      `;
    }
  }

  private updateMetricsDisplay(): void {
    if (!this.ws || !this.config.showMetrics) return;

    const metrics = this.ws.metrics;

    const sttTtftEl = this.shadow.getElementById('metric-stt-ttft');
    const ttsTtfbEl = this.shadow.getElementById('metric-tts-ttfb');
    const sentEl = this.shadow.getElementById('metric-sent');
    const receivedEl = this.shadow.getElementById('metric-received');

    if (sttTtftEl) {
      sttTtftEl.textContent = metrics.sttTtft ? `${Math.round(metrics.sttTtft)}ms` : '-';
    }
    if (ttsTtfbEl) {
      ttsTtfbEl.textContent = metrics.ttsTtfb ? `${Math.round(metrics.ttsTtfb)}ms` : '-';
    }
    if (sentEl) {
      sentEl.textContent = String(metrics.messagesSent);
    }
    if (receivedEl) {
      receivedEl.textContent = String(metrics.messagesReceived);
    }

    this.dispatchEvent(
      new CustomEvent('metrics', {
        detail: metrics,
        bubbles: true,
        composed: true,
      })
    );
  }

  // Public API methods

  /**
   * Speak text through TTS
   */
  speak(text: string, flush = false): void {
    if (this.ws && this.state.isAny('connected', 'listening', 'speaking')) {
      this.ws.speak(text, flush);
    }
  }

  /**
   * Stop current TTS playback
   */
  clear(): void {
    if (this.ws) {
      this.ws.clear();
    }
    if (this.player) {
      this.player.stop();
    }
    if (this.state.is('speaking')) {
      this.state.transition('connected');
    }
  }

  /**
   * Get current metrics
   */
  getMetrics(): WidgetMetrics | null {
    return this.ws?.metrics || null;
  }

  /**
   * Get current state
   */
  getState(): WidgetState {
    return this.state.state;
  }

  /**
   * Check if connected
   */
  get connected(): boolean {
    return this.state.isAny('connected', 'listening', 'speaking');
  }
}

// Define custom element
export function defineWidget(): void {
  if (!customElements.get('bud-widget')) {
    customElements.define('bud-widget', BudWidget);
  }
}
