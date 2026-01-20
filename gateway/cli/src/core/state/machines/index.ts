/**
 * WaaV CLI State Machines
 *
 * XState machines for managing complex UI flows
 */

export {
  voiceMachine,
  type VoiceMachineContext,
  type VoiceMachineEvent,
  type VoiceMachineState,
  isVoiceActive,
  canStartListening,
  getVoiceStatusColor,
  getVoiceStatusText,
} from './voiceMachine.js';

export {
  setupMachine,
  type SetupMachineContext,
  type SetupMachineEvent,
  type SetupStep,
  SETUP_STEPS,
  getCurrentStep,
  getSetupProgress,
  isSetupComplete,
  getSetupSummary,
} from './setupMachine.js';

export {
  providerMachine,
  type ProviderMachineContext,
  type ProviderMachineEvent,
  type ProviderType,
  type ProviderInfo,
  STT_PROVIDERS,
  TTS_PROVIDERS,
  REALTIME_PROVIDERS,
  getProvidersByType,
  getProviderById,
  getProviderTypeLabel,
  validateApiKeyFormat,
} from './providerMachine.js';
