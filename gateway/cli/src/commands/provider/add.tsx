/**
 * WaaV CLI Provider Add Command
 */

import { logger } from '../../utils/logger.js';
import { configManager } from '../../core/config/manager.js';
import { PROVIDER_ENV_VARS } from '../../core/config/defaults.js';
import type { GlobalOptions } from '../../cli.js';

export async function providerAddCommand(provider: string, options: GlobalOptions): Promise<void> {
  await configManager.initialize({
    configPath: options.config,
    profile: options.profile,
  });

  // Check if provider exists in known providers
  const envVar = PROVIDER_ENV_VARS[provider]?.api_key;
  const envValue = envVar ? process.env[envVar] : undefined;

  if (envValue) {
    configManager.setProvider(provider, { api_key: envValue });
    await configManager.save();
    logger.success(`Added provider ${provider} using ${envVar} environment variable`);
  } else {
    logger.info(`To add ${provider}, set the ${envVar || `${provider.toUpperCase()}_API_KEY`} environment variable`);
    logger.info('Or run "waav init" for interactive setup');
  }
}

export default providerAddCommand;
