/**
 * WaaV CLI DAG Editor Screen
 *
 * Create and edit DAG pipelines.
 */

import React from 'react';
import { Text, Box, useInput, useApp } from 'ink';
import { useNavigation } from '../../hooks/useNavigation.js';
import type { ScreenProps } from '../../types/navigation.js';

export const DAGEditorScreen: React.FC<ScreenProps> = () => {
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
      <Text bold color="cyan">
        {params.dagName ? `Edit Pipeline: ${params.dagName}` : 'New Pipeline'}
      </Text>
      <Box marginTop={1}>
        <Text dimColor>Pipeline editor will be available here.</Text>
      </Box>
      <Box marginTop={1}>
        <Text dimColor>[ESC] Back  [Q] Quit</Text>
      </Box>
    </Box>
  );
};

export default DAGEditorScreen;
