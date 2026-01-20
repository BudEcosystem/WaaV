/**
 * WaaV CLI Server Logs Screen
 *
 * View and filter server logs.
 */

import React from 'react';
import { Text, Box, useInput, useApp } from 'ink';
import { useNavigation } from '../../hooks/useNavigation.js';
import type { ScreenProps } from '../../types/navigation.js';

export const ServerLogsScreen: React.FC<ScreenProps> = () => {
  const { exit } = useApp();
  const { goBack, canGoBack, goHome } = useNavigation();

  useInput((input, key) => {
    if (key.escape) {
      canGoBack ? goBack() : goHome();
    }
    if (input === 'q') exit();
  });

  return (
    <Box flexDirection="column" paddingY={1}>
      <Text bold color="cyan">Server Logs</Text>
      <Box marginTop={1}>
        <Text dimColor>Log viewer will be available here.</Text>
      </Box>
      <Box marginTop={1}>
        <Text dimColor>For full log viewing, use: waav server logs -f</Text>
      </Box>
      <Box marginTop={1}>
        <Text dimColor>[ESC] Back  [Q] Quit</Text>
      </Box>
    </Box>
  );
};

export default ServerLogsScreen;
