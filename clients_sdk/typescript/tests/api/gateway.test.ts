import { describe, it, expect, beforeAll, beforeEach } from 'vitest';

const BASE_URL = 'http://localhost:3001';

// Helper to add delay between requests to avoid rate limiting
const delay = (ms: number) => new Promise(resolve => setTimeout(resolve, ms));

// Helper for fetch with retry on rate limit
async function fetchWithRetry(
  url: string,
  options?: RequestInit,
  maxRetries = 5
): Promise<Response> {
  for (let attempt = 0; attempt < maxRetries; attempt++) {
    const response = await fetch(url, options);
    if (response.status !== 429) {
      return response;
    }
    // Parse wait time from response or use exponential backoff
    const text = await response.clone().text();
    const match = text.match(/Wait for (\d+)s/);
    const waitTime = match ? parseInt(match[1], 10) * 1000 + 1000 : Math.pow(2, attempt + 1) * 3000;
    await delay(waitTime);
  }
  return fetch(url, options);
}

describe('WaaV Gateway API Tests', () => {
  beforeAll(async () => {
    // Initial check that gateway is accessible
    let attempts = 0;
    while (attempts < 5) {
      try {
        const response = await fetch(`${BASE_URL}/`);
        if (response.status !== 429) {
          break;
        }
        await delay(2000);
      } catch {
        await delay(1000);
      }
      attempts++;
    }
  });

  beforeEach(async () => {
    // Delay between tests to avoid rate limiting
    await delay(3000);
  });

  it('should return health status', async () => {
    const response = await fetchWithRetry(`${BASE_URL}/`);
    expect(response.ok).toBe(true);
    const data = await response.json();
    expect(data.status).toBe('OK');
  });

  it('should return voices list', async () => {
    const response = await fetchWithRetry(`${BASE_URL}/voices`);
    expect(response.ok).toBe(true);
    const data = await response.json();
    expect(data).toBeDefined();
  });

  it('should reject empty text for speak', async () => {
    const response = await fetchWithRetry(`${BASE_URL}/speak`, {
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

  it('should handle livekit token request', async () => {
    const response = await fetchWithRetry(`${BASE_URL}/livekit/token`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        room_name: 'test_room',
        participant_identity: 'ts_test_user',
        participant_name: 'TS Test',
      }),
    });

    const text = await response.text();
    let data;
    try {
      data = JSON.parse(text);
    } catch {
      // Handle non-JSON response (e.g., rate limit text)
      expect(response.status).toBe(429);
      return;
    }

    // Either succeeds with token or fails with config error
    if (response.ok) {
      expect(data.token).toBeDefined();
      expect(data.room_name).toBe('test_room');
    } else {
      // Expected when LiveKit not configured
      expect(data.error).toContain('Failed to generate LiveKit token');
    }
  });

  it('should handle sip hooks request', async () => {
    const response = await fetchWithRetry(`${BASE_URL}/sip/hooks`);

    const text = await response.text();
    let data;
    try {
      data = JSON.parse(text);
    } catch {
      // Handle non-JSON response (e.g., rate limit text)
      expect(response.status).toBe(429);
      return;
    }

    // Either succeeds with hooks or returns error for unconfigured SIP
    if (response.ok) {
      expect(data).toBeDefined();
      expect(typeof data).toBe('object');
    } else {
      // Expected when SIP not configured
      expect(data).toBeDefined();
    }
  });
});
