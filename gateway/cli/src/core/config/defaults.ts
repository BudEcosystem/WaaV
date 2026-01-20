/**
 * WaaV CLI Configuration Defaults
 *
 * Default configuration values and example configurations
 */

import type { WaavConfig, ProviderConfig } from '../../types/config.js';

/**
 * Default provider configurations with common settings
 */
export const DEFAULT_PROVIDER_CONFIGS: Record<string, Partial<ProviderConfig>> = {
  // STT Providers
  deepgram: {
    model: 'nova-2',
    language: 'en-US',
    options: {
      punctuate: true,
      smart_format: true,
      interim_results: true,
    },
  },
  google: {
    language: 'en-US',
    options: {
      enable_automatic_punctuation: true,
      model: 'latest_long',
    },
  },
  azure: {
    language: 'en-US',
    region: 'eastus',
    options: {
      profanity_filter: false,
    },
  },
  openai: {
    model: 'whisper-1',
    language: 'en',
  },
  assemblyai: {
    options: {
      punctuate: true,
      format_text: true,
    },
  },
  groq: {
    model: 'whisper-large-v3',
    language: 'en',
  },

  // TTS Providers
  elevenlabs: {
    model: 'eleven_turbo_v2_5',
    voice_id: '21m00Tcm4TlvDq8ikWAM', // Rachel
    options: {
      stability: 0.5,
      similarity_boost: 0.75,
    },
  },
  cartesia: {
    model: 'sonic-english',
    voice_id: 'a0e99841-438c-4a64-b679-ae501e7d6091', // Barbershop Man
    options: {
      output_format: 'pcm_16000',
    },
  },
  openai_tts: {
    model: 'tts-1',
    voice_id: 'alloy',
    options: {
      speed: 1.0,
    },
  },
  play_ht: {
    voice_id: 's3://voice-cloning-zero-shot/775ae416-49bb-4fb6-bd45-740f205d20a1/sadfemalesad/manifest.json',
    options: {
      quality: 'premium',
    },
  },

  // Realtime Providers
  openai_realtime: {
    model: 'gpt-4o-realtime-preview',
    options: {
      modalities: ['text', 'audio'],
      turn_detection: {
        type: 'server_vad',
        threshold: 0.5,
      },
    },
  },
  hume: {
    options: {
      language_model_api_key: '${OPENAI_API_KEY}',
    },
  },
};

/**
 * Example configuration for different use cases
 */
export const EXAMPLE_CONFIGS = {
  /**
   * Minimal configuration for quick start
   */
  minimal: {
    version: 1,
    profile: 'default',
    gateway: {
      url: 'http://localhost:3001',
    },
    defaults: {
      stt_provider: 'deepgram',
      tts_provider: 'elevenlabs',
    },
    providers: {
      deepgram: {
        api_key: '${DEEPGRAM_API_KEY}',
      },
      elevenlabs: {
        api_key: '${ELEVENLABS_API_KEY}',
      },
    },
  } as unknown as Partial<WaavConfig>,

  /**
   * Full configuration with all options
   */
  full: {
    version: 1,
    profile: 'default',

    gateway: {
      url: 'http://localhost:3001',
      auth: {
        api_key: '${WAAV_API_KEY}',
      },
      timeout: 30000,
      retry: {
        attempts: 3,
        delay: 1000,
        backoff_multiplier: 2,
      },
    },

    defaults: {
      stt_provider: 'deepgram',
      tts_provider: 'elevenlabs',
      realtime_provider: 'openai_realtime',
    },

    providers: {
      deepgram: {
        api_key: '${DEEPGRAM_API_KEY}',
        model: 'nova-2',
        language: 'en-US',
      },
      elevenlabs: {
        api_key: '${ELEVENLABS_API_KEY}',
        voice_id: '21m00Tcm4TlvDq8ikWAM',
        model: 'eleven_turbo_v2_5',
      },
      openai: {
        api_key: '${OPENAI_API_KEY}',
        model: 'whisper-1',
      },
      openai_realtime: {
        api_key: '${OPENAI_API_KEY}',
        model: 'gpt-4o-realtime-preview',
      },
    },

    audio: {
      input_device: 'default',
      output_device: 'default',
      sample_rate: 16000,
      channels: 1,
      bit_depth: 16,
      vad: {
        enabled: true,
        threshold: 0.5,
        speech_pad_ms: 300,
        silence_duration_ms: 1000,
      },
    },

    voice: {
      mode: 'vad',
      push_to_talk_key: 'space',
      auto_play_response: true,
      show_transcript: true,
      beep_on_listen: false,
    },

    ui: {
      theme: 'dark',
      animations: true,
      sounds: false,
      show_tips: true,
      verbose: false,
    },

    pipelines: {},

    profiles: {
      production: {
        gateway: {
          url: 'https://api.waav.io',
          auth: {
            api_key: '${WAAV_PROD_API_KEY}',
          },
        },
      },
      development: {
        gateway: {
          url: 'http://localhost:3001',
        },
        defaults: {
          stt_provider: 'openai',
          tts_provider: 'openai_tts',
        },
      },
    },
  } as unknown as WaavConfig,

  /**
   * Configuration for voice assistant use case
   */
  voiceAssistant: {
    version: 1,
    profile: 'default',
    gateway: {
      url: 'http://localhost:3001',
    },
    defaults: {
      stt_provider: 'deepgram',
      tts_provider: 'cartesia',
      realtime_provider: 'openai_realtime',
    },
    providers: {
      deepgram: {
        api_key: '${DEEPGRAM_API_KEY}',
        model: 'nova-2',
        options: {
          smart_format: true,
          interim_results: true,
        },
      },
      cartesia: {
        api_key: '${CARTESIA_API_KEY}',
        voice_id: 'a0e99841-438c-4a64-b679-ae501e7d6091',
        model: 'sonic-english',
      },
      openai_realtime: {
        api_key: '${OPENAI_API_KEY}',
        model: 'gpt-4o-realtime-preview',
        options: {
          voice: 'alloy',
          instructions: 'You are a helpful voice assistant.',
        },
      },
    },
    audio: {
      sample_rate: 24000,
      vad: {
        enabled: true,
        threshold: 0.4,
        silence_duration_ms: 800,
      },
    },
    voice: {
      mode: 'vad',
      auto_play_response: true,
    },
  } as unknown as Partial<WaavConfig>,

  /**
   * Configuration for transcription use case
   */
  transcription: {
    version: 1,
    profile: 'default',
    gateway: {
      url: 'http://localhost:3001',
    },
    defaults: {
      stt_provider: 'assemblyai',
    },
    providers: {
      assemblyai: {
        api_key: '${ASSEMBLYAI_API_KEY}',
        options: {
          speaker_labels: true,
          punctuate: true,
          format_text: true,
          disfluencies: false,
        },
      },
    },
    audio: {
      sample_rate: 16000,
      vad: {
        enabled: false,
      },
    },
  } as unknown as Partial<WaavConfig>,
};

/**
 * Environment variable mappings for providers
 */
export const PROVIDER_ENV_VARS: Record<string, { api_key?: string; additional?: Record<string, string> }> = {
  deepgram: { api_key: 'DEEPGRAM_API_KEY' },
  elevenlabs: { api_key: 'ELEVENLABS_API_KEY' },
  google: {
    api_key: 'GOOGLE_API_KEY',
    additional: { credentials: 'GOOGLE_APPLICATION_CREDENTIALS' },
  },
  azure: {
    api_key: 'AZURE_SPEECH_KEY',
    additional: { region: 'AZURE_SPEECH_REGION' },
  },
  openai: { api_key: 'OPENAI_API_KEY' },
  openai_realtime: { api_key: 'OPENAI_API_KEY' },
  openai_tts: { api_key: 'OPENAI_API_KEY' },
  assemblyai: { api_key: 'ASSEMBLYAI_API_KEY' },
  cartesia: { api_key: 'CARTESIA_API_KEY' },
  play_ht: {
    api_key: 'PLAYHT_API_KEY',
    additional: { user_id: 'PLAYHT_USER_ID' },
  },
  hume: { api_key: 'HUME_API_KEY' },
  groq: { api_key: 'GROQ_API_KEY' },
  lmnt: { api_key: 'LMNT_API_KEY' },
  resemble: { api_key: 'RESEMBLE_API_KEY' },
  aws_transcribe: {
    additional: {
      access_key_id: 'AWS_ACCESS_KEY_ID',
      secret_access_key: 'AWS_SECRET_ACCESS_KEY',
      region: 'AWS_REGION',
    },
  },
  aws_polly: {
    additional: {
      access_key_id: 'AWS_ACCESS_KEY_ID',
      secret_access_key: 'AWS_SECRET_ACCESS_KEY',
      region: 'AWS_REGION',
    },
  },
};

/**
 * Get provider environment variable name
 */
export function getProviderEnvVar(providerId: string, key: 'api_key' | string = 'api_key'): string | undefined {
  const provider = PROVIDER_ENV_VARS[providerId];
  if (!provider) {
    return undefined;
  }

  if (key === 'api_key') {
    return provider.api_key;
  }

  return provider.additional?.[key];
}

/**
 * Get all environment variables for a provider
 */
export function getProviderEnvVars(providerId: string): Record<string, string | undefined> {
  const provider = PROVIDER_ENV_VARS[providerId];
  if (!provider) {
    return {};
  }

  const vars: Record<string, string | undefined> = {};

  if (provider.api_key) {
    vars.api_key = provider.api_key;
  }

  if (provider.additional) {
    for (const [key, envVar] of Object.entries(provider.additional)) {
      vars[key] = envVar;
    }
  }

  return vars;
}
