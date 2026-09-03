// =============================================================================
// Voice Cloning Types
// =============================================================================

/**
 * Supported voice cloning providers.
 *
 * The FULL set sourced 1:1 from the gateway `VoiceCloneProvider`
 * (gateway/src/handlers/voices.rs; P4 widened it from the 3-provider SDK list).
 * A bare `string` is also accepted by {@link cloneVoice} for forward-compat.
 */
export const VOICE_CLONE_PROVIDERS = [
  'hume',
  'elevenlabs',
  'lmnt',
  'cartesia',
  'playht',
  'speechify',
  'resemble',
] as const;

export type VoiceCloneProvider = (typeof VOICE_CLONE_PROVIDERS)[number] | (string & {});

/**
 * Voice-clone mode (gateway `CloneMode`).
 * - `instant` (default): IVC / clip clone; returns `ready` immediately or near-instantly.
 * - `professional`: async high-fidelity (ElevenLabs PVC / Resemble / PlayHT PVC);
 *   the returned `voiceId` is polled until `ready`.
 */
export type CloneMode = 'instant' | 'professional';

/**
 * Canonical lifecycle status of a cloned voice (gateway `CloneStatus`).
 * `ready`/`failed` are TERMINAL; the others mean the clone is still being produced.
 * `processing` is KEPT as a backward-compatible alias for the old SDK status.
 */
export type VoiceCloneStatus =
  | 'ready'
  | 'verifying'
  | 'training'
  | 'queued'
  | 'failed'
  // Backward-compatible alias for the pre-P4 SDK status (in-progress).
  | 'processing';

/** Whether a clone status is terminal (ready or failed → stop polling). */
export function isTerminalCloneStatus(status: VoiceCloneStatus): boolean {
  return status === 'ready' || status === 'failed';
}

/**
 * Request to clone a voice.
 *
 * Mirrors the gateway `VoiceCloneRequest` (`POST /voices/clone`). Supply samples as
 * already-base64/URL `audioSamples` (canonical wire field) OR as raw `audioFiles`
 * (ArrayBuffers, base64-encoded at send time). `name` + `provider` are required.
 */
export interface VoiceCloneRequest {
  /** Name for the cloned voice */
  name: string;
  /** Provider to use for cloning */
  provider: VoiceCloneProvider;
  /** Audio samples as base64 strings or URLs (canonical gateway wire field). */
  audioSamples?: string[];
  /** Raw audio buffers (convenience; base64-encoded into `audioSamples` at send). */
  audioFiles?: ArrayBuffer[];
  /** Optional description (Hume design / ElevenLabs label) */
  description?: string;
  /** Optional labels/tags (ElevenLabs) */
  labels?: Record<string, string> | string[];
  /** Clone MODE: `instant` (default) or `professional` (async high-fidelity). */
  mode?: CloneMode;
  /** Design-from-existing: the base voice to clone/derive from (provider-specific). */
  baseVoiceId?: string;
  /** Remove background noise from samples (ElevenLabs IVC / LMNT enhance). */
  removeBackgroundNoise?: boolean;
  /** Sample text for voice generation (Hume only). */
  sampleText?: string;
}

/**
 * Response from voice cloning operation (gateway `VoiceCloneResponse`).
 */
export interface VoiceCloneResponse {
  /** Unique ID of the cloned voice (usable as a TTS voiceId once ready) */
  voiceId: string;
  /** Name of the voice */
  name: string;
  /** Provider used */
  provider: VoiceCloneProvider;
  /** Canonical status: ready | verifying | training | queued | failed */
  status: VoiceCloneStatus;
  /** Whether the voice still needs verification before use (ElevenLabs IVC) */
  requiresVerification?: boolean;
  /** Creation timestamp (ISO 8601) */
  createdAt: string;
  /** Optional metadata */
  metadata?: Record<string, unknown>;
  /** Error message if status is 'failed' */
  error?: string;
}

/**
 * Filter for listing cloned voices.
 */
export interface VoiceCloneFilter {
  /** Filter by provider */
  provider?: VoiceCloneProvider;
  /** Filter by status */
  status?: VoiceCloneStatus;
  /** Maximum number of results */
  limit?: number;
  /** Offset for pagination */
  offset?: number;
}

// =============================================================================
// Recording Types
// =============================================================================

/**
 * Recording status.
 */
export type RecordingStatus = 'recording' | 'completed' | 'failed' | 'processing';

/**
 * Audio format for recordings.
 */
export type RecordingFormat = 'wav' | 'mp3' | 'ogg' | 'flac' | 'webm';

/**
 * Information about a recording.
 */
export interface RecordingInfo {
  /** Stream ID associated with the recording */
  streamId: string;
  /** Room name (for LiveKit recordings) */
  roomName?: string;
  /** Duration in seconds */
  duration: number;
  /** Size in bytes */
  size: number;
  /** Audio format */
  format: RecordingFormat;
  /** Creation timestamp (ISO 8601) */
  createdAt: string;
  /** Current status */
  status: RecordingStatus;
  /** Sample rate in Hz */
  sampleRate?: number;
  /** Number of channels */
  channels?: number;
  /** Bit depth */
  bitDepth?: number;
  /** Optional metadata */
  metadata?: Record<string, unknown>;
}

/**
 * Filter for listing recordings.
 */
export interface RecordingFilter {
  /** Filter by room name */
  roomName?: string;
  /** Filter by stream ID */
  streamId?: string;
  /** Filter by status */
  status?: RecordingStatus;
  /** Start date (ISO 8601) */
  startDate?: string;
  /** End date (ISO 8601) */
  endDate?: string;
  /** Filter by format */
  format?: RecordingFormat;
  /** Maximum number of results */
  limit?: number;
  /** Offset for pagination */
  offset?: number;
}

/**
 * Options for downloading a recording.
 */
export interface RecordingDownloadOptions {
  /** Desired output format (will transcode if different) */
  format?: RecordingFormat;
  /** Desired sample rate (will resample if different) */
  sampleRate?: number;
  /** Desired number of channels (will mix if different) */
  channels?: number;
}

/**
 * Paginated list of recordings.
 */
export interface RecordingList {
  /** Recordings in this page */
  recordings: RecordingInfo[];
  /** Total count of recordings matching filter */
  total: number;
  /** Whether there are more results */
  hasMore?: boolean;
}

// =============================================================================
// Wire Format Helpers
// =============================================================================

/**
 * Convert RecordingInfo from wire format (snake_case).
 */
export function deserializeRecordingInfo(wire: Record<string, unknown>): RecordingInfo {
  return {
    streamId: wire.stream_id as string,
    roomName: wire.room_name as string | undefined,
    duration: wire.duration as number,
    size: wire.size as number,
    format: wire.format as RecordingFormat,
    createdAt: wire.created_at as string,
    status: wire.status as RecordingStatus,
    sampleRate: wire.sample_rate as number | undefined,
    channels: wire.channels as number | undefined,
    bitDepth: wire.bit_depth as number | undefined,
    metadata: wire.metadata as Record<string, unknown> | undefined,
  };
}

/**
 * Convert VoiceCloneResponse from wire format (snake_case).
 */
export function deserializeVoiceCloneResponse(wire: Record<string, unknown>): VoiceCloneResponse {
  return {
    voiceId: wire.voice_id as string,
    name: wire.name as string,
    provider: wire.provider as VoiceCloneProvider,
    status: wire.status as VoiceCloneStatus,
    requiresVerification: wire.requires_verification as boolean | undefined,
    createdAt: (wire.created_at as string | undefined) ?? '',
    metadata: wire.metadata as Record<string, unknown> | undefined,
    error: wire.error as string | undefined,
  };
}

/**
 * Build query string for RecordingFilter.
 */
export function buildRecordingFilterQuery(filter: RecordingFilter): string {
  const params = new URLSearchParams();

  if (filter.roomName) params.set('room_name', filter.roomName);
  if (filter.streamId) params.set('stream_id', filter.streamId);
  if (filter.status) params.set('status', filter.status);
  if (filter.startDate) params.set('start_date', filter.startDate);
  if (filter.endDate) params.set('end_date', filter.endDate);
  if (filter.format) params.set('format', filter.format);
  if (filter.limit !== undefined) params.set('limit', String(filter.limit));
  if (filter.offset !== undefined) params.set('offset', String(filter.offset));

  const query = params.toString();
  return query ? `?${query}` : '';
}

/**
 * Build query string for VoiceCloneFilter.
 */
export function buildVoiceCloneFilterQuery(filter: VoiceCloneFilter): string {
  const params = new URLSearchParams();

  if (filter.provider) params.set('provider', filter.provider);
  if (filter.status) params.set('status', filter.status);
  if (filter.limit !== undefined) params.set('limit', String(filter.limit));
  if (filter.offset !== undefined) params.set('offset', String(filter.offset));

  const query = params.toString();
  return query ? `?${query}` : '';
}
