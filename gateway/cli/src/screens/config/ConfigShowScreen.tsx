/**
 * WaaV CLI Configuration Show Screen
 *
 * Display current configuration.
 */

import React from 'react';
import { Text, Box, useInput, useApp } from 'ink';
import { useNavigation } from '../../hooks/useNavigation.js';
import { useConfig, useConfiguredProviders } from '../../hooks/useAppState.js';
import type { ScreenProps } from '../../types/navigation.js';

export const ConfigShowScreen: React.FC<ScreenProps> = () => {
  const { exit } = useApp();
  const { goBack, canGoBack, goHome, navigate } = useNavigation();
  const config = useConfig();
  const { stt, tts, realtime, count: providerCount } = useConfiguredProviders();

  useInput((input, key) => {
    if (key.escape) {
      canGoBack ? goBack() : goHome();
    }
    if (input === 'q') exit();
    if (input === 'e') navigate('config_editor');
  });

  return (
    <Box flexDirection="column" paddingY={1}>
      <Text bold color="cyan">Configuration</Text>

      <Box marginTop={1} flexDirection="column" borderStyle="single" borderColor="gray" paddingX={2} paddingY={1}>
        <Text bold>Gateway</Text>
        <Box marginLeft={2} flexDirection="column">
          <Box>
            <Box width={15}><Text dimColor>URL:</Text></Box>
            <Text>{config.gateway?.url || 'Not set'}</Text>
          </Box>
          <Box>
            <Box width={15}><Text dimColor>Auth:</Text></Box>
            <Text>{config.gateway?.auth?.api_key ? 'Configured' : 'None'}</Text>
          </Box>
        </Box>
      </Box>

      <Box marginTop={1} flexDirection="column" borderStyle="single" borderColor="gray" paddingX={2} paddingY={1}>
        <Text bold>Default Providers</Text>
        <Box marginLeft={2} flexDirection="column">
          <Box>
            <Box width={15}><Text dimColor>STT:</Text></Box>
            <Text>{stt || 'Not set'}</Text>
          </Box>
          <Box>
            <Box width={15}><Text dimColor>TTS:</Text></Box>
            <Text>{tts || 'Not set'}</Text>
          </Box>
          <Box>
            <Box width={15}><Text dimColor>Realtime:</Text></Box>
            <Text>{realtime || 'Not set'}</Text>
          </Box>
        </Box>
      </Box>

      <Box marginTop={1} flexDirection="column" borderStyle="single" borderColor="gray" paddingX={2} paddingY={1}>
        <Text bold>Providers</Text>
        <Box marginLeft={2}>
          <Text dimColor>{providerCount} configured</Text>
        </Box>
      </Box>

      <Box marginTop={1}>
        <Text dimColor>[E] Edit Config  [ESC] Back  [Q] Quit</Text>
      </Box>
    </Box>
  );
};

export default ConfigShowScreen;
