/**
 * WaaV CLI Provider Test Command
 */

import { logger } from '../../utils/logger.js';
import { configManager } from '../../core/config/manager.js';
import { GatewayClient } from '../../core/gateway/client.js';
import type { GlobalOptions } from '../../cli.js';

export async function providerTestCommand(provider: string, options: GlobalOptions): Promise<void> {
  await configManager.initialize({
    configPath: options.config,
    profile: options.profile,
  });

  const config = configManager.get();
  const client = new GatewayClient({ baseUrl: options.gateway ?? config.gateway.url });

  logger.info(`Testing ${provider}...`);

  try {
    const result = await client.testProvider('tts', provider);
    if (result.success) {
      logger.success(`${provider} is working! Latency: ${result.latency_ms}ms`);
    } else {
      logger.error(`${provider} test failed: ${result.error}`);
    }
  } catch (err) {
    logger.error(`Test failed: ${err instanceof Error ? err.message : String(err)}`);
  }
}

export default providerTestCommand;
