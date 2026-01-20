/**
 * WaaV CLI Provider Configuration State Machine
 *
 * Manages the provider configuration flow
 */

import { createMachine, assign } from 'xstate';

/**
 * Provider type
 */
export type ProviderType = 'stt' | 'tts' | 'realtime';

/**
 * Provider info
 */
export interface ProviderInfo {
  id: string;
  name: string;
  type: ProviderType;
  description: string;
  features: string[];
  requiresApiKey: boolean;
  envVar?: string;
  models?: string[];
  languages?: string[];
  configured: boolean;
}

/**
 * Provider machine context
 */
export interface ProviderMachineContext {
  // Provider selection
  providerType: ProviderType | null;
  selectedProvider: ProviderInfo | null;
  availableProviders: ProviderInfo[];

  // Configuration
  apiKey: string;
  model: string;
  language: string;
  voiceId: string;
  additionalOptions: Record<string, unknown>;

  // Validation
  isValidating: boolean;
  isValid: boolean;
  validationMessage: string | null;

  // Test
  isTesting: boolean;
  testResult: 'success' | 'failure' | null;
  testMessage: string | null;

  // Error handling
  error: string | null;

  // Save state
  isSaving: boolean;
  saved: boolean;
}

/**
 * Provider machine events
 */
export type ProviderMachineEvent =
  | { type: 'SELECT_TYPE'; providerType: ProviderType }
  | { type: 'SELECT_PROVIDER'; provider: ProviderInfo }
  | { type: 'SET_API_KEY'; apiKey: string }
  | { type: 'SET_MODEL'; model: string }
  | { type: 'SET_LANGUAGE'; language: string }
  | { type: 'SET_VOICE_ID'; voiceId: string }
  | { type: 'SET_OPTION'; key: string; value: unknown }
  | { type: 'VALIDATE' }
  | { type: 'VALIDATION_SUCCESS' }
  | { type: 'VALIDATION_FAILED'; message: string }
  | { type: 'TEST' }
  | { type: 'TEST_SUCCESS'; message: string }
  | { type: 'TEST_FAILED'; message: string }
  | { type: 'SAVE' }
  | { type: 'SAVE_SUCCESS' }
  | { type: 'SAVE_FAILED'; error: string }
  | { type: 'RESET' }
  | { type: 'BACK' }
  | { type: 'CANCEL' }
  | { type: 'ERROR'; error: string }
  | { type: 'CLEAR_ERROR' };

/**
 * Initial context
 */
const initialContext: ProviderMachineContext = {
  providerType: null,
  selectedProvider: null,
  availableProviders: [],
  apiKey: '',
  model: '',
  language: 'en-US',
  voiceId: '',
  additionalOptions: {},
  isValidating: false,
  isValid: false,
  validationMessage: null,
  isTesting: false,
  testResult: null,
  testMessage: null,
  error: null,
  isSaving: false,
  saved: false,
};

/**
 * Provider configuration state machine
 *
 * States:
 * - selectType: Choose provider type (STT/TTS/Realtime)
 * - selectProvider: Choose specific provider
 * - configure: Configure provider settings
 * - validate: Validate configuration
 * - test: Test provider connection
 * - save: Save configuration
 * - complete: Configuration complete
 */
export const providerMachine = createMachine({
  id: 'provider',
  initial: 'selectType',
  context: initialContext,
  types: {} as {
    context: ProviderMachineContext;
    events: ProviderMachineEvent;
  },
  states: {
    selectType: {
      on: {
        SELECT_TYPE: {
          target: 'selectProvider',
          actions: assign({
            providerType: ({ event }) => event.providerType,
            error: null,
          }),
        },
        CANCEL: 'cancelled',
      },
    },

    selectProvider: {
      on: {
        SELECT_PROVIDER: {
          target: 'configure',
          actions: assign({
            selectedProvider: ({ event }) => event.provider,
            model: ({ event }) => event.provider.models?.[0] ?? '',
            error: null,
          }),
        },
        BACK: 'selectType',
        CANCEL: 'cancelled',
      },
    },

    configure: {
      initial: 'editing',
      states: {
        editing: {
          on: {
            SET_API_KEY: {
              actions: assign({
                apiKey: ({ event }) => event.apiKey,
                isValid: false,
              }),
            },
            SET_MODEL: {
              actions: assign({
                model: ({ event }) => event.model,
              }),
            },
            SET_LANGUAGE: {
              actions: assign({
                language: ({ event }) => event.language,
              }),
            },
            SET_VOICE_ID: {
              actions: assign({
                voiceId: ({ event }) => event.voiceId,
              }),
            },
            SET_OPTION: {
              actions: assign({
                additionalOptions: ({ context, event }) => ({
                  ...context.additionalOptions,
                  [event.key]: event.value,
                }),
              }),
            },
            VALIDATE: 'validating',
            TEST: {
              target: 'testing',
              guard: ({ context }) => context.isValid,
            },
            SAVE: {
              target: 'saving',
              guard: ({ context }) => context.isValid,
            },
          },
        },
        validating: {
          entry: assign({ isValidating: true }),
          exit: assign({ isValidating: false }),
          on: {
            VALIDATION_SUCCESS: {
              target: 'editing',
              actions: assign({
                isValid: true,
                validationMessage: null,
                error: null,
              }),
            },
            VALIDATION_FAILED: {
              target: 'editing',
              actions: assign({
                isValid: false,
                validationMessage: ({ event }) => event.message,
              }),
            },
          },
        },
        testing: {
          entry: assign({ isTesting: true }),
          exit: assign({ isTesting: false }),
          on: {
            TEST_SUCCESS: {
              target: 'editing',
              actions: assign({
                testResult: 'success',
                testMessage: ({ event }) => event.message,
                error: null,
              }),
            },
            TEST_FAILED: {
              target: 'editing',
              actions: assign({
                testResult: 'failure',
                testMessage: ({ event }) => event.message,
              }),
            },
          },
        },
        saving: {
          entry: assign({ isSaving: true }),
          exit: assign({ isSaving: false }),
          on: {
            SAVE_SUCCESS: {
              target: '#provider.complete',
              actions: assign({
                saved: true,
                error: null,
              }),
            },
            SAVE_FAILED: {
              target: 'editing',
              actions: assign({
                saved: false,
                error: ({ event }) => event.error,
              }),
            },
          },
        },
      },
      on: {
        BACK: 'selectProvider',
        CANCEL: 'cancelled',
        ERROR: {
          actions: assign({
            error: ({ event }) => event.error,
          }),
        },
        CLEAR_ERROR: {
          actions: assign({ error: null }),
        },
      },
    },

    complete: {
      type: 'final',
      entry: assign({ saved: true }),
    },

    cancelled: {
      type: 'final',
    },
  },
});

/**
 * Common STT providers
 */
export const STT_PROVIDERS: ProviderInfo[] = [
  {
    id: 'deepgram',
    name: 'Deepgram',
    type: 'stt',
    description: 'Fast, accurate speech recognition with Nova-2',
    features: ['Real-time', 'Multiple languages', 'Speaker diarization'],
    requiresApiKey: true,
    envVar: 'DEEPGRAM_API_KEY',
    models: ['nova-2', 'nova', 'enhanced', 'base'],
    languages: ['en-US', 'en-GB', 'es', 'fr', 'de', 'it', 'pt', 'ja', 'ko', 'zh'],
    configured: false,
  },
  {
    id: 'openai',
    name: 'OpenAI Whisper',
    type: 'stt',
    description: 'OpenAI Whisper API for transcription',
    features: ['High accuracy', 'Multiple languages', 'Translation'],
    requiresApiKey: true,
    envVar: 'OPENAI_API_KEY',
    models: ['whisper-1'],
    languages: ['en', 'es', 'fr', 'de', 'it', 'pt', 'ja', 'ko', 'zh'],
    configured: false,
  },
  {
    id: 'google',
    name: 'Google Cloud Speech',
    type: 'stt',
    description: 'Google Cloud Speech-to-Text',
    features: ['125+ languages', 'Chirp models', 'Speaker diarization'],
    requiresApiKey: true,
    envVar: 'GOOGLE_API_KEY',
    models: ['latest_long', 'latest_short', 'chirp'],
    languages: ['en-US', 'en-GB', 'es-ES', 'fr-FR', 'de-DE'],
    configured: false,
  },
  {
    id: 'azure',
    name: 'Azure Speech',
    type: 'stt',
    description: 'Microsoft Azure Speech Services',
    features: ['Custom models', 'Real-time', 'Batch transcription'],
    requiresApiKey: true,
    envVar: 'AZURE_SPEECH_KEY',
    languages: ['en-US', 'en-GB', 'es-ES', 'fr-FR', 'de-DE'],
    configured: false,
  },
  {
    id: 'assemblyai',
    name: 'AssemblyAI',
    type: 'stt',
    description: 'AI-powered speech recognition',
    features: ['Speaker labels', 'Content moderation', 'PII redaction'],
    requiresApiKey: true,
    envVar: 'ASSEMBLYAI_API_KEY',
    configured: false,
  },
];

/**
 * Common TTS providers
 */
export const TTS_PROVIDERS: ProviderInfo[] = [
  {
    id: 'elevenlabs',
    name: 'ElevenLabs',
    type: 'tts',
    description: 'Ultra-realistic voice synthesis',
    features: ['Voice cloning', 'Emotional control', 'Multiple voices'],
    requiresApiKey: true,
    envVar: 'ELEVENLABS_API_KEY',
    models: ['eleven_turbo_v2_5', 'eleven_multilingual_v2', 'eleven_monolingual_v1'],
    configured: false,
  },
  {
    id: 'cartesia',
    name: 'Cartesia',
    type: 'tts',
    description: 'Low-latency voice synthesis',
    features: ['Ultra-low latency', 'Streaming', 'Natural voices'],
    requiresApiKey: true,
    envVar: 'CARTESIA_API_KEY',
    models: ['sonic-english', 'sonic-multilingual'],
    configured: false,
  },
  {
    id: 'openai_tts',
    name: 'OpenAI TTS',
    type: 'tts',
    description: 'OpenAI text-to-speech',
    features: ['Multiple voices', 'HD quality', 'Streaming'],
    requiresApiKey: true,
    envVar: 'OPENAI_API_KEY',
    models: ['tts-1', 'tts-1-hd'],
    configured: false,
  },
  {
    id: 'play_ht',
    name: 'PlayHT',
    type: 'tts',
    description: 'AI voice generation',
    features: ['Voice cloning', 'Multiple languages', 'Emotions'],
    requiresApiKey: true,
    envVar: 'PLAYHT_API_KEY',
    configured: false,
  },
  {
    id: 'google_tts',
    name: 'Google Cloud TTS',
    type: 'tts',
    description: 'Google Cloud Text-to-Speech',
    features: ['WaveNet voices', 'Multiple languages', 'SSML'],
    requiresApiKey: true,
    envVar: 'GOOGLE_API_KEY',
    configured: false,
  },
];

/**
 * Common Realtime providers
 */
export const REALTIME_PROVIDERS: ProviderInfo[] = [
  {
    id: 'openai_realtime',
    name: 'OpenAI Realtime',
    type: 'realtime',
    description: 'OpenAI GPT-4o Realtime API',
    features: ['Voice-to-voice', 'Function calling', 'Low latency'],
    requiresApiKey: true,
    envVar: 'OPENAI_API_KEY',
    models: ['gpt-4o-realtime-preview'],
    configured: false,
  },
  {
    id: 'hume',
    name: 'Hume AI',
    type: 'realtime',
    description: 'Emotionally intelligent voice AI',
    features: ['Emotion detection', 'Empathic responses', 'Voice conversation'],
    requiresApiKey: true,
    envVar: 'HUME_API_KEY',
    configured: false,
  },
];

/**
 * Get providers by type
 */
export function getProvidersByType(type: ProviderType): ProviderInfo[] {
  switch (type) {
    case 'stt':
      return STT_PROVIDERS;
    case 'tts':
      return TTS_PROVIDERS;
    case 'realtime':
      return REALTIME_PROVIDERS;
  }
}

/**
 * Get provider by ID
 */
export function getProviderById(id: string): ProviderInfo | undefined {
  return [...STT_PROVIDERS, ...TTS_PROVIDERS, ...REALTIME_PROVIDERS].find(p => p.id === id);
}

/**
 * Get provider type label
 */
export function getProviderTypeLabel(type: ProviderType): string {
  switch (type) {
    case 'stt':
      return 'Speech-to-Text';
    case 'tts':
      return 'Text-to-Speech';
    case 'realtime':
      return 'Realtime Voice';
  }
}

/**
 * Validate API key format
 */
export function validateApiKeyFormat(providerId: string, apiKey: string): { valid: boolean; message?: string } {
  if (!apiKey || apiKey.trim() === '') {
    return { valid: false, message: 'API key is required' };
  }

  // Provider-specific validation
  switch (providerId) {
    case 'openai':
    case 'openai_tts':
    case 'openai_realtime':
      if (!apiKey.startsWith('sk-')) {
        return { valid: false, message: 'OpenAI API key should start with "sk-"' };
      }
      break;
    case 'anthropic':
      if (!apiKey.startsWith('sk-ant-')) {
        return { valid: false, message: 'Anthropic API key should start with "sk-ant-"' };
      }
      break;
  }

  return { valid: true };
}

export default providerMachine;
