import { describe, it, expect } from 'vitest';
import {
  AudioFeatures,
  TurnDetectionConfig,
  NoiseFilterConfig,
  VADConfig,
  createAudioFeatures,
  serializeAudioFeatures,
  DEFAULT_TURN_DETECTION,
  DEFAULT_NOISE_FILTER,
  DEFAULT_VAD,
} from '../../src/types/audio-features';

describe('Turn Detection Config', () => {
  it('should have sensible defaults', () => {
    expect(DEFAULT_TURN_DETECTION.enabled).toBe(false);
    expect(DEFAULT_TURN_DETECTION.threshold).toBeGreaterThan(0);
    expect(DEFAULT_TURN_DETECTION.threshold).toBeLessThanOrEqual(1);
    expect(DEFAULT_TURN_DETECTION.silenceMs).toBeGreaterThan(0);
  });

  it('should create turn detection config with custom values', () => {
    const config: TurnDetectionConfig = {
      enabled: true,
      threshold: 0.7,
      silenceMs: 800,
      prefixPaddingMs: 300,
      createResponseMs: 500,
    };

    expect(config.enabled).toBe(true);
    expect(config.threshold).toBe(0.7);
    expect(config.silenceMs).toBe(800);
  });

  it('should clamp threshold to valid range 0-1', () => {
    const features = createAudioFeatures({
      turnDetection: { enabled: true, threshold: 1.5 },
    });

    expect(features.turnDetection?.threshold).toBeLessThanOrEqual(1);
  });

  it('should enforce minimum silence duration', () => {
    const features = createAudioFeatures({
      turnDetection: { enabled: true, silenceMs: 10 },
    });

    // Minimum silence should be at least 100ms
    expect(features.turnDetection?.silenceMs).toBeGreaterThanOrEqual(100);
  });
});

describe('Noise Filter Config', () => {
  it('should have sensible defaults', () => {
    expect(DEFAULT_NOISE_FILTER.enabled).toBe(false);
    expect(DEFAULT_NOISE_FILTER.strength).toBe('medium');
  });

  it('should accept all strength levels', () => {
    const strengths: Array<'low' | 'medium' | 'high'> = ['low', 'medium', 'high'];

    for (const strength of strengths) {
      const config: NoiseFilterConfig = {
        enabled: true,
        strength,
      };
      expect(config.strength).toBe(strength);
    }
  });

  it('should support numeric strength values', () => {
    const config: NoiseFilterConfig = {
      enabled: true,
      strengthValue: 0.75, // 0-1 range
    };

    expect(config.strengthValue).toBe(0.75);
  });
});

describe('VAD (Voice Activity Detection) Config', () => {
  it('should have sensible defaults', () => {
    expect(DEFAULT_VAD.enabled).toBe(true); // VAD should be on by default
    expect(DEFAULT_VAD.threshold).toBeGreaterThan(0);
  });

  it('should support different VAD modes', () => {
    const config: VADConfig = {
      enabled: true,
      threshold: 0.5,
      mode: 'aggressive', // For noisy environments
    };

    expect(config.mode).toBe('aggressive');
  });

  it('should support voice detection callback', () => {
    const config: VADConfig = {
      enabled: true,
      onSpeechStart: () => {},
      onSpeechEnd: () => {},
    };

    expect(config.onSpeechStart).toBeDefined();
    expect(config.onSpeechEnd).toBeDefined();
  });
});

describe('Audio Features Creation', () => {
  it('should create empty features with defaults', () => {
    const features = createAudioFeatures({});

    expect(features.turnDetection?.enabled).toBe(false);
    expect(features.noiseFiltering?.enabled).toBe(false);
    expect(features.vad?.enabled).toBe(true); // VAD on by default
  });

  it('should create features with turn detection enabled', () => {
    const features = createAudioFeatures({
      turnDetection: { enabled: true },
    });

    expect(features.turnDetection?.enabled).toBe(true);
    expect(features.turnDetection?.threshold).toBe(DEFAULT_TURN_DETECTION.threshold);
  });

  it('should create features with noise filtering enabled', () => {
    const features = createAudioFeatures({
      noiseFiltering: { enabled: true, strength: 'high' },
    });

    expect(features.noiseFiltering?.enabled).toBe(true);
    expect(features.noiseFiltering?.strength).toBe('high');
  });

  it('should create features with all options combined', () => {
    const features = createAudioFeatures({
      turnDetection: { enabled: true, threshold: 0.8 },
      noiseFiltering: { enabled: true, strength: 'medium' },
      vad: { enabled: true, threshold: 0.6 },
    });

    expect(features.turnDetection?.enabled).toBe(true);
    expect(features.noiseFiltering?.enabled).toBe(true);
    expect(features.vad?.enabled).toBe(true);
  });
});

describe('Audio Features Serialization', () => {
  it('should serialize to wire format with snake_case keys', () => {
    const features: AudioFeatures = {
      turnDetection: {
        enabled: true,
        threshold: 0.7,
        silenceMs: 500,
        prefixPaddingMs: 200,
        createResponseMs: 400,
      },
      noiseFiltering: {
        enabled: true,
        strength: 'high',
      },
      vad: {
        enabled: true,
        threshold: 0.5,
      },
    };

    const wire = serializeAudioFeatures(features);

    expect(wire.turn_detection).toBeDefined();
    expect(wire.turn_detection.enabled).toBe(true);
    expect(wire.turn_detection.threshold).toBe(0.7);
    expect(wire.turn_detection.silence_ms).toBe(500);
    expect(wire.turn_detection.prefix_padding_ms).toBe(200);
    expect(wire.turn_detection.create_response_ms).toBe(400);

    expect(wire.noise_filtering).toBeDefined();
    expect(wire.noise_filtering.enabled).toBe(true);
    expect(wire.noise_filtering.strength).toBe('high');

    expect(wire.vad).toBeDefined();
    expect(wire.vad.enabled).toBe(true);
    expect(wire.vad.threshold).toBe(0.5);
  });

  it('should omit disabled features in wire format', () => {
    const features: AudioFeatures = {
      turnDetection: { enabled: false },
      noiseFiltering: { enabled: false },
      vad: { enabled: false },
    };

    const wire = serializeAudioFeatures(features);

    // Disabled features should still be present but with enabled: false
    expect(wire.turn_detection?.enabled).toBe(false);
    expect(wire.noise_filtering?.enabled).toBe(false);
    expect(wire.vad?.enabled).toBe(false);
  });
});

describe('Audio Features Integration', () => {
  it('should work with session config', () => {
    const sessionFeatures = createAudioFeatures({
      turnDetection: {
        enabled: true,
        threshold: 0.8,
        silenceMs: 600,
      },
      noiseFiltering: {
        enabled: true,
        strength: 'medium',
      },
    });

    // Verify the config can be spread into session config
    const mockSessionConfig = {
      url: 'wss://example.com/ws',
      audioFeatures: sessionFeatures,
    };

    expect(mockSessionConfig.audioFeatures.turnDetection?.enabled).toBe(true);
    expect(mockSessionConfig.audioFeatures.noiseFiltering?.enabled).toBe(true);
  });

  it('should handle partial updates', () => {
    const initial = createAudioFeatures({
      turnDetection: { enabled: true, threshold: 0.5 },
    });

    // Simulate update
    const updated = createAudioFeatures({
      ...initial,
      turnDetection: {
        ...initial.turnDetection,
        threshold: 0.8,
      },
    });

    expect(updated.turnDetection?.threshold).toBe(0.8);
    expect(updated.turnDetection?.enabled).toBe(true);
  });
});
