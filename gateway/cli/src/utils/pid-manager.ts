/**
 * WaaV CLI PID Manager
 *
 * Utilities for managing the gateway server process lifecycle
 * including PID file management, process health checks, and signals.
 */

import { existsSync, mkdirSync, readFileSync, writeFileSync, unlinkSync } from 'node:fs';
import { join } from 'node:path';
import { homedir, platform } from 'node:os';
import { exec } from 'node:child_process';
import { promisify } from 'node:util';

const execAsync = promisify(exec);

/**
 * PID file location
 */
function getPidDir(): string {
  if (platform() === 'win32') {
    const appData = process.env['APPDATA'] ?? join(homedir(), 'AppData', 'Roaming');
    return join(appData, 'waav');
  }
  return join(homedir(), '.waav');
}

function getPidFilePath(): string {
  return join(getPidDir(), 'gateway.pid');
}

function getLogFilePath(): string {
  return join(getPidDir(), 'logs', 'gateway.log');
}

/**
 * Ensure the PID directory exists
 */
function ensurePidDir(): void {
  const dir = getPidDir();
  if (!existsSync(dir)) {
    mkdirSync(dir, { recursive: true });
  }
  const logsDir = join(dir, 'logs');
  if (!existsSync(logsDir)) {
    mkdirSync(logsDir, { recursive: true });
  }
}

/**
 * PID file data structure
 */
export interface PidData {
  pid: number;
  startTime: number;
  binaryPath: string;
  configPath?: string;
  port: number;
}

/**
 * Write PID file
 */
export function writePidFile(data: PidData): void {
  ensurePidDir();
  const pidPath = getPidFilePath();
  writeFileSync(pidPath, JSON.stringify(data, null, 2), 'utf-8');
}

/**
 * Read PID file
 */
export function readPidFile(): PidData | null {
  const pidPath = getPidFilePath();
  if (!existsSync(pidPath)) {
    return null;
  }

  try {
    const content = readFileSync(pidPath, 'utf-8');
    return JSON.parse(content) as PidData;
  } catch {
    return null;
  }
}

/**
 * Remove PID file
 */
export function removePidFile(): boolean {
  const pidPath = getPidFilePath();
  if (existsSync(pidPath)) {
    try {
      unlinkSync(pidPath);
      return true;
    } catch {
      return false;
    }
  }
  return true;
}

/**
 * Check if a process is running by PID
 */
export async function isProcessRunning(pid: number): Promise<boolean> {
  try {
    if (platform() === 'win32') {
      const { stdout } = await execAsync(`tasklist /FI "PID eq ${pid}" /NH`, { timeout: 5000 });
      return stdout.includes(String(pid));
    } else {
      // Send signal 0 to check if process exists
      process.kill(pid, 0);
      return true;
    }
  } catch {
    return false;
  }
}

/**
 * Get process info by PID
 */
export interface ProcessInfo {
  pid: number;
  command: string;
  cpuPercent?: number;
  memoryMb?: number;
  startTime?: Date;
}

export async function getProcessInfo(pid: number): Promise<ProcessInfo | null> {
  try {
    if (platform() === 'win32') {
      const { stdout } = await execAsync(
        `wmic process where ProcessId=${pid} get CommandLine,WorkingSetSize /format:list`,
        { timeout: 5000 }
      );
      const lines = stdout.split('\n').filter(l => l.trim());
      const cmdLine = lines.find(l => l.startsWith('CommandLine='))?.split('=')[1] ?? '';
      const memBytes = parseInt(lines.find(l => l.startsWith('WorkingSetSize='))?.split('=')[1] ?? '0', 10);

      return {
        pid,
        command: cmdLine,
        memoryMb: Math.round(memBytes / (1024 * 1024)),
      };
    } else {
      const { stdout } = await execAsync(
        `ps -p ${pid} -o pid=,command=,%cpu=,%mem=,lstart=`,
        { timeout: 5000 }
      );
      const parts = stdout.trim().split(/\s+/);
      if (parts.length < 2) {
        return null;
      }

      return {
        pid,
        command: parts.slice(1, -4).join(' '),
        cpuPercent: parseFloat(parts[parts.length - 4] ?? '0'),
        memoryMb: parseFloat(parts[parts.length - 3] ?? '0'),
      };
    }
  } catch {
    return null;
  }
}

/**
 * Send signal to process
 */
export type ProcessSignal = 'SIGTERM' | 'SIGKILL' | 'SIGINT' | 'SIGHUP';

export async function sendSignal(pid: number, signal: ProcessSignal): Promise<boolean> {
  try {
    if (platform() === 'win32') {
      // Windows doesn't support Unix signals, use taskkill
      const force = signal === 'SIGKILL' ? '/F' : '';
      await execAsync(`taskkill /PID ${pid} ${force}`, { timeout: 10000 });
      return true;
    } else {
      const signalMap: Record<ProcessSignal, number> = {
        'SIGTERM': 15,
        'SIGKILL': 9,
        'SIGINT': 2,
        'SIGHUP': 1,
      };
      process.kill(pid, signalMap[signal]);
      return true;
    }
  } catch {
    return false;
  }
}

/**
 * Stop the gateway process gracefully
 */
export async function stopGatewayProcess(timeoutMs: number = 10000): Promise<{
  success: boolean;
  message: string;
}> {
  const pidData = readPidFile();
  if (!pidData) {
    return { success: false, message: 'No PID file found. Is the gateway running?' };
  }

  const isRunning = await isProcessRunning(pidData.pid);
  if (!isRunning) {
    removePidFile();
    return { success: true, message: 'Gateway process not running (stale PID file removed)' };
  }

  // Try graceful shutdown first (SIGTERM)
  const termSent = await sendSignal(pidData.pid, 'SIGTERM');
  if (!termSent) {
    return { success: false, message: 'Failed to send SIGTERM signal' };
  }

  // Wait for process to exit
  const startTime = Date.now();
  while (Date.now() - startTime < timeoutMs) {
    const stillRunning = await isProcessRunning(pidData.pid);
    if (!stillRunning) {
      removePidFile();
      return { success: true, message: 'Gateway stopped gracefully' };
    }
    await new Promise(resolve => setTimeout(resolve, 500));
  }

  // Force kill if still running
  const killSent = await sendSignal(pidData.pid, 'SIGKILL');
  if (!killSent) {
    return { success: false, message: 'Failed to force kill process' };
  }

  // Wait a bit more
  await new Promise(resolve => setTimeout(resolve, 1000));

  const finalCheck = await isProcessRunning(pidData.pid);
  if (!finalCheck) {
    removePidFile();
    return { success: true, message: 'Gateway force stopped' };
  }

  return { success: false, message: 'Failed to stop gateway process' };
}

/**
 * Get gateway status
 */
export interface GatewayStatus {
  running: boolean;
  pid?: number;
  port?: number;
  uptime?: number;
  binaryPath?: string;
  processInfo?: ProcessInfo;
}

export async function getGatewayStatus(): Promise<GatewayStatus> {
  const pidData = readPidFile();
  if (!pidData) {
    return { running: false };
  }

  const isRunning = await isProcessRunning(pidData.pid);
  if (!isRunning) {
    removePidFile();
    return { running: false };
  }

  const processInfo = await getProcessInfo(pidData.pid);
  const uptimeMs = Date.now() - pidData.startTime;

  return {
    running: true,
    pid: pidData.pid,
    port: pidData.port,
    uptime: uptimeMs,
    binaryPath: pidData.binaryPath,
    processInfo: processInfo ?? undefined,
  };
}

/**
 * Get log file path
 */
export function getLogPath(): string {
  ensurePidDir();
  return getLogFilePath();
}

/**
 * Get the waav data directory
 */
export function getWaavDataDir(): string {
  ensurePidDir();
  return getPidDir();
}

export default {
  writePidFile,
  readPidFile,
  removePidFile,
  isProcessRunning,
  getProcessInfo,
  sendSignal,
  stopGatewayProcess,
  getGatewayStatus,
  getLogPath,
  getWaavDataDir,
};
