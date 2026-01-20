/**
 * WaaV CLI Setup Wizard State Machine
 *
 * Manages the first-time setup wizard flow
 */

import { createMachine, assign } from 'xstate';

/**
 * Setup wizard context
 */
export interface SetupMachineContext {
  // Gateway configuration
  gatewayUrl: string;
  gatewayConnected: boolean;

  // Provider selection
  sttProvider: string;
  ttsProvider: string;
  sttApiKey: string;
  ttsApiKey: string;

  // Provider validation
  sttValidated: boolean;
  ttsValidated: boolean;

  // Audio configuration
  audioInputDevice: string;
  audioOutputDevice: string;
  audioDevicesTested: boolean;

  // Progress tracking
  currentStep: number;
  totalSteps: number;

  // Error handling
  error: string | null;
  validationErrors: Record<string, string>;

  // Completion
  configSaved: boolean;
}

/**
 * Setup wizard events
 */
export type SetupMachineEvent =
  | { type: 'START' }
  | { type: 'NEXT' }
  | { type: 'BACK' }
  | { type: 'SKIP' }
  | { type: 'SET_GATEWAY_URL'; url: string }
  | { type: 'TEST_GATEWAY' }
  | { type: 'GATEWAY_CONNECTED' }
  | { type: 'GATEWAY_FAILED'; error: string }
  | { type: 'SELECT_STT_PROVIDER'; provider: string }
  | { type: 'SET_STT_API_KEY'; apiKey: string }
  | { type: 'VALIDATE_STT' }
  | { type: 'STT_VALIDATED' }
  | { type: 'STT_VALIDATION_FAILED'; error: string }
  | { type: 'SELECT_TTS_PROVIDER'; provider: string }
  | { type: 'SET_TTS_API_KEY'; apiKey: string }
  | { type: 'VALIDATE_TTS' }
  | { type: 'TTS_VALIDATED' }
  | { type: 'TTS_VALIDATION_FAILED'; error: string }
  | { type: 'SELECT_AUDIO_INPUT'; device: string }
  | { type: 'SELECT_AUDIO_OUTPUT'; device: string }
  | { type: 'TEST_AUDIO' }
  | { type: 'AUDIO_TEST_PASSED' }
  | { type: 'AUDIO_TEST_FAILED'; error: string }
  | { type: 'SAVE_CONFIG' }
  | { type: 'CONFIG_SAVED' }
  | { type: 'CONFIG_SAVE_FAILED'; error: string }
  | { type: 'COMPLETE' }
  | { type: 'CANCEL' }
  | { type: 'ERROR'; error: string }
  | { type: 'CLEAR_ERROR' };

/**
 * Initial context
 */
const initialContext: SetupMachineContext = {
  gatewayUrl: 'http://localhost:3001',
  gatewayConnected: false,
  sttProvider: 'deepgram',
  ttsProvider: 'elevenlabs',
  sttApiKey: '',
  ttsApiKey: '',
  sttValidated: false,
  ttsValidated: false,
  audioInputDevice: 'default',
  audioOutputDevice: 'default',
  audioDevicesTested: false,
  currentStep: 0,
  totalSteps: 6,
  error: null,
  validationErrors: {},
  configSaved: false,
};

/**
 * Setup wizard state machine
 *
 * Steps:
 * 1. welcome: Introduction screen
 * 2. gateway: Configure gateway URL
 * 3. sttProvider: Select and configure STT provider
 * 4. ttsProvider: Select and configure TTS provider
 * 5. audio: Configure audio devices
 * 6. summary: Review and save configuration
 * 7. complete: Setup complete
 */
export const setupMachine = createMachine({
  id: 'setup',
  initial: 'welcome',
  context: initialContext,
  types: {} as {
    context: SetupMachineContext;
    events: SetupMachineEvent;
  },
  states: {
    welcome: {
      entry: assign({ currentStep: 1 }),
      on: {
        START: 'gateway',
        CANCEL: 'cancelled',
      },
    },

    gateway: {
      initial: 'input',
      entry: assign({ currentStep: 2 }),
      states: {
        input: {
          on: {
            SET_GATEWAY_URL: {
              actions: assign({
                gatewayUrl: ({ event }) => event.url,
              }),
            },
            TEST_GATEWAY: 'testing',
            SKIP: {
              target: '#setup.sttProvider',
              actions: assign({ gatewayConnected: false }),
            },
          },
        },
        testing: {
          on: {
            GATEWAY_CONNECTED: {
              target: '#setup.sttProvider',
              actions: assign({ gatewayConnected: true, error: null }),
            },
            GATEWAY_FAILED: {
              target: 'input',
              actions: assign({
                gatewayConnected: false,
                error: ({ event }) => event.error,
              }),
            },
          },
        },
      },
      on: {
        BACK: 'welcome',
        CANCEL: 'cancelled',
      },
    },

    sttProvider: {
      initial: 'select',
      entry: assign({ currentStep: 3 }),
      states: {
        select: {
          on: {
            SELECT_STT_PROVIDER: {
              target: 'configure',
              actions: assign({
                sttProvider: ({ event }) => event.provider,
              }),
            },
          },
        },
        configure: {
          on: {
            SET_STT_API_KEY: {
              actions: assign({
                sttApiKey: ({ event }) => event.apiKey,
              }),
            },
            VALIDATE_STT: 'validating',
            SKIP: {
              target: '#setup.ttsProvider',
              actions: assign({ sttValidated: false }),
            },
          },
        },
        validating: {
          on: {
            STT_VALIDATED: {
              target: '#setup.ttsProvider',
              actions: assign({ sttValidated: true, error: null }),
            },
            STT_VALIDATION_FAILED: {
              target: 'configure',
              actions: assign({
                sttValidated: false,
                error: ({ event }) => event.error,
              }),
            },
          },
        },
      },
      on: {
        BACK: 'gateway',
        CANCEL: 'cancelled',
      },
    },

    ttsProvider: {
      initial: 'select',
      entry: assign({ currentStep: 4 }),
      states: {
        select: {
          on: {
            SELECT_TTS_PROVIDER: {
              target: 'configure',
              actions: assign({
                ttsProvider: ({ event }) => event.provider,
              }),
            },
          },
        },
        configure: {
          on: {
            SET_TTS_API_KEY: {
              actions: assign({
                ttsApiKey: ({ event }) => event.apiKey,
              }),
            },
            VALIDATE_TTS: 'validating',
            SKIP: {
              target: '#setup.audio',
              actions: assign({ ttsValidated: false }),
            },
          },
        },
        validating: {
          on: {
            TTS_VALIDATED: {
              target: '#setup.audio',
              actions: assign({ ttsValidated: true, error: null }),
            },
            TTS_VALIDATION_FAILED: {
              target: 'configure',
              actions: assign({
                ttsValidated: false,
                error: ({ event }) => event.error,
              }),
            },
          },
        },
      },
      on: {
        BACK: 'sttProvider',
        CANCEL: 'cancelled',
      },
    },

    audio: {
      initial: 'configure',
      entry: assign({ currentStep: 5 }),
      states: {
        configure: {
          on: {
            SELECT_AUDIO_INPUT: {
              actions: assign({
                audioInputDevice: ({ event }) => event.device,
              }),
            },
            SELECT_AUDIO_OUTPUT: {
              actions: assign({
                audioOutputDevice: ({ event }) => event.device,
              }),
            },
            TEST_AUDIO: 'testing',
            SKIP: {
              target: '#setup.summary',
              actions: assign({ audioDevicesTested: false }),
            },
          },
        },
        testing: {
          on: {
            AUDIO_TEST_PASSED: {
              target: '#setup.summary',
              actions: assign({ audioDevicesTested: true, error: null }),
            },
            AUDIO_TEST_FAILED: {
              target: 'configure',
              actions: assign({
                audioDevicesTested: false,
                error: ({ event }) => event.error,
              }),
            },
          },
        },
      },
      on: {
        BACK: 'ttsProvider',
        CANCEL: 'cancelled',
      },
    },

    summary: {
      initial: 'review',
      entry: assign({ currentStep: 6 }),
      states: {
        review: {
          on: {
            SAVE_CONFIG: 'saving',
          },
        },
        saving: {
          on: {
            CONFIG_SAVED: {
              target: '#setup.complete',
              actions: assign({ configSaved: true, error: null }),
            },
            CONFIG_SAVE_FAILED: {
              target: 'review',
              actions: assign({
                configSaved: false,
                error: ({ event }) => event.error,
              }),
            },
          },
        },
      },
      on: {
        BACK: 'audio',
        CANCEL: 'cancelled',
      },
    },

    complete: {
      type: 'final',
      entry: assign({ configSaved: true }),
    },

    cancelled: {
      type: 'final',
    },
  },
});

/**
 * Setup step information
 */
export interface SetupStep {
  id: string;
  title: string;
  description: string;
  optional: boolean;
}

export const SETUP_STEPS: SetupStep[] = [
  {
    id: 'welcome',
    title: 'Welcome',
    description: 'Introduction to WaaV CLI',
    optional: false,
  },
  {
    id: 'gateway',
    title: 'Gateway',
    description: 'Configure the gateway server URL',
    optional: false,
  },
  {
    id: 'sttProvider',
    title: 'STT Provider',
    description: 'Select and configure Speech-to-Text provider',
    optional: true,
  },
  {
    id: 'ttsProvider',
    title: 'TTS Provider',
    description: 'Select and configure Text-to-Speech provider',
    optional: true,
  },
  {
    id: 'audio',
    title: 'Audio',
    description: 'Configure audio input and output devices',
    optional: true,
  },
  {
    id: 'summary',
    title: 'Summary',
    description: 'Review and save configuration',
    optional: false,
  },
];

/**
 * Get current step info
 */
export function getCurrentStep(stepNumber: number): SetupStep | undefined {
  return SETUP_STEPS[stepNumber - 1];
}

/**
 * Get progress percentage
 */
export function getSetupProgress(context: SetupMachineContext): number {
  return Math.round((context.currentStep / context.totalSteps) * 100);
}

/**
 * Check if setup is complete
 */
export function isSetupComplete(context: SetupMachineContext): boolean {
  return context.configSaved;
}

/**
 * Get setup summary
 */
export function getSetupSummary(context: SetupMachineContext): Record<string, string> {
  return {
    'Gateway URL': context.gatewayUrl,
    'Gateway Status': context.gatewayConnected ? 'Connected' : 'Not tested',
    'STT Provider': context.sttProvider,
    'STT Status': context.sttValidated ? 'Validated' : 'Not validated',
    'TTS Provider': context.ttsProvider,
    'TTS Status': context.ttsValidated ? 'Validated' : 'Not validated',
    'Audio Input': context.audioInputDevice,
    'Audio Output': context.audioOutputDevice,
    'Audio Status': context.audioDevicesTested ? 'Tested' : 'Not tested',
  };
}

export default setupMachine;
