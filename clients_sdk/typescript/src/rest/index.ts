/**
 * REST Client module for @bud-foundry/sdk
 */

export { RestClient } from './client.js';
export type { RestClientOptions } from './client.js';

// Voice cloning and recording API
export {
  cloneVoice,
  listClonedVoices,
  deleteClonedVoice,
  getClonedVoiceStatus,
  getRecording,
  downloadRecording,
  listRecordings,
  deleteRecording,
  startRecording,
  stopRecording,
  VoiceAPIError,
} from './voice.js';
