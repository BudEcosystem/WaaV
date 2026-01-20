/**
 * WaaV CLI Main Menu Screen
 *
 * Primary navigation menu with keyboard shortcuts for the SPA architecture.
 * Adapted from the original MainMenu component to use navigation context.
 */

import React, { useState } from 'react';
import { Text, Box, useInput, useApp } from 'ink';
import { useNavigation } from '../hooks/useNavigation.js';
import { useGatewayStatus, useVersion, useConfiguredProviders } from '../hooks/useAppState.js';
import { CompactSplash, GRADIENTS } from '../components/branding/index.js';
import { Badge } from '../components/common/index.js';
import type { ScreenProps, ScreenName } from '../types/navigation.js';

/**
 * Menu item definition
 */
interface MenuItem {
  id: string;
  label: string;
  description: string;
  shortcut: string;
  screen?: ScreenName;
  action?: () => void;
  disabled?: boolean;
}

/**
 * Format uptime for display
 */
function formatUptime(seconds: number): string {
  if (seconds < 60) {
    return `${seconds}s`;
  }
  if (seconds < 3600) {
    const mins = Math.floor(seconds / 60);
    return `${mins}m`;
  }
  const hours = Math.floor(seconds / 3600);
  const mins = Math.floor((seconds % 3600) / 60);
  return `${hours}h ${mins}m`;
}

/**
 * Main Menu Screen Component
 */
export const MainMenuScreen: React.FC<ScreenProps> = () => {
  const { exit } = useApp();
  const { navigate } = useNavigation();
  const { status: gatewayStatus, isConnected } = useGatewayStatus();
  const version = useVersion();
  const { stt, tts, count: providerCount } = useConfiguredProviders();

  const [selectedIndex, setSelectedIndex] = useState(0);

  // Menu items configuration
  const menuItems: MenuItem[] = [
    {
      id: 'providers',
      label: 'Browse Providers',
      description: 'Search and configure 70+ voice AI providers',
      shortcut: '1',
      screen: 'provider_browser',
    },
    {
      id: 'voice',
      label: 'Start Voice Mode',
      description: 'Begin a voice conversation with the AI',
      shortcut: '2',
      screen: 'voice_mode',
    },
    {
      id: 'dashboard',
      label: 'Dashboard',
      description: 'View analytics and monitoring',
      shortcut: '3',
      screen: 'dashboard',
    },
    {
      id: 'config',
      label: 'Configuration',
      description: 'View and edit settings',
      shortcut: '4',
      screen: 'config_show',
    },
    {
      id: 'setup',
      label: 'Setup Wizard',
      description: 'First-time setup and configuration',
      shortcut: '5',
      screen: 'setup_wizard',
    },
    {
      id: 'help',
      label: 'Help',
      description: 'View CLI commands and documentation',
      shortcut: 'h',
      screen: 'help',
    },
    {
      id: 'exit',
      label: 'Exit',
      description: 'Quit the CLI',
      shortcut: 'q',
      action: () => exit(),
    },
  ];

  // Handle keyboard input
  useInput((input, key) => {
    // Handle escape to exit
    if (key.escape) {
      exit();
      return;
    }

    // Handle arrow navigation
    if (key.upArrow) {
      setSelectedIndex(i => Math.max(0, i - 1));
    } else if (key.downArrow) {
      setSelectedIndex(i => Math.min(menuItems.length - 1, i + 1));
    }

    // Handle enter to select
    if (key.return) {
      const item = menuItems[selectedIndex];
      if (item?.action) {
        item.action();
      } else if (item?.screen) {
        navigate(item.screen);
      }
      return;
    }

    // Handle shortcut keys
    const item = menuItems.find(m => m.shortcut === input.toLowerCase());
    if (item) {
      if (item.action) {
        item.action();
      } else if (item.screen) {
        navigate(item.screen);
      }
    }
  });

  return (
    <Box flexDirection="column" paddingY={1}>
      {/* Header */}
      <CompactSplash version={version} />

      {/* Status bar */}
      <Box marginY={1} paddingX={1}>
        <Box marginRight={3}>
          <Text dimColor>Gateway: </Text>
          {isConnected ? (
            <Badge label="Connected" variant="success" />
          ) : (
            <Badge label="Offline" variant="error" />
          )}
        </Box>
        <Box marginRight={3}>
          <Text dimColor>Providers: </Text>
          <Text color={providerCount > 0 ? 'green' : 'yellow'}>
            {providerCount} configured
          </Text>
        </Box>
        {gatewayStatus && (
          <Box>
            <Text dimColor>Uptime: </Text>
            <Text>{formatUptime(gatewayStatus.uptime_seconds)}</Text>
          </Box>
        )}
      </Box>

      {/* Main menu */}
      <Box flexDirection="row">
        {/* Menu items */}
        <Box flexDirection="column" width={50} borderStyle="single" borderColor="gray" paddingX={2} paddingY={1}>
          <Box marginBottom={1}>
            <Text bold>{GRADIENTS.waav('Main Menu')}</Text>
          </Box>

          {menuItems.map((item, index) => {
            const isSelected = index === selectedIndex;
            return (
              <Box key={item.id} marginBottom={index < menuItems.length - 1 ? 0 : 0}>
                <Box width={3}>
                  <Text color={isSelected ? 'cyan' : 'gray'}>
                    {isSelected ? '▶' : ' '}
                  </Text>
                </Box>
                <Box width={3}>
                  <Text color="yellow" dimColor={!isSelected}>
                    [{item.shortcut}]
                  </Text>
                </Box>
                <Box width={20}>
                  <Text bold={isSelected} color={isSelected ? 'cyan' : undefined}>
                    {item.label}
                  </Text>
                </Box>
              </Box>
            );
          })}
        </Box>

        {/* Description panel */}
        <Box flexDirection="column" flexGrow={1} borderStyle="single" borderColor="gray" paddingX={2} paddingY={1} marginLeft={1}>
          <Box marginBottom={1}>
            <Text bold color="cyan">{menuItems[selectedIndex]?.label}</Text>
          </Box>
          <Text wrap="wrap">{menuItems[selectedIndex]?.description}</Text>

          {/* Context-specific info */}
          {menuItems[selectedIndex]?.id === 'providers' && (
            <Box marginTop={1} flexDirection="column">
              <Text dimColor>Available providers:</Text>
              <Text>• STT: 27 providers (Deepgram, Whisper, etc.)</Text>
              <Text>• TTS: 32 providers (ElevenLabs, Coqui, etc.)</Text>
              <Text>• Realtime: 7 providers (OpenAI, LiveKit, etc.)</Text>
            </Box>
          )}

          {menuItems[selectedIndex]?.id === 'voice' && isConnected && (
            <Box marginTop={1} flexDirection="column">
              <Text dimColor>Current configuration:</Text>
              <Text>• STT: {stt || 'Not set'}</Text>
              <Text>• TTS: {tts || 'Not set'}</Text>
            </Box>
          )}

          {menuItems[selectedIndex]?.id === 'dashboard' && gatewayStatus && (
            <Box marginTop={1} flexDirection="column">
              <Text dimColor>Gateway stats:</Text>
              <Text>• Active connections: {gatewayStatus.connections}</Text>
              <Text>• Version: {gatewayStatus.version}</Text>
            </Box>
          )}
        </Box>
      </Box>

      {/* Footer */}
      <Box marginTop={1}>
        <Text dimColor>
          [↑↓] Navigate  [Enter] Select  [1-5/h/q] Quick access  [Esc] Exit
        </Text>
      </Box>
    </Box>
  );
};

export default MainMenuScreen;
