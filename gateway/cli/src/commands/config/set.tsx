/**
 * WaaV CLI Config Set Command
 *
 * Set a configuration value
 */

import { configManager } from '../../core/config/manager.js';
import { logger } from '../../utils/logger.js';
import type { GlobalOptions } from '../../cli.js';

/**
 * Config set command handler
 */
export async function configSetCommand(key: string, value: string, options: GlobalOptions): Promise<void> {
  // Initialize config
  await configManager.initialize({
    configPath: options.config,
    profile: options.profile,
  });

  try {
    // Parse value (try JSON first, then use as string)
    let parsedValue: unknown = value;
    try {
      parsedValue = JSON.parse(value);
    } catch {
      // Value is a plain string
      // Check for boolean-like strings
      if (value.toLowerCase() === 'true') {
        parsedValue = true;
      } else if (value.toLowerCase() === 'false') {
        parsedValue = false;
      } else if (!isNaN(Number(value)) && value !== '') {
        parsedValue = Number(value);
      }
    }

    // Set the value
    configManager.setValue(key, parsedValue);

    // Save configuration
    await configManager.save();

    if (options.json) {
      console.log(JSON.stringify({ success: true, key, value: parsedValue }));
    } else {
      logger.success(`Set ${key} = ${JSON.stringify(parsedValue)}`);
    }
  } catch (error) {
    if (options.json) {
      console.log(JSON.stringify({
        success: false,
        error: error instanceof Error ? error.message : String(error),
      }));
    } else {
      logger.error(`Failed to set configuration: ${error instanceof Error ? error.message : String(error)}`);
    }
    process.exit(1);
  }
}

export default configSetCommand;
