/**
 * P3 proxy/alias model-name SDK surface (TypeScript).
 *
 * A developer names a server-defined alias; the gateway resolves it to a full
 * {stt,tts,llm,dag} bundle BEFORE provider construction and echoes the concrete
 * providers back on `ready` as `resolved_alias`. Re-pointing the alias server-side
 * is a zero-client-change operation (proven live in the gateway P3 commit).
 */
import { describe, it, expect } from 'vitest';
import { serializeMessage, createConfigMessage, deserializeMessage } from '../../src/ws/messages.js';
import { buildAgentSessionConfig } from '../../src/pipelines/agent.js';

describe('alias — OUT (reaches the wire as a top-level field)', () => {
  it('serializes `alias` as a single top-level config field', () => {
    const wire = JSON.parse(
      serializeMessage(createConfigMessage(undefined, undefined, undefined, undefined, { alias: 'support-bot' }))
    );
    expect(wire.type).toBe('config');
    expect(wire.alias).toBe('support-bot');
  });

  it('omits `alias` when not set', () => {
    const wire = JSON.parse(
      serializeMessage(createConfigMessage(undefined, undefined, undefined, undefined, { audio: false }))
    );
    expect(wire.alias).toBeUndefined();
  });
});

describe('alias — bud.agent flagship helper', () => {
  it('alias + explicit llm: both reach the session config', () => {
    const out = buildAgentSessionConfig(
      { alias: 'support-bot', llm: { baseUrl: 'http://x/v1', model: 'm' } },
      'ws://x'
    );
    expect(out.alias).toBe('support-bot');
    expect(out.conversation).toBeDefined();
  });

  it('alias ALONE is a complete agent (the alias supplies the LLM bundle)', () => {
    const out = buildAgentSessionConfig({ alias: 'support-bot' }, 'ws://x');
    expect(out.alias).toBe('support-bot');
    // No conversation block is sent — the gateway alias resolves the LLM.
    expect(out.conversation).toBeUndefined();
  });

  it('an explicit field overrides the alias default (client-wins merge)', () => {
    const out = buildAgentSessionConfig(
      { alias: 'support-bot', stt: { provider: 'openai' } },
      'ws://x'
    );
    expect(out.alias).toBe('support-bot');
    expect(out.stt.provider).toBe('openai');
  });

  it('requires an LLM from somewhere (neither llm nor alias → throws)', () => {
    expect(() => buildAgentSessionConfig({} as never, 'ws://x')).toThrow(/llm.*alias/i);
  });
});

describe('alias — IN (resolved_alias echo on ready)', () => {
  it('parses the resolved-alias echo from the ready ack', () => {
    const msg = deserializeMessage(
      JSON.stringify({
        type: 'ready',
        stream_id: 'abc',
        protocol_version: '1.0',
        resolved_alias: { name: 'support-bot', kind: 'agent', tts: { provider: 'cartesia' } },
      })
    );
    expect(msg.type).toBe('ready');
    // @ts-expect-error narrow to ReadyMessage at runtime
    expect(msg.resolved_alias?.name).toBe('support-bot');
    // @ts-expect-error narrow to ReadyMessage at runtime
    expect(msg.resolved_alias?.tts?.provider).toBe('cartesia');
  });
});
