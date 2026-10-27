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
  // Segmented speech-to-text (gateway docs/segmented-stt/customer-contract-reference.md section 5).
  /** The model's client holds audio until the stream ends: text comes at `audio_end` or hang-up. */
  | 'stt_buffered_until_commit'
  /** Unknown `transcription_mode` value, treated as absent. */
  | 'stt_transcription_mode_invalid'
  /** A voice-agent session sent a `transcription_mode`; the agent's setting applies. */
  | 'stt_transcription_mode_ignored'
  /** The requested `transcription_mode` could not be met. */
  | 'stt_mode_unavailable'
  /** An SDK placeholder model (e.g. `nova-3` for another provider) was treated as no model. */
  | 'stt_placeholder_model_ignored'
  /** Capabilities came from a provider or global default row. */
  | 'stt_capability_assumed'
  /** The model is deprecated or has a shutdown date. */
  | 'stt_model_deprecated'
  /** No language (or `auto`) on a segmented session; set `language`. Usually a `ready.stt` notice. */
  | 'stt_language_unset'
  /** The declared transport could not be built; a fallback is used. */
  | 'stt_transport_fallback'
  /** The session is segmented without asking: text after each pause, with the stated latency. */
  | 'stt_segmented_mode'
  /** The slow latency figure is above 2,500 ms. */
  | 'stt_latency_slow'
  /** Fewer request fields are sent to the vendor. */
  | 'stt_fields_reduced'
  /** The vendor bills a minimum duration per upload. */
  | 'stt_min_billed_duration'
  /** Expected uploads per minute exceed the limiter's threshold. */
  | 'stt_capacity_low'
  /** The energy detector is in use instead of Silero. */
  | 'stt_detector_fallback'
  /** Request settings were ignored, clamped, assumed or surcharged. */
  | 'stt_setting_not_applied'
  /** Deployment or agent settings were not applied. */
  | 'deployment_setting_not_applied'
  // Forward-compat: any future gateway code still types as a string.
  | (string & {});

import type { ConfigWarningMessage, SttWarningMessage } from './messages.js';

// Re-export the raw wire message types so they are importable alongside the events.
export type { ConfigWarningMessage, SttWarningMessage } from './messages.js';

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

// =============================================================================
// stt_warning (gateway OutgoingMessage::SttWarning)
//
// A speech-to-text problem in the middle of a call that does NOT end it: a lost
// segment, dropped audio, rate limiting. The gateway never sends these as
// `error` (which clients treat as a disconnect), and the SDK never surfaces them
// as `error` either.
// =============================================================================

/**
 * Known `stt_warning` codes. Typed for autocompletion; any other string is
 * accepted so a newer gateway code still surfaces.
 */
export type SttWarningCode =
  /** A segment failed after its retry, or timed out; its words are lost. */
  | 'stt_segment_failed'
  /** Three caller turns in a row were lost (sessions without an agent). */
  | 'stt_degraded'
  /** The upload limiter is under pressure. */
  | 'stt_rate_limited'
  /** Audio intake overflowed and audio was discarded. */
  | 'stt_audio_dropped'
  /** The vendor refused a request field mid-call; fewer fields are sent. */
  | 'stt_fields_reduced'
  /** The detector fell back to the energy detector mid-call. */
  | 'stt_detector_fallback'
  /** A fallback vendor took over; `ready.stt` is stale. */
  | 'stt_fallback_engaged'
  | (string & {});

/**
 * A typed mid-call speech-to-text warning.
 *
 * Wire shape (gateway): `{ type:"stt_warning", code, message, detail? }`.
 */
export interface SttWarningEvent {
  /** Stable machine code (e.g. "stt_segment_failed"). */
  code: SttWarningCode;
  /** Human-readable explanation. */
  message: string;
  /** Optional free-form JSON detail. */
  detail?: Record<string, unknown>;
  /** Original wire message for advanced use. */
  raw: SttWarningMessage;
}
