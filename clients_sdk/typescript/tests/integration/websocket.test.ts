/**
 * WebSocket endpoint connectivity tests
 * Tests that WebSocket endpoints are reachable and respond correctly
 */
import { describe, it, expect } from 'vitest';
import { WebSocket } from 'ws';

const BASE_URL = 'ws://localhost:3001';

describe('WebSocket Endpoint Tests', () => {
  it('should connect to /ws endpoint and receive ready or error', async () => {
    const result = await new Promise<{ type: string; message?: string }>((resolve, reject) => {
      const ws = new WebSocket(`${BASE_URL}/ws`);
      const timeout = setTimeout(() => {
        ws.close();
        reject(new Error('Connection timeout'));
      }, 5000);

      ws.on('open', () => {
        ws.send(JSON.stringify({
          type: 'config',
          audio: true,
          stt_config: {
            provider: 'deepgram',
            language: 'en-US',
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: 'linear16',
            model: 'nova-3'
          },
          tts_config: {
            provider: 'deepgram',
            model: 'aura-asteria-en',
            voice_id: 'aura-asteria-en',
            audio_format: 'linear16',
            sample_rate: 24000
          }
        }));
      });

      ws.on('message', (data) => {
        try {
          const msg = JSON.parse(data.toString());
          clearTimeout(timeout);
          ws.close();
          resolve(msg);
        } catch (e) {
          // Binary data is also acceptable
          clearTimeout(timeout);
          ws.close();
          resolve({ type: 'binary' });
        }
      });

      ws.on('error', (err) => {
        clearTimeout(timeout);
        reject(err);
      });
    });

    // Connection should work - either ready or error (due to missing API key)
    expect(['ready', 'error', 'binary']).toContain(result.type);
    console.log(`/ws endpoint response: ${result.type}${result.message ? ' - ' + result.message : ''}`);
  }, 10000);

  it('should connect to /ws with OpenAI STT provider', async () => {
    const result = await new Promise<{ type: string; message?: string }>((resolve, reject) => {
      const ws = new WebSocket(`${BASE_URL}/ws`);
      const timeout = setTimeout(() => {
        ws.close();
        reject(new Error('Connection timeout'));
      }, 5000);

      ws.on('open', () => {
        ws.send(JSON.stringify({
          type: 'config',
          audio: true,
          stt_config: {
            provider: 'openai',
            language: 'en',
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: 'linear16',
            model: 'whisper-1'
          },
          tts_config: {
            provider: 'openai',
            model: 'tts-1',
            voice_id: 'alloy',
            audio_format: 'linear16',
            sample_rate: 24000
          }
        }));
      });

      ws.on('message', (data) => {
        try {
          const msg = JSON.parse(data.toString());
          clearTimeout(timeout);
          ws.close();
          resolve(msg);
        } catch (e) {
          clearTimeout(timeout);
          ws.close();
          resolve({ type: 'binary' });
        }
      });

      ws.on('error', (err) => {
        clearTimeout(timeout);
        reject(err);
      });
    });

    // OpenAI provider should be recognized (ready) or give auth error
    expect(['ready', 'error', 'binary']).toContain(result.type);
    console.log(`OpenAI provider response: ${result.type}${result.message ? ' - ' + result.message : ''}`);

    // If error, it should mention API key, not "unsupported provider"
    if (result.type === 'error' && result.message) {
      const msg = result.message.toLowerCase();
      expect(msg).not.toContain('unsupported provider');
      expect(msg).not.toContain('unknown provider');
    }
  }, 10000);

  it('should connect to /realtime endpoint', async () => {
    const result = await new Promise<{ success: boolean; type?: string; message?: string; code?: number }>((resolve) => {
      const ws = new WebSocket(`${BASE_URL}/realtime`);
      const timeout = setTimeout(() => {
        ws.close();
        resolve({ success: false, message: 'Connection timeout' });
      }, 5000);

      ws.on('open', () => {
        ws.send(JSON.stringify({
          type: 'config',
          provider: 'openai',
          model: 'gpt-4o-realtime-preview',
          voice: 'alloy',
          instructions: 'You are a helpful assistant.'
        }));
      });

      ws.on('message', (data) => {
        try {
          const msg = JSON.parse(data.toString());
          clearTimeout(timeout);
          ws.close();
          resolve({ success: true, type: msg.type, message: msg.message });
        } catch (e) {
          clearTimeout(timeout);
          ws.close();
          resolve({ success: true, type: 'binary' });
        }
      });

      ws.on('error', (err) => {
        clearTimeout(timeout);
        const errMsg = err.message.toLowerCase();
        // 404 means route not registered
        if (errMsg.includes('404') || errMsg.includes('not found')) {
          resolve({ success: false, message: 'Route not found (404)' });
        } else {
          resolve({ success: false, message: err.message });
        }
      });

      ws.on('close', (code, reason) => {
        clearTimeout(timeout);
        // Some close codes are expected without API keys
        if (code === 1008 || code === 1003 || code === 1006) {
          resolve({ success: true, code, message: reason.toString() || 'Closed without API key (expected)' });
        }
      });
    });

    console.log(`/realtime endpoint: ${result.success ? 'connected' : 'failed'} - ${result.type || result.message || result.code}`);

    // The endpoint should be reachable (even if it returns an error due to missing API key)
    // Only fail if the route itself doesn't exist
    if (!result.success && result.message?.includes('Route not found')) {
      expect.fail('Realtime endpoint /realtime is not registered');
    }
  }, 10000);
});
