// =============================================================================
// Voice Cloning Types
// =============================================================================

/**
 * Supported voice cloning providers.
 */
export const VOICE_CLONE_PROVIDERS = ['elevenlabs', 'playht', 'resemble'] as const;

export type VoiceCloneProvider = (typeof VOICE_CLONE_PROVIDERS)[number];

/**
 * Request to clone a voice.
 */
export interface VoiceCloneRequest {
  /** Name for the cloned voice */
  name: string;
  /** Audio files for voice cloning (raw audio data) */
  audioFiles: ArrayBuffer[];
  /** Provider to use for cloning */
  provider: VoiceCloneProvider;
  /** Optional description */
  description?: string;
  /** Optional labels/tags */
  labels?: string[];
}

/**
 * Response from voice cloning operation.
 */
export interface VoiceCloneResponse {
  /** Unique ID of the cloned voice */
  voiceId: string;
  /** Name of the voice */
  name: string;
  /** Provider used */
  provider: VoiceCloneProvider;
  /** Current status */
  status: 'ready' | 'processing' | 'failed';
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
  status?: 'ready' | 'processing' | 'failed';
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
    status: wire.status as VoiceCloneResponse['status'],
    createdAt: wire.created_at as string,
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
