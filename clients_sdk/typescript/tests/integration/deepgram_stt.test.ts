import { describe, it, expect, beforeAll } from 'vitest';
import { WebSocket } from 'ws';
import * as fs from 'fs';
import * as path from 'path';

const WS_URL = 'ws://localhost:3001/ws';
const AUDIO_FILE = '/tmp/audio_test_data/speech_16k.wav';

interface STTResult {
  type: string;
  transcript?: string;
  is_final?: boolean;
  confidence?: number;
}

describe('DeepGram STT Integration Tests', () => {
  let audioBuffer: Buffer;
  const SAMPLE_RATE = 16000;

  beforeAll(async () => {
    // Check if audio file exists
    if (!fs.existsSync(AUDIO_FILE)) {
      throw new Error(`Audio file not found: ${AUDIO_FILE}. Run Python tests first to download test data.`);
    }
    // Read WAV file (skip 44-byte header for raw PCM)
    const fileBuffer = fs.readFileSync(AUDIO_FILE);
    audioBuffer = fileBuffer.subarray(44); // Skip WAV header
  });

  it('should establish WebSocket connection and receive ready', async () => {
    const ws = new WebSocket(WS_URL);

    const ready = await new Promise<any>((resolve, reject) => {
      const timeout = setTimeout(() => reject(new Error('Connection timeout')), 10000);

      ws.on('open', () => {
        // Send config
        ws.send(JSON.stringify({
          type: 'config',
          audio: true,
          stt_config: {
            provider: 'deepgram',
            language: 'en-US',
            sample_rate: SAMPLE_RATE,
            channels: 1,
            punctuation: true,
            encoding: 'linear16',
            model: 'nova-3',
            interim_results: true
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

      ws.on('message', (data: Buffer | string) => {
        const msg = JSON.parse(data.toString());
        if (msg.type === 'ready') {
          clearTimeout(timeout);
          resolve(msg);
        } else if (msg.type === 'error') {
          clearTimeout(timeout);
          reject(new Error(msg.message || 'Unknown error'));
        }
      });

      ws.on('error', (err) => {
        clearTimeout(timeout);
        reject(err);
      });
    });

    expect(ready.type).toBe('ready');
    expect(ready.stream_id).toBeDefined();
    ws.close();
  });

  it('should transcribe audio and return STT results', async () => {
    const ws = new WebSocket(WS_URL, { maxPayload: 10 * 1024 * 1024 });
    const transcripts: string[] = [];
    let firstTokenTime: number | null = null;
    const startTime = Date.now();

    await new Promise<void>((resolve, reject) => {
      const timeout = setTimeout(() => resolve(), 15000); // Max 15s for test
      let isReady = false;

      ws.on('open', () => {
        // Send config
        ws.send(JSON.stringify({
          type: 'config',
          audio: true,
          stt_config: {
            provider: 'deepgram',
            language: 'en-US',
            sample_rate: SAMPLE_RATE,
            channels: 1,
            punctuation: true,
            encoding: 'linear16',
            model: 'nova-3',
            interim_results: true
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

      ws.on('message', async (data: Buffer | string) => {
        if (Buffer.isBuffer(data) && data.length < 100) {
          // Skip small binary messages
          return;
        }

        try {
          const msg = JSON.parse(data.toString());

          if (msg.type === 'ready' && !isReady) {
            isReady = true;
            // Stream audio chunks (5s max)
            const chunkSize = 3200; // 100ms at 16kHz
            const maxBytes = 5 * SAMPLE_RATE * 2; // 5 seconds
            let offset = 0;

            const sendChunks = async () => {
              while (offset < Math.min(audioBuffer.length, maxBytes)) {
                const chunk = audioBuffer.subarray(offset, offset + chunkSize);
                ws.send(chunk);
                offset += chunkSize;
                await new Promise(r => setTimeout(r, 50)); // 50ms pace
              }
            };

            sendChunks().catch(console.error);
          }

          if (msg.type === 'stt_result') {
            if (msg.transcript && firstTokenTime === null) {
              firstTokenTime = Date.now();
            }
            if (msg.is_final && msg.transcript) {
              transcripts.push(msg.transcript);
            }
          }
        } catch (e) {
          // Ignore parse errors for binary data
        }
      });

      ws.on('error', (err) => {
        clearTimeout(timeout);
        reject(err);
      });

      ws.on('close', () => {
        clearTimeout(timeout);
        resolve();
      });
    });

    ws.close();

    // Verify results
    expect(transcripts.length).toBeGreaterThan(0);
    const fullTranscript = transcripts.join(' ').toLowerCase();
    expect(fullTranscript).toContain('birch'); // Expected word from test audio

    // Check TTFT
    if (firstTokenTime) {
      const ttft = firstTokenTime - startTime;
      console.log(`TTFT: ${ttft}ms`);
      // TTFT should be reasonable (< 5s for streaming)
      expect(ttft).toBeLessThan(5000);
    }
  }, 20000); // 20s test timeout

  it('should handle multiple rapid connections', async () => {
    const numConnections = 3;
    const results = await Promise.all(
      Array(numConnections).fill(0).map(async (_, i) => {
        const ws = new WebSocket(WS_URL);

        return new Promise<{ success: boolean; connectTime: number }>((resolve) => {
          const start = Date.now();
          const timeout = setTimeout(() => {
            ws.close();
            resolve({ success: false, connectTime: Date.now() - start });
          }, 10000);

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
                model: 'nova-3',
                interim_results: false
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
              if (msg.type === 'ready') {
                clearTimeout(timeout);
                ws.close();
                resolve({ success: true, connectTime: Date.now() - start });
              }
            } catch (e) {}
          });

          ws.on('error', () => {
            clearTimeout(timeout);
            ws.close();
            resolve({ success: false, connectTime: Date.now() - start });
          });
        });
      })
    );

    const successful = results.filter(r => r.success);
    console.log(`Successful connections: ${successful.length}/${numConnections}`);
    console.log(`Avg connect time: ${successful.reduce((a, r) => a + r.connectTime, 0) / successful.length}ms`);

    // At least 2 out of 3 should succeed
    expect(successful.length).toBeGreaterThanOrEqual(2);
  }, 15000);
});
