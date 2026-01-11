// =============================================================================
// Audio Features Types
// =============================================================================

/**
 * Turn detection configuration.
 * Detects when a speaker has finished speaking.
 */
export interface TurnDetectionConfig {
  /** Enable turn detection */
  enabled: boolean;
  /** Detection threshold (0.0-1.0) */
  threshold?: number;
  /** Silence duration in ms to trigger turn end */
  silenceMs?: number;
  /** Padding before speech in ms */
  prefixPaddingMs?: number;
  /** Delay before creating response in ms */
  createResponseMs?: number;
}

/**
 * Noise filtering configuration.
 */
export interface NoiseFilterConfig {
  /** Enable noise filtering */
  enabled: boolean;
  /** Noise reduction strength */
  strength?: 'low' | 'medium' | 'high';
  /** Numeric strength value (0.0-1.0), overrides strength if provided */
  strengthValue?: number;
}

/**
 * VAD (Voice Activity Detection) configuration.
 */
export interface VADConfig {
  /** Enable VAD */
  enabled: boolean;
  /** Detection threshold (0.0-1.0) */
  threshold?: number;
  /** VAD mode for different environments */
  mode?: 'normal' | 'aggressive' | 'very_aggressive';
  /** Callback when speech starts */
  onSpeechStart?: () => void;
  /** Callback when speech ends */
  onSpeechEnd?: () => void;
}

/**
 * Combined audio features configuration.
 */
export interface AudioFeatures {
  /** Turn detection settings */
  turnDetection?: TurnDetectionConfig;
  /** Noise filtering settings */
  noiseFiltering?: NoiseFilterConfig;
  /** Voice activity detection settings */
  vad?: VADConfig;
}

// =============================================================================
// Default Configurations
// =============================================================================

/**
 * Default turn detection configuration.
 */
export const DEFAULT_TURN_DETECTION: TurnDetectionConfig = {
  enabled: false,
  threshold: 0.5,
  silenceMs: 500,
  prefixPaddingMs: 200,
  createResponseMs: 300,
};

/**
 * Default noise filter configuration.
 */
export const DEFAULT_NOISE_FILTER: NoiseFilterConfig = {
  enabled: false,
  strength: 'medium',
};

/**
 * Default VAD configuration.
 */
export const DEFAULT_VAD: VADConfig = {
  enabled: true, // VAD is on by default
  threshold: 0.5,
  mode: 'normal',
};

// =============================================================================
// Constants
// =============================================================================

/** Minimum allowed silence duration in ms */
const MIN_SILENCE_MS = 100;

/** Maximum allowed threshold value */
const MAX_THRESHOLD = 1.0;

/** Minimum allowed threshold value */
const MIN_THRESHOLD = 0.0;

// =============================================================================
// Factory Functions
// =============================================================================

/**
 * Create audio features configuration with defaults.
 * Validates and clamps values to valid ranges.
 */
export function createAudioFeatures(
  config: Partial<{
    turnDetection: Partial<TurnDetectionConfig>;
    noiseFiltering: Partial<NoiseFilterConfig>;
    vad: Partial<VADConfig>;
  }>
): AudioFeatures {
  const features: AudioFeatures = {};

  // Turn detection with validation
  if (config.turnDetection) {
    features.turnDetection = {
      ...DEFAULT_TURN_DETECTION,
      ...config.turnDetection,
    };

    // Clamp threshold to valid range
    if (features.turnDetection.threshold !== undefined) {
      features.turnDetection.threshold = Math.min(
        MAX_THRESHOLD,
        Math.max(MIN_THRESHOLD, features.turnDetection.threshold)
      );
    }

    // Enforce minimum silence duration
    if (features.turnDetection.silenceMs !== undefined) {
      features.turnDetection.silenceMs = Math.max(MIN_SILENCE_MS, features.turnDetection.silenceMs);
    }
  } else {
    features.turnDetection = { ...DEFAULT_TURN_DETECTION };
  }

  // Noise filtering
  if (config.noiseFiltering) {
    features.noiseFiltering = {
      ...DEFAULT_NOISE_FILTER,
      ...config.noiseFiltering,
    };

    // Clamp strength value if provided
    if (features.noiseFiltering.strengthValue !== undefined) {
      features.noiseFiltering.strengthValue = Math.min(
        MAX_THRESHOLD,
        Math.max(MIN_THRESHOLD, features.noiseFiltering.strengthValue)
      );
    }
  } else {
    features.noiseFiltering = { ...DEFAULT_NOISE_FILTER };
  }

  // VAD
  if (config.vad) {
    features.vad = {
      ...DEFAULT_VAD,
      ...config.vad,
    };

    // Clamp threshold
    if (features.vad.threshold !== undefined) {
      features.vad.threshold = Math.min(
        MAX_THRESHOLD,
        Math.max(MIN_THRESHOLD, features.vad.threshold)
      );
    }
  } else {
    features.vad = { ...DEFAULT_VAD };
  }

  return features;
}

// =============================================================================
// Wire Format Types (snake_case for protocol)
// =============================================================================

interface TurnDetectionWire {
  enabled: boolean;
  threshold?: number;
  silence_ms?: number;
  prefix_padding_ms?: number;
  create_response_ms?: number;
}

interface NoiseFilterWire {
  enabled: boolean;
  strength?: 'low' | 'medium' | 'high';
  strength_value?: number;
}

interface VADWire {
  enabled: boolean;
  threshold?: number;
  mode?: 'normal' | 'aggressive' | 'very_aggressive';
}

interface AudioFeaturesWire {
  turn_detection?: TurnDetectionWire;
  noise_filtering?: NoiseFilterWire;
  vad?: VADWire;
}

// =============================================================================
// Serialization
// =============================================================================

/**
 * Serialize audio features to wire format (snake_case).
 */
export function serializeAudioFeatures(features: AudioFeatures): AudioFeaturesWire {
  const wire: AudioFeaturesWire = {};

  if (features.turnDetection) {
    wire.turn_detection = {
      enabled: features.turnDetection.enabled,
    };

    if (features.turnDetection.threshold !== undefined) {
      wire.turn_detection.threshold = features.turnDetection.threshold;
    }
    if (features.turnDetection.silenceMs !== undefined) {
      wire.turn_detection.silence_ms = features.turnDetection.silenceMs;
    }
    if (features.turnDetection.prefixPaddingMs !== undefined) {
      wire.turn_detection.prefix_padding_ms = features.turnDetection.prefixPaddingMs;
    }
    if (features.turnDetection.createResponseMs !== undefined) {
      wire.turn_detection.create_response_ms = features.turnDetection.createResponseMs;
    }
  }

  if (features.noiseFiltering) {
    wire.noise_filtering = {
      enabled: features.noiseFiltering.enabled,
    };

    if (features.noiseFiltering.strength !== undefined) {
      wire.noise_filtering.strength = features.noiseFiltering.strength;
    }
    if (features.noiseFiltering.strengthValue !== undefined) {
      wire.noise_filtering.strength_value = features.noiseFiltering.strengthValue;
    }
  }

  if (features.vad) {
    wire.vad = {
      enabled: features.vad.enabled,
    };

    if (features.vad.threshold !== undefined) {
      wire.vad.threshold = features.vad.threshold;
    }
    if (features.vad.mode !== undefined) {
      wire.vad.mode = features.vad.mode;
    }
  }

  return wire;
}

/**
 * Deserialize audio features from wire format.
 */
export function deserializeAudioFeatures(wire: AudioFeaturesWire): AudioFeatures {
  const features: AudioFeatures = {};

  if (wire.turn_detection) {
    features.turnDetection = {
      enabled: wire.turn_detection.enabled,
      threshold: wire.turn_detection.threshold,
      silenceMs: wire.turn_detection.silence_ms,
      prefixPaddingMs: wire.turn_detection.prefix_padding_ms,
      createResponseMs: wire.turn_detection.create_response_ms,
    };
  }

  if (wire.noise_filtering) {
    features.noiseFiltering = {
      enabled: wire.noise_filtering.enabled,
      strength: wire.noise_filtering.strength,
      strengthValue: wire.noise_filtering.strength_value,
    };
  }

  if (wire.vad) {
    features.vad = {
      enabled: wire.vad.enabled,
      threshold: wire.vad.threshold,
      mode: wire.vad.mode,
    };
  }

  return features;
}
