// =============================================================================
// config_warning advisory (gateway OutgoingMessage::ConfigWarning)
//
// Source of truth: gateway/src/handlers/ws/messages.rs:321-332 (wire shape) +
// the emitter gateway/src/handlers/ws/config_lint.rs.
//
// A config_warning is a NON-FATAL advisory: the gateway emits it when a config
// is accepted but something was silently degraded — an unknown/misnested key
// (serde dropped it), an emotion the provider ignores, a reasoning model placed
// on the spoken path, etc. It NEVER closes the session. The SDK surfaces it as
// a typed `config_warning` event so a beginner learns "emotion=excited ignored
// by deepgram" / "you sent a key that doesn't exist" instead of a silent no-op.
// =============================================================================

/**
 * Known machine `code` values the gateway emits. This is a typed union of the
 * codes documented today, but the parser accepts ANY string for forward-compat
 * (a new gateway code must still surface, not be dropped).
 */
export type ConfigWarningCode =
  /**
   * A config message contained keys serde silently dropped (a typo OR wrong
   * nesting). `detail.ignored_keys` lists every dotted path; the message
   * appends targeted nesting hints. Emitted by config_lint.rs (P0).
   */
  | 'unknown_config_keys'
  /** A reasoning model was placed on the spoken `model` path → high TTFT. */
  | 'reasoning_model_on_voice_path'
  /** Reasoning effort was clamped up to an adaptive-only model's floor. */
  | 'reasoning_effort_clamped'
  /** An emotion/style was ignored because the chosen provider does not support it. */
  | 'emotion_ignored_for_provider'
  /** The requested language is unsupported by the chosen provider. */
  | 'language_unsupported'
  // Forward-compat: any future gateway code still types as a string.
  | (string & {});

import type { ConfigWarningMessage } from './messages.js';

// Re-export the raw wire message type so it is importable alongside the event.
export type { ConfigWarningMessage } from './messages.js';

/**
 * A typed gateway config advisory.
 *
 * Wire shape (gateway): `{ type:"config_warning", code, message, detail? }`.
 * `detail` is arbitrary JSON (e.g. `{ ignored_keys: [...] }` for
 * `unknown_config_keys`, or `{ applied, floor }` for `reasoning_effort_clamped`).
 */
export interface ConfigWarningEvent {
  /** Stable machine code (e.g. "unknown_config_keys"). */
  code: ConfigWarningCode;
  /** Human-readable explanation + a one-line fix hint. */
  message: string;
  /** Optional free-form JSON detail (present for some codes only). */
  detail?: Record<string, unknown>;
  /** Original wire message for advanced use. */
  raw: ConfigWarningMessage;
}
