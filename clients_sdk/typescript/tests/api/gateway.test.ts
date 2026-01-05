import { describe, it, expect, beforeAll } from 'vitest';

const BASE_URL = 'http://localhost:3001';

describe('WaaV Gateway API Tests', () => {
  beforeAll(async () => {
    // Wait for gateway to be ready
    const response = await fetch(`${BASE_URL}/`);
    expect(response.ok).toBe(true);
  });

  it('should return health status', async () => {
    const response = await fetch(`${BASE_URL}/`);
    expect(response.ok).toBe(true);
    const data = await response.json();
    expect(data.status).toBe('OK');
  });

  it('should return voices list', async () => {
    const response = await fetch(`${BASE_URL}/voices`);
    expect(response.ok).toBe(true);
    const data = await response.json();
    expect(data).toBeDefined();
  });

  it('should reject empty text for speak', async () => {
    const response = await fetch(`${BASE_URL}/speak`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        text: '',
        tts_config: {
          provider: 'deepgram',
          model: 'aura-asteria-en',
          voice_id: 'aura-asteria-en',
          audio_format: 'linear16',
          sample_rate: 24000,
        },
      }),
    });
    expect(response.status).toBe(400);
  });

  it('should generate livekit token', async () => {
    const response = await fetch(`${BASE_URL}/livekit/token`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        room_name: 'test_room',
        participant_identity: 'ts_test_user',
        participant_name: 'TS Test',
      }),
    });
    expect(response.ok).toBe(true);
    const data = await response.json();
    expect(data.token).toBeDefined();
    expect(data.room_name).toBe('test_room');
  });

  it('should return sip hooks', async () => {
    const response = await fetch(`${BASE_URL}/sip/hooks`);
    expect(response.ok).toBe(true);
    const data = await response.json();
    // SIP hooks returns an object with hooks array
    expect(data).toBeDefined();
    expect(typeof data).toBe('object');
  });
});
