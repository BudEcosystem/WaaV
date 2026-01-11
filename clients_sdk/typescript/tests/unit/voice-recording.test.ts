import { describe, it, expect, vi, beforeEach } from 'vitest';
import {
  VoiceCloneRequest,
  VoiceCloneResponse,
  VoiceCloneProvider,
  RecordingInfo,
  RecordingFilter,
  VOICE_CLONE_PROVIDERS,
} from '../../src/types/voice';
import {
  cloneVoice,
  listClonedVoices,
  deleteClonedVoice,
  getRecording,
  downloadRecording,
  listRecordings,
} from '../../src/rest/voice';

// Mock fetch for testing
const mockFetch = vi.fn();
vi.stubGlobal('fetch', mockFetch);

describe('Voice Cloning Types', () => {
  it('should have all voice clone providers defined', () => {
    const expectedProviders: VoiceCloneProvider[] = ['elevenlabs', 'playht', 'resemble'];
    for (const provider of expectedProviders) {
      expect(VOICE_CLONE_PROVIDERS).toContain(provider);
    }
  });

  it('should define VoiceCloneRequest interface correctly', () => {
    const request: VoiceCloneRequest = {
      name: 'My Voice',
      audioFiles: [new ArrayBuffer(1024)],
      provider: 'elevenlabs',
      description: 'A custom voice',
    };

    expect(request.name).toBe('My Voice');
    expect(request.provider).toBe('elevenlabs');
    expect(request.audioFiles).toHaveLength(1);
  });

  it('should define VoiceCloneResponse interface correctly', () => {
    const response: VoiceCloneResponse = {
      voiceId: 'voice_123',
      name: 'My Voice',
      provider: 'elevenlabs',
      status: 'ready',
      createdAt: '2024-01-01T00:00:00Z',
      metadata: { labels: ['custom'] },
    };

    expect(response.voiceId).toBe('voice_123');
    expect(response.status).toBe('ready');
  });

  it('should support all status values', () => {
    const statuses: VoiceCloneResponse['status'][] = ['ready', 'processing', 'failed'];

    for (const status of statuses) {
      const response: VoiceCloneResponse = {
        voiceId: 'v1',
        name: 'Test',
        provider: 'elevenlabs',
        status,
        createdAt: new Date().toISOString(),
      };
      expect(response.status).toBe(status);
    }
  });
});

describe('Voice Cloning API', () => {
  beforeEach(() => {
    mockFetch.mockReset();
  });

  describe('cloneVoice', () => {
    it('should send correct request for ElevenLabs', async () => {
      mockFetch.mockResolvedValueOnce({
        ok: true,
        json: () => Promise.resolve({
          voice_id: 'voice_123',
          name: 'My Voice',
          provider: 'elevenlabs',
          status: 'processing',
          created_at: '2024-01-01T00:00:00Z',
        }),
      });

      const result = await cloneVoice('http://localhost:3001', {
        name: 'My Voice',
        audioFiles: [new ArrayBuffer(1024)],
        provider: 'elevenlabs',
        description: 'Test voice',
      });

      expect(mockFetch).toHaveBeenCalledWith(
        'http://localhost:3001/voices/clone',
        expect.objectContaining({
          method: 'POST',
        })
      );

      expect(result.voiceId).toBe('voice_123');
      expect(result.status).toBe('processing');
    });

    it('should send correct request for PlayHT', async () => {
      mockFetch.mockResolvedValueOnce({
        ok: true,
        json: () => Promise.resolve({
          voice_id: 'playht_voice_456',
          name: 'PlayHT Voice',
          provider: 'playht',
          status: 'ready',
          created_at: '2024-01-01T00:00:00Z',
        }),
      });

      const result = await cloneVoice('http://localhost:3001', {
        name: 'PlayHT Voice',
        audioFiles: [new ArrayBuffer(2048)],
        provider: 'playht',
      });

      expect(result.provider).toBe('playht');
    });

    it('should send correct request for Resemble', async () => {
      mockFetch.mockResolvedValueOnce({
        ok: true,
        json: () => Promise.resolve({
          voice_id: 'resemble_voice_789',
          name: 'Resemble Voice',
          provider: 'resemble',
          status: 'ready',
          created_at: '2024-01-01T00:00:00Z',
        }),
      });

      const result = await cloneVoice('http://localhost:3001', {
        name: 'Resemble Voice',
        audioFiles: [new ArrayBuffer(4096)],
        provider: 'resemble',
      });

      expect(result.provider).toBe('resemble');
    });

    it('should handle API errors', async () => {
      mockFetch.mockResolvedValueOnce({
        ok: false,
        status: 400,
        json: () => Promise.resolve({ error: 'Invalid audio format' }),
      });

      await expect(cloneVoice('http://localhost:3001', {
        name: 'Test',
        audioFiles: [],
        provider: 'elevenlabs',
      })).rejects.toThrow();
    });
  });

  describe('listClonedVoices', () => {
    it('should return list of cloned voices', async () => {
      mockFetch.mockResolvedValueOnce({
        ok: true,
        json: () => Promise.resolve({
          voices: [
            { voice_id: 'v1', name: 'Voice 1', provider: 'elevenlabs', status: 'ready', created_at: '2024-01-01T00:00:00Z' },
            { voice_id: 'v2', name: 'Voice 2', provider: 'playht', status: 'ready', created_at: '2024-01-02T00:00:00Z' },
          ],
        }),
      });

      const voices = await listClonedVoices('http://localhost:3001');

      expect(voices).toHaveLength(2);
      expect(voices[0].voiceId).toBe('v1');
      expect(voices[1].voiceId).toBe('v2');
    });

    it('should filter by provider', async () => {
      mockFetch.mockResolvedValueOnce({
        ok: true,
        json: () => Promise.resolve({
          voices: [
            { voice_id: 'v1', name: 'Voice 1', provider: 'elevenlabs', status: 'ready', created_at: '2024-01-01T00:00:00Z' },
          ],
        }),
      });

      await listClonedVoices('http://localhost:3001', { provider: 'elevenlabs' });

      expect(mockFetch).toHaveBeenCalledWith(
        expect.stringContaining('provider=elevenlabs'),
        expect.any(Object)
      );
    });
  });

  describe('deleteClonedVoice', () => {
    it('should delete voice successfully', async () => {
      mockFetch.mockResolvedValueOnce({
        ok: true,
        json: () => Promise.resolve({}),
      });

      await expect(deleteClonedVoice('http://localhost:3001', 'voice_123')).resolves.not.toThrow();

      expect(mockFetch).toHaveBeenCalledWith(
        'http://localhost:3001/voices/voice_123',
        expect.objectContaining({
          method: 'DELETE',
        })
      );
    });

    it('should handle not found error', async () => {
      mockFetch.mockResolvedValueOnce({
        ok: false,
        status: 404,
        json: () => Promise.resolve({ error: 'Voice not found' }),
      });

      await expect(deleteClonedVoice('http://localhost:3001', 'nonexistent')).rejects.toThrow();
    });
  });
});

describe('Recording Types', () => {
  it('should define RecordingInfo interface', () => {
    const recording: RecordingInfo = {
      streamId: 'stream_123',
      roomName: 'test-room',
      duration: 120.5,
      size: 1024000,
      format: 'wav',
      createdAt: '2024-01-01T00:00:00Z',
      status: 'completed',
    };

    expect(recording.streamId).toBe('stream_123');
    expect(recording.duration).toBe(120.5);
    expect(recording.status).toBe('completed');
  });

  it('should define RecordingFilter interface', () => {
    const filter: RecordingFilter = {
      roomName: 'test-room',
      startDate: '2024-01-01',
      endDate: '2024-01-31',
      status: 'completed',
      limit: 10,
      offset: 0,
    };

    expect(filter.roomName).toBe('test-room');
    expect(filter.limit).toBe(10);
  });
});

describe('Recording API', () => {
  beforeEach(() => {
    mockFetch.mockReset();
  });

  describe('getRecording', () => {
    it('should return recording info', async () => {
      mockFetch.mockResolvedValueOnce({
        ok: true,
        json: () => Promise.resolve({
          stream_id: 'stream_123',
          room_name: 'test-room',
          duration: 120.5,
          size: 1024000,
          format: 'wav',
          created_at: '2024-01-01T00:00:00Z',
          status: 'completed',
        }),
      });

      const recording = await getRecording('http://localhost:3001', 'stream_123');

      expect(recording.streamId).toBe('stream_123');
      expect(recording.duration).toBe(120.5);
    });

    it('should handle not found error', async () => {
      mockFetch.mockResolvedValueOnce({
        ok: false,
        status: 404,
        json: () => Promise.resolve({ error: 'Recording not found' }),
      });

      await expect(getRecording('http://localhost:3001', 'nonexistent')).rejects.toThrow();
    });
  });

  describe('downloadRecording', () => {
    it('should download recording as Blob', async () => {
      const mockBlob = new Blob(['audio data'], { type: 'audio/wav' });
      mockFetch.mockResolvedValueOnce({
        ok: true,
        blob: () => Promise.resolve(mockBlob),
      });

      const blob = await downloadRecording('http://localhost:3001', 'stream_123');

      expect(mockFetch).toHaveBeenCalledWith(
        'http://localhost:3001/recording/stream_123',
        expect.any(Object)
      );
      expect(blob).toBeInstanceOf(Blob);
    });

    it('should support format parameter', async () => {
      const mockBlob = new Blob(['audio data'], { type: 'audio/mp3' });
      mockFetch.mockResolvedValueOnce({
        ok: true,
        blob: () => Promise.resolve(mockBlob),
      });

      await downloadRecording('http://localhost:3001', 'stream_123', { format: 'mp3' });

      expect(mockFetch).toHaveBeenCalledWith(
        expect.stringContaining('format=mp3'),
        expect.any(Object)
      );
    });
  });

  describe('listRecordings', () => {
    it('should return list of recordings', async () => {
      mockFetch.mockResolvedValueOnce({
        ok: true,
        json: () => Promise.resolve({
          recordings: [
            { stream_id: 's1', room_name: 'room1', duration: 60, size: 512000, format: 'wav', created_at: '2024-01-01T00:00:00Z', status: 'completed' },
            { stream_id: 's2', room_name: 'room2', duration: 120, size: 1024000, format: 'wav', created_at: '2024-01-02T00:00:00Z', status: 'completed' },
          ],
          total: 2,
        }),
      });

      const result = await listRecordings('http://localhost:3001');

      expect(result.recordings).toHaveLength(2);
      expect(result.total).toBe(2);
    });

    it('should apply filters', async () => {
      mockFetch.mockResolvedValueOnce({
        ok: true,
        json: () => Promise.resolve({
          recordings: [],
          total: 0,
        }),
      });

      await listRecordings('http://localhost:3001', {
        roomName: 'test-room',
        status: 'completed',
        limit: 10,
      });

      expect(mockFetch).toHaveBeenCalledWith(
        expect.stringMatching(/room_name=test-room.*status=completed.*limit=10/),
        expect.any(Object)
      );
    });

    it('should paginate results', async () => {
      mockFetch.mockResolvedValueOnce({
        ok: true,
        json: () => Promise.resolve({
          recordings: [],
          total: 100,
          has_more: true,
        }),
      });

      const result = await listRecordings('http://localhost:3001', {
        limit: 10,
        offset: 20,
      });

      expect(result.hasMore).toBe(true);
    });
  });
});
