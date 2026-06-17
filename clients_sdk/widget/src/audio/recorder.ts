/**
 * Audio recorder (D3: off-main-thread capture via AudioWorklet).
 *
 * Capture runs on the audio render thread in an AudioWorkletProcessor that
 * accumulates 128-sample render quanta into 20ms frames (320 samples @16k),
 * converts Float32 -> Int16 IN the worklet, and posts the Int16 frame to the
 * main thread as a TRANSFERABLE ArrayBuffer (zero-copy). This frees the main
 * thread (no React/GC/layout contention -> no jank/underrun) and gives a 20ms
 * input cadence instead of the ScriptProcessor's 256ms@16k buffer.
 *
 * The deprecated ScriptProcessorNode is kept ONLY as a legacy fallback for
 * browsers without AudioWorklet (`typeof AudioWorkletNode === 'undefined'`).
 *
 * The worklet module is registered via an inline Blob URL so no separate bundle
 * entry / fetchable asset is required (the widget ships as a single IIFE/ESM).
 *
 * VAD (RMS-based speech/silence) is computed on the main thread from each
 * received Int16 frame so behaviour is identical across both capture paths.
 */

export type RecorderOptions = {
  sampleRate?: number;
  channels?: number;
  echoCancellation?: boolean;
  noiseSuppression?: boolean;
};

/**
 * The AudioWorkletProcessor source, registered from an inline Blob URL.
 *
 * Runs on the audio render thread. `process()` is called per 128-sample quantum;
 * we fill a reusable Int16 frame buffer and, once 20ms (FRAME_SAMPLES) is ready,
 * copy it into a fresh ArrayBuffer and postMessage it transferable. Allocation in
 * process() is kept minimal (one small buffer per 20ms frame, transferred away).
 */
const WORKLET_SOURCE = `
class BudCaptureProcessor extends AudioWorkletProcessor {
  constructor(options) {
    super();
    const opts = (options && options.processorOptions) || {};
    // 20ms @ the context sample rate (e.g. 320 samples @16k). Computed from the
    // real render-thread sampleRate so it is correct even if the context rate
    // differs from the requested rate.
    this.frameSamples = Math.max(1, Math.round(sampleRate * 0.02));
    this.buffer = new Int16Array(this.frameSamples);
    this.offset = 0;
  }

  process(inputs) {
    const input = inputs[0];
    if (!input || input.length === 0) return true;
    const channel = input[0];
    if (!channel) return true;

    for (let i = 0; i < channel.length; i++) {
      let s = channel[i];
      if (s > 1) s = 1; else if (s < -1) s = -1;
      this.buffer[this.offset++] = s < 0 ? s * 0x8000 : s * 0x7fff;
      if (this.offset >= this.frameSamples) {
        // Copy the completed frame into a fresh buffer and transfer it (zero-copy).
        const out = this.buffer.slice(0);
        this.port.postMessage(out.buffer, [out.buffer]);
        this.offset = 0;
      }
    }
    return true;
  }
}
registerProcessor('bud-capture', BudCaptureProcessor);
`;

export class AudioRecorder {
  private stream: MediaStream | null = null;
  private audioContext: AudioContext | null = null;
  private processor: ScriptProcessorNode | null = null;
  private workletNode: AudioWorkletNode | null = null;
  private source: MediaStreamAudioSourceNode | null = null;
  private workletUrl: string | null = null;
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

      // Prefer the off-main-thread AudioWorklet; fall back to ScriptProcessor.
      if (this.workletSupported()) {
        await this.startWorklet();
      } else {
        this.startScriptProcessor();
      }
    } catch (error) {
      throw new Error(`Failed to start recording: ${error}`);
    }
  }

  /** AudioWorklet is usable only if both the node and the addModule API exist. */
  private workletSupported(): boolean {
    return (
      typeof AudioWorkletNode !== 'undefined' &&
      !!this.audioContext &&
      !!this.audioContext.audioWorklet &&
      typeof this.audioContext.audioWorklet.addModule === 'function' &&
      typeof Blob !== 'undefined' &&
      typeof URL !== 'undefined' &&
      typeof URL.createObjectURL === 'function'
    );
  }

  /** D3 primary path: register the inline worklet and stream 20ms Int16 frames. */
  private async startWorklet(): Promise<void> {
    if (!this.audioContext || !this.source) return;

    const blob = new Blob([WORKLET_SOURCE], { type: 'application/javascript' });
    this.workletUrl = URL.createObjectURL(blob);
    await this.audioContext.audioWorklet.addModule(this.workletUrl);

    this.workletNode = new AudioWorkletNode(this.audioContext, 'bud-capture', {
      numberOfInputs: 1,
      numberOfOutputs: 0,
      channelCount: this.options.channels || 1,
    });

    this.workletNode.port.onmessage = (event: MessageEvent) => {
      // Transferable ArrayBuffer carrying one 20ms Int16 frame.
      const frame = new Int16Array(event.data as ArrayBuffer);
      this.handleFrame(frame);
    };

    // No output: connecting to a node sink would route mic audio to speakers.
    // The worklet pulls input as long as its source is connected.
    this.source.connect(this.workletNode);
  }

  /** Legacy fallback for browsers without AudioWorklet. */
  private startScriptProcessor(): void {
    if (!this.audioContext || !this.source) return;

    // Deprecated but widely supported; only used when AudioWorklet is absent.
    const bufferSize = 4096;
    this.processor = this.audioContext.createScriptProcessor(
      bufferSize,
      this.options.channels || 1,
      this.options.channels || 1
    );

    this.processor.onaudioprocess = (event) => {
      const inputData = event.inputBuffer.getChannelData(0);
      const int16Data = this.float32ToInt16(inputData);
      this.handleFrame(int16Data);
    };

    this.source.connect(this.processor);
    // ScriptProcessor only pulls audio while connected to a destination.
    this.processor.connect(this.audioContext.destination);
  }

  /**
   * Per-frame main-thread handling shared by both capture paths: RMS-based VAD
   * (speech/silence edges) + forwarding the Int16 frame to onData.
   */
  private handleFrame(int16Data: Int16Array): void {
    const rms = this.calculateRMSInt16(int16Data);
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

    if (this.onDataCallback) {
      this.onDataCallback(int16Data);
    }
  }

  stop(): void {
    if (this.workletNode) {
      this.workletNode.port.onmessage = null;
      this.workletNode.disconnect();
      this.workletNode = null;
    }

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

    // Release the inline worklet Blob URL.
    if (this.workletUrl) {
      try {
        URL.revokeObjectURL(this.workletUrl);
      } catch (e) {
        // ignore
      }
      this.workletUrl = null;
    }

    this.isSpeaking = false;
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

  /** RMS in [0,1) computed from Int16 samples (matches the old Float32 RMS scale). */
  private calculateRMSInt16(buffer: Int16Array): number {
    let sum = 0;
    for (let i = 0; i < buffer.length; i++) {
      const v = buffer[i] / 32768;
      sum += v * v;
    }
    return buffer.length ? Math.sqrt(sum / buffer.length) : 0;
  }

  get isRecording(): boolean {
    return this.stream !== null;
  }
}
