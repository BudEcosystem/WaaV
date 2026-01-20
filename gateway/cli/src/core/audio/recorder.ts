/**
 * WaaV CLI Audio Recorder
 *
 * Cross-platform audio recording using sox/arecord
 */

import { spawn, type ChildProcess } from 'child_process';
import { EventEmitter } from 'events';
import { AudioError, ErrorCode } from '../../utils/errors.js';
import { logger } from '../../utils/logger.js';
import {
  getPlatform,
  checkSoxInstalled,
  getAudioBackend,
  type AudioBackend,
} from '../../utils/platform.js';

/**
 * Audio recorder events
 */
export interface AudioRecorderEvents {
  data: (buffer: Buffer) => void;
  error: (error: Error) => void;
  start: () => void;
  stop: () => void;
  pause: () => void;
  resume: () => void;
  level: (level: number) => void;
}

/**
 * Audio recorder configuration
 */
export interface AudioRecorderConfig {
  sampleRate?: number;
  channels?: number;
  bitDepth?: number;
  device?: string;
  silenceThreshold?: number;
  silenceDuration?: number;
  recordProgram?: 'sox' | 'arecord' | 'auto';
}

/**
 * Recorder state
 */
export type RecorderState = 'idle' | 'recording' | 'paused' | 'error';

/**
 * Audio Recorder
 *
 * Records audio from the microphone using system audio tools
 */
export class AudioRecorder extends EventEmitter {
  private sampleRate: number;
  private channels: number;
  private bitDepth: number;
  private device: string;
  private recordProgram: 'sox' | 'arecord';

  private process: ChildProcess | null = null;
  private state: RecorderState = 'idle';
  private audioBackend: AudioBackend;
  private isPaused = false;

  // Level monitoring
  private levelBuffer: number[] = [];
  private levelUpdateInterval: NodeJS.Timeout | null = null;

  constructor(config: AudioRecorderConfig = {}) {
    super();
    this.sampleRate = config.sampleRate ?? 16000;
    this.channels = config.channels ?? 1;
    this.bitDepth = config.bitDepth ?? 16;
    this.device = config.device ?? 'default';

    // Detect audio backend
    this.audioBackend = getAudioBackend();

    // Determine record program
    if (config.recordProgram === 'auto' || !config.recordProgram) {
      const soxCheck = checkSoxInstalled();
      this.recordProgram = soxCheck.installed ? 'sox' : 'arecord';
    } else {
      this.recordProgram = config.recordProgram;
    }

    logger.debug(`AudioRecorder: backend=${this.audioBackend}, program=${this.recordProgram}`);
  }

  /**
   * Get current state
   */
  getState(): RecorderState {
    return this.state;
  }

  /**
   * Check if recording
   */
  isRecording(): boolean {
    return this.state === 'recording';
  }

  /**
   * Start recording
   */
  async start(): Promise<void> {
    if (this.state === 'recording') {
      throw new AudioError('Already recording');
    }

    // Verify sox is installed
    const soxCheck = checkSoxInstalled();
    if (!soxCheck.installed && this.recordProgram === 'sox') {
      throw new AudioError('sox is not installed', {
        code: ErrorCode.AUDIO_SOX_NOT_INSTALLED,
        suggestion: 'Install sox: sudo apt install sox libsox-fmt-all (Linux) or brew install sox (macOS)',
      });
    }

    return new Promise((resolve, reject) => {
      try {
        const args = this.buildRecordArgs();
        logger.debug(`Starting recording: ${this.recordProgram} ${args.join(' ')}`);

        this.process = spawn(this.recordProgram === 'sox' ? 'sox' : 'arecord', args, {
          stdio: ['pipe', 'pipe', 'pipe'],
        });

        // Handle stdout (audio data)
        this.process.stdout?.on('data', (data: Buffer) => {
          if (this.isPaused) {
            return;
          }

          this.emit('data', data);
          this.updateAudioLevel(data);
        });

        // Handle stderr (errors/info)
        this.process.stderr?.on('data', (data: Buffer) => {
          const message = data.toString().trim();
          if (message && !message.includes('Recording')) {
            logger.debug(`Recorder stderr: ${message}`);
          }
        });

        // Handle process errors
        this.process.on('error', (error) => {
          this.state = 'error';
          const audioError = new AudioError(`Recording failed: ${error.message}`, {
            code: ErrorCode.AUDIO_RECORDING_FAILED,
            cause: error,
          });
          this.emit('error', audioError);
          reject(audioError);
        });

        // Handle process exit
        this.process.on('close', (code) => {
          this.stopLevelMonitoring();
          if (code !== 0 && code !== null && this.state === 'recording') {
            logger.warn(`Recorder process exited with code ${code}`);
          }
          if (this.state !== 'error') {
            this.state = 'idle';
          }
        });

        // Give it a moment to start
        setTimeout(() => {
          if (this.process && !this.process.killed) {
            this.state = 'recording';
            this.startLevelMonitoring();
            this.emit('start');
            resolve();
          }
        }, 100);
      } catch (error) {
        this.state = 'error';
        const audioError = new AudioError(`Failed to start recording: ${error instanceof Error ? error.message : String(error)}`, {
          code: ErrorCode.AUDIO_RECORDING_FAILED,
          cause: error instanceof Error ? error : undefined,
        });
        reject(audioError);
      }
    });
  }

  /**
   * Stop recording
   */
  stop(): void {
    if (!this.process) {
      return;
    }

    this.stopLevelMonitoring();

    // Send SIGTERM to gracefully stop
    this.process.kill('SIGTERM');

    // Force kill after timeout
    const timeout = setTimeout(() => {
      if (this.process && !this.process.killed) {
        this.process.kill('SIGKILL');
      }
    }, 1000);

    this.process.once('close', () => {
      clearTimeout(timeout);
    });

    this.process = null;
    this.state = 'idle';
    this.isPaused = false;
    this.emit('stop');
    logger.debug('Recording stopped');
  }

  /**
   * Pause recording
   */
  pause(): void {
    if (this.state !== 'recording') {
      return;
    }

    this.isPaused = true;
    this.state = 'paused';
    this.emit('pause');
    logger.debug('Recording paused');
  }

  /**
   * Resume recording
   */
  resume(): void {
    if (this.state !== 'paused') {
      return;
    }

    this.isPaused = false;
    this.state = 'recording';
    this.emit('resume');
    logger.debug('Recording resumed');
  }

  /**
   * Build command line arguments for recording
   */
  private buildRecordArgs(): string[] {
    const platform = getPlatform();

    if (this.recordProgram === 'sox') {
      // sox arguments for recording
      const args: string[] = [];

      // Input device
      if (platform === 'linux') {
        if (this.audioBackend === 'pulseaudio') {
          args.push('-t', 'pulseaudio', this.device === 'default' ? 'default' : this.device);
        } else {
          args.push('-t', 'alsa', this.device === 'default' ? 'hw:0,0' : this.device);
        }
      } else if (platform === 'darwin') {
        args.push('-t', 'coreaudio', this.device === 'default' ? 'default' : this.device);
      } else if (platform === 'win32') {
        args.push('-t', 'waveaudio', this.device === 'default' ? '-1' : this.device);
      }

      // Output format (raw PCM to stdout)
      args.push(
        '-t', 'raw',
        '-b', String(this.bitDepth),
        '-e', 'signed-integer',
        '-c', String(this.channels),
        '-r', String(this.sampleRate),
        '-',  // Output to stdout
      );

      return args;
    } else {
      // arecord arguments (Linux only)
      return [
        '-D', this.device === 'default' ? 'default' : this.device,
        '-f', `S${this.bitDepth}_LE`,
        '-c', String(this.channels),
        '-r', String(this.sampleRate),
        '-t', 'raw',
        '-q', // Quiet mode
      ];
    }
  }

  /**
   * Update audio level from buffer
   */
  private updateAudioLevel(buffer: Buffer): void {
    // Calculate RMS level from 16-bit PCM data
    let sumSquares = 0;
    const samples = buffer.length / 2; // 16-bit = 2 bytes per sample

    for (let i = 0; i < buffer.length; i += 2) {
      const sample = buffer.readInt16LE(i);
      sumSquares += sample * sample;
    }

    const rms = Math.sqrt(sumSquares / samples);
    const level = Math.min(1, rms / 32768); // Normalize to 0-1

    this.levelBuffer.push(level);

    // Keep only recent samples
    if (this.levelBuffer.length > 10) {
      this.levelBuffer.shift();
    }
  }

  /**
   * Start level monitoring
   */
  private startLevelMonitoring(): void {
    this.stopLevelMonitoring();
    this.levelUpdateInterval = setInterval(() => {
      if (this.levelBuffer.length > 0) {
        const avgLevel = this.levelBuffer.reduce((a, b) => a + b, 0) / this.levelBuffer.length;
        this.emit('level', avgLevel);
      }
    }, 100);
  }

  /**
   * Stop level monitoring
   */
  private stopLevelMonitoring(): void {
    if (this.levelUpdateInterval) {
      clearInterval(this.levelUpdateInterval);
      this.levelUpdateInterval = null;
    }
    this.levelBuffer = [];
  }

  /**
   * Get audio configuration
   */
  getConfig(): { sampleRate: number; channels: number; bitDepth: number; device: string } {
    return {
      sampleRate: this.sampleRate,
      channels: this.channels,
      bitDepth: this.bitDepth,
      device: this.device,
    };
  }

  /**
   * Set device
   */
  setDevice(device: string): void {
    if (this.state === 'recording') {
      throw new AudioError('Cannot change device while recording');
    }
    this.device = device;
  }

  /**
   * Set sample rate
   */
  setSampleRate(sampleRate: number): void {
    if (this.state === 'recording') {
      throw new AudioError('Cannot change sample rate while recording');
    }
    this.sampleRate = sampleRate;
  }

  // Type-safe event methods
  on<K extends keyof AudioRecorderEvents>(event: K, listener: AudioRecorderEvents[K]): this {
    return super.on(event, listener as (...args: unknown[]) => void);
  }

  once<K extends keyof AudioRecorderEvents>(event: K, listener: AudioRecorderEvents[K]): this {
    return super.once(event, listener as (...args: unknown[]) => void);
  }

  emit<K extends keyof AudioRecorderEvents>(event: K, ...args: Parameters<AudioRecorderEvents[K]>): boolean {
    return super.emit(event, ...args);
  }

  off<K extends keyof AudioRecorderEvents>(event: K, listener: AudioRecorderEvents[K]): this {
    return super.off(event, listener as (...args: unknown[]) => void);
  }
}
