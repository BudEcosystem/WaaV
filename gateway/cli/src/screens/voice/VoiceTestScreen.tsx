/**
 * WaaV CLI Voice Test Screen
 *
 * Test microphone and speaker setup.
 */

import React from 'react';
import { Text, Box, useInput, useApp } from 'ink';
import { useNavigation } from '../../hooks/useNavigation.js';
import type { ScreenProps } from '../../types/navigation.js';

export const VoiceTestScreen: React.FC<ScreenProps> = () => {
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
      <Text bold color="cyan">Audio Test</Text>
      <Box marginTop={1}>
        <Text dimColor>Audio device testing will be available here.</Text>
      </Box>
      <Box marginTop={1}>
        <Text dimColor>[ESC] Back  [Q] Quit</Text>
      </Box>
    </Box>
  );
};

export default VoiceTestScreen;
