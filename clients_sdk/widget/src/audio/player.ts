/**
 * Audio player for PCM audio playback
 */

export type PlayerOptions = {
  sampleRate?: number;
  channels?: number;
};

export class AudioPlayer {
  private audioContext: AudioContext | null = null;
  private options: PlayerOptions;
  private queue: AudioBuffer[] = [];
  private isPlaying = false;
  private currentSource: AudioBufferSourceNode | null = null;
  private onPlaybackEndCallback: (() => void) | null = null;
  private gainNode: GainNode | null = null;

  constructor(options: PlayerOptions = {}) {
    this.options = {
      sampleRate: options.sampleRate || 24000,
      channels: options.channels || 1,
    };
  }

  async initialize(): Promise<void> {
    this.audioContext = new AudioContext({
      sampleRate: this.options.sampleRate,
    });
    this.gainNode = this.audioContext.createGain();
    this.gainNode.connect(this.audioContext.destination);
  }

  async play(audioData: ArrayBuffer): Promise<void> {
    if (!this.audioContext) {
      await this.initialize();
    }

    if (!this.audioContext || !this.gainNode) return;

    // Resume context if suspended (required for autoplay policy)
    if (this.audioContext.state === 'suspended') {
      await this.audioContext.resume();
    }

    // Convert Int16 PCM to AudioBuffer
    const audioBuffer = this.int16ToAudioBuffer(audioData);

    // Queue the audio
    this.queue.push(audioBuffer);

    // Start playing if not already
    if (!this.isPlaying) {
      this.playNext();
    }
  }

  private playNext(): void {
    if (this.queue.length === 0) {
      this.isPlaying = false;
      if (this.onPlaybackEndCallback) {
        this.onPlaybackEndCallback();
      }
      return;
    }

    if (!this.audioContext || !this.gainNode) return;

    this.isPlaying = true;
    const buffer = this.queue.shift()!;

    const source = this.audioContext.createBufferSource();
    source.buffer = buffer;
    source.connect(this.gainNode);

    source.onended = () => {
      this.currentSource = null;
      this.playNext();
    };

    this.currentSource = source;
    source.start();
  }

  stop(): void {
    if (this.currentSource) {
      try {
        this.currentSource.stop();
      } catch (e) {
        // Ignore if already stopped
      }
      this.currentSource = null;
    }
    this.queue = [];
    this.isPlaying = false;
  }

  setVolume(volume: number): void {
    if (this.gainNode) {
      this.gainNode.gain.value = Math.max(0, Math.min(1, volume));
    }
  }

  onPlaybackEnd(callback: () => void): void {
    this.onPlaybackEndCallback = callback;
  }

  private int16ToAudioBuffer(data: ArrayBuffer): AudioBuffer {
    if (!this.audioContext) {
      throw new Error('AudioContext not initialized');
    }

    const int16Array = new Int16Array(data);
    const numSamples = int16Array.length;

    const audioBuffer = this.audioContext.createBuffer(
      this.options.channels || 1,
      numSamples,
      this.options.sampleRate || 24000
    );

    const channelData = audioBuffer.getChannelData(0);
    for (let i = 0; i < numSamples; i++) {
      channelData[i] = int16Array[i] / 32768;
    }

    return audioBuffer;
  }

  get playing(): boolean {
    return this.isPlaying;
  }

  close(): void {
    this.stop();
    if (this.audioContext) {
      this.audioContext.close();
      this.audioContext = null;
    }
  }
}
