/**
 * P5 SDK tests (TypeScript): gateway-native realtime + translation + batched STT.
 *
 * Covers the three P5 SDK deliverables:
 *   1. TRANSLATION: `STTConfig.translation` nests under `stt_config.translation`
 *      on the /ws wire (via serializeMessage), and on the batch envelope.
 *   2. BATCH: `bud.transcribeBatch` builds the right `POST /transcribe/batch` body
 *      (audio source, flattened StandardSTTConfig, batch-only knobs, translation)
 *      and polls `GET /transcribe/batch/{job_id}` until terminal.
 *   3. REALTIME: `bud.realtime(...)` builds the gateway PROVIDER-AGNOSTIC
 *      `/realtime` config message (NOT OpenAI's `session.update`) for all 12
 *      providers; the unified event stream + callbacks decode the gateway IN msgs.
 */
import { describe, it, expect, vi } from 'vitest';
import { serializeMessage } from '../../src/ws/messages.js';
import type { SDKConfigMessage } from '../../src/ws/messages.js';
import { BudClient } from '../../src/bud.js';
import {
  GatewayRealtime,
  gatewayRealtimeConfigToWire,
  REALTIME_PROVIDERS,
  getSupportedRealtimeProviders,
} from '../../src/pipelines/realtime-gateway.js';

function wireOf(msg: SDKConfigMessage): Record<string, any> {
  return JSON.parse(serializeMessage(msg));
}

// =============================================================================
// (1) TRANSLATION — canonical block on the STT surface
// =============================================================================

describe('P5 translation block → stt_config.translation', () => {
  it('nests target languages + partials under stt_config.translation', () => {
    const wire = wireOf({
      type: 'config',
      stt: {
        provider: 'speechmatics',
        language: 'en-US',
        translation: { targetLanguages: ['es-ES', 'de-DE'], partials: true },
      },
    });
    expect(wire.stt_config.translation).toEqual({
      target_languages: ['es-ES', 'de-DE'],
      partials: true,
    });
  });

  it('serializes the English fast-path (translateToEnglish)', () => {
    const wire = wireOf({
      type: 'config',
      stt: { provider: 'openai', language: 'auto', translation: { translateToEnglish: true } },
    });
    expect(wire.stt_config.translation).toEqual({ translate_to_english: true });
  });

  it('omits translation when unset', () => {
    const wire = wireOf({
      type: 'config',
      stt: { provider: 'deepgram', language: 'en-US' },
    });
    expect(wire.stt_config.translation).toBeUndefined();
  });
});

// =============================================================================
// (2) BATCHED / ASYNC STT — envelope + poll-until-done
// =============================================================================

/** A fetch stub scripting the submit + poll responses. */
function makeFetchStub(responses: Array<{ status?: number; body: unknown }>) {
  const calls: Array<{ url: string; method: string; body: any }> = [];
  let i = 0;
  const fetchFn = vi.fn(async (url: string, init: RequestInit) => {
    const r = responses[Math.min(i, responses.length - 1)];
    i += 1;
    calls.push({
      url,
      method: (init.method as string) ?? 'GET',
      body: init.body ? JSON.parse(init.body as string) : undefined,
    });
    return {
      ok: (r.status ?? 200) < 400,
      status: r.status ?? 200,
      headers: { get: (h: string) => (h.toLowerCase() === 'content-type' ? 'application/json' : null) },
      json: async () => r.body,
      text: async () => JSON.stringify(r.body),
      arrayBuffer: async () => new ArrayBuffer(0),
    } as unknown as Response;
  });
  return { fetchFn, calls };
}

describe('P5 bud.transcribeBatch', () => {
  it('builds the POST body with URL audio + flattened StandardSTTConfig + batch knobs + translation', async () => {
    const { fetchFn, calls } = makeFetchStub([
      { body: { job_id: 'j1', status: 'completed', result: { text: 'hi' } } },
    ]);
    const bud = new BudClient({ baseUrl: 'http://127.0.0.1:9', fetch: fetchFn });

    const job = await bud.transcribeBatch({
      audio: 'https://example.com/a.wav',
      config: { provider: 'deepgram', language: 'en-US', model: 'nova-3', diarize: true },
      batch: { summarize: true, detectLanguage: true, alternatives: 3 },
      translation: { targetLanguages: ['es-ES'] },
    });

    expect(job.status).toBe('completed');
    const submit = calls[0];
    expect(submit.method).toBe('POST');
    expect(submit.url).toBe('http://127.0.0.1:9/transcribe/batch');
    // URL audio source (NOT inline bytes).
    expect(submit.body.url).toBe('https://example.com/a.wav');
    expect(submit.body.audio_base64).toBeUndefined();
    // StandardSTTConfig flattened + reused verbatim (diarize → features).
    expect(submit.body.provider).toBe('deepgram');
    expect(submit.body.model).toBe('nova-3');
    expect(submit.body.features.diarization).toBe(true);
    // Batch-only knobs (snake_case on the wire).
    expect(submit.body.batch).toEqual({ summarize: true, detect_language: true, alternatives: 3 });
    // Translation block.
    expect(submit.body.translation).toEqual({ target_languages: ['es-ES'] });
  });

  it('encodes inline bytes (ArrayBuffer) to base64 with content_type', async () => {
    const { fetchFn, calls } = makeFetchStub([{ body: { job_id: 'j2', status: 'completed', result: {} } }]);
    const bud = new BudClient({ baseUrl: 'http://127.0.0.1:9', fetch: fetchFn });

    const bytes = new Uint8Array([0, 1, 2, 3]);
    await bud.transcribeBatch({
      audio: bytes.buffer,
      contentType: 'audio/wav',
      config: { provider: 'openai' },
    });
    expect(calls[0].body.url).toBeUndefined();
    expect(calls[0].body.audio_base64).toBe('AAECAw=='); // base64 of 00 01 02 03
    expect(calls[0].body.content_type).toBe('audio/wav');
  });

  it('polls GET /transcribe/batch/{job_id} until completed', async () => {
    const { fetchFn, calls } = makeFetchStub([
      { body: { job_id: 'abc', status: 'queued' } }, // submit
      { body: { job_id: 'abc', status: 'processing' } }, // poll 1
      { body: { job_id: 'abc', status: 'completed', result: { text: 'done' } } }, // poll 2
    ]);
    const bud = new BudClient({ baseUrl: 'http://127.0.0.1:9', fetch: fetchFn });

    const job = await bud.transcribeBatch({
      audio: 'https://x/a.wav',
      pollIntervalSeconds: 0,
    });
    expect(job.status).toBe('completed');
    expect((job.result as { text: string }).text).toBe('done');
    // 1 submit POST + 2 GET polls.
    expect(calls[0].method).toBe('POST');
    expect(calls[1]).toMatchObject({ method: 'GET', url: 'http://127.0.0.1:9/transcribe/batch/abc' });
    expect(calls[2]).toMatchObject({ method: 'GET', url: 'http://127.0.0.1:9/transcribe/batch/abc' });
  });

  it('callback mode returns the ack and does NOT poll', async () => {
    const { fetchFn, calls } = makeFetchStub([{ body: { job_id: 'cb', status: 'queued' } }]);
    const bud = new BudClient({ baseUrl: 'http://127.0.0.1:9', fetch: fetchFn });

    const job = await bud.transcribeBatch({
      audio: 'https://x/a.wav',
      callbackUrl: 'https://my.app/webhook',
      callbackMethod: 'PUT',
    });
    expect(job).toEqual({ job_id: 'cb', status: 'queued' });
    expect(calls).toHaveLength(1); // only the submit, no GET
    expect(calls[0].body.callback_url).toBe('https://my.app/webhook');
    expect(calls[0].body.callback_method).toBe('PUT');
  });

  it('throws when neither url nor audio is provided', async () => {
    const { fetchFn } = makeFetchStub([{ body: {} }]);
    const bud = new BudClient({ baseUrl: 'http://127.0.0.1:9', fetch: fetchFn });
    await expect(bud.transcribeBatch({ config: { provider: 'deepgram' } })).rejects.toThrow();
  });
});

// =============================================================================
// (3) GATEWAY-NATIVE REALTIME — provider-agnostic /realtime config + events
// =============================================================================

describe('P5 gateway-native bud.realtime', () => {
  it('exposes all 12 realtime providers', () => {
    const expected = [
      'openai', 'hume', 'azure', 'grok', 'inworld', 'deepgram',
      'elevenlabs', 'gemini', 'ultravox', 'nova_sonic', 'speechmatics', 'yandex',
    ];
    expect([...REALTIME_PROVIDERS].sort()).toEqual([...expected].sort());
    expect([...getSupportedRealtimeProviders()].sort()).toEqual([...expected].sort());
  });

  it('builds the gateway /realtime config message (NOT the OpenAI-native session.update)', () => {
    const bud = new BudClient({ baseUrl: 'http://127.0.0.1:3009' });
    const session = bud.realtime({
      provider: 'openai',
      voice: 'alloy',
      instructions: 'You are helpful.',
      temperature: 0.7,
      maxResponseTokens: 512,
      modalities: ['text', 'audio'],
    });
    expect(session).toBeInstanceOf(GatewayRealtime);
    const msg = session.configMessage();

    // Gateway provider-agnostic envelope.
    expect(msg.type).toBe('config');
    expect(msg.provider).toBe('openai');
    expect(msg.voice).toBe('alloy');
    expect(msg.instructions).toBe('You are helpful.');
    expect(msg.temperature).toBe(0.7);
    expect(msg.max_response_tokens).toBe(512);
    expect(msg.modalities).toEqual(['text', 'audio']);

    // NOT the OpenAI-native protocol.
    expect(msg.type).not.toBe('session.update');
    expect(msg.session).toBeUndefined();
    expect(msg.max_response_output_tokens).toBeUndefined();
  });

  it('works for every provider (provider is just a field)', () => {
    const bud = new BudClient({ baseUrl: 'http://127.0.0.1:3009' });
    for (const provider of REALTIME_PROVIDERS) {
      const msg = bud.realtime({ provider }).configMessage();
      expect(msg.provider).toBe(provider);
      expect(msg.type).toBe('config');
    }
  });

  it('rejects an unknown provider', () => {
    const bud = new BudClient({ baseUrl: 'http://127.0.0.1:3009' });
    // @ts-expect-error intentionally invalid provider
    expect(() => bud.realtime({ provider: 'not-a-provider' })).toThrow();
  });

  it('serializes turn_detection + tools (gateway ToolConfig shape) + reasoning_effort', () => {
    const msg = gatewayRealtimeConfigToWire({
      provider: 'deepgram',
      turnDetection: { mode: 'server_vad', threshold: 0.6, silenceDurationMs: 400 },
      tools: [{ name: 'get_weather', description: 'wx', parameters: { type: 'object' } }],
      reasoningEffort: 'low',
    });
    expect(msg.turn_detection).toEqual({ mode: 'server_vad', threshold: 0.6, silence_duration_ms: 400 });
    expect(msg.tools).toEqual([
      { type: 'function', function: { name: 'get_weather', description: 'wx', parameters: { type: 'object' } } },
    ]);
    expect(msg.reasoning_effort).toBe('low');
  });

  it('serializes semantic turn detection + alias', () => {
    const msg = gatewayRealtimeConfigToWire({
      provider: 'openai',
      turnDetection: { mode: 'semantic', eagerness: 'high' },
      alias: 'support-bot',
    });
    expect(msg.turn_detection).toEqual({ mode: 'semantic', eagerness: 'high' });
    expect(msg.alias).toBe('support-bot');
  });

  it('routes to the gateway /realtime URL', () => {
    const bud = new BudClient({ baseUrl: 'http://127.0.0.1:3009' });
    const session = bud.realtime({ provider: 'openai' });
    // The URL is private; assert via a connect attempt against a custom WS.
    let connectedUrl = '';
    class CaptureWS {
      onopen: (() => void) | null = null;
      onmessage: ((e: any) => void) | null = null;
      onerror: ((e: any) => void) | null = null;
      onclose: (() => void) | null = null;
      binaryType = '';
      constructor(url: string) {
        connectedUrl = url;
      }
      send() {}
      close() {}
    }
    const bud2 = new BudClient({
      baseUrl: 'http://127.0.0.1:3009',
      WebSocket: CaptureWS as unknown as typeof WebSocket,
    });
    const s2 = bud2.realtime({ provider: 'openai' });
    void session;
    // Kick connect; the CaptureWS never fires onopen so we just inspect the URL.
    void s2.connect();
    expect(connectedUrl).toBe('ws://127.0.0.1:3009/realtime');
  });
});

describe('P5 gateway-native realtime — IN message decoding', () => {
  function newSession() {
    return new GatewayRealtime({
      url: 'ws://x/realtime',
      config: { provider: 'openai' },
      WebSocket: class {
        send() {}
        close() {}
      } as unknown as typeof WebSocket,
    });
  }

  it('decodes transcript / session_created / function_call / error / speech / response', () => {
    const s = newSession();
    const seen: Record<string, any[]> = {
      transcript: [], sessionCreated: [], functionCall: [], error: [], speechEvent: [], responseDone: [],
    };
    s.on('transcript', (e) => seen.transcript.push(e));
    s.on('sessionCreated', (e) => seen.sessionCreated.push(e));
    s.on('functionCall', (e) => seen.functionCall.push(e));
    s.on('error', (e) => seen.error.push(e));
    s.on('speechEvent', (e) => seen.speechEvent.push(e));
    s.on('responseDone', (e) => seen.responseDone.push(e));

    // Drive the private dispatcher via the message handler.
    const handle = (obj: unknown) =>
      (s as unknown as { handleMessage: (e: MessageEvent) => void }).handleMessage({
        data: JSON.stringify(obj),
      } as MessageEvent);

    handle({ type: 'session_created', session_id: 'sess_1', provider: 'openai', model: 'gpt-realtime' });
    handle({ type: 'transcript', text: 'hello', role: 'user', is_final: true });
    handle({ type: 'function_call', call_id: 'c1', name: 'wx', arguments: '{"city":"Tokyo"}' });
    handle({ type: 'speech_event', event: 'started', audio_ms: 120 });
    handle({ type: 'response_done', response_id: 'r1' });
    handle({ type: 'error', code: 'no_key', message: 'API key not configured' });

    expect(seen.sessionCreated[0]).toMatchObject({ sessionId: 'sess_1', provider: 'openai', model: 'gpt-realtime' });
    expect(s.sessionId).toBe('sess_1');
    expect(seen.transcript[0]).toMatchObject({ text: 'hello', role: 'user', isFinal: true });
    expect(seen.functionCall[0]).toMatchObject({ name: 'wx', arguments: { city: 'Tokyo' }, callId: 'c1' });
    expect(seen.speechEvent[0]).toMatchObject({ event: 'started', audioMs: 120 });
    expect(seen.responseDone[0]).toMatchObject({ responseId: 'r1' });
    expect(seen.error[0]).toMatchObject({ message: 'API key not configured', code: 'no_key' });
  });

  it('decodes binary frames as audio events', () => {
    const s = newSession();
    const audio: any[] = [];
    s.on('audio', (e) => audio.push(e));
    const buf = new Uint8Array([16, 32, 48]).buffer;
    (s as unknown as { handleMessage: (e: MessageEvent) => void }).handleMessage({ data: buf } as MessageEvent);
    expect(audio[0].audio).toBe(buf);
    expect(audio[0].sampleRate).toBe(24000);
  });
});
