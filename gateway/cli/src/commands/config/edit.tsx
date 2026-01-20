/**
 * WaaV CLI Config Edit Command
 *
 * Open configuration in editor
 */

import { spawn } from 'child_process';
import { configManager } from '../../core/config/manager.js';
import { logger } from '../../utils/logger.js';
import { getPlatform } from '../../utils/platform.js';
import type { GlobalOptions } from '../../cli.js';

/**
 * Get the default editor for the platform
 */
function getDefaultEditor(): string {
  // Check EDITOR environment variable first
  if (process.env['EDITOR']) {
    return process.env['EDITOR'];
  }

  // Check VISUAL environment variable
  if (process.env['VISUAL']) {
    return process.env['VISUAL'];
  }

  // Platform-specific defaults
  const platform = getPlatform();

  if (platform === 'win32') {
    return 'notepad';
  }

  // Try common editors on Unix-like systems
  return 'nano';
}

/**
 * Config edit command handler
 */
export async function configEditCommand(options: GlobalOptions): Promise<void> {
  // Initialize config
  await configManager.initialize({
    configPath: options.config,
    profile: options.profile,
  });

  const configPath = configManager.getConfigPath();
  const editor = getDefaultEditor();

  logger.info(`Opening ${configPath} in ${editor}...`);

  // Spawn editor
  const child = spawn(editor, [configPath], {
    stdio: 'inherit',
    shell: true,
  });

  // Wait for editor to close
  await new Promise<void>((resolve, reject) => {
    child.on('error', (error) => {
      logger.error(`Failed to open editor: ${error.message}`);
      reject(error);
    });

    child.on('close', (code) => {
      if (code === 0) {
        logger.success('Configuration saved');
        resolve();
      } else {
        logger.warn(`Editor exited with code ${code}`);
        resolve();
      }
    });
  });

  // Reload and validate configuration
  try {
    await configManager.load();
    const validation = configManager.validate();

    if (!validation.valid) {
      logger.warn('Configuration has validation warnings:');
      for (const error of validation.errors) {
        logger.warn(`  - ${error}`);
      }
    }
  } catch (error) {
    logger.error(`Configuration error: ${error instanceof Error ? error.message : String(error)}`);
  }
}

export default configEditCommand;
