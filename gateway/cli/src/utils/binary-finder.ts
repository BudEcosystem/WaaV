/**
 * WaaV CLI Binary Finder
 *
 * Utilities for locating the WaaV Gateway binary across different
 * installation methods (cargo build, npm package, system install).
 */

import { existsSync, statSync } from 'node:fs';
import { access, constants } from 'node:fs/promises';
import { exec } from 'node:child_process';
import { promisify } from 'node:util';
import { join, dirname, resolve } from 'node:path';
import { homedir, platform } from 'node:os';

const execAsync = promisify(exec);

/**
 * Binary search result
 */
export interface BinarySearchResult {
  path: string;
  source: 'cargo-debug' | 'cargo-release' | 'system' | 'local' | 'npm' | 'docker';
  version?: string;
}

/**
 * Check if a file exists and is executable
 */
async function isExecutable(path: string): Promise<boolean> {
  try {
    if (!existsSync(path)) {
      return false;
    }
    const stats = statSync(path);
    if (!stats.isFile()) {
      return false;
    }
    // On Windows, check file extension
    if (platform() === 'win32') {
      return path.endsWith('.exe');
    }
    // On Unix, check execute permission
    await access(path, constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

/**
 * Get version from binary
 */
async function getVersion(binaryPath: string): Promise<string | undefined> {
  try {
    const { stdout } = await execAsync(`"${binaryPath}" --version`, { timeout: 5000 });
    const match = stdout.match(/waav[- ]gateway[- ]?v?(\d+\.\d+\.\d+)/i);
    return match?.[1];
  } catch {
    return undefined;
  }
}

/**
 * Find the project root directory by looking for Cargo.toml
 */
async function findProjectRoot(startDir: string = process.cwd()): Promise<string | null> {
  let currentDir = resolve(startDir);
  const root = platform() === 'win32' ? currentDir.split(':')[0] + ':\\' : '/';

  while (currentDir !== root) {
    const cargoPath = join(currentDir, 'gateway', 'Cargo.toml');
    const cargoPathDirect = join(currentDir, 'Cargo.toml');

    if (existsSync(cargoPath) || existsSync(cargoPathDirect)) {
      return existsSync(cargoPath) ? join(currentDir, 'gateway') : currentDir;
    }

    currentDir = dirname(currentDir);
  }

  return null;
}

/**
 * Search locations for the gateway binary
 */
export async function findGatewayBinary(): Promise<BinarySearchResult | null> {
  const binaryName = platform() === 'win32' ? 'waav-gateway.exe' : 'waav-gateway';
  const candidates: Array<{ path: string; source: BinarySearchResult['source'] }> = [];

  // 1. Check for cargo debug build (development)
  const projectRoot = await findProjectRoot();
  if (projectRoot) {
    candidates.push(
      { path: join(projectRoot, 'target', 'debug', binaryName), source: 'cargo-debug' },
      { path: join(projectRoot, 'target', 'release', binaryName), source: 'cargo-release' },
    );
  }

  // 2. Check WaaV specific paths
  const waavDir = join(homedir(), '.waav');
  candidates.push(
    { path: join(waavDir, 'bin', binaryName), source: 'local' },
  );

  // 3. Check common installation paths
  if (platform() !== 'win32') {
    candidates.push(
      { path: '/usr/local/bin/waav-gateway', source: 'system' },
      { path: '/usr/bin/waav-gateway', source: 'system' },
      { path: join(homedir(), '.local', 'bin', 'waav-gateway'), source: 'local' },
      { path: join(homedir(), '.cargo', 'bin', 'waav-gateway'), source: 'cargo-release' },
    );
  } else {
    // Windows paths
    const appData = process.env['APPDATA'] ?? join(homedir(), 'AppData', 'Roaming');
    candidates.push(
      { path: join(appData, 'waav', 'bin', binaryName), source: 'local' },
      { path: join(homedir(), '.cargo', 'bin', binaryName), source: 'cargo-release' },
    );
  }

  // 4. Check PATH using 'which' or 'where'
  try {
    const cmd = platform() === 'win32' ? 'where waav-gateway' : 'which waav-gateway';
    const { stdout } = await execAsync(cmd, { timeout: 5000 });
    const pathFromWhich = stdout.trim().split('\n')[0];
    if (pathFromWhich) {
      candidates.unshift({ path: pathFromWhich, source: 'system' });
    }
  } catch {
    // Binary not in PATH
  }

  // Check each candidate
  for (const candidate of candidates) {
    if (await isExecutable(candidate.path)) {
      const version = await getVersion(candidate.path);
      return {
        path: candidate.path,
        source: candidate.source,
        version,
      };
    }
  }

  return null;
}

/**
 * Check if Docker is available and has waav-gateway image
 */
export async function findDockerImage(): Promise<boolean> {
  try {
    const { stdout } = await execAsync('docker images waav-gateway --format "{{.Repository}}"', {
      timeout: 10000,
    });
    return stdout.trim().includes('waav-gateway');
  } catch {
    return false;
  }
}

/**
 * Get the recommended way to run the gateway based on available options
 */
export async function getRecommendedRunMethod(): Promise<{
  method: 'binary' | 'cargo' | 'docker' | 'none';
  command: string;
  description: string;
}> {
  // Check for binary
  const binary = await findGatewayBinary();
  if (binary) {
    return {
      method: 'binary',
      command: binary.path,
      description: `Found ${binary.source} binary${binary.version ? ` v${binary.version}` : ''}`,
    };
  }

  // Check for cargo project
  const projectRoot = await findProjectRoot();
  if (projectRoot) {
    const cargoToml = join(projectRoot, 'Cargo.toml');
    if (existsSync(cargoToml)) {
      return {
        method: 'cargo',
        command: `cd "${projectRoot}" && cargo run --release`,
        description: 'Found Cargo project',
      };
    }
  }

  // Check for Docker image
  const hasDocker = await findDockerImage();
  if (hasDocker) {
    return {
      method: 'docker',
      command: 'docker run -p 3001:3001 waav-gateway',
      description: 'Found Docker image',
    };
  }

  return {
    method: 'none',
    command: '',
    description: 'No gateway installation found',
  };
}

export default findGatewayBinary;
