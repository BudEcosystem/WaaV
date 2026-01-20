/**
 * WaaV CLI State Store
 *
 * Global state management and machine actor utilities
 */

import { createActor, type AnyActorLogic, type SnapshotFrom, type EventFromLogic } from 'xstate';
import { voiceMachine, setupMachine, providerMachine } from './machines/index.js';

/**
 * Machine actor wrapper with utilities
 */
export interface MachineActor<TMachine extends AnyActorLogic> {
  actor: ReturnType<typeof createActor<TMachine>>;
  getState: () => SnapshotFrom<TMachine>;
  send: (event: EventFromLogic<TMachine>) => void;
  subscribe: (callback: (state: SnapshotFrom<TMachine>) => void) => () => void;
  start: () => void;
  stop: () => void;
}

/**
 * Create a machine actor with utilities
 */
export function createMachineActor<TMachine extends AnyActorLogic>(
  machine: TMachine
): MachineActor<TMachine> {
  const actor = createActor(machine);

  return {
    actor,
    getState: () => actor.getSnapshot(),
    send: (event) => actor.send(event),
    subscribe: (callback) => {
      const subscription = actor.subscribe((state) => callback(state));
      return () => subscription.unsubscribe();
    },
    start: () => actor.start(),
    stop: () => actor.stop(),
  };
}

/**
 * Voice machine actor
 */
export function createVoiceActor() {
  return createMachineActor(voiceMachine);
}

/**
 * Setup machine actor
 */
export function createSetupActor() {
  return createMachineActor(setupMachine);
}

/**
 * Provider machine actor
 */
export function createProviderActor() {
  return createMachineActor(providerMachine);
}

/**
 * Global store for CLI state
 */
export interface GlobalStore {
  // Voice session
  voiceActor: ReturnType<typeof createVoiceActor> | null;

  // Current session info
  sessionId: string | null;
  gatewayUrl: string;

  // Feature flags
  features: {
    animations: boolean;
    sounds: boolean;
    verbose: boolean;
  };
}

/**
 * Create global store
 */
export function createGlobalStore(options?: Partial<GlobalStore>): GlobalStore {
  return {
    voiceActor: null,
    sessionId: null,
    gatewayUrl: 'http://localhost:3001',
    features: {
      animations: true,
      sounds: false,
      verbose: false,
    },
    ...options,
  };
}

/**
 * Default global store instance
 */
let globalStore: GlobalStore | null = null;

/**
 * Get or create global store
 */
export function getGlobalStore(): GlobalStore {
  if (!globalStore) {
    globalStore = createGlobalStore();
  }
  return globalStore;
}

/**
 * Reset global store
 */
export function resetGlobalStore(): void {
  if (globalStore?.voiceActor) {
    globalStore.voiceActor.stop();
  }
  globalStore = null;
}

/**
 * Simple state container for non-machine state
 */
export class StateContainer<T> {
  private state: T;
  private listeners: Set<(state: T) => void> = new Set();

  constructor(initialState: T) {
    this.state = initialState;
  }

  getState(): T {
    return this.state;
  }

  setState(updater: T | ((prev: T) => T)): void {
    const newState = typeof updater === 'function'
      ? (updater as (prev: T) => T)(this.state)
      : updater;

    this.state = newState;
    this.notify();
  }

  subscribe(listener: (state: T) => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  private notify(): void {
    for (const listener of this.listeners) {
      listener(this.state);
    }
  }
}

/**
 * Create a state container
 */
export function createStateContainer<T>(initialState: T): StateContainer<T> {
  return new StateContainer(initialState);
}

/**
 * Session state container
 */
export interface SessionState {
  isConnected: boolean;
  streamId: string | null;
  gatewayVersion: string | null;
  connectedAt: number | null;
  transcriptCount: number;
  audioBytesSent: number;
  audioBytesReceived: number;
}

const initialSessionState: SessionState = {
  isConnected: false,
  streamId: null,
  gatewayVersion: null,
  connectedAt: null,
  transcriptCount: 0,
  audioBytesSent: 0,
  audioBytesReceived: 0,
};

/**
 * Create session state container
 */
export function createSessionState(): StateContainer<SessionState> {
  return createStateContainer(initialSessionState);
}

/**
 * UI state container
 */
export interface UIState {
  currentScreen: string;
  previousScreen: string | null;
  isLoading: boolean;
  loadingMessage: string | null;
  notification: {
    type: 'info' | 'success' | 'warning' | 'error';
    message: string;
  } | null;
  confirmDialog: {
    title: string;
    message: string;
    onConfirm: () => void;
    onCancel: () => void;
  } | null;
}

const initialUIState: UIState = {
  currentScreen: 'main',
  previousScreen: null,
  isLoading: false,
  loadingMessage: null,
  notification: null,
  confirmDialog: null,
};

/**
 * Create UI state container
 */
export function createUIState(): StateContainer<UIState> {
  return createStateContainer(initialUIState);
}

export default {
  createMachineActor,
  createVoiceActor,
  createSetupActor,
  createProviderActor,
  createGlobalStore,
  getGlobalStore,
  resetGlobalStore,
  createStateContainer,
  createSessionState,
  createUIState,
};
