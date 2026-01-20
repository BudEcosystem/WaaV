/**
 * WaaV CLI State Management
 *
 * XState machines and global state utilities
 */

// Export all machines
export * from './machines/index.js';

// Export store utilities
export {
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
  StateContainer,
  type MachineActor,
  type GlobalStore,
  type SessionState,
  type UIState,
} from './store.js';
