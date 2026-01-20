/**
 * WaaV CLI Platform Detection
 *
 * Utilities for detecting platform capabilities and audio dependencies
 */

import { execSync, exec } from 'child_process';
import { promisify } from 'util';
import os from 'os';
import path from 'path';

const execAsync = promisify(exec);

// Platform types
export type Platform = 'linux' | 'darwin' | 'win32';
export type Architecture = 'x64' | 'arm64' | 'arm' | 'ia32';

// Audio backend types
export type AudioBackend = 'pulseaudio' | 'alsa' | 'coreaudio' | 'wasapi' | 'unknown';

// Platform information
export interface PlatformInfo {
  platform: Platform;
  arch: Architecture;
  release: string;
  kernel: string;
  hostname: string;
  homeDir: string;
  tempDir: string;
  cpuCores: number;
  totalMemory: number;
  freeMemory: number;
}

// Audio system information
export interface AudioSystemInfo {
  backend: AudioBackend;
  soxInstalled: boolean;
  soxVersion?: string;
  inputDevices: AudioDeviceInfo[];
  outputDevices: AudioDeviceInfo[];
  defaultInput?: AudioDeviceInfo;
  defaultOutput?: AudioDeviceInfo;
}

// Audio device information
export interface AudioDeviceInfo {
  id: string;
  name: string;
  isDefault: boolean;
  sampleRates?: number[];
  channels?: number;
}

// Dependency check result
export interface DependencyCheck {
  name: string;
  required: boolean;
  installed: boolean;
  version?: string;
  path?: string;
  suggestion?: string;
}

/**
 * Get current platform
 */
export function getPlatform(): Platform {
  return process.platform as Platform;
}

/**
 * Get CPU architecture
 */
export function getArchitecture(): Architecture {
  return process.arch as Architecture;
}

/**
 * Check if running on Linux
 */
export function isLinux(): boolean {
  return process.platform === 'linux';
}

/**
 * Check if running on macOS
 */
export function isMacOS(): boolean {
  return process.platform === 'darwin';
}

/**
 * Check if running on Windows
 */
export function isWindows(): boolean {
  return process.platform === 'win32';
}

/**
 * Get platform information
 */
export function getPlatformInfo(): PlatformInfo {
  return {
    platform: getPlatform(),
    arch: getArchitecture(),
    release: os.release(),
    kernel: os.version(),
    hostname: os.hostname(),
    homeDir: os.homedir(),
    tempDir: os.tmpdir(),
    cpuCores: os.cpus().length,
    totalMemory: os.totalmem(),
    freeMemory: os.freemem(),
  };
}

/**
 * Get the config directory path for the current platform
 */
export function getConfigDir(): string {
  const home = os.homedir();

  if (isWindows()) {
    return process.env['APPDATA'] || path.join(home, 'AppData', 'Roaming');
  }

  if (isMacOS()) {
    return path.join(home, 'Library', 'Application Support');
  }

  // Linux and others follow XDG spec
  return process.env['XDG_CONFIG_HOME'] || path.join(home, '.config');
}

/**
 * Get the data directory path for the current platform
 */
export function getDataDir(): string {
  const home = os.homedir();

  if (isWindows()) {
    return process.env['LOCALAPPDATA'] || path.join(home, 'AppData', 'Local');
  }

  if (isMacOS()) {
    return path.join(home, 'Library', 'Application Support');
  }

  // Linux and others follow XDG spec
  return process.env['XDG_DATA_HOME'] || path.join(home, '.local', 'share');
}

/**
 * Get the cache directory path for the current platform
 */
export function getCacheDir(): string {
  const home = os.homedir();

  if (isWindows()) {
    return process.env['LOCALAPPDATA'] || path.join(home, 'AppData', 'Local');
  }

  if (isMacOS()) {
    return path.join(home, 'Library', 'Caches');
  }

  // Linux and others follow XDG spec
  return process.env['XDG_CACHE_HOME'] || path.join(home, '.cache');
}

/**
 * Check if sox is installed
 */
export function checkSoxInstalled(): { installed: boolean; version?: string } {
  try {
    const result = execSync('sox --version', { encoding: 'utf-8', stdio: ['pipe', 'pipe', 'pipe'] });
    const match = result.match(/SoX v?([\d.]+)/);
    return {
      installed: true,
      version: match?.[1],
    };
  } catch {
    return { installed: false };
  }
}

/**
 * Check if a command exists
 */
export function commandExists(command: string): boolean {
  try {
    const checkCommand = isWindows() ? `where ${command}` : `which ${command}`;
    execSync(checkCommand, { encoding: 'utf-8', stdio: ['pipe', 'pipe', 'pipe'] });
    return true;
  } catch {
    return false;
  }
}

/**
 * Get audio backend for the current platform
 */
export function getAudioBackend(): AudioBackend {
  if (isWindows()) {
    return 'wasapi';
  }

  if (isMacOS()) {
    return 'coreaudio';
  }

  // Linux: check for PulseAudio first, then ALSA
  if (commandExists('pactl')) {
    return 'pulseaudio';
  }

  if (commandExists('arecord')) {
    return 'alsa';
  }

  return 'unknown';
}

/**
 * List audio input devices
 */
export async function listInputDevices(): Promise<AudioDeviceInfo[]> {
  const platform = getPlatform();
  const devices: AudioDeviceInfo[] = [];

  try {
    if (platform === 'linux') {
      // Try PulseAudio first
      if (commandExists('pactl')) {
        const { stdout } = await execAsync('pactl list sources short');
        const lines = stdout.trim().split('\n');
        for (const line of lines) {
          const parts = line.split('\t');
          if (parts.length >= 2) {
            devices.push({
              id: parts[1] ?? parts[0] ?? 'unknown',
              name: parts[1] ?? 'Unknown Device',
              isDefault: parts[1]?.includes('default') ?? false,
            });
          }
        }
      } else if (commandExists('arecord')) {
        // Fall back to ALSA
        const { stdout } = await execAsync('arecord -l');
        const matches = stdout.matchAll(/card (\d+):.*\[(.+?)\]/g);
        for (const match of matches) {
          devices.push({
            id: `hw:${match[1]}`,
            name: match[2] ?? 'Unknown',
            isDefault: match[1] === '0',
          });
        }
      }
    } else if (platform === 'darwin') {
      // macOS: Use system_profiler
      try {
        const { stdout } = await execAsync('system_profiler SPAudioDataType -json');
        const data = JSON.parse(stdout);
        const audioData = data?.SPAudioDataType?.[0];
        if (audioData?._items) {
          for (const item of audioData._items) {
            if (item.coreaudio_default_audio_input_device === 'yes') {
              devices.push({
                id: item._name,
                name: item._name,
                isDefault: true,
              });
            }
          }
        }
      } catch {
        // Fallback
        devices.push({
          id: 'default',
          name: 'Default Input',
          isDefault: true,
        });
      }
    } else if (platform === 'win32') {
      // Windows: Would use PowerShell, simplified for now
      devices.push({
        id: 'default',
        name: 'Default Microphone',
        isDefault: true,
      });
    }
  } catch {
    // Return default device on error
    devices.push({
      id: 'default',
      name: 'Default Input',
      isDefault: true,
    });
  }

  return devices;
}

/**
 * List audio output devices
 */
export async function listOutputDevices(): Promise<AudioDeviceInfo[]> {
  const platform = getPlatform();
  const devices: AudioDeviceInfo[] = [];

  try {
    if (platform === 'linux') {
      if (commandExists('pactl')) {
        const { stdout } = await execAsync('pactl list sinks short');
        const lines = stdout.trim().split('\n');
        for (const line of lines) {
          const parts = line.split('\t');
          if (parts.length >= 2) {
            devices.push({
              id: parts[1] ?? parts[0] ?? 'unknown',
              name: parts[1] ?? 'Unknown Device',
              isDefault: parts[1]?.includes('default') ?? false,
            });
          }
        }
      }
    } else {
      // Simplified for other platforms
      devices.push({
        id: 'default',
        name: 'Default Output',
        isDefault: true,
      });
    }
  } catch {
    devices.push({
      id: 'default',
      name: 'Default Output',
      isDefault: true,
    });
  }

  return devices;
}

/**
 * Get audio system information
 */
export async function getAudioSystemInfo(): Promise<AudioSystemInfo> {
  const soxCheck = checkSoxInstalled();
  const inputDevices = await listInputDevices();
  const outputDevices = await listOutputDevices();

  return {
    backend: getAudioBackend(),
    soxInstalled: soxCheck.installed,
    soxVersion: soxCheck.version,
    inputDevices,
    outputDevices,
    defaultInput: inputDevices.find(d => d.isDefault),
    defaultOutput: outputDevices.find(d => d.isDefault),
  };
}

/**
 * Check all dependencies
 */
export async function checkDependencies(): Promise<DependencyCheck[]> {
  const checks: DependencyCheck[] = [];

  // Node.js version
  const nodeVersion = process.version.slice(1);
  const nodeMajor = parseInt(nodeVersion.split('.')[0] ?? '0', 10);
  checks.push({
    name: 'Node.js',
    required: true,
    installed: nodeMajor >= 18,
    version: nodeVersion,
    suggestion: nodeMajor < 18 ? 'Upgrade to Node.js 18 or later' : undefined,
  });

  // sox for audio recording
  const sox = checkSoxInstalled();
  checks.push({
    name: 'sox',
    required: true,
    installed: sox.installed,
    version: sox.version,
    suggestion: !sox.installed
      ? isLinux()
        ? 'Install with: sudo apt install sox libsox-fmt-all'
        : isMacOS()
          ? 'Install with: brew install sox'
          : 'Download from https://sox.sourceforge.net/'
      : undefined,
  });

  // Platform-specific audio dependencies
  if (isLinux()) {
    const hasPulseAudio = commandExists('pactl');
    const hasAlsa = commandExists('arecord');
    checks.push({
      name: 'PulseAudio/ALSA',
      required: true,
      installed: hasPulseAudio || hasAlsa,
      suggestion: !hasPulseAudio && !hasAlsa
        ? 'Install PulseAudio or ALSA audio system'
        : undefined,
    });
  }

  return checks;
}

/**
 * Get installation instructions for missing dependencies
 */
export function getInstallInstructions(): string {
  const platform = getPlatform();
  const lines: string[] = [];

  lines.push('To use WaaV CLI voice features, you need to install audio dependencies:\n');

  if (platform === 'linux') {
    lines.push('Ubuntu/Debian:');
    lines.push('  sudo apt update');
    lines.push('  sudo apt install sox libsox-fmt-all pulseaudio');
    lines.push('');
    lines.push('Fedora:');
    lines.push('  sudo dnf install sox pulseaudio');
    lines.push('');
    lines.push('Arch Linux:');
    lines.push('  sudo pacman -S sox pulseaudio');
  } else if (platform === 'darwin') {
    lines.push('macOS (using Homebrew):');
    lines.push('  brew install sox');
  } else if (platform === 'win32') {
    lines.push('Windows:');
    lines.push('  1. Download sox from https://sox.sourceforge.net/');
    lines.push('  2. Add sox to your PATH environment variable');
  }

  return lines.join('\n');
}

/**
 * Format bytes as human-readable string
 */
export function formatBytes(bytes: number): string {
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let i = 0;
  while (bytes >= 1024 && i < units.length - 1) {
    bytes /= 1024;
    i++;
  }
  return `${bytes.toFixed(1)} ${units[i]}`;
}

/**
 * Detect the Linux distribution
 */
export function detectLinuxDistro(): 'debian' | 'fedora' | 'arch' | 'unknown' {
  if (!isLinux()) {
    return 'unknown';
  }

  try {
    // Check /etc/os-release first
    const osRelease = execSync('cat /etc/os-release 2>/dev/null', { encoding: 'utf-8' });

    if (osRelease.includes('ID=ubuntu') || osRelease.includes('ID=debian') ||
        osRelease.includes('ID_LIKE=debian') || osRelease.includes('ID=linuxmint') ||
        osRelease.includes('ID=pop')) {
      return 'debian';
    }

    if (osRelease.includes('ID=fedora') || osRelease.includes('ID=rhel') ||
        osRelease.includes('ID=centos') || osRelease.includes('ID_LIKE=fedora')) {
      return 'fedora';
    }

    if (osRelease.includes('ID=arch') || osRelease.includes('ID_LIKE=arch') ||
        osRelease.includes('ID=manjaro') || osRelease.includes('ID=endeavouros')) {
      return 'arch';
    }
  } catch {
    // Fallback detection
    if (commandExists('apt')) {
      return 'debian';
    }
    if (commandExists('dnf') || commandExists('yum')) {
      return 'fedora';
    }
    if (commandExists('pacman')) {
      return 'arch';
    }
  }

  return 'unknown';
}

/**
 * Detect the package manager
 */
export function detectPackageManager(): 'apt' | 'dnf' | 'yum' | 'pacman' | 'brew' | 'choco' | 'unknown' {
  if (isLinux()) {
    const distro = detectLinuxDistro();
    switch (distro) {
      case 'debian':
        return 'apt';
      case 'fedora':
        return commandExists('dnf') ? 'dnf' : 'yum';
      case 'arch':
        return 'pacman';
    }
  }

  if (isMacOS()) {
    if (commandExists('brew')) {
      return 'brew';
    }
  }

  if (isWindows()) {
    if (commandExists('choco')) {
      return 'choco';
    }
  }

  return 'unknown';
}

/**
 * Install result
 */
export interface InstallResult {
  success: boolean;
  message: string;
  error?: string;
}

/**
 * Install missing dependencies automatically
 */
export async function installDependencies(
  options: {
    onProgress?: (message: string) => void;
    sudoPassword?: string;
  } = {}
): Promise<InstallResult> {
  const { onProgress = console.log } = options;
  const packageManager = detectPackageManager();

  // Check what's missing
  const sox = checkSoxInstalled();

  if (sox.installed) {
    return { success: true, message: 'All dependencies already installed' };
  }

  onProgress('Detecting package manager...');

  if (packageManager === 'unknown') {
    return {
      success: false,
      message: 'Could not detect package manager',
      error: 'Please install sox manually using your system\'s package manager'
    };
  }

  onProgress(`Using package manager: ${packageManager}`);

  // Build the install command
  let installCommand = '';

  switch (packageManager) {
    case 'apt':
      installCommand = 'sudo apt-get update && sudo apt-get install -y sox libsox-fmt-all';
      break;
    case 'dnf':
      installCommand = 'sudo dnf install -y sox';
      break;
    case 'yum':
      installCommand = 'sudo yum install -y sox';
      break;
    case 'pacman':
      installCommand = 'sudo pacman -S --noconfirm sox';
      break;
    case 'brew':
      installCommand = 'brew install sox';
      break;
    case 'choco':
      installCommand = 'choco install sox -y';
      break;
    default:
      return {
        success: false,
        message: 'Unsupported package manager',
        error: 'Please install sox manually'
      };
  }

  onProgress(`Installing dependencies with: ${installCommand}`);

  try {
    // Execute the install command
    await execAsync(installCommand, {
      timeout: 300000, // 5 minute timeout
      maxBuffer: 10 * 1024 * 1024, // 10MB buffer
    });

    // Verify installation
    const verifyCheck = checkSoxInstalled();
    if (verifyCheck.installed) {
      return {
        success: true,
        message: `Successfully installed sox ${verifyCheck.version || ''}`
      };
    } else {
      return {
        success: false,
        message: 'Installation completed but verification failed',
        error: 'sox command not found after installation'
      };
    }
  } catch (error) {
    const errorMessage = error instanceof Error ? error.message : String(error);

    // Check if it's a permission error
    if (errorMessage.includes('permission denied') ||
        errorMessage.includes('sudo') ||
        errorMessage.includes('not permitted')) {
      return {
        success: false,
        message: 'Permission denied',
        error: `Please run with sudo or enter your password:\n${installCommand}`
      };
    }

    return {
      success: false,
      message: 'Installation failed',
      error: errorMessage
    };
  }
}

/**
 * Check and install dependencies if missing
 * Returns true if all dependencies are available after the call
 */
export async function ensureDependencies(
  options: {
    autoInstall?: boolean;
    onProgress?: (message: string) => void;
    onError?: (error: string) => void;
  } = {}
): Promise<boolean> {
  const { autoInstall = true, onProgress = console.log, onError = console.error } = options;

  // Check current state
  const deps = await checkDependencies();
  const missing = deps.filter(d => d.required && !d.installed);

  if (missing.length === 0) {
    return true;
  }

  onProgress(`Missing dependencies: ${missing.map(d => d.name).join(', ')}`);

  if (!autoInstall) {
    onError(getInstallInstructions());
    return false;
  }

  // Try to install
  const result = await installDependencies({ onProgress });

  if (result.success) {
    onProgress(result.message);
    return true;
  } else {
    onError(result.error || result.message);
    return false;
  }
}

/**
 * Format duration as human-readable string
 */
export function formatDuration(ms: number): string {
  if (ms < 1000) {
    return `${ms}ms`;
  }
  if (ms < 60000) {
    return `${(ms / 1000).toFixed(1)}s`;
  }
  if (ms < 3600000) {
    const minutes = Math.floor(ms / 60000);
    const seconds = Math.floor((ms % 60000) / 1000);
    return `${minutes}m ${seconds}s`;
  }
  const hours = Math.floor(ms / 3600000);
  const minutes = Math.floor((ms % 3600000) / 60000);
  return `${hours}h ${minutes}m`;
}
