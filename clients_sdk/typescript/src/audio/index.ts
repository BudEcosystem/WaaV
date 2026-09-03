/**
 * Audio module for @bud-foundry/sdk
 */

export { AudioProcessor, AUDIO_FORMATS, type AudioFormat } from './processor.js';
export { PCMPlayer, createPCMPlayer, type PlayerConfig, type PlayerState, type PlayerEventHandlers } from './player.js';
export { JitterBuffer, type JitterBufferConfig, type ResolvedJitterBufferConfig, type JitterBufferHooks } from './jitter-buffer.js';
export { AudioRecorder, createRecorder, type RecorderConfig, type RecorderState, type RecorderEventHandlers } from './recorder.js';
export { VAD, createVAD, type VADConfig, type VADState, type VADEvent, type VADEventHandlers } from './vad.js';
export { SileroVAD, type SileroVADConfig, type SileroSession, type SileroTensor } from './vad-silero.js';
export {
  supportsWebCodecsOpus,
  OpusEncoder,
  OpusDecoder,
  DEFAULT_OPUS_BITRATE,
  type OpusEncoderOptions,
  type OpusDecoderOptions,
  type WebCodecsCtors,
} from './opus.js';
