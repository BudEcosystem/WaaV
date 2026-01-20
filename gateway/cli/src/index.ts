#!/usr/bin/env node
/**
 * WaaV CLI Entry Point
 *
 * @waav/cli - Beautiful interactive CLI for WaaV Voice AI Gateway
 *
 * Usage:
 *   npx @waav/cli [command] [options]
 *   waav [command] [options]
 *
 * Commands:
 *   init                  First-time setup wizard
 *   config show|set|edit  Configuration management
 *   provider list|add|test|browse  Provider management
 *   dag list|create|validate|visualize  DAG pipeline management
 *   server start|stop|status|logs  Gateway server control
 *   voice start|test|configure  Voice interaction mode
 *   dashboard             Interactive analytics dashboard
 *   doctor                Diagnose issues
 *
 * Global Options:
 *   -c, --config <path>   Config file path
 *   -p, --profile <name>  Configuration profile
 *   -g, --gateway <url>   Gateway URL override
 *   --json                JSON output mode
 *   -v, --verbose         Verbose logging
 *   -q, --quiet           Suppress output
 *   --no-color            Disable colors
 *   --no-animation        Disable animations
 *   -V, --version         Output version number
 *   -h, --help            Display help
 */

import { main } from './cli.js';

// Run the CLI
main().catch((error) => {
  console.error('Fatal error:', error);
  process.exit(1);
});
