/**
 * Agent Session — the flagship `bud.agent(...)` conversation loop.
 *
 * This is the ONLY SDK entry point that reaches the gateway's built-in
 * conversation loop (STT → built-in LLM → TTS) with reasoning, barge-in and
 * latency masking. It serializes `conversation_config` and nests turn detection
 * into `stt_config.turn_detection`, then exposes ONE unified event stream
 * (transcript | audio | message | configWarning | error | ready).
 *
 * A thin wrapper over {@link WebSocketSession} (vs. the browser-audio-heavy
 * BudTalk pipeline) so it runs cleanly in Node for headless agents and tests.
 */

import type { STTConfig, TTSConfig, ConversationConfig } from '../types/config.js';
import type { TurnDetectionConfig } from '../types/audio-features.js';
import type { ReconnectConfig } from '../ws/reconnect.js';
import type { ConnectGate } from '../ws/connect-gate.js';
import { WebSocketSession } from '../ws/session.js';
import type {
  SessionEventMap,
  SessionEventHandler,
  TranscriptEvent,
  AudioEvent,
  ReadyEvent,
} from '../ws/events.js';

/**
 * Turn-detection knobs accepted by `bud.agent(...)`. `eagerEot` is a convenience
 * that ALSO flips `conversation_config.eager_eot` (the gateway requires both for
 * eager end-of-turn). The remaining fields mirror {@link TurnDetectionConfig}.
 */
export interface AgentTurnConfig {
  /** Enable ML turn detection. Defaults to true when a `turn` block is given. */
  enabled?: boolean;
  /** Detection threshold (0.0-1.0). */
  threshold?: number;
  /**
   * Eager end-of-turn: start the LLM speculatively on a turn-complete
   * prediction. Sets BOTH `stt_config.turn_detection.eager` and
   * `conversation_config.eager_eot`.
   */
  eagerEot?: boolean;
}

/**
 * Options for {@link BudClient.agent}. `llm` is the only thing a beginner must
 * provide (its `baseUrl` + `model`); STT/TTS default to Deepgram.
 */
export interface AgentConfig {
  /** STT configuration (defaults: deepgram / en-US / nova-3). */
  stt?: Partial<STTConfig>;
  /** TTS configuration (defaults: deepgram / aura-asteria-en). */
  tts?: Partial<TTSConfig>;
  /**
   * Conversation/LLM-loop configuration. Carries `baseUrl` and `model` plus the
   * optional reasoning / latency-filler / barge-in knobs. OPTIONAL only when an
   * `alias` is given (the server-defined alias then supplies the LLM bundle);
   * required otherwise.
   */
  llm?: ConversationConfig;
  /**
   * Proxy/alias name (P3): a single server-defined name that resolves to a full
   * {stt,tts,llm,dag} bundle. `bud.agent({ alias: 'support-bot' })` is a complete
   * agent — ops can re-point what "support-bot" means with zero client change.
   * Any explicit `stt`/`tts`/`llm` here overrides the alias default.
   */
  alias?: string;
  /** Turn-detection knobs (see {@link AgentTurnConfig}). */
  turn?: AgentTurnConfig;
  /** Reconnection configuration (or false to disable). */
  reconnect?: ReconnectConfig | false;
  /** Optional stream ID for session tracking. */
  streamId?: string;
  /** Custom WebSocket implementation (Node). */
  WebSocket?: typeof WebSocket;
}

/**
 * Defaults applied when the caller omits STT/TTS bits. The STT defaults include
 * the gateway-REQUIRED fields (sample_rate / channels / punctuation) so the
 * config message always deserializes server-side (STTWebSocketConfig requires
 * them, config.rs:296-313).
 */
const DEFAULT_AGENT_STT: Pick<STTConfig, 'provider' | 'language' | 'model' | 'sampleRate' | 'channels' | 'punctuation'> = {
  provider: 'deepgram',
  language: 'en-US',
  model: 'nova-3',
  sampleRate: 16000,
  channels: 1,
  punctuation: true,
};
const DEFAULT_AGENT_TTS: Pick<TTSConfig, 'provider' | 'model' | 'voiceId'> = {
  provider: 'deepgram',
  model: 'aura-asteria-en',
  voiceId: 'aura-asteria-en',
};

/**
 * Build the resolved {@link WebSocketSession} config (STT/TTS/conversation/turn)
 * from the beginner-facing {@link AgentConfig}. Exposed for unit tests so the
 * exact wire shape can be asserted without a live connection.
 */
export function buildAgentSessionConfig(
  config: AgentConfig,
  url: string,
  apiKey?: string,
  connectGate?: ConnectGate
): {
  url: string;
  apiKey?: string;
  WebSocket?: typeof WebSocket;
  reconnect?: ReconnectConfig | false;
  audio: true;
  streamId?: string;
  stt: STTConfig;
  tts: TTSConfig;
  conversation?: ConversationConfig;
  alias?: string;
  turnDetection?: TurnDetectionConfig;
  connectGate?: ConnectGate;
} {
  // An agent needs an LLM from SOMEWHERE: an explicit `llm`, or a server `alias`
  // that resolves one. (stt/tts have sane defaults; the LLM loop does not.)
  if (!config.llm && !config.alias) {
    throw new Error('bud.agent requires `llm` (or an `alias` that supplies it)');
  }

  const stt: STTConfig = { ...DEFAULT_AGENT_STT, ...config.stt };
  const tts: TTSConfig = { ...DEFAULT_AGENT_TTS, ...config.tts };

  // Clone the conversation config so flipping eager_eot doesn't mutate caller
  // state. When only an `alias` is given, the gateway's alias resolves the LLM
  // bundle server-side, so the client sends no conversation block here.
  const conversation: ConversationConfig | undefined = config.llm ? { ...config.llm } : undefined;

  let turnDetection: TurnDetectionConfig | undefined;
  if (config.turn) {
    turnDetection = {
      enabled: config.turn.enabled ?? true,
    };
    if (config.turn.threshold !== undefined) turnDetection.threshold = config.turn.threshold;
    if (config.turn.eagerEot !== undefined) turnDetection.eager = config.turn.eagerEot;
    // Eager EoT requires BOTH the turn-detection eager flag and the conversation
    // eager_eot flag; forward it unless the caller set eager_eot explicitly.
    if (config.turn.eagerEot && conversation && conversation.eagerEot === undefined) {
      conversation.eagerEot = true;
    }
  }

  const out = {
    url,
    audio: true as const,
    stt,
    tts,
  } as ReturnType<typeof buildAgentSessionConfig>;
  if (conversation !== undefined) out.conversation = conversation;
  if (config.alias !== undefined) out.alias = config.alias;
  if (apiKey !== undefined) out.apiKey = apiKey;
  if (config.WebSocket !== undefined) out.WebSocket = config.WebSocket;
  if (config.reconnect !== undefined) out.reconnect = config.reconnect;
  if (config.streamId !== undefined) out.streamId = config.streamId;
  if (turnDetection !== undefined) out.turnDetection = turnDetection;
  // D7: pass the shared connect gate through (BudClient injects it) so the agent
  // session's connect paces with the rest of the client's sessions.
  if (connectGate !== undefined) out.connectGate = connectGate;
  return out;
}

/**
 * A connected (or connectable) agent conversation. Wraps a
 * {@link WebSocketSession}: `connect()` then listen on the unified event stream.
 *
 * @example
 * ```ts
 * const agent = bud.agent({
 *   stt: { provider: 'deepgram', language: 'en-US' },
 *   tts: { provider: 'deepgram', voiceId: 'aura-asteria-en' },
 *   llm: { baseUrl: 'http://localhost:11434/v1', model: 'llama3.2:1b',
 *          reasoningEffort: 'minimal', latencyFiller: 'auto' },
 *   turn: { eagerEot: true },
 * });
 * agent.on('transcript', (t) => console.log('user:', t.text));
 * agent.on('audio', (a) => playback(a.audio));   // bot speech
 * agent.on('configWarning', (w) => console.warn(w.code, w.message));
 * await agent.connect();
 * agent.sendAudio(pcm);
 * ```
 */
export class AgentSession {
  private session: WebSocketSession;

  constructor(config: AgentConfig, url: string, apiKey?: string, connectGate?: ConnectGate) {
    this.session = new WebSocketSession(buildAgentSessionConfig(config, url, apiKey, connectGate));
  }

  /** Connect and wait for the gateway `ready` handshake. */
  async connect(): Promise<ReadyEvent> {
    await this.session.connect();
    return this.session.waitForReady();
  }

  /** Disconnect the session. */
  async disconnect(): Promise<void> {
    return this.session.disconnect();
  }

  /** Stream microphone / file audio to the gateway (raw PCM frames). */
  sendAudio(data: ArrayBuffer | Uint8Array): void {
    this.session.sendAudio(data);
  }

  /**
   * Signal end-of-input so the gateway finalizes the current transcript and
   * lets the conversation loop respond (gateway `audio_end`).
   */
  audioEnd(): void {
    this.session.audioEnd();
  }

  /** Barge-in: stop the bot's current speech immediately (gateway `clear`). */
  clear(): void {
    this.session.clear();
  }

  /** Inject a text turn into the conversation (gateway `send_message`). */
  sendMessage(message: string, role = 'user'): void {
    this.session.sendMessage(message, role);
  }

  /** Subscribe to a session event (transcript | audio | configWarning | error | ready | ...). */
  on<K extends keyof SessionEventMap>(event: K, handler: SessionEventHandler<K>): () => void {
    return this.session.on(event, handler);
  }

  /** Subscribe once. */
  once<K extends keyof SessionEventMap>(event: K, handler: SessionEventHandler<K>): () => void {
    return this.session.once(event, handler);
  }

  /** Unsubscribe. */
  off<K extends keyof SessionEventMap>(event: K, handler: SessionEventHandler<K>): void {
    this.session.off(event, handler);
  }

  /** Whether the session is connected. */
  isConnected(): boolean {
    return this.session.isConnected();
  }

  /** Whether the gateway `ready` handshake has completed. */
  isReady(): boolean {
    return this.session.isReady();
  }

  /** Gateway-assigned session id (after `ready`). */
  getSessionId(): string | null {
    return this.session.getSessionId();
  }

  /** Escape hatch: the underlying {@link WebSocketSession}. */
  getSession(): WebSocketSession {
    return this.session;
  }
}

export type { TranscriptEvent, AudioEvent, ReadyEvent };
