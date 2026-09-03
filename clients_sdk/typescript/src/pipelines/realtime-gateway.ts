// =============================================================================
// GatewayRealtime Pipeline
// Gateway-native, provider-agnostic realtime S2S client.
//
// Unlike BudRealtime (pipelines/realtime.ts — the OpenAI/Hume provider-NATIVE
// escape hatch), this client speaks ONLY the WaaV gateway's provider-agnostic
// `/realtime` WebSocket protocol. Because the gateway abstracts every provider
// behind one wire, the SAME surface works for ALL 12 realtime/S2S providers —
// `provider` is just a field.
//
// The exact wire is mirrored from the gateway handler
// (gateway/src/handlers/realtime/messages.rs + handler.rs):
//
// OUT (client -> gateway), JSON text frames (audio is RAW binary, NOT base64):
//   {"type":"config", ...RealtimeSessionConfig}  — first message; selects
//      provider/model/voice/instructions/turn_detection/tools/... and connects
//      the provider server-side.
//   raw binary PCM frames                          — input audio.
//   {"type":"text","text":...}
//   {"type":"create_response","response"?:{...}}    — optional per-response override.
//   {"type":"cancel_response"}                      — barge-in / interrupt.
//   {"type":"commit_audio"} / {"type":"clear_audio"}
//   {"type":"function_result","call_id":...,"result":...}
//   {"type":"update_session", ...RealtimeSessionConfig}
//
// IN (gateway -> client):
//   {"type":"session_created","session_id":...,"provider":...,"model":...}
//   {"type":"session_updated"}
//   {"type":"transcript","text":...,"role":"user|assistant","is_final":bool}
//   {"type":"speech_event","event":"started|stopped","audio_ms":int}
//   {"type":"function_call","call_id":...,"name":...,"arguments":...}
//   {"type":"response_started","response_id":...}
//   {"type":"response_done","response_id":...}
//   {"type":"error","code"?:...,"message":...}
//   {"type":"closing","reason":...}
//   raw binary frames                              — output audio (PCM).
// =============================================================================

import { EventEmitter } from 'events';

/**
 * The 12 realtime/S2S providers reachable through the gateway `/realtime`
 * endpoint (provider-agnostic — the gateway routes each behind the SAME wire).
 * Mirror of the gateway `get_supported_realtime_providers()`
 * (gateway/src/core/realtime/mod.rs:187).
 */
export const REALTIME_PROVIDERS = [
  'openai',
  'hume',
  'azure',
  'grok',
  'inworld',
  'deepgram',
  'elevenlabs',
  'gemini',
  'ultravox',
  'nova_sonic',
  'speechmatics',
  'yandex',
] as const;

/** A realtime provider supported by the gateway `/realtime` endpoint. */
export type GatewayRealtimeProvider = (typeof REALTIME_PROVIDERS)[number];

/** Return the realtime providers the gateway `/realtime` endpoint supports. */
export function getSupportedRealtimeProviders(): readonly GatewayRealtimeProvider[] {
  return REALTIME_PROVIDERS;
}

/** Connection state. */
export type GatewayRealtimeState = 'disconnected' | 'connecting' | 'connected' | 'error';

/**
 * Turn-detection config for the gateway `/realtime` config message. Serializes
 * to the gateway `TurnDetectionConfig` tagged enum
 * (`{"mode":"server_vad"|"semantic"|"manual", ...}`).
 */
export interface GatewayTurnDetection {
  mode: 'server_vad' | 'semantic' | 'manual';
  /** server_vad only */
  threshold?: number;
  /** server_vad only */
  silenceDurationMs?: number;
  /** server_vad only */
  prefixPaddingMs?: number;
  /** semantic only */
  eagerness?: string;
}

/** Function/tool definition exposed to the model. */
export interface GatewayRealtimeTool {
  name: string;
  description?: string;
  parameters?: Record<string, unknown>;
}

/**
 * Provider-agnostic gateway `/realtime` session config. Mirrors the gateway
 * `RealtimeSessionConfig` (gateway/src/handlers/realtime/messages.rs). Every
 * field is optional except `provider`; unset fields are omitted from the wire.
 */
export interface GatewayRealtimeConfig {
  /** Realtime provider — one of {@link REALTIME_PROVIDERS}. */
  provider: GatewayRealtimeProvider;
  model?: string;
  voice?: string;
  instructions?: string;
  temperature?: number;
  maxResponseTokens?: number;
  turnDetection?: GatewayTurnDetection;
  tools?: GatewayRealtimeTool[];
  modalities?: string[];
  transcribeInput?: boolean;
  transcriptionModel?: string;
  inputAudioFormat?: string;
  outputAudioFormat?: string;
  /** `minimal|low|medium|high` for reasoning-capable realtime models. */
  reasoningEffort?: string;
  /** `near_field` | `far_field`. */
  inputAudioNoiseReduction?: string;
  /** Server-side alias bundle name (P3). */
  alias?: string;
}

// ---- Events ----------------------------------------------------------------

export interface GatewayTranscriptEvent {
  text: string;
  isFinal: boolean;
  role: 'user' | 'assistant';
}

export interface GatewayAudioEvent {
  audio: ArrayBuffer;
  sampleRate?: number;
}

export interface GatewayFunctionCallEvent {
  name: string;
  arguments: Record<string, unknown>;
  callId: string;
}

export interface GatewaySpeechEvent {
  event: string; // "started" | "stopped"
  audioMs: number;
}

export interface GatewayErrorEvent {
  message: string;
  code?: string;
}

export interface GatewaySessionCreatedEvent {
  sessionId: string;
  provider: string;
  model: string;
}

export interface GatewayResponseEvent {
  responseId: string;
}

/** A single item on the unified async event stream. */
export type GatewayRealtimeEvent =
  | { type: 'session_created'; session: GatewaySessionCreatedEvent }
  | { type: 'transcript'; transcript: GatewayTranscriptEvent }
  | { type: 'audio'; audio: GatewayAudioEvent }
  | { type: 'speech_event'; speech: GatewaySpeechEvent }
  | { type: 'function_call'; functionCall: GatewayFunctionCallEvent }
  | { type: 'response_started'; response: GatewayResponseEvent }
  | { type: 'response_done'; response: GatewayResponseEvent }
  | { type: 'error'; error: GatewayErrorEvent }
  | { type: 'closing'; reason: string };

/** Typed event map for the EventEmitter callback surface. */
export interface GatewayRealtimeEvents {
  audio: (event: GatewayAudioEvent) => void;
  transcript: (event: GatewayTranscriptEvent) => void;
  functionCall: (event: GatewayFunctionCallEvent) => void;
  speechEvent: (event: GatewaySpeechEvent) => void;
  sessionCreated: (event: GatewaySessionCreatedEvent) => void;
  responseStarted: (event: GatewayResponseEvent) => void;
  responseDone: (event: GatewayResponseEvent) => void;
  error: (error: GatewayErrorEvent) => void;
  connected: () => void;
  disconnected: () => void;
  closing: (reason: string) => void;
}

/** Per-response override for {@link GatewayRealtime.createResponse}. */
export interface GatewayResponseOverride {
  instructions?: string;
  voice?: string;
  modalities?: string[];
  maxOutputTokens?: number;
  outOfBand?: boolean;
  metadata?: Record<string, unknown>;
}

/** Options for {@link GatewayRealtime}. */
export interface GatewayRealtimeOptions {
  /** Gateway `/realtime` WebSocket URL (e.g. `ws://host:port/realtime`). */
  url: string;
  /** Provider-agnostic session config. */
  config: GatewayRealtimeConfig;
  /** Optional gateway auth bearer token. */
  apiKey?: string;
  /** Custom WebSocket implementation (Node.js / tests). */
  WebSocket?: typeof WebSocket;
}

/**
 * Build the `{type:"config", ...}` wire message for a gateway realtime config.
 * Exported so callers/tests can assert the exact provider-agnostic wire.
 */
export function gatewayRealtimeConfigToWire(config: GatewayRealtimeConfig): Record<string, unknown> {
  const msg: Record<string, unknown> = { type: 'config', provider: config.provider };
  const set = (k: string, v: unknown) => {
    if (v !== undefined && v !== null) msg[k] = v;
  };
  set('model', config.model);
  set('voice', config.voice);
  set('instructions', config.instructions);
  set('temperature', config.temperature);
  set('max_response_tokens', config.maxResponseTokens);
  set('modalities', config.modalities);
  set('transcribe_input', config.transcribeInput);
  set('transcription_model', config.transcriptionModel);
  set('input_audio_format', config.inputAudioFormat);
  set('output_audio_format', config.outputAudioFormat);
  set('reasoning_effort', config.reasoningEffort);
  set('input_audio_noise_reduction', config.inputAudioNoiseReduction);
  set('alias', config.alias);

  if (config.turnDetection) {
    const td = config.turnDetection;
    const tdWire: Record<string, unknown> = { mode: td.mode };
    if (td.mode === 'server_vad') {
      if (td.threshold !== undefined) tdWire.threshold = td.threshold;
      if (td.silenceDurationMs !== undefined) tdWire.silence_duration_ms = td.silenceDurationMs;
      if (td.prefixPaddingMs !== undefined) tdWire.prefix_padding_ms = td.prefixPaddingMs;
    } else if (td.mode === 'semantic' && td.eagerness !== undefined) {
      tdWire.eagerness = td.eagerness;
    }
    msg.turn_detection = tdWire;
  }

  if (config.tools && config.tools.length > 0) {
    // Gateway ToolConfig shape: {type:"function", function:{name,description,parameters}}.
    msg.tools = config.tools.map((t) => {
      const fn: Record<string, unknown> = { name: t.name };
      if (t.description !== undefined) fn.description = t.description;
      if (t.parameters !== undefined) fn.parameters = t.parameters;
      return { type: 'function', function: fn };
    });
  }

  return msg;
}

/**
 * A gateway-native, provider-agnostic realtime session. Speaks ONLY the gateway
 * `/realtime` wire, so the same object works for all 12 realtime providers.
 *
 * Construct via {@link BudClient.realtime} (recommended) or directly.
 *
 * Two consumption styles (use either or both):
 *  - Callbacks: `session.on('transcript', handler)` (events: `audio`,
 *    `transcript`, `functionCall`, `speechEvent`, `sessionCreated`,
 *    `responseStarted`, `responseDone`, `error`, `connected`, `disconnected`,
 *    `closing`).
 *  - Unified async stream: `for await (const ev of session.events()) { ... }`.
 *
 * @example
 * ```ts
 * const session = bud.realtime({ provider: 'openai', voice: 'alloy', instructions: 'You are helpful.' });
 * session.on('transcript', (t) => console.log(t.text));
 * session.on('audio', (a) => playback(a.audio));
 * await session.connect();
 * session.sendAudio(pcm);
 * ```
 */
export class GatewayRealtime extends EventEmitter {
  private ws: WebSocket | null = null;
  private _state: GatewayRealtimeState = 'disconnected';
  private _sessionId: string | null = null;
  private readonly url: string;
  private readonly _config: GatewayRealtimeConfig;
  private readonly apiKey?: string;
  private readonly WebSocketImpl: typeof WebSocket;

  // Output sample-rate hint for audio events (gateway S2S output is pcm16@24kHz
  // for the OpenAI-family providers).
  private static readonly OUTPUT_SAMPLE_RATE = 24000;

  // Unified async-stream plumbing (queue + pending-pull resolvers).
  private queue: GatewayRealtimeEvent[] = [];
  private pullResolvers: Array<(v: IteratorResult<GatewayRealtimeEvent>) => void> = [];
  private streamEnded = false;

  constructor(options: GatewayRealtimeOptions) {
    super();
    if (!options.config?.provider) {
      throw new Error('provider is required');
    }
    if (!(REALTIME_PROVIDERS as readonly string[]).includes(options.config.provider)) {
      throw new Error(
        `Unsupported realtime provider: ${options.config.provider}. ` +
          `Supported: ${REALTIME_PROVIDERS.join(', ')}`,
      );
    }
    this.url = options.url;
    this._config = options.config;
    this.apiKey = options.apiKey;
    const WS = options.WebSocket ?? (globalThis as { WebSocket?: typeof WebSocket }).WebSocket;
    if (!WS) {
      throw new Error('No WebSocket implementation available (pass options.WebSocket in Node).');
    }
    this.WebSocketImpl = WS;
  }

  get provider(): GatewayRealtimeProvider {
    return this._config.provider;
  }

  get config(): GatewayRealtimeConfig {
    return this._config;
  }

  get state(): GatewayRealtimeState {
    return this._state;
  }

  get sessionId(): string | null {
    return this._sessionId;
  }

  /** The exact `{type:"config", ...}` wire message this session will send. */
  configMessage(): Record<string, unknown> {
    return gatewayRealtimeConfigToWire(this._config);
  }

  /** Open the gateway WebSocket and send the `config` message. */
  async connect(): Promise<void> {
    if (this._state === 'connected' || this._state === 'connecting') return;
    this._state = 'connecting';

    return new Promise((resolve, reject) => {
      try {
        // Browser WebSocket has no header support; Node `ws` accepts a 2nd arg.
        // Auth header is best-effort (only applied where the impl supports it).
        let ws: WebSocket;
        if (this.apiKey) {
          try {
            ws = new (this.WebSocketImpl as unknown as new (
              url: string,
              opts: { headers: Record<string, string> },
            ) => WebSocket)(this.url, { headers: { Authorization: `Bearer ${this.apiKey}` } });
          } catch {
            ws = new this.WebSocketImpl(this.url);
          }
        } else {
          ws = new this.WebSocketImpl(this.url);
        }
        this.ws = ws;
        // Binary frames should arrive as ArrayBuffer.
        try {
          (ws as { binaryType?: string }).binaryType = 'arraybuffer';
        } catch {
          /* not all impls expose binaryType */
        }

        ws.onopen = () => {
          this._state = 'connected';
          this.emit('connected');
          // First message: provider-agnostic config (connects provider server-side).
          ws.send(JSON.stringify(this.configMessage()));
          resolve();
        };
        ws.onmessage = (event: MessageEvent) => this.handleMessage(event);
        ws.onerror = (err: Event) => {
          const e: GatewayErrorEvent = { message: 'WebSocket error' };
          this.emit('error', e);
          if (this._state === 'connecting') {
            this._state = 'error';
            reject(err instanceof Error ? err : new Error('WebSocket error'));
          }
        };
        ws.onclose = () => {
          this.ws = null;
          if (this._state !== 'error') this._state = 'disconnected';
          this.emit('disconnected');
          this.endStream();
        };
      } catch (error) {
        this._state = 'error';
        reject(error);
      }
    });
  }

  /** Close the session and end the event stream. */
  async disconnect(): Promise<void> {
    if (this.ws) {
      try {
        this.ws.close(1000, 'Normal closure');
      } catch {
        /* ignore */
      }
      this.ws = null;
    }
    if (this._state !== 'disconnected') {
      this._state = 'disconnected';
      this.emit('disconnected');
    }
    this.endStream();
  }

  /** Stream input audio as a raw binary PCM frame (NOT base64). */
  sendAudio(audio: ArrayBuffer): void {
    this.ensureOpen();
    this.ws!.send(audio);
  }

  /** Send a text turn to the conversation. */
  sendText(text: string): void {
    this.sendJson({ type: 'text', text });
  }

  /**
   * Ask the model to generate a response. With no override this sends a bare
   * `{type:"create_response"}` (session defaults); an override adds a
   * per-response `ClientResponseConfig`.
   */
  createResponse(override?: GatewayResponseOverride): void {
    const msg: Record<string, unknown> = { type: 'create_response' };
    if (override) {
      const response: Record<string, unknown> = {};
      if (override.instructions !== undefined) response.instructions = override.instructions;
      if (override.voice !== undefined) response.voice = override.voice;
      if (override.modalities !== undefined) response.modalities = override.modalities;
      if (override.maxOutputTokens !== undefined) response.max_output_tokens = override.maxOutputTokens;
      if (override.outOfBand !== undefined) response.out_of_band = override.outOfBand;
      if (override.metadata !== undefined) response.metadata = override.metadata;
      if (Object.keys(response).length > 0) msg.response = response;
    }
    this.sendJson(msg);
  }

  /** Barge-in: cancel the in-flight response (gateway runs the full sequence). */
  interrupt(): void {
    this.sendJson({ type: 'cancel_response' });
  }

  /** Commit the input audio buffer (manual turn detection). */
  commitAudio(): void {
    this.sendJson({ type: 'commit_audio' });
  }

  /** Clear the input audio buffer. */
  clearAudio(): void {
    this.sendJson({ type: 'clear_audio' });
  }

  /** Return a tool/function result. */
  submitFunctionResult(callId: string, result: unknown): void {
    const payload = typeof result === 'string' ? result : JSON.stringify(result);
    this.sendJson({ type: 'function_result', call_id: callId, result: payload });
  }

  /** Update the live session config mid-stream (`update_session`). */
  updateSession(config?: GatewayRealtimeConfig): void {
    const wire = gatewayRealtimeConfigToWire(config ?? this._config);
    wire.type = 'update_session';
    this.sendJson(wire);
  }

  /**
   * The unified async event stream (same items the callbacks fire). Ends when
   * the socket closes.
   *
   * @example `for await (const ev of session.events()) { ... }`
   */
  async *events(): AsyncGenerator<GatewayRealtimeEvent> {
    while (true) {
      if (this.queue.length > 0) {
        yield this.queue.shift()!;
        continue;
      }
      if (this.streamEnded) return;
      const next = await new Promise<IteratorResult<GatewayRealtimeEvent>>((resolve) => {
        this.pullResolvers.push(resolve);
      });
      if (next.done) return;
      yield next.value;
    }
  }

  // ---- private -------------------------------------------------------------

  private ensureOpen(): void {
    if (this._state !== 'connected' || !this.ws) {
      throw new Error('Not connected');
    }
  }

  private sendJson(message: Record<string, unknown>): void {
    this.ensureOpen();
    this.ws!.send(JSON.stringify(message));
  }

  private pushEvent(ev: GatewayRealtimeEvent): void {
    const resolver = this.pullResolvers.shift();
    if (resolver) {
      resolver({ value: ev, done: false });
    } else {
      this.queue.push(ev);
    }
  }

  private endStream(): void {
    if (this.streamEnded) return;
    this.streamEnded = true;
    while (this.pullResolvers.length > 0) {
      this.pullResolvers.shift()!({ value: undefined as never, done: true });
    }
  }

  private handleMessage(event: MessageEvent): void {
    const data = event.data;
    // Binary frame == output audio (PCM).
    if (data instanceof ArrayBuffer) {
      const audio: GatewayAudioEvent = { audio: data, sampleRate: GatewayRealtime.OUTPUT_SAMPLE_RATE };
      this.emit('audio', audio);
      this.pushEvent({ type: 'audio', audio });
      return;
    }
    // Node `ws` may deliver Buffer; normalize to ArrayBuffer.
    if (typeof data !== 'string') {
      const buf = data as { buffer?: ArrayBuffer; byteOffset?: number; byteLength?: number };
      if (buf.buffer) {
        const ab = buf.buffer.slice(buf.byteOffset ?? 0, (buf.byteOffset ?? 0) + (buf.byteLength ?? 0));
        const audio: GatewayAudioEvent = { audio: ab, sampleRate: GatewayRealtime.OUTPUT_SAMPLE_RATE };
        this.emit('audio', audio);
        this.pushEvent({ type: 'audio', audio });
        return;
      }
    }

    let message: Record<string, unknown>;
    try {
      message = JSON.parse(data as string);
    } catch {
      return;
    }
    this.dispatch(message);
  }

  private dispatch(message: Record<string, unknown>): void {
    const type = message.type as string;
    switch (type) {
      case 'session_created': {
        this._sessionId = (message.session_id as string) ?? null;
        const ev: GatewaySessionCreatedEvent = {
          sessionId: (message.session_id as string) ?? '',
          provider: (message.provider as string) ?? this._config.provider,
          model: (message.model as string) ?? '',
        };
        this.emit('sessionCreated', ev);
        this.pushEvent({ type: 'session_created', session: ev });
        break;
      }
      case 'session_updated':
        // No payload; nothing to surface beyond the implicit ack.
        break;
      case 'transcript': {
        const ev: GatewayTranscriptEvent = {
          text: (message.text as string) ?? '',
          isFinal: (message.is_final as boolean) ?? true,
          role: ((message.role as string) ?? 'assistant') as 'user' | 'assistant',
        };
        this.emit('transcript', ev);
        this.pushEvent({ type: 'transcript', transcript: ev });
        break;
      }
      case 'speech_event': {
        const ev: GatewaySpeechEvent = {
          event: (message.event as string) ?? '',
          audioMs: Number(message.audio_ms ?? 0),
        };
        this.emit('speechEvent', ev);
        this.pushEvent({ type: 'speech_event', speech: ev });
        break;
      }
      case 'function_call': {
        let args: Record<string, unknown> = {};
        try {
          args = JSON.parse((message.arguments as string) ?? '{}');
        } catch {
          args = {};
        }
        const ev: GatewayFunctionCallEvent = {
          name: (message.name as string) ?? '',
          arguments: args,
          callId: (message.call_id as string) ?? '',
        };
        this.emit('functionCall', ev);
        this.pushEvent({ type: 'function_call', functionCall: ev });
        break;
      }
      case 'response_started': {
        const ev: GatewayResponseEvent = { responseId: (message.response_id as string) ?? '' };
        this.emit('responseStarted', ev);
        this.pushEvent({ type: 'response_started', response: ev });
        break;
      }
      case 'response_done': {
        const ev: GatewayResponseEvent = { responseId: (message.response_id as string) ?? '' };
        this.emit('responseDone', ev);
        this.pushEvent({ type: 'response_done', response: ev });
        break;
      }
      case 'error': {
        const ev: GatewayErrorEvent = {
          message: (message.message as string) ?? 'Unknown error',
          code: message.code as string | undefined,
        };
        this.emit('error', ev);
        this.pushEvent({ type: 'error', error: ev });
        break;
      }
      case 'closing': {
        const reason = (message.reason as string) ?? '';
        this.emit('closing', reason);
        this.pushEvent({ type: 'closing', reason });
        break;
      }
      default:
        break;
    }
  }
}
