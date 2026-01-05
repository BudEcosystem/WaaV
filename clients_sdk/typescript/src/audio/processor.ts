/**
 * Audio Processor
 * Handles audio format conversions for the SDK
 */

/**
 * Audio format information
 */
export interface AudioFormat {
  /** Sample rate in Hz */
  sampleRate: number;
  /** Number of channels */
  channels: number;
  /** Bits per sample (16 for Int16, 32 for Float32) */
  bitsPerSample: 16 | 32;
  /** Whether data is floating point */
  isFloat: boolean;
}

/**
 * Default audio formats
 */
export const AUDIO_FORMATS = {
  /** Standard STT input format */
  STT_INPUT: { sampleRate: 16000, channels: 1, bitsPerSample: 16, isFloat: false } as AudioFormat,
  /** Standard TTS output format */
  TTS_OUTPUT: { sampleRate: 24000, channels: 1, bitsPerSample: 16, isFloat: false } as AudioFormat,
  /** Web Audio API format */
  WEB_AUDIO: { sampleRate: 48000, channels: 1, bitsPerSample: 32, isFloat: true } as AudioFormat,
} as const;

/**
 * Audio processor for format conversions
 */
export class AudioProcessor {
  /**
   * Convert Float32Array to Int16Array (for sending to server)
   * @param float32 Input audio in Float32 format (range -1.0 to 1.0)
   * @returns Audio in Int16 format (range -32768 to 32767)
   */
  static float32ToInt16(float32: Float32Array): Int16Array {
    const int16 = new Int16Array(float32.length);
    for (let i = 0; i < float32.length; i++) {
      // Clamp to -1.0 to 1.0 range
      const sample = Math.max(-1, Math.min(1, float32[i]!));
      // Convert to 16-bit integer
      int16[i] = sample < 0 ? sample * 32768 : sample * 32767;
    }
    return int16;
  }

  /**
   * Convert Int16Array to Float32Array (for Web Audio API)
   * @param int16 Input audio in Int16 format (range -32768 to 32767)
   * @returns Audio in Float32 format (range -1.0 to 1.0)
   */
  static int16ToFloat32(int16: Int16Array): Float32Array {
    const float32 = new Float32Array(int16.length);
    for (let i = 0; i < int16.length; i++) {
      const sample = int16[i]!;
      // Convert from 16-bit integer to float
      float32[i] = sample < 0 ? sample / 32768 : sample / 32767;
    }
    return float32;
  }

  /**
   * Convert ArrayBuffer containing Int16 PCM to Float32Array
   * @param buffer ArrayBuffer containing Int16 PCM data
   * @returns Float32Array for Web Audio API
   */
  static pcmBufferToFloat32(buffer: ArrayBuffer): Float32Array {
    const int16 = new Int16Array(buffer);
    return AudioProcessor.int16ToFloat32(int16);
  }

  /**
   * Convert Float32Array to ArrayBuffer containing Int16 PCM
   * @param float32 Float32Array audio data
   * @returns ArrayBuffer with Int16 PCM data
   */
  static float32ToPcmBuffer(float32: Float32Array): ArrayBuffer {
    const int16 = AudioProcessor.float32ToInt16(float32);
    return int16.buffer as ArrayBuffer;
  }

  /**
   * Resample audio to a different sample rate
   * Uses linear interpolation for simplicity
   * @param input Input audio data
   * @param inputSampleRate Input sample rate in Hz
   * @param outputSampleRate Output sample rate in Hz
   * @returns Resampled audio data
   */
  static resample(input: Float32Array, inputSampleRate: number, outputSampleRate: number): Float32Array {
    if (inputSampleRate === outputSampleRate) {
      return input;
    }

    const ratio = inputSampleRate / outputSampleRate;
    const outputLength = Math.ceil(input.length / ratio);
    const output = new Float32Array(outputLength);

    for (let i = 0; i < outputLength; i++) {
      const srcIndex = i * ratio;
      const srcIndexInt = Math.floor(srcIndex);
      const srcIndexFrac = srcIndex - srcIndexInt;

      if (srcIndexInt + 1 < input.length) {
        // Linear interpolation between two samples
        output[i] = input[srcIndexInt]! * (1 - srcIndexFrac) + input[srcIndexInt + 1]! * srcIndexFrac;
      } else if (srcIndexInt < input.length) {
        output[i] = input[srcIndexInt]!;
      } else {
        output[i] = 0;
      }
    }

    return output;
  }

  /**
   * Resample Int16 audio
   */
  static resampleInt16(input: Int16Array, inputSampleRate: number, outputSampleRate: number): Int16Array {
    const float32 = AudioProcessor.int16ToFloat32(input);
    const resampled = AudioProcessor.resample(float32, inputSampleRate, outputSampleRate);
    return AudioProcessor.float32ToInt16(resampled);
  }

  /**
   * Merge multiple audio channels into mono
   * @param channels Array of channel data (each Float32Array)
   * @returns Mono audio (average of all channels)
   */
  static mergeToMono(channels: Float32Array[]): Float32Array {
    if (channels.length === 0) {
      return new Float32Array(0);
    }

    if (channels.length === 1) {
      return channels[0]!;
    }

    const length = Math.min(...channels.map((c) => c.length));
    const mono = new Float32Array(length);

    for (let i = 0; i < length; i++) {
      let sum = 0;
      for (const channel of channels) {
        sum += channel[i]!;
      }
      mono[i] = sum / channels.length;
    }

    return mono;
  }

  /**
   * Split stereo audio into separate channels
   * @param interleaved Interleaved stereo audio (L R L R L R ...)
   * @returns Array of two Float32Arrays [left, right]
   */
  static splitStereo(interleaved: Float32Array): [Float32Array, Float32Array] {
    const length = Math.floor(interleaved.length / 2);
    const left = new Float32Array(length);
    const right = new Float32Array(length);

    for (let i = 0; i < length; i++) {
      left[i] = interleaved[i * 2]!;
      right[i] = interleaved[i * 2 + 1]!;
    }

    return [left, right];
  }

  /**
   * Calculate RMS (Root Mean Square) level of audio
   * @param audio Audio data
   * @returns RMS level (0.0 to 1.0 for normalized audio)
   */
  static calculateRMS(audio: Float32Array): number {
    if (audio.length === 0) return 0;

    let sumSquares = 0;
    for (let i = 0; i < audio.length; i++) {
      sumSquares += audio[i]! * audio[i]!;
    }

    return Math.sqrt(sumSquares / audio.length);
  }

  /**
   * Calculate peak level of audio
   * @param audio Audio data
   * @returns Peak level (0.0 to 1.0 for normalized audio)
   */
  static calculatePeak(audio: Float32Array): number {
    if (audio.length === 0) return 0;

    let peak = 0;
    for (let i = 0; i < audio.length; i++) {
      const abs = Math.abs(audio[i]!);
      if (abs > peak) peak = abs;
    }

    return peak;
  }

  /**
   * Convert decibels to linear amplitude
   * @param db Decibel value
   * @returns Linear amplitude
   */
  static dbToLinear(db: number): number {
    return Math.pow(10, db / 20);
  }

  /**
   * Convert linear amplitude to decibels
   * @param linear Linear amplitude
   * @returns Decibel value
   */
  static linearToDb(linear: number): number {
    if (linear <= 0) return -Infinity;
    return 20 * Math.log10(linear);
  }

  /**
   * Apply gain to audio
   * @param audio Audio data
   * @param gainDb Gain in decibels
   * @returns Audio with gain applied
   */
  static applyGain(audio: Float32Array, gainDb: number): Float32Array {
    const gainLinear = AudioProcessor.dbToLinear(gainDb);
    const output = new Float32Array(audio.length);

    for (let i = 0; i < audio.length; i++) {
      output[i] = audio[i]! * gainLinear;
    }

    return output;
  }

  /**
   * Normalize audio to target peak level
   * @param audio Audio data
   * @param targetPeakDb Target peak level in dB (default -1 dB)
   * @returns Normalized audio
   */
  static normalize(audio: Float32Array, targetPeakDb = -1): Float32Array {
    const currentPeak = AudioProcessor.calculatePeak(audio);
    if (currentPeak === 0) return audio;

    const targetPeakLinear = AudioProcessor.dbToLinear(targetPeakDb);
    const gain = targetPeakLinear / currentPeak;

    const output = new Float32Array(audio.length);
    for (let i = 0; i < audio.length; i++) {
      output[i] = audio[i]! * gain;
    }

    return output;
  }

  /**
   * Concatenate multiple audio buffers
   * @param buffers Array of Float32Arrays to concatenate
   * @returns Single concatenated Float32Array
   */
  static concatenate(buffers: Float32Array[]): Float32Array {
    const totalLength = buffers.reduce((sum, buf) => sum + buf.length, 0);
    const result = new Float32Array(totalLength);

    let offset = 0;
    for (const buffer of buffers) {
      result.set(buffer, offset);
      offset += buffer.length;
    }

    return result;
  }

  /**
   * Slice audio buffer
   * @param audio Audio data
   * @param startSample Start sample index
   * @param endSample End sample index (exclusive)
   * @returns Sliced audio
   */
  static slice(audio: Float32Array, startSample: number, endSample?: number): Float32Array {
    return audio.slice(startSample, endSample);
  }

  /**
   * Create silence of specified duration
   * @param sampleRate Sample rate in Hz
   * @param durationMs Duration in milliseconds
   * @returns Float32Array of silence
   */
  static createSilence(sampleRate: number, durationMs: number): Float32Array {
    const samples = Math.ceil((sampleRate * durationMs) / 1000);
    return new Float32Array(samples);
  }

  /**
   * Detect if audio contains silence (based on RMS threshold)
   * @param audio Audio data
   * @param thresholdDb Threshold in dB (default -40 dB)
   * @returns True if audio is below threshold
   */
  static isSilence(audio: Float32Array, thresholdDb = -40): boolean {
    const rms = AudioProcessor.calculateRMS(audio);
    const rmsDb = AudioProcessor.linearToDb(rms);
    return rmsDb < thresholdDb;
  }
}
