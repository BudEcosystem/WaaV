/**
 * bud.agent(...) flagship helper tests (P1).
 *
 * Asserts the beginner helper builds the right session config: sane STT/TTS
 * defaults, the conversation loop, and the eager-EoT coupling (turn.eagerEot
 * flips BOTH stt_config.turn_detection.eager AND conversation_config.eager_eot).
 * The end-to-end wire shape it produces is asserted by round-tripping through
 * the real serializer.
 */
import { describe, it, expect } from 'vitest';
import { buildAgentSessionConfig } from '../../src/pipelines/agent.js';
import { BudClient } from '../../src/bud.js';
import { AgentSession } from '../../src/pipelines/agent.js';
import { createConfigMessage, serializeMessage } from '../../src/ws/messages.js';

const URL = 'ws://127.0.0.1:3085/ws';

describe('buildAgentSessionConfig', () => {
  it('applies Deepgram STT/TTS defaults and requires only llm', () => {
    const cfg = buildAgentSessionConfig(
      { llm: { baseUrl: 'http://localhost:11434/v1', model: 'llama3.2:1b' } },
      URL
    );
    expect(cfg.url).toBe(URL);
    expect(cfg.audio).toBe(true);
    // STT defaults include the gateway-required sample_rate/channels/punctuation.
    expect(cfg.stt).toEqual({
      provider: 'deepgram',
      language: 'en-US',
      model: 'nova-3',
      sampleRate: 16000,
      channels: 1,
      punctuation: true,
    });
    expect(cfg.tts).toEqual({ provider: 'deepgram', model: 'aura-asteria-en', voiceId: 'aura-asteria-en' });
    expect(cfg.conversation).toEqual({ baseUrl: 'http://localhost:11434/v1', model: 'llama3.2:1b' });
    expect(cfg.turnDetection).toBeUndefined();
  });

  it('merges caller STT/TTS over the defaults', () => {
    const cfg = buildAgentSessionConfig(
      {
        stt: { provider: 'assemblyai', language: 'es-ES' },
        tts: { provider: 'cartesia', voiceId: 'sonic-x' },
        llm: { baseUrl: 'u', model: 'm' },
      },
      URL
    );
    expect(cfg.stt.provider).toBe('assemblyai');
    expect(cfg.stt.language).toBe('es-ES');
    expect(cfg.stt.model).toBe('nova-3'); // default kept where not overridden
    expect(cfg.tts.provider).toBe('cartesia');
    expect(cfg.tts.voiceId).toBe('sonic-x');
  });

  it('turn.eagerEot flips BOTH turn_detection.eager AND conversation.eager_eot', () => {
    const cfg = buildAgentSessionConfig(
      { llm: { baseUrl: 'u', model: 'm' }, turn: { eagerEot: true, threshold: 0.7 } },
      URL
    );
    expect(cfg.turnDetection).toEqual({ enabled: true, threshold: 0.7, eager: true });
    expect(cfg.conversation.eagerEot).toBe(true);
  });

  it('does not override an explicit conversation.eagerEot=false when turn.eagerEot is set', () => {
    const cfg = buildAgentSessionConfig(
      { llm: { baseUrl: 'u', model: 'm', eagerEot: false }, turn: { eagerEot: true } },
      URL
    );
    // Caller explicitly disabled eager_eot — respect it (only the turn flag flips).
    expect(cfg.conversation.eagerEot).toBe(false);
    expect(cfg.turnDetection?.eager).toBe(true);
  });

  it('does not mutate the caller-provided llm object', () => {
    const llm = { baseUrl: 'u', model: 'm' };
    buildAgentSessionConfig({ llm, turn: { eagerEot: true } }, URL);
    expect('eagerEot' in llm).toBe(false); // clone, not mutate
  });

  it('threads apiKey into the session config', () => {
    const cfg = buildAgentSessionConfig({ llm: { baseUrl: 'u', model: 'm' } }, URL, 'sk-test');
    expect(cfg.apiKey).toBe('sk-test');
  });

  it('produces a wire config that carries conversation_config + nested turn detection', () => {
    const cfg = buildAgentSessionConfig(
      {
        stt: { provider: 'deepgram', language: 'en-US' },
        tts: { provider: 'deepgram', voiceId: 'aura-asteria-en' },
        llm: { baseUrl: 'http://localhost:11434/v1', model: 'llama3.2:1b', latencyFiller: 'auto' },
        turn: { eagerEot: true },
      },
      URL
    );
    // Reproduce what WebSocketSession.sendConfig() will serialize.
    const wire = JSON.parse(
      serializeMessage(
        createConfigMessage(cfg.stt, cfg.tts, undefined, undefined, {
          conversation: cfg.conversation,
          turnDetection: cfg.turnDetection,
          audio: cfg.audio,
        })
      )
    );
    expect(wire.conversation_config.base_url).toBe('http://localhost:11434/v1');
    expect(wire.conversation_config.latency_filler).toBe('auto');
    expect(wire.conversation_config.eager_eot).toBe(true);
    expect(wire.stt_config.turn_detection).toEqual({ enabled: true, eager: true });
    expect(wire.tts_config.voice_id).toBe('aura-asteria-en');
  });
});

describe('BudClient.agent', () => {
  it('exposes agent() returning an unconnected AgentSession', () => {
    const bud = new BudClient({ baseUrl: 'http://127.0.0.1:3085' });
    const agent = bud.agent({ llm: { baseUrl: 'u', model: 'm' } });
    expect(agent).toBeInstanceOf(AgentSession);
    expect(agent.isConnected()).toBe(false);
    expect(agent.isReady()).toBe(false);
  });
});
