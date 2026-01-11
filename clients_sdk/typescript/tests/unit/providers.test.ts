import { describe, it, expect } from 'vitest';
import {
  STT_PROVIDERS,
  TTS_PROVIDERS,
  REALTIME_PROVIDERS,
  isValidSTTProvider,
  isValidTTSProvider,
  isValidRealtimeProvider,
  getProviderCapabilities,
  type STTProvider,
  type TTSProvider,
  type RealtimeProvider,
} from '../../src/types/providers';

describe('STT Provider Types', () => {
  const expectedSTTProviders: STTProvider[] = [
    'deepgram',
    'google',
    'azure',
    'cartesia',
    'gateway',
    'assemblyai',
    'aws-transcribe',
    'ibm-watson',
    'groq',
    'openai-whisper',
  ];

  it('should have all 10 STT providers defined', () => {
    expect(STT_PROVIDERS).toHaveLength(10);
  });

  it('should include all expected STT providers', () => {
    for (const provider of expectedSTTProviders) {
      expect(STT_PROVIDERS).toContain(provider);
    }
  });

  it('should validate correct STT providers', () => {
    expect(isValidSTTProvider('deepgram')).toBe(true);
    expect(isValidSTTProvider('google')).toBe(true);
    expect(isValidSTTProvider('azure')).toBe(true);
    expect(isValidSTTProvider('assemblyai')).toBe(true);
    expect(isValidSTTProvider('aws-transcribe')).toBe(true);
    expect(isValidSTTProvider('ibm-watson')).toBe(true);
    expect(isValidSTTProvider('groq')).toBe(true);
    expect(isValidSTTProvider('openai-whisper')).toBe(true);
  });

  it('should reject invalid STT providers', () => {
    expect(isValidSTTProvider('invalid')).toBe(false);
    expect(isValidSTTProvider('')).toBe(false);
    expect(isValidSTTProvider('openai')).toBe(false); // TTS only
  });

  it('should return capabilities for each STT provider', () => {
    for (const provider of expectedSTTProviders) {
      const caps = getProviderCapabilities(provider, 'stt');
      expect(caps).toBeDefined();
      expect(caps.streaming).toBeDefined();
      expect(caps.languages).toBeInstanceOf(Array);
    }
  });
});

describe('TTS Provider Types', () => {
  const expectedTTSProviders: TTSProvider[] = [
    'deepgram',
    'elevenlabs',
    'google',
    'azure',
    'cartesia',
    'openai',
    'aws-polly',
    'ibm-watson',
    'hume',
    'lmnt',
    'playht',
    'kokoro',
  ];

  it('should have all 12 TTS providers defined', () => {
    expect(TTS_PROVIDERS).toHaveLength(12);
  });

  it('should include all expected TTS providers', () => {
    for (const provider of expectedTTSProviders) {
      expect(TTS_PROVIDERS).toContain(provider);
    }
  });

  it('should validate correct TTS providers', () => {
    expect(isValidTTSProvider('elevenlabs')).toBe(true);
    expect(isValidTTSProvider('deepgram')).toBe(true);
    expect(isValidTTSProvider('openai')).toBe(true);
    expect(isValidTTSProvider('aws-polly')).toBe(true);
    expect(isValidTTSProvider('hume')).toBe(true);
    expect(isValidTTSProvider('lmnt')).toBe(true);
    expect(isValidTTSProvider('playht')).toBe(true);
    expect(isValidTTSProvider('kokoro')).toBe(true);
  });

  it('should reject invalid TTS providers', () => {
    expect(isValidTTSProvider('invalid')).toBe(false);
    expect(isValidTTSProvider('')).toBe(false);
    expect(isValidTTSProvider('whisper')).toBe(false); // STT only
  });

  it('should return capabilities for each TTS provider', () => {
    for (const provider of expectedTTSProviders) {
      const caps = getProviderCapabilities(provider, 'tts');
      expect(caps).toBeDefined();
      expect(caps.streaming).toBeDefined();
      expect(caps.supportsEmotion).toBeDefined();
      expect(caps.voices).toBeInstanceOf(Array);
    }
  });

  it('should mark providers that support emotion', () => {
    // These providers support emotion
    expect(getProviderCapabilities('elevenlabs', 'tts').supportsEmotion).toBe(true);
    expect(getProviderCapabilities('hume', 'tts').supportsEmotion).toBe(true);
    expect(getProviderCapabilities('azure', 'tts').supportsEmotion).toBe(true);
    // These do not
    expect(getProviderCapabilities('deepgram', 'tts').supportsEmotion).toBe(false);
  });
});

describe('Realtime Provider Types', () => {
  const expectedRealtimeProviders: RealtimeProvider[] = [
    'openai-realtime',
    'hume-evi',
  ];

  it('should have all 2 Realtime providers defined', () => {
    expect(REALTIME_PROVIDERS).toHaveLength(2);
  });

  it('should include all expected Realtime providers', () => {
    for (const provider of expectedRealtimeProviders) {
      expect(REALTIME_PROVIDERS).toContain(provider);
    }
  });

  it('should validate correct Realtime providers', () => {
    expect(isValidRealtimeProvider('openai-realtime')).toBe(true);
    expect(isValidRealtimeProvider('hume-evi')).toBe(true);
  });

  it('should reject invalid Realtime providers', () => {
    expect(isValidRealtimeProvider('invalid')).toBe(false);
    expect(isValidRealtimeProvider('openai')).toBe(false);
    expect(isValidRealtimeProvider('hume')).toBe(false);
    expect(isValidRealtimeProvider('')).toBe(false);
  });

  it('should return capabilities for realtime providers', () => {
    const openaiCaps = getProviderCapabilities('openai-realtime', 'realtime');
    expect(openaiCaps.streaming).toBe(true);
    expect(openaiCaps.supportsFunctionCalling).toBe(true);
    expect(openaiCaps.supportsInterruption).toBe(true);

    const humeCaps = getProviderCapabilities('hume-evi', 'realtime');
    expect(humeCaps.streaming).toBe(true);
    expect(humeCaps.supportsEmotion).toBe(true);
    expect(humeCaps.supportsInterruption).toBe(true);
  });
});
