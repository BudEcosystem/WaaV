/**
 * WaaV CLI Voice Mode State Machine
 *
 * Manages the complex state transitions for voice conversation mode
 */

import { createMachine, assign } from 'xstate';

/**
 * Voice mode context
 */
export interface VoiceMachineContext {
  // Connection state
  gatewayUrl: string;
  streamId: string | null;
  connectionAttempts: number;
  maxConnectionAttempts: number;

  // Audio state
  audioLevel: number;
  isMuted: boolean;
  vadEnabled: boolean;

  // Transcript
  currentTranscript: string;
  transcriptHistory: Array<{
    speaker: 'user' | 'assistant';
    text: string;
    timestamp: number;
    confidence?: number;
  }>;

  // Error handling
  error: string | null;
  lastError: string | null;

  // Statistics
  totalAudioDuration: number;
  totalUtterances: number;
}

/**
 * Voice mode events
 */
export type VoiceMachineEvent =
  | { type: 'CONNECT'; gatewayUrl: string }
  | { type: 'CONNECTED'; streamId: string }
  | { type: 'CONNECTION_FAILED'; error: string }
  | { type: 'DISCONNECT' }
  | { type: 'DISCONNECTED' }
  | { type: 'START_LISTENING' }
  | { type: 'STOP_LISTENING' }
  | { type: 'AUDIO_DATA'; level: number }
  | { type: 'VAD_SPEECH_START' }
  | { type: 'VAD_SPEECH_END' }
  | { type: 'TRANSCRIPT_PARTIAL'; text: string }
  | { type: 'TRANSCRIPT_FINAL'; text: string; confidence: number }
  | { type: 'TTS_START' }
  | { type: 'TTS_AUDIO'; data: Uint8Array }
  | { type: 'TTS_END' }
  | { type: 'INTERRUPT' }
  | { type: 'MUTE' }
  | { type: 'UNMUTE' }
  | { type: 'TOGGLE_VAD' }
  | { type: 'ERROR'; error: string }
  | { type: 'RETRY' }
  | { type: 'CLEAR_ERROR' };

/**
 * Initial context
 */
const initialContext: VoiceMachineContext = {
  gatewayUrl: '',
  streamId: null,
  connectionAttempts: 0,
  maxConnectionAttempts: 3,
  audioLevel: 0,
  isMuted: false,
  vadEnabled: true,
  currentTranscript: '',
  transcriptHistory: [],
  error: null,
  lastError: null,
  totalAudioDuration: 0,
  totalUtterances: 0,
};

/**
 * Voice mode state machine
 *
 * States:
 * - idle: Initial state, not connected
 * - connecting: Attempting to connect to gateway
 * - ready: Connected, waiting for user to start
 * - listening: Actively capturing audio
 * - processing: Processing user speech (STT)
 * - speaking: TTS audio playing
 * - error: Error state with retry option
 * - disconnected: Clean disconnection
 */
export const voiceMachine = createMachine({
  id: 'voice',
  initial: 'idle',
  context: initialContext,
  types: {} as {
    context: VoiceMachineContext;
    events: VoiceMachineEvent;
  },
  states: {
    idle: {
      on: {
        CONNECT: {
          target: 'connecting',
          actions: assign({
            gatewayUrl: ({ event }) => event.gatewayUrl,
            connectionAttempts: 0,
            error: null,
          }),
        },
      },
    },

    connecting: {
      entry: assign({
        connectionAttempts: ({ context }) => context.connectionAttempts + 1,
      }),
      on: {
        CONNECTED: {
          target: 'ready',
          actions: assign({
            streamId: ({ event }) => event.streamId,
            connectionAttempts: 0,
          }),
        },
        CONNECTION_FAILED: [
          {
            target: 'connecting',
            guard: ({ context }) =>
              context.connectionAttempts < context.maxConnectionAttempts,
            actions: assign({
              lastError: ({ event }) => event.error,
            }),
          },
          {
            target: 'error',
            actions: assign({
              error: ({ event }) => event.error,
            }),
          },
        ],
        DISCONNECT: 'disconnected',
      },
    },

    ready: {
      on: {
        START_LISTENING: 'listening',
        DISCONNECT: 'disconnected',
        ERROR: {
          target: 'error',
          actions: assign({
            error: ({ event }) => event.error,
          }),
        },
      },
    },

    listening: {
      on: {
        AUDIO_DATA: {
          actions: assign({
            audioLevel: ({ event }) => event.level,
          }),
        },
        TRANSCRIPT_PARTIAL: {
          actions: assign({
            currentTranscript: ({ event }) => event.text,
          }),
        },
        TRANSCRIPT_FINAL: {
          target: 'processing',
          actions: assign({
            currentTranscript: '',
            transcriptHistory: ({ context, event }) => [
              ...context.transcriptHistory,
              {
                speaker: 'user' as const,
                text: event.text,
                timestamp: Date.now(),
                confidence: event.confidence,
              },
            ],
            totalUtterances: ({ context }) => context.totalUtterances + 1,
          }),
        },
        STOP_LISTENING: 'ready',
        VAD_SPEECH_END: 'processing',
        MUTE: {
          actions: assign({ isMuted: true }),
        },
        UNMUTE: {
          actions: assign({ isMuted: false }),
        },
        DISCONNECT: 'disconnected',
        ERROR: {
          target: 'error',
          actions: assign({
            error: ({ event }) => event.error,
          }),
        },
      },
    },

    processing: {
      on: {
        TTS_START: 'speaking',
        TRANSCRIPT_FINAL: {
          actions: assign({
            transcriptHistory: ({ context, event }) => [
              ...context.transcriptHistory,
              {
                speaker: 'user' as const,
                text: event.text,
                timestamp: Date.now(),
                confidence: event.confidence,
              },
            ],
          }),
        },
        START_LISTENING: 'listening',
        DISCONNECT: 'disconnected',
        ERROR: {
          target: 'error',
          actions: assign({
            error: ({ event }) => event.error,
          }),
        },
      },
    },

    speaking: {
      on: {
        TTS_AUDIO: {
          // Handle incoming audio chunks
        },
        TTS_END: {
          target: 'ready',
          actions: assign({
            transcriptHistory: ({ context }) => {
              // Mark that assistant finished speaking
              return context.transcriptHistory;
            },
          }),
        },
        INTERRUPT: {
          target: 'listening',
          // Cancel TTS playback
        },
        DISCONNECT: 'disconnected',
        ERROR: {
          target: 'error',
          actions: assign({
            error: ({ event }) => event.error,
          }),
        },
      },
    },

    error: {
      on: {
        RETRY: {
          target: 'connecting',
          actions: assign({
            error: null,
          }),
        },
        DISCONNECT: 'disconnected',
        CLEAR_ERROR: {
          target: 'idle',
          actions: assign({
            error: null,
          }),
        },
      },
    },

    disconnected: {
      type: 'final',
      entry: assign({
        streamId: null,
        audioLevel: 0,
        currentTranscript: '',
      }),
    },
  },
});

/**
 * Voice machine state names
 */
export type VoiceMachineState =
  | 'idle'
  | 'connecting'
  | 'ready'
  | 'listening'
  | 'processing'
  | 'speaking'
  | 'error'
  | 'disconnected';

/**
 * Check if voice machine is in an active state
 */
export function isVoiceActive(state: VoiceMachineState): boolean {
  return ['listening', 'processing', 'speaking'].includes(state);
}

/**
 * Check if voice machine can start listening
 */
export function canStartListening(state: VoiceMachineState): boolean {
  return state === 'ready';
}

/**
 * Get status color for UI
 */
export function getVoiceStatusColor(state: VoiceMachineState): string {
  switch (state) {
    case 'listening':
      return 'red';
    case 'speaking':
      return 'green';
    case 'processing':
      return 'yellow';
    case 'error':
      return 'red';
    case 'ready':
      return 'cyan';
    default:
      return 'gray';
  }
}

/**
 * Get status text for UI
 */
export function getVoiceStatusText(state: VoiceMachineState, context: VoiceMachineContext): string {
  switch (state) {
    case 'idle':
      return 'Not connected';
    case 'connecting':
      return `Connecting (attempt ${context.connectionAttempts}/${context.maxConnectionAttempts})...`;
    case 'ready':
      return 'Ready - Press Space to talk';
    case 'listening':
      return context.isMuted ? 'Muted' : 'Listening...';
    case 'processing':
      return 'Processing...';
    case 'speaking':
      return 'Speaking...';
    case 'error':
      return `Error: ${context.error}`;
    case 'disconnected':
      return 'Disconnected';
    default:
      return 'Unknown';
  }
}

export default voiceMachine;
