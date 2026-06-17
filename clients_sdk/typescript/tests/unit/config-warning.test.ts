/**
 * config_warning surfacing tests (P1).
 *
 * The gateway emits a non-fatal `config_warning` advisory when a config is
 * accepted but something was silently degraded (unknown/misnested key, emotion
 * ignored by the provider, reasoning model on the voice path). The SDK must
 * surface it as a typed `configWarning` event — a beginner learns the no-op
 * instead of it vanishing.
 */
import { describe, it, expect, vi } from 'vitest';
import { deserializeMessage } from '../../src/ws/messages.js';
import type { ConfigWarningMessage } from '../../src/types/messages.js';
import { WebSocketSession } from '../../src/ws/session.js';
import type { ConfigWarningEvent } from '../../src/types/warnings.js';

describe('config_warning deserialization (exact gateway wire)', () => {
  it('parses {type:config_warning, code, message, detail}', () => {
    // EXACT gateway wire for OutgoingMessage::ConfigWarning (config_lint.rs).
    const wire = JSON.stringify({
      type: 'config_warning',
      code: 'unknown_config_keys',
      message:
        'Ignored unknown/misnested config keys: turn_detection (did you mean to nest it under stt_config?)',
      detail: { ignored_keys: ['turn_detection', 'reasoning_effort'] },
    });
    const msg = deserializeMessage(wire) as ConfigWarningMessage;
    expect(msg.type).toBe('config_warning');
    expect(msg.code).toBe('unknown_config_keys');
    expect(msg.message).toContain('Ignored unknown/misnested config keys');
    expect(msg.detail).toEqual({ ignored_keys: ['turn_detection', 'reasoning_effort'] });
  });

  it('parses a config_warning with no detail', () => {
    const wire = JSON.stringify({
      type: 'config_warning',
      code: 'reasoning_model_on_voice_path',
      message: 'reasoning_model is on the spoken path; expect high TTFT',
    });
    const msg = deserializeMessage(wire) as ConfigWarningMessage;
    expect(msg.code).toBe('reasoning_model_on_voice_path');
    expect(msg.detail).toBeUndefined();
  });
});

describe('WebSocketSession surfaces config_warning as a typed event', () => {
  // Build a session and feed it a config_warning by invoking the private
  // message handler the same way the connection layer would.
  function feed(session: WebSocketSession, raw: object): void {
    // @ts-expect-error — exercise the private message dispatch directly.
    session.handleMessage(raw);
  }

  it('emits a `configWarning` event with code/message/detail/raw', () => {
    const session = new WebSocketSession({ url: 'ws://127.0.0.1:3085/ws', reconnect: false });
    const handler = vi.fn<[ConfigWarningEvent], void>();
    session.on('configWarning', handler);

    feed(session, {
      type: 'config_warning',
      code: 'emotion_ignored_for_provider',
      message: "emotion='excited' was ignored by deepgram (provider has no emotion support)",
      detail: { provider: 'deepgram', emotion: 'excited' },
    });

    expect(handler).toHaveBeenCalledTimes(1);
    const ev = handler.mock.calls[0]![0];
    expect(ev.code).toBe('emotion_ignored_for_provider');
    expect(ev.message).toContain('ignored by deepgram');
    expect(ev.detail).toEqual({ provider: 'deepgram', emotion: 'excited' });
    expect(ev.raw.type).toBe('config_warning');
  });

  it('does NOT route a config_warning to the `error` event (it is non-fatal)', () => {
    const session = new WebSocketSession({ url: 'ws://127.0.0.1:3085/ws', reconnect: false });
    const onError = vi.fn();
    const onWarn = vi.fn();
    session.on('error', onError);
    session.on('configWarning', onWarn);

    feed(session, { type: 'config_warning', code: 'language_unsupported', message: 'lang xx unsupported' });

    expect(onWarn).toHaveBeenCalledTimes(1);
    expect(onError).not.toHaveBeenCalled();
  });
});
