/**
 * WaaV CLI Audio Module
 *
 * Re-exports audio utilities
 */

export {
  AudioRecorder,
  type AudioRecorderConfig,
  type AudioRecorderEvents,
  type RecorderState,
} from './recorder.js';

export {
  AudioPlayer,
  type AudioPlayerConfig,
  type AudioPlayerEvents,
  type PlayerState,
} from './player.js';

export {
  VAD,
  type VADConfig,
  type VADEvents,
  type VADState,
} from './vad.js';
