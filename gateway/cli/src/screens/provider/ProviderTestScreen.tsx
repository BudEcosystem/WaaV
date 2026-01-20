/**
 * WaaV CLI Provider Test Screen
 *
 * Test provider connectivity and functionality.
 */

import React from 'react';
import { Text, Box, useInput, useApp } from 'ink';
import { useNavigation } from '../../hooks/useNavigation.js';
import type { ScreenProps } from '../../types/navigation.js';

export const ProviderTestScreen: React.FC<ScreenProps> = () => {
  const { exit } = useApp();
  const { goBack, canGoBack, goHome, params } = useNavigation();

  useInput((input, key) => {
    if (key.escape) {
      canGoBack ? goBack() : goHome();
    }
    if (input === 'q') exit();
  });

  return (
    <Box flexDirection="column" paddingY={1}>
      <Text bold color="cyan">Test Provider: {params.providerId || 'Unknown'}</Text>
      <Box marginTop={1}>
        <Text dimColor>Provider testing will be available here.</Text>
      </Box>
      <Box marginTop={1}>
        <Text dimColor>[ESC] Back  [Q] Quit</Text>
      </Box>
    </Box>
  );
};

export default ProviderTestScreen;
