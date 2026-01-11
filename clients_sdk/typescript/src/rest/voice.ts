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
  buildRecordingFilterQuery,
  buildVoiceCloneFilterQuery,
} from '../types/voice';

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
  // Create form data for multipart upload
  const formData = new FormData();
  formData.append('name', request.name);
  formData.append('provider', request.provider);

  if (request.description) {
    formData.append('description', request.description);
  }

  if (request.labels) {
    formData.append('labels', JSON.stringify(request.labels));
  }

  // Append audio files
  for (let i = 0; i < request.audioFiles.length; i++) {
    const blob = new Blob([request.audioFiles[i]], { type: 'audio/wav' });
    formData.append(`audio_${i}`, blob, `audio_${i}.wav`);
  }

  const headers: HeadersInit = {};
  if (apiKey) {
    headers['Authorization'] = `Bearer ${apiKey}`;
  }

  const response = await fetch(`${baseUrl}/voices/clone`, {
    method: 'POST',
    headers,
    body: formData,
  });

  return handleResponse(response, deserializeVoiceCloneResponse);
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
