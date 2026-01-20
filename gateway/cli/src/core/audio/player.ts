/**
 * WaaV CLI Audio Player
 *
 * Cross-platform audio playback using speaker/play-sound
 */

import { spawn, type ChildProcess } from 'child_process';
import { EventEmitter } from 'events';
import { PassThrough } from 'stream';
import { AudioError, ErrorCode } from '../../utils/errors.js';
import { logger } from '../../utils/logger.js';
import { getPlatform, getAudioBackend } from '../../utils/platform.js';

/**
 * Audio player events
 */
export interface AudioPlayerEvents {
  start: () => void;
  stop: () => void;
  complete: () => void;
  error: (error: Error) => void;
  drain: () => void;
}

/**
 * Audio player configuration
 */
export interface AudioPlayerConfig {
  sampleRate?: number;
  channels?: number;
  bitDepth?: number;
  device?: string;
  volume?: number;
}

/**
 * Player state
 */
export type PlayerState = 'idle' | 'playing' | 'stopping' | 'error';

/**
 * Audio Player
 *
 * Plays audio to the speakers using system audio tools
 */
export class AudioPlayer extends EventEmitter {
  private sampleRate: number;
  private channels: number;
  private bitDepth: number;
  private device: string;
  private volume: number;

  private process: ChildProcess | null = null;
  private state: PlayerState = 'idle';
  private audioStream: PassThrough | null = null;
  private pendingBuffers: Buffer[] = [];
  private totalBytesPlayed = 0;

  constructor(config: AudioPlayerConfig = {}) {
    super();
    this.sampleRate = config.sampleRate ?? 16000;
    this.channels = config.channels ?? 1;
    this.bitDepth = config.bitDepth ?? 16;
    this.device = config.device ?? 'default';
    this.volume = config.volume ?? 1.0;
  }

  /**
   * Get current state
   */
  getState(): PlayerState {
    return this.state;
  }

  /**
   * Check if playing
   */
  isPlaying(): boolean {
    return this.state === 'playing';
  }

  /**
   * Start playback
   */
  start(): void {
    if (this.state === 'playing') {
      return;
    }

    try {
      const args = this.buildPlayArgs();
      const platform = getPlatform();
      const command = platform === 'win32' ? 'sox' : this.getPlayCommand();

      logger.debug(`Starting playback: ${command} ${args.join(' ')}`);

      this.process = spawn(command, args, {
        stdio: ['pipe', 'pipe', 'pipe'],
      });

      // Create audio stream
      this.audioStream = new PassThrough();
      this.audioStream.pipe(this.process.stdin!);

      // Handle process events
      this.process.on('error', (error) => {
        this.state = 'error';
        const audioError = new AudioError(`Playback failed: ${error.message}`, {
          code: ErrorCode.AUDIO_PLAYBACK_FAILED,
          cause: error,
        });
        this.emit('error', audioError);
      });

      this.process.on('close', (code) => {
        if (this.state === 'playing') {
          this.state = 'idle';
          this.emit('complete');
          logger.debug(`Playback completed (code=${code})`);
        }
        this.cleanup();
      });

      this.process.stderr?.on('data', (data: Buffer) => {
        const message = data.toString().trim();
        if (message && !message.includes('play WARN')) {
          logger.debug(`Player stderr: ${message}`);
        }
      });

      // Handle drain event for flow control
      this.audioStream.on('drain', () => {
        this.emit('drain');
      });

      this.state = 'playing';
      this.emit('start');

      // Write any pending buffers
      this.flushPendingBuffers();
    } catch (error) {
      this.state = 'error';
      throw new AudioError(`Failed to start playback: ${error instanceof Error ? error.message : String(error)}`, {
        code: ErrorCode.AUDIO_PLAYBACK_FAILED,
        cause: error instanceof Error ? error : undefined,
      });
    }
  }

  /**
   * Stop playback
   */
  stop(): void {
    if (!this.process) {
      return;
    }

    this.state = 'stopping';

    // End the audio stream gracefully
    if (this.audioStream) {
      this.audioStream.end();
    }

    // Kill process after timeout
    const timeout = setTimeout(() => {
      if (this.process && !this.process.killed) {
        this.process.kill('SIGKILL');
      }
    }, 500);

    this.process.once('close', () => {
      clearTimeout(timeout);
      this.cleanup();
    });

    this.emit('stop');
    logger.debug('Playback stopped');
  }

  /**
   * Write audio data to the player
   */
  write(buffer: Buffer | Uint8Array): boolean {
    const data = Buffer.from(buffer);

    // If not playing, start playback
    if (this.state === 'idle') {
      this.pendingBuffers.push(data);
      this.start();
      return true;
    }

    // If playing, write to stream
    if (this.audioStream && this.state === 'playing') {
      this.totalBytesPlayed += data.length;
      return this.audioStream.write(data);
    }

    // Queue for later
    this.pendingBuffers.push(data);
    return false;
  }

  /**
   * Signal end of audio data
   */
  end(): void {
    if (this.audioStream && this.state === 'playing') {
      this.audioStream.end();
    }
  }

  /**
   * Play a complete buffer
   */
  async play(buffer: Buffer | Uint8Array): Promise<void> {
    return new Promise((resolve, reject) => {
      try {
        this.once('complete', () => resolve());
        this.once('error', (error) => reject(error));

        this.write(buffer);
        this.end();
      } catch (error) {
        reject(error);
      }
    });
  }

  /**
   * Play audio from a file
   */
  async playFile(filePath: string): Promise<void> {
    const platform = getPlatform();

    return new Promise((resolve, reject) => {
      let command: string;
      let args: string[];

      if (platform === 'darwin') {
        command = 'afplay';
        args = [filePath];
      } else if (platform === 'linux') {
        command = 'aplay';
        args = [filePath];
      } else {
        command = 'powershell';
        args = ['-c', `(New-Object Media.SoundPlayer "${filePath}").PlaySync()`];
      }

      const proc = spawn(command, args);

      proc.on('error', (error) => {
        reject(new AudioError(`Failed to play file: ${error.message}`, {
          code: ErrorCode.AUDIO_PLAYBACK_FAILED,
          cause: error,
        }));
      });

      proc.on('close', (code) => {
        if (code === 0) {
          resolve();
        } else {
          reject(new AudioError(`Playback failed with code ${code}`));
        }
      });
    });
  }

  /**
   * Get play command for current platform
   */
  private getPlayCommand(): string {
    const platform = getPlatform();

    if (platform === 'darwin') {
      return 'play'; // sox play command
    }

    // Linux
    const backend = getAudioBackend();
    if (backend === 'pulseaudio') {
      return 'play'; // sox with pulseaudio
    }

    return 'aplay';
  }

  /**
   * Build command line arguments
   */
  private buildPlayArgs(): string[] {
    const platform = getPlatform();
    const backend = getAudioBackend();

    if (platform === 'linux' && backend === 'alsa') {
      // aplay arguments
      return [
        '-D', this.device === 'default' ? 'default' : this.device,
        '-f', `S${this.bitDepth}_LE`,
        '-c', String(this.channels),
        '-r', String(this.sampleRate),
        '-t', 'raw',
        '-q', // Quiet mode
      ];
    }

    // sox play arguments
    const args: string[] = [];

    // Input format (raw PCM from stdin)
    args.push(
      '-t', 'raw',
      '-b', String(this.bitDepth),
      '-e', 'signed-integer',
      '-c', String(this.channels),
      '-r', String(this.sampleRate),
      '-', // Input from stdin
    );

    // Output device
    if (platform === 'linux') {
      if (backend === 'pulseaudio') {
        args.push('-t', 'pulseaudio', this.device === 'default' ? 'default' : this.device);
      } else {
        args.push('-t', 'alsa', this.device === 'default' ? 'default' : this.device);
      }
    } else if (platform === 'darwin') {
      args.push('-t', 'coreaudio', this.device === 'default' ? 'default' : this.device);
    } else if (platform === 'win32') {
      args.push('-t', 'waveaudio', this.device === 'default' ? '-1' : this.device);
    }

    // Volume adjustment
    if (this.volume !== 1.0) {
      args.push('vol', String(this.volume));
    }

    return args;
  }

  /**
   * Flush pending buffers
   */
  private flushPendingBuffers(): void {
    while (this.pendingBuffers.length > 0 && this.audioStream && this.state === 'playing') {
      const buffer = this.pendingBuffers.shift();
      if (buffer) {
        this.audioStream.write(buffer);
        this.totalBytesPlayed += buffer.length;
      }
    }
  }

  /**
   * Cleanup resources
   */
  private cleanup(): void {
    this.process = null;
    this.audioStream = null;
    this.pendingBuffers = [];
    this.state = 'idle';
  }

  /**
   * Get audio configuration
   */
  getConfig(): { sampleRate: number; channels: number; bitDepth: number; device: string; volume: number } {
    return {
      sampleRate: this.sampleRate,
      channels: this.channels,
      bitDepth: this.bitDepth,
      device: this.device,
      volume: this.volume,
    };
  }

  /**
   * Set volume (0.0 to 1.0)
   */
  setVolume(volume: number): void {
    this.volume = Math.max(0, Math.min(1, volume));
  }

  /**
   * Set device
   */
  setDevice(device: string): void {
    if (this.state === 'playing') {
      throw new AudioError('Cannot change device while playing');
    }
    this.device = device;
  }

  /**
   * Set sample rate
   */
  setSampleRate(sampleRate: number): void {
    if (this.state === 'playing') {
      throw new AudioError('Cannot change sample rate while playing');
    }
    this.sampleRate = sampleRate;
  }

  /**
   * Get total bytes played
   */
  getTotalBytesPlayed(): number {
    return this.totalBytesPlayed;
  }

  /**
   * Get playback duration in seconds
   */
  getPlaybackDuration(): number {
    const bytesPerSample = this.bitDepth / 8;
    const bytesPerSecond = this.sampleRate * this.channels * bytesPerSample;
    return this.totalBytesPlayed / bytesPerSecond;
  }

  // Type-safe event methods
  on<K extends keyof AudioPlayerEvents>(event: K, listener: AudioPlayerEvents[K]): this {
    return super.on(event, listener as (...args: unknown[]) => void);
  }

  once<K extends keyof AudioPlayerEvents>(event: K, listener: AudioPlayerEvents[K]): this {
    return super.once(event, listener as (...args: unknown[]) => void);
  }

  emit<K extends keyof AudioPlayerEvents>(event: K, ...args: Parameters<AudioPlayerEvents[K]>): boolean {
    return super.emit(event, ...args);
  }

  off<K extends keyof AudioPlayerEvents>(event: K, listener: AudioPlayerEvents[K]): this {
    return super.off(event, listener as (...args: unknown[]) => void);
  }
}
