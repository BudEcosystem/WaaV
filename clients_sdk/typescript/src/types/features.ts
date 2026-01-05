/**
 * Feature Flags for Audio Processing
 */

/**
 * Complete feature flags interface
 */
export interface FeatureFlags {
  /** Voice Activity Detection - detects when user is speaking */
  vad?: boolean;
  /** Noise cancellation using DeepFilterNet on gateway */
  noiseCancellation?: boolean;
  /** Speaker diarization - identify different speakers (Deepgram) */
  speakerDiarization?: boolean;
  /** Interim/partial STT results */
  interimResults?: boolean;
  /** Auto-punctuation in transcriptions */
  punctuation?: boolean;
  /** Profanity filter (Deepgram, Azure) */
  profanityFilter?: boolean;
  /** Smart formatting for numbers, dates, etc. (Deepgram) */
  smartFormat?: boolean;
  /** Word-level timestamps */
  wordTimestamps?: boolean;
  /** Browser echo cancellation */
  echoCancellation?: boolean;
  /** Include filler words (um, uh) in transcription (Deepgram) */
  fillerWords?: boolean;
}

/**
 * Feature flag with provider support information
 */
export interface FeatureFlagInfo {
  /** Feature key name */
  key: keyof FeatureFlags;
  /** Human-readable name */
  name: string;
  /** Description */
  description: string;
  /** Default value */
  defaultValue: boolean;
  /** Providers that support this feature */
  supportedProviders: string[];
}

/**
 * Complete feature flag registry
 */
export const FEATURE_FLAGS: FeatureFlagInfo[] = [
  {
    key: 'vad',
    name: 'Voice Activity Detection',
    description: 'Detects when the user is speaking vs silence',
    defaultValue: true,
    supportedProviders: ['deepgram', 'google', 'elevenlabs', 'azure', 'cartesia', 'gateway'],
  },
  {
    key: 'noiseCancellation',
    name: 'Noise Cancellation',
    description: 'Reduces background noise using DeepFilterNet',
    defaultValue: false,
    supportedProviders: ['gateway'],
  },
  {
    key: 'speakerDiarization',
    name: 'Speaker Diarization',
    description: 'Identifies and labels different speakers',
    defaultValue: false,
    supportedProviders: ['deepgram'],
  },
  {
    key: 'interimResults',
    name: 'Interim Results',
    description: 'Receive partial transcription results as user speaks',
    defaultValue: true,
    supportedProviders: ['deepgram', 'google', 'elevenlabs', 'azure', 'cartesia'],
  },
  {
    key: 'punctuation',
    name: 'Auto-Punctuation',
    description: 'Automatically add punctuation to transcriptions',
    defaultValue: true,
    supportedProviders: ['deepgram', 'google', 'elevenlabs', 'azure', 'cartesia'],
  },
  {
    key: 'profanityFilter',
    name: 'Profanity Filter',
    description: 'Filter or mask profane words in transcriptions',
    defaultValue: false,
    supportedProviders: ['deepgram', 'azure'],
  },
  {
    key: 'smartFormat',
    name: 'Smart Formatting',
    description: 'Format numbers, dates, and other entities',
    defaultValue: true,
    supportedProviders: ['deepgram'],
  },
  {
    key: 'wordTimestamps',
    name: 'Word Timestamps',
    description: 'Get timing information for each word',
    defaultValue: false,
    supportedProviders: ['deepgram', 'google', 'azure', 'cartesia'],
  },
  {
    key: 'echoCancellation',
    name: 'Echo Cancellation',
    description: 'Browser-based acoustic echo cancellation',
    defaultValue: true,
    supportedProviders: ['browser'],
  },
  {
    key: 'fillerWords',
    name: 'Filler Words',
    description: 'Include filler words (um, uh, like) in transcription',
    defaultValue: false,
    supportedProviders: ['deepgram'],
  },
];

/**
 * Default feature flags configuration
 */
export const DEFAULT_FEATURE_FLAGS: FeatureFlags = {
  vad: true,
  noiseCancellation: false,
  speakerDiarization: false,
  interimResults: true,
  punctuation: true,
  profanityFilter: false,
  smartFormat: true,
  wordTimestamps: false,
  echoCancellation: true,
  fillerWords: false,
};

/**
 * Check if a provider supports a feature
 */
export function isFeatureSupported(feature: keyof FeatureFlags, provider: string): boolean {
  const info = FEATURE_FLAGS.find((f) => f.key === feature);
  if (!info) return false;
  return info.supportedProviders.includes(provider) || info.supportedProviders.includes('gateway');
}

/**
 * Get supported features for a provider
 */
export function getSupportedFeatures(provider: string): (keyof FeatureFlags)[] {
  return FEATURE_FLAGS.filter(
    (f) => f.supportedProviders.includes(provider) || f.supportedProviders.includes('gateway')
  ).map((f) => f.key);
}

/**
 * Merge user features with defaults, filtering unsupported features
 */
export function mergeFeatures(
  userFeatures: Partial<FeatureFlags>,
  provider: string
): FeatureFlags {
  const merged = { ...DEFAULT_FEATURE_FLAGS };

  for (const [key, value] of Object.entries(userFeatures)) {
    if (value !== undefined && isFeatureSupported(key as keyof FeatureFlags, provider)) {
      (merged as Record<string, boolean>)[key] = value;
    }
  }

  return merged;
}
