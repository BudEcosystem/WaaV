/**
 * WaaV CLI Help Screen
 *
 * Shows all available commands and documentation
 */

import React from 'react';
import { Text, Box, useInput, useApp } from 'ink';
import { GRADIENTS } from '../branding/index.js';

interface HelpScreenProps {
  onBack: () => void;
}

interface CommandHelp {
  command: string;
  description: string;
  subcommands?: { name: string; description: string }[];
}

const COMMANDS: CommandHelp[] = [
  {
    command: 'waav',
    description: 'Launch interactive main menu',
    subcommands: [],
  },
  {
    command: 'waav init',
    description: 'First-time setup wizard',
  },
  {
    command: 'waav provider',
    description: 'Provider management',
    subcommands: [
      { name: 'list', description: 'List all providers (-s search, -i interactive)' },
      { name: 'browse', description: 'Interactive provider browser with search' },
      { name: 'add <name>', description: 'Add/configure a provider' },
      { name: 'test <name>', description: 'Test provider connectivity' },
    ],
  },
  {
    command: 'waav config',
    description: 'Configuration management',
    subcommands: [
      { name: 'show', description: 'Display current configuration' },
      { name: 'set <key> <value>', description: 'Set configuration value' },
      { name: 'edit', description: 'Open config in editor' },
    ],
  },
  {
    command: 'waav voice',
    description: 'Voice interaction mode',
    subcommands: [
      { name: 'start', description: 'Start voice conversation' },
      { name: 'test', description: 'Test mic/speaker setup' },
      { name: 'configure', description: 'Audio device configuration' },
    ],
  },
  {
    command: 'waav server',
    description: 'Gateway server control',
    subcommands: [
      { name: 'start', description: 'Start the gateway server' },
      { name: 'stop', description: 'Stop the gateway server' },
      { name: 'status', description: 'Show server status' },
      { name: 'logs', description: 'Tail server logs' },
    ],
  },
  {
    command: 'waav dag',
    description: 'DAG pipeline management',
    subcommands: [
      { name: 'list', description: 'List saved DAG pipelines' },
      { name: 'create', description: 'Create new DAG pipeline' },
      { name: 'validate <name>', description: 'Validate DAG configuration' },
      { name: 'visualize <name>', description: 'ASCII visualization of DAG' },
    ],
  },
  {
    command: 'waav dashboard',
    description: 'Interactive analytics dashboard',
  },
  {
    command: 'waav doctor',
    description: 'Diagnose issues',
  },
];

export const HelpScreen: React.FC<HelpScreenProps> = ({ onBack }) => {
  const { exit } = useApp();

  useInput((input, key) => {
    if (key.escape || input === 'q' || input === 'b') {
      onBack();
    }
    if (key.ctrl && input === 'c') {
      exit();
    }
  });

  return (
    <Box flexDirection="column" paddingY={1}>
      {/* Header */}
      <Box marginBottom={1}>
        <Text bold>{GRADIENTS.waav('WaaV CLI Help')}</Text>
      </Box>

      {/* Commands */}
      <Box flexDirection="column" borderStyle="single" borderColor="gray" paddingX={2} paddingY={1}>
        {COMMANDS.map((cmd, index) => (
          <Box key={cmd.command} flexDirection="column" marginBottom={index < COMMANDS.length - 1 ? 1 : 0}>
            <Box>
              <Text bold color="cyan">{cmd.command}</Text>
              <Text dimColor> - {cmd.description}</Text>
            </Box>
            {cmd.subcommands && cmd.subcommands.length > 0 && (
              <Box flexDirection="column" marginLeft={2}>
                {cmd.subcommands.map(sub => (
                  <Box key={sub.name}>
                    <Text color="yellow">{sub.name}</Text>
                    <Text dimColor> - {sub.description}</Text>
                  </Box>
                ))}
              </Box>
            )}
          </Box>
        ))}
      </Box>

      {/* Global options */}
      <Box flexDirection="column" marginTop={1} borderStyle="single" borderColor="gray" paddingX={2} paddingY={1}>
        <Box marginBottom={1}>
          <Text bold>Global Options</Text>
        </Box>
        <Box>
          <Box width={25}><Text color="yellow">-c, --config &lt;path&gt;</Text></Box>
          <Text dimColor>Config file path</Text>
        </Box>
        <Box>
          <Box width={25}><Text color="yellow">-p, --profile &lt;name&gt;</Text></Box>
          <Text dimColor>Configuration profile</Text>
        </Box>
        <Box>
          <Box width={25}><Text color="yellow">-g, --gateway &lt;url&gt;</Text></Box>
          <Text dimColor>Gateway URL override</Text>
        </Box>
        <Box>
          <Box width={25}><Text color="yellow">--json</Text></Box>
          <Text dimColor>JSON output mode</Text>
        </Box>
        <Box>
          <Box width={25}><Text color="yellow">-v, --verbose</Text></Box>
          <Text dimColor>Verbose logging</Text>
        </Box>
      </Box>

      {/* Footer */}
      <Box marginTop={1}>
        <Text dimColor>
          [b/q/Esc] Back to menu  |  For more info: waav --help
        </Text>
      </Box>
    </Box>
  );
};

export default HelpScreen;
