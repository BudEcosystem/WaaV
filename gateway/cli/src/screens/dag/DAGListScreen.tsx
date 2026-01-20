/**
 * WaaV CLI DAG List Screen
 *
 * List and manage DAG pipelines.
 */

import React from 'react';
import { Text, Box, useInput, useApp } from 'ink';
import { useNavigation } from '../../hooks/useNavigation.js';
import type { ScreenProps } from '../../types/navigation.js';

export const DAGListScreen: React.FC<ScreenProps> = () => {
  const { exit } = useApp();
  const { goBack, canGoBack, goHome, navigate } = useNavigation();

  useInput((input, key) => {
    if (key.escape) {
      canGoBack ? goBack() : goHome();
    }
    if (input === 'q') exit();
    if (input === 'n') navigate('dag_editor');
  });

  return (
    <Box flexDirection="column" paddingY={1}>
      <Text bold color="cyan">DAG Pipelines</Text>
      <Box marginTop={1}>
        <Text dimColor>No pipelines configured yet.</Text>
      </Box>
      <Box marginTop={1}>
        <Text dimColor>Pipeline management will be available here.</Text>
      </Box>
      <Box marginTop={1}>
        <Text dimColor>[N] New Pipeline  [ESC] Back  [Q] Quit</Text>
      </Box>
    </Box>
  );
};

export default DAGListScreen;
