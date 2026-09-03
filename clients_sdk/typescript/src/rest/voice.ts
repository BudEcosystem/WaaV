// =============================================================================
// Voice Cloning and Recording REST API
// =============================================================================

import {
  VoiceCloneRequest,
  VoiceCloneResponse,
  VoiceCloneFilter,
  RecordingInfo,
  RecordingFilter,
  RecordingDownloadOptions,
  RecordingList,
  deserializeRecordingInfo,
  deserializeVoiceCloneResponse,
  isTerminalCloneStatus,
  buildRecordingFilterQuery,
  buildVoiceCloneFilterQuery,
} from '../types/voice';

/** Base64-encode an ArrayBuffer (chunked to avoid call-stack limits on big buffers). */
function arrayBufferToBase64(buffer: ArrayBuffer): string {
  const bytes = new Uint8Array(buffer);
  const chunkSize = 0x8000;
  const chunks: string[] = [];
  for (let i = 0; i < bytes.length; i += chunkSize) {
    const chunk = bytes.subarray(i, i + chunkSize);
    chunks.push(String.fromCharCode(...chunk));
  }
  return btoa(chunks.join(''));
}

// =============================================================================
// Error Handling
// =============================================================================

/**
 * Error thrown by voice/recording API operations.
 */
export class VoiceAPIError extends Error {
  constructor(
    message: string,
    public statusCode: number,
    public details?: Record<string, unknown>
  ) {
    super(message);
    this.name = 'VoiceAPIError';
  }
}

/**
 * Handle API response errors.
 */
async function handleResponse<T>(
  response: Response,
  deserialize: (data: Record<string, unknown>) => T
): Promise<T> {
  if (!response.ok) {
    let errorDetails: Record<string, unknown> | undefined;
    try {
      errorDetails = await response.json();
    } catch {
      // Response may not be JSON
    }
    throw new VoiceAPIError(
      errorDetails?.error as string || `API request failed with status ${response.status}`,
      response.status,
      errorDetails
    );
  }

  const data = await response.json();
  return deserialize(data);
}

// =============================================================================
// Voice Cloning API
// =============================================================================

/**
 * Clone a voice using audio samples.
 *
 * @param baseUrl - Gateway base URL
 * @param request - Voice cloning request
 * @param apiKey - Optional API key for authentication
 * @returns Promise resolving to the cloned voice response
 */
export async function cloneVoice(
  baseUrl: string,
  request: VoiceCloneRequest,
  apiKey?: string
): Promise<VoiceCloneResponse> {
  // Canonical gateway VoiceCloneRequest body (JSON). Samples go as a base64/URL
  // string array under `audio_samples` (the gateway field) — `audioFiles`
  // ArrayBuffers are base64-encoded into it. The old multipart `audio_N` upload was
  // wrong and silently dropped server-side.
  const samples: string[] = [...(request.audioSamples ?? [])];
  for (const file of request.audioFiles ?? []) {
    if (file === undefined) continue;
    samples.push(arrayBufferToBase64(file));
  }

  const body: Record<string, unknown> = {
    provider: request.provider,
    name: request.name,
    audio_samples: samples,
    mode: request.mode ?? 'instant',
    remove_background_noise: request.removeBackgroundNoise ?? false,
  };
  if (request.description !== undefined) body.description = request.description;
  if (request.labels !== undefined) body.labels = request.labels;
  if (request.baseVoiceId !== undefined) body.base_voice_id = request.baseVoiceId;
  if (request.sampleText !== undefined) body.sample_text = request.sampleText;

  const headers: HeadersInit = {
    'Content-Type': 'application/json',
  };
  if (apiKey) {
    headers['Authorization'] = `Bearer ${apiKey}`;
  }

  const response = await fetch(`${baseUrl}/voices/clone`, {
    method: 'POST',
    headers,
    body: JSON.stringify(body),
  });

  return handleResponse(response, deserializeVoiceCloneResponse);
}

/**
 * Clone a voice and resolve once it reaches a terminal status (`ready`/`failed`).
 *
 * For an `instant` clone (already `ready`) this resolves immediately. For a
 * still-running `professional` clone the gateway exposes no `GET /voices/{id}`
 * status endpoint this phase, so this rejects with a clear error rather than spin —
 * finish verification/training via the provider, then use the `voiceId` once ready.
 *
 * @returns The terminal {@link VoiceCloneResponse} (its `voiceId` is a usable TTS voiceId).
 */
export async function cloneVoiceAndWait(
  baseUrl: string,
  request: VoiceCloneRequest,
  apiKey?: string
): Promise<VoiceCloneResponse> {
  const result = await cloneVoice(baseUrl, request, apiKey);
  if (isTerminalCloneStatus(result.status)) {
    if (result.status === 'ready') return result;
    throw new VoiceAPIError(
      `voice clone failed (voiceId=${result.voiceId}, provider=${result.provider})`,
      0
    );
  }
  // Non-terminal (queued/verifying/training): no gateway poll endpoint this phase.
  throw new VoiceAPIError(
    `voice clone is still '${result.status}' (voiceId=${result.voiceId}); the gateway ` +
      'does not yet expose a clone-status endpoint to poll. Finish verification/training ' +
      'via the provider, then use the voiceId as a TTS voiceId once ready.',
    0
  );
}

/**
 * List cloned voices.
 *
 * @param baseUrl - Gateway base URL
 * @param filter - Optional filter parameters
 * @param apiKey - Optional API key for authentication
 * @returns Promise resolving to array of cloned voices
 */
export async function listClonedVoices(
  baseUrl: string,
  filter?: VoiceCloneFilter,
  apiKey?: string
): Promise<VoiceCloneResponse[]> {
  const query = filter ? buildVoiceCloneFilterQuery(filter) : '';

  const headers: HeadersInit = {
    'Content-Type': 'application/json',
  };
  if (apiKey) {
    headers['Authorization'] = `Bearer ${apiKey}`;
  }

  const response = await fetch(`${baseUrl}/voices${query}`, {
    method: 'GET',
    headers,
  });

  return handleResponse(response, (data) => {
    const voices = data.voices as Array<Record<string, unknown>>;
    return voices.map(deserializeVoiceCloneResponse);
  });
}

/**
 * Delete a cloned voice.
 *
 * @param baseUrl - Gateway base URL
 * @param voiceId - ID of the voice to delete
 * @param apiKey - Optional API key for authentication
 */
export async function deleteClonedVoice(
  baseUrl: string,
  voiceId: string,
  apiKey?: string
): Promise<void> {
  const headers: HeadersInit = {
    'Content-Type': 'application/json',
  };
  if (apiKey) {
    headers['Authorization'] = `Bearer ${apiKey}`;
  }

  const response = await fetch(`${baseUrl}/voices/${voiceId}`, {
    method: 'DELETE',
    headers,
  });

  if (!response.ok) {
    let errorDetails: Record<string, unknown> | undefined;
    try {
      errorDetails = await response.json();
    } catch {
      // Response may not be JSON
    }
    throw new VoiceAPIError(
      errorDetails?.error as string || `Failed to delete voice ${voiceId}`,
      response.status,
      errorDetails
    );
  }
}

/**
 * Get status of a cloned voice.
 *
 * @param baseUrl - Gateway base URL
 * @param voiceId - ID of the voice
 * @param apiKey - Optional API key for authentication
 * @returns Promise resolving to the voice status
 */
export async function getClonedVoiceStatus(
  baseUrl: string,
  voiceId: string,
  apiKey?: string
): Promise<VoiceCloneResponse> {
  const headers: HeadersInit = {
    'Content-Type': 'application/json',
  };
  if (apiKey) {
    headers['Authorization'] = `Bearer ${apiKey}`;
  }

  const response = await fetch(`${baseUrl}/voices/${voiceId}`, {
    method: 'GET',
    headers,
  });

  return handleResponse(response, deserializeVoiceCloneResponse);
}

// =============================================================================
// Recording API
// =============================================================================

/**
 * Get information about a recording.
 *
 * @param baseUrl - Gateway base URL
 * @param streamId - Stream ID of the recording
 * @param apiKey - Optional API key for authentication
 * @returns Promise resolving to recording info
 */
export async function getRecording(
  baseUrl: string,
  streamId: string,
  apiKey?: string
): Promise<RecordingInfo> {
  const headers: HeadersInit = {
    'Content-Type': 'application/json',
  };
  if (apiKey) {
    headers['Authorization'] = `Bearer ${apiKey}`;
  }

  const response = await fetch(`${baseUrl}/recording/${streamId}/info`, {
    method: 'GET',
    headers,
  });

  return handleResponse(response, deserializeRecordingInfo);
}

/**
 * Download a recording.
 *
 * @param baseUrl - Gateway base URL
 * @param streamId - Stream ID of the recording
 * @param options - Optional download options (format conversion, etc.)
 * @param apiKey - Optional API key for authentication
 * @returns Promise resolving to recording blob
 */
export async function downloadRecording(
  baseUrl: string,
  streamId: string,
  options?: RecordingDownloadOptions,
  apiKey?: string
): Promise<Blob> {
  const params = new URLSearchParams();
  if (options?.format) params.set('format', options.format);
  if (options?.sampleRate) params.set('sample_rate', String(options.sampleRate));
  if (options?.channels) params.set('channels', String(options.channels));

  const query = params.toString();
  const url = `${baseUrl}/recording/${streamId}${query ? `?${query}` : ''}`;

  const headers: HeadersInit = {};
  if (apiKey) {
    headers['Authorization'] = `Bearer ${apiKey}`;
  }

  const response = await fetch(url, {
    method: 'GET',
    headers,
  });

  if (!response.ok) {
    let errorDetails: Record<string, unknown> | undefined;
    try {
      errorDetails = await response.json();
    } catch {
      // Response is binary, not JSON
    }
    throw new VoiceAPIError(
      errorDetails?.error as string || `Failed to download recording ${streamId}`,
      response.status,
      errorDetails
    );
  }

  return response.blob();
}

/**
 * List recordings.
 *
 * @param baseUrl - Gateway base URL
 * @param filter - Optional filter parameters
 * @param apiKey - Optional API key for authentication
 * @returns Promise resolving to paginated recording list
 */
export async function listRecordings(
  baseUrl: string,
  filter?: RecordingFilter,
  apiKey?: string
): Promise<RecordingList> {
  const query = filter ? buildRecordingFilterQuery(filter) : '';

  const headers: HeadersInit = {
    'Content-Type': 'application/json',
  };
  if (apiKey) {
    headers['Authorization'] = `Bearer ${apiKey}`;
  }

  const response = await fetch(`${baseUrl}/recordings${query}`, {
    method: 'GET',
    headers,
  });

  return handleResponse(response, (data) => {
    const recordings = (data.recordings as Array<Record<string, unknown>>).map(
      deserializeRecordingInfo
    );
    return {
      recordings,
      total: data.total as number,
      hasMore: data.has_more as boolean | undefined,
    };
  });
}

/**
 * Delete a recording.
 *
 * @param baseUrl - Gateway base URL
 * @param streamId - Stream ID of the recording to delete
 * @param apiKey - Optional API key for authentication
 */
export async function deleteRecording(
  baseUrl: string,
  streamId: string,
  apiKey?: string
): Promise<void> {
  const headers: HeadersInit = {
    'Content-Type': 'application/json',
  };
  if (apiKey) {
    headers['Authorization'] = `Bearer ${apiKey}`;
  }

  const response = await fetch(`${baseUrl}/recording/${streamId}`, {
    method: 'DELETE',
    headers,
  });

  if (!response.ok) {
    let errorDetails: Record<string, unknown> | undefined;
    try {
      errorDetails = await response.json();
    } catch {
      // Response may not be JSON
    }
    throw new VoiceAPIError(
      errorDetails?.error as string || `Failed to delete recording ${streamId}`,
      response.status,
      errorDetails
    );
  }
}

/**
 * Start recording a stream.
 *
 * @param baseUrl - Gateway base URL
 * @param streamId - Stream ID to record
 * @param options - Recording options
 * @param apiKey - Optional API key for authentication
 * @returns Promise resolving to recording info
 */
export async function startRecording(
  baseUrl: string,
  streamId: string,
  options?: {
    format?: string;
    sampleRate?: number;
    channels?: number;
  },
  apiKey?: string
): Promise<RecordingInfo> {
  const headers: HeadersInit = {
    'Content-Type': 'application/json',
  };
  if (apiKey) {
    headers['Authorization'] = `Bearer ${apiKey}`;
  }

  const response = await fetch(`${baseUrl}/recording/${streamId}/start`, {
    method: 'POST',
    headers,
    body: JSON.stringify({
      format: options?.format,
      sample_rate: options?.sampleRate,
      channels: options?.channels,
    }),
  });

  return handleResponse(response, deserializeRecordingInfo);
}

/**
 * Stop recording a stream.
 *
 * @param baseUrl - Gateway base URL
 * @param streamId - Stream ID to stop recording
 * @param apiKey - Optional API key for authentication
 * @returns Promise resolving to final recording info
 */
export async function stopRecording(
  baseUrl: string,
  streamId: string,
  apiKey?: string
): Promise<RecordingInfo> {
  const headers: HeadersInit = {
    'Content-Type': 'application/json',
  };
  if (apiKey) {
    headers['Authorization'] = `Bearer ${apiKey}`;
  }

  const response = await fetch(`${baseUrl}/recording/${streamId}/stop`, {
    method: 'POST',
    headers,
  });

  return handleResponse(response, deserializeRecordingInfo);
}
