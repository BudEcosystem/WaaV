/**
 * Audio recorder using MediaRecorder and Web Audio API
 */

export type RecorderOptions = {
  sampleRate?: number;
  channels?: number;
  echoCancellation?: boolean;
  noiseSuppression?: boolean;
};

export class AudioRecorder {
  private stream: MediaStream | null = null;
  private audioContext: AudioContext | null = null;
  private processor: ScriptProcessorNode | null = null;
  private source: MediaStreamAudioSourceNode | null = null;
  private options: RecorderOptions;
  private onDataCallback: ((data: Int16Array) => void) | null = null;
  private onSilenceCallback: (() => void) | null = null;
  private onSpeechCallback: (() => void) | null = null;
  private silenceThreshold = 0.01;
  private silenceTimeout = 1500; // ms
  private lastSpeechTime = 0;
  private isSpeaking = false;

  constructor(options: RecorderOptions = {}) {
    this.options = {
      sampleRate: options.sampleRate || 16000,
      channels: options.channels || 1,
      echoCancellation: options.echoCancellation ?? true,
      noiseSuppression: options.noiseSuppression ?? true,
    };
  }

  async start(): Promise<void> {
    try {
      this.stream = await navigator.mediaDevices.getUserMedia({
        audio: {
          echoCancellation: this.options.echoCancellation,
          noiseSuppression: this.options.noiseSuppression,
          sampleRate: this.options.sampleRate,
          channelCount: this.options.channels,
        },
      });

      this.audioContext = new AudioContext({
        sampleRate: this.options.sampleRate,
      });

      this.source = this.audioContext.createMediaStreamSource(this.stream);

      // Use ScriptProcessorNode for raw audio access
      // Note: This is deprecated but widely supported
      // AudioWorklet would be better but requires more setup
      const bufferSize = 4096;
      this.processor = this.audioContext.createScriptProcessor(
        bufferSize,
        this.options.channels || 1,
        this.options.channels || 1
      );

      this.processor.onaudioprocess = (event) => {
        const inputData = event.inputBuffer.getChannelData(0);

        // Convert Float32 to Int16
        const int16Data = this.float32ToInt16(inputData);

        // VAD check
        const rms = this.calculateRMS(inputData);
        const now = performance.now();

        if (rms > this.silenceThreshold) {
          this.lastSpeechTime = now;
          if (!this.isSpeaking) {
            this.isSpeaking = true;
            if (this.onSpeechCallback) {
              this.onSpeechCallback();
            }
          }
        } else if (this.isSpeaking && now - this.lastSpeechTime > this.silenceTimeout) {
          this.isSpeaking = false;
          if (this.onSilenceCallback) {
            this.onSilenceCallback();
          }
        }

        // Send audio data
        if (this.onDataCallback) {
          this.onDataCallback(int16Data);
        }
      };

      this.source.connect(this.processor);
      this.processor.connect(this.audioContext.destination);
    } catch (error) {
      throw new Error(`Failed to start recording: ${error}`);
    }
  }

  stop(): void {
    if (this.processor) {
      this.processor.disconnect();
      this.processor = null;
    }

    if (this.source) {
      this.source.disconnect();
      this.source = null;
    }

    if (this.audioContext) {
      this.audioContext.close();
      this.audioContext = null;
    }

    if (this.stream) {
      this.stream.getTracks().forEach((track) => track.stop());
      this.stream = null;
    }
  }

  onData(callback: (data: Int16Array) => void): void {
    this.onDataCallback = callback;
  }

  onSilence(callback: () => void): void {
    this.onSilenceCallback = callback;
  }

  onSpeech(callback: () => void): void {
    this.onSpeechCallback = callback;
  }

  private float32ToInt16(buffer: Float32Array): Int16Array {
    const result = new Int16Array(buffer.length);
    for (let i = 0; i < buffer.length; i++) {
      const s = Math.max(-1, Math.min(1, buffer[i]));
      result[i] = s < 0 ? s * 0x8000 : s * 0x7fff;
    }
    return result;
  }

  private calculateRMS(buffer: Float32Array): number {
    let sum = 0;
    for (let i = 0; i < buffer.length; i++) {
      sum += buffer[i] * buffer[i];
    }
    return Math.sqrt(sum / buffer.length);
  }

  get isRecording(): boolean {
    return this.stream !== null;
  }
}
