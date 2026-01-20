/**
 * WaaV CLI Main Application
 *
 * Root component and Commander command setup.
 * Uses SPA architecture with AppShell for seamless navigation.
 */

import React from 'react';
import { render } from 'ink';
import { Command } from 'commander';
import { logger } from './utils/logger.js';
import { AppShell } from './layout/AppShell.js';
import { SCREEN_REGISTRY } from './screens/index.js';
import type { ScreenName } from './types/index.js';

/**
 * CLI version
 */
export const CLI_VERSION = '1.0.0';

/**
 * Global CLI options
 */
export interface GlobalOptions {
  config?: string;
  profile?: string;
  gateway?: string;
  json?: boolean;
  verbose?: boolean;
  quiet?: boolean;
  noColor?: boolean;
  noAnimation?: boolean;
}

/**
 * App props for the main application
 */
interface AppProps {
  initialScreen?: ScreenName;
  options?: GlobalOptions;
  showSplash?: boolean;
}

/**
 * Main App Component
 *
 * This is now a thin wrapper around AppShell that provides
 * the screen registry and initial configuration.
 */
export const App: React.FC<AppProps> = ({
  initialScreen = 'main_menu',
  options = {},
  showSplash = false,
}) => {
  return (
    <AppShell
      initialScreen={initialScreen}
      options={options}
      showSplash={showSplash}
      version={CLI_VERSION}
      screenRegistry={SCREEN_REGISTRY}
      showHeader={true}
      showFooter={true}
      compact={false}
      gatewayRefreshInterval={5000}
    />
  );
};

/**
 * Create the CLI program with Commander
 */
export function createProgram(): Command {
  const program = new Command();

  program
    .name('waav')
    .description('WaaV Voice AI Gateway CLI - 70+ providers, DAG routing, LiveKit integration')
    .version(CLI_VERSION, '-V, --version', 'Output the version number')
    .option('-c, --config <path>', 'Config file path')
    .option('-p, --profile <name>', 'Configuration profile')
    .option('-g, --gateway <url>', 'Gateway URL override')
    .option('--json', 'JSON output mode')
    .option('-v, --verbose', 'Verbose logging')
    .option('-q, --quiet', 'Suppress output')
    .option('--no-color', 'Disable colors')
    .option('--no-animation', 'Disable animations');

  // Init command
  program
    .command('init')
    .description('First-time setup wizard')
    .action(async () => {
      const opts = program.opts() as GlobalOptions;
      // Launch app at setup wizard screen
      renderApp(opts, false, 'setup_wizard');
    });

  // Config command group
  const configCmd = program
    .command('config')
    .description('Configuration management');

  configCmd
    .command('show')
    .description('Display current configuration')
    .action(async () => {
      const opts = program.opts() as GlobalOptions;
      renderApp(opts, false, 'config_show');
    });

  configCmd
    .command('set <key> <value>')
    .description('Set configuration value')
    .action(async (key: string, value: string) => {
      const { configSetCommand } = await import('./commands/config/set.js');
      await configSetCommand(key, value, program.opts() as GlobalOptions);
    });

  configCmd
    .command('edit')
    .description('Open config in editor')
    .action(async () => {
      const { configEditCommand } = await import('./commands/config/edit.js');
      await configEditCommand(program.opts() as GlobalOptions);
    });

  configCmd
    .command('interactive')
    .alias('i')
    .description('Interactive configuration editor (TUI)')
    .action(async () => {
      const opts = program.opts() as GlobalOptions;
      renderApp(opts, false, 'config_editor');
    });

  // Provider command group
  const providerCmd = program
    .command('provider')
    .description('Provider management');

  providerCmd
    .command('list')
    .description('List all providers (STT/TTS/Realtime)')
    .option('-t, --type <type>', 'Filter by type (stt/tts/realtime)')
    .option('-c, --configured', 'Show only configured providers')
    .option('-s, --search <query>', 'Search providers by name, description, or tags')
    .option('-i, --interactive', 'Interactive mode with real-time search')
    .action(async (options) => {
      const { providerListCommand } = await import('./commands/provider/list.js');
      await providerListCommand({ ...program.opts(), ...options } as GlobalOptions);
    });

  providerCmd
    .command('add <provider>')
    .description('Add/configure a provider')
    .action(async (provider: string) => {
      const { providerAddCommand } = await import('./commands/provider/add.js');
      await providerAddCommand(provider, program.opts() as GlobalOptions);
    });

  providerCmd
    .command('test <provider>')
    .description('Test provider connectivity')
    .action(async (provider: string) => {
      const { providerTestCommand } = await import('./commands/provider/test.js');
      await providerTestCommand(provider, program.opts() as GlobalOptions);
    });

  providerCmd
    .command('browse')
    .description('Interactive provider browser with search')
    .option('-t, --type <type>', 'Filter by type (stt/tts/realtime)')
    .action(async () => {
      const opts = program.opts() as GlobalOptions;
      renderApp(opts, false, 'provider_browser');
    });

  // DAG command group
  const dagCmd = program
    .command('dag')
    .description('DAG pipeline management');

  dagCmd
    .command('list')
    .description('List saved DAG pipelines')
    .action(async () => {
      const opts = program.opts() as GlobalOptions;
      renderApp(opts, false, 'dag_list');
    });

  dagCmd
    .command('create')
    .description('Create new DAG pipeline')
    .action(async () => {
      const opts = program.opts() as GlobalOptions;
      renderApp(opts, false, 'dag_editor');
    });

  dagCmd
    .command('validate <name>')
    .description('Validate DAG configuration')
    .action(async (name: string) => {
      const { dagValidateCommand } = await import('./commands/dag/validate.js');
      await dagValidateCommand(name, program.opts() as GlobalOptions);
    });

  dagCmd
    .command('visualize <name>')
    .description('ASCII visualization of DAG')
    .action(async (name: string) => {
      const { dagVisualizeCommand } = await import('./commands/dag/visualize.js');
      await dagVisualizeCommand(name, program.opts() as GlobalOptions);
    });

  // Server command group
  const serverCmd = program
    .command('server')
    .description('Gateway server control');

  serverCmd
    .command('start')
    .description('Start the gateway server')
    .option('-d, --detach', 'Run in background')
    .action(async (options) => {
      const { serverStartCommand } = await import('./commands/server/start.js');
      await serverStartCommand({ ...program.opts(), ...options } as GlobalOptions);
    });

  serverCmd
    .command('stop')
    .description('Stop the gateway server')
    .action(async () => {
      const { serverStopCommand } = await import('./commands/server/stop.js');
      await serverStopCommand(program.opts() as GlobalOptions);
    });

  serverCmd
    .command('status')
    .description('Show server status')
    .action(async () => {
      const opts = program.opts() as GlobalOptions;
      renderApp(opts, false, 'server_status');
    });

  serverCmd
    .command('logs')
    .description('Tail server logs with htop-style filtering')
    .option('-f, --follow', 'Follow log output in real-time')
    .option('-n, --lines <n>', 'Number of lines to display', '500')
    .option('-l, --level <level>', 'Filter by log level (error, warn, info, debug, trace)')
    .option('--filter <text>', 'Filter logs by text content')
    .option('-p, --provider <name>', 'Filter by provider (deepgram, elevenlabs, etc.)')
    .action(async (options) => {
      const { serverLogsCommand } = await import('./commands/server/logs.js');
      await serverLogsCommand({ ...program.opts(), ...options } as GlobalOptions);
    });

  // Voice command group
  const voiceCmd = program
    .command('voice')
    .description('Voice interaction mode');

  voiceCmd
    .command('start')
    .description('Start voice conversation')
    .option('-s, --stt <provider>', 'STT provider')
    .option('-t, --tts <provider>', 'TTS provider')
    .option('-m, --mode <mode>', 'Voice mode (vad/push_to_talk/continuous)')
    .action(async () => {
      const opts = program.opts() as GlobalOptions;
      renderApp(opts, false, 'voice_mode');
    });

  voiceCmd
    .command('test')
    .description('Test mic/speaker setup')
    .action(async () => {
      const opts = program.opts() as GlobalOptions;
      renderApp(opts, false, 'voice_test');
    });

  voiceCmd
    .command('configure')
    .description('Audio device configuration')
    .action(async () => {
      const opts = program.opts() as GlobalOptions;
      renderApp(opts, false, 'voice_config');
    });

  // Dashboard command
  program
    .command('dashboard')
    .description('Interactive analytics dashboard')
    .action(async () => {
      const opts = program.opts() as GlobalOptions;
      renderApp(opts, false, 'dashboard');
    });

  // Doctor command
  program
    .command('doctor')
    .description('Diagnose issues')
    .action(async () => {
      const { doctorCommand } = await import('./commands/doctor.js');
      await doctorCommand(program.opts() as GlobalOptions);
    });

  // Plugin command group
  const pluginCmd = program
    .command('plugin')
    .description('Plugin development tools');

  pluginCmd
    .command('scaffold [name]')
    .description('Generate a new WaaV plugin from template')
    .option('-t, --type <type>', 'Plugin type (stt/tts/realtime)', 'stt')
    .option('-l, --local', 'Local inference mode (vs cloud API)', false)
    .option('-o, --output <dir>', 'Output directory', '.')
    .option('-y, --yes', 'Skip confirmation prompts', false)
    .action(async (name: string | undefined, options) => {
      const { pluginScaffoldCommand } = await import('./commands/plugin/scaffold.js');
      await pluginScaffoldCommand(name, { ...program.opts(), ...options } as GlobalOptions);
    });

  // Completion command
  program
    .command('completion <shell>')
    .description('Generate shell completion scripts (bash/zsh/fish/powershell)')
    .action(async (shell: string) => {
      const { completionCommand } = await import('./commands/completion.js');
      await completionCommand(shell, program.opts() as GlobalOptions);
    });

  return program;
}

/**
 * Render the interactive CLI app
 *
 * @param options - Global CLI options
 * @param showSplash - Whether to show splash screen first
 * @param initialScreen - Screen to start at (defaults to main_menu)
 */
export function renderApp(
  options: GlobalOptions = {},
  showSplash = false,
  initialScreen: ScreenName = 'main_menu'
): void {
  render(
    <App
      options={options}
      showSplash={showSplash}
      initialScreen={initialScreen}
    />
  );
}

/**
 * Main CLI entry function
 */
export async function main(): Promise<void> {
  const program = createProgram();

  // Show splash and main menu if no command provided
  if (process.argv.length <= 2) {
    renderApp({}, true);
    return;
  }

  // Parse and execute command
  try {
    await program.parseAsync(process.argv);
  } catch (error) {
    logger.error('Command failed', error);
    process.exit(1);
  }
}
