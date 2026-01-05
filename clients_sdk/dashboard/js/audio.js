/**
 * Audio Manager for Dashboard
 */

export class AudioManager {
  constructor(options = {}) {
    this.options = {
      sampleRate: options.sampleRate || 16000,
      channels: options.channels || 1,
    };

    this.stream = null;
    this.audioContext = null;
    this.processor = null;
    this.source = null;

    this.onData = null;
    this.onLevel = null;
  }

  async startRecording() {
    try {
      this.stream = await navigator.mediaDevices.getUserMedia({
        audio: {
          sampleRate: this.options.sampleRate,
          channelCount: this.options.channels,
          echoCancellation: true,
          noiseSuppression: true,
        },
      });

      this.audioContext = new AudioContext({
        sampleRate: this.options.sampleRate,
      });

      this.source = this.audioContext.createMediaStreamSource(this.stream);

      // Use ScriptProcessorNode for raw audio access
      const bufferSize = 4096;
      this.processor = this.audioContext.createScriptProcessor(
        bufferSize,
        this.options.channels,
        this.options.channels
      );

      this.processor.onaudioprocess = (event) => {
        const inputData = event.inputBuffer.getChannelData(0);

        // Convert Float32 to Int16
        const int16Data = this.float32ToInt16(inputData);

        // Calculate level for visualization
        if (this.onLevel) {
          const level = this.calculateRMS(inputData);
          this.onLevel(level);
        }

        // Send audio data
        if (this.onData) {
          this.onData(int16Data);
        }
      };

      this.source.connect(this.processor);
      this.processor.connect(this.audioContext.destination);

      return true;
    } catch (error) {
      console.error('Failed to start recording:', error);
      throw error;
    }
  }

  stopRecording() {
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

  float32ToInt16(buffer) {
    const result = new Int16Array(buffer.length);
    for (let i = 0; i < buffer.length; i++) {
      const s = Math.max(-1, Math.min(1, buffer[i]));
      result[i] = s < 0 ? s * 0x8000 : s * 0x7fff;
    }
    return result;
  }

  calculateRMS(buffer) {
    let sum = 0;
    for (let i = 0; i < buffer.length; i++) {
      sum += buffer[i] * buffer[i];
    }
    return Math.sqrt(sum / buffer.length);
  }

  get isRecording() {
    return this.stream !== null;
  }
}

/**
 * Audio Player using Web Audio API
 */
export class AudioPlayer {
  constructor(options = {}) {
    this.options = {
      sampleRate: options.sampleRate || 24000,
      channels: options.channels || 1,
    };

    this.audioContext = null;
    this.queue = [];
    this.isPlaying = false;
    this.currentSource = null;
    this.gainNode = null;

    this.onPlaybackEnd = null;
  }

  async initialize() {
    this.audioContext = new AudioContext({
      sampleRate: this.options.sampleRate,
    });
    this.gainNode = this.audioContext.createGain();
    this.gainNode.connect(this.audioContext.destination);
  }

  async play(audioData) {
    if (!this.audioContext) {
      await this.initialize();
    }

    if (this.audioContext.state === 'suspended') {
      await this.audioContext.resume();
    }

    const audioBuffer = this.int16ToAudioBuffer(audioData);
    this.queue.push(audioBuffer);

    if (!this.isPlaying) {
      this.playNext();
    }
  }

  playNext() {
    if (this.queue.length === 0) {
      this.isPlaying = false;
      if (this.onPlaybackEnd) {
        this.onPlaybackEnd();
      }
      return;
    }

    this.isPlaying = true;
    const buffer = this.queue.shift();

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

  stop() {
    if (this.currentSource) {
      try {
        this.currentSource.stop();
      } catch (e) {
        // Ignore
      }
      this.currentSource = null;
    }
    this.queue = [];
    this.isPlaying = false;
  }

  setVolume(volume) {
    if (this.gainNode) {
      this.gainNode.gain.value = Math.max(0, Math.min(1, volume));
    }
  }

  int16ToAudioBuffer(data) {
    let int16Array;
    if (data instanceof Int16Array) {
      int16Array = data;
    } else if (data instanceof ArrayBuffer) {
      int16Array = new Int16Array(data);
    } else {
      throw new Error('Invalid audio data type');
    }

    const numSamples = int16Array.length;
    const audioBuffer = this.audioContext.createBuffer(
      this.options.channels,
      numSamples,
      this.options.sampleRate
    );

    const channelData = audioBuffer.getChannelData(0);
    for (let i = 0; i < numSamples; i++) {
      channelData[i] = int16Array[i] / 32768;
    }

    return audioBuffer;
  }

  close() {
    this.stop();
    if (this.audioContext) {
      this.audioContext.close();
      this.audioContext = null;
    }
  }
}
