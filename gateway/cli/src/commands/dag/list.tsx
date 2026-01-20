/**
 * WaaV CLI DAG List Command
 */

import { logger } from '../../utils/logger.js';
import { configManager } from '../../core/config/manager.js';
import type { GlobalOptions } from '../../cli.js';

export async function dagListCommand(options: GlobalOptions): Promise<void> {
  await configManager.initialize({
    configPath: options.config,
    profile: options.profile,
  });

  const config = configManager.get();
  const pipelines = Object.keys(config.pipelines);

  if (pipelines.length === 0) {
    logger.info('No DAG pipelines configured');
    logger.info('Use "waav dag create" to create a new pipeline');
  } else {
    logger.info('DAG Pipelines:');
    for (const name of pipelines) {
      const pipeline = config.pipelines[name];
      logger.plain(`  - ${name}${pipeline?.description ? `: ${pipeline.description}` : ''}`);
    }
  }
}

export default dagListCommand;
