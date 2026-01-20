/**
 * WaaV CLI Setup Wizard Screen
 *
 * First-time setup wizard for new users.
 */

import React from 'react';
import { Text, Box, useInput, useApp } from 'ink';
import { useNavigation } from '../../hooks/useNavigation.js';
import { GRADIENTS } from '../../components/branding/index.js';
import type { ScreenProps } from '../../types/navigation.js';

export const SetupWizardScreen: React.FC<ScreenProps> = () => {
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
      <Box marginBottom={1}>
        <Text bold>{GRADIENTS.waav('WaaV Setup Wizard')}</Text>
      </Box>

      <Box flexDirection="column" borderStyle="single" borderColor="gray" paddingX={2} paddingY={1}>
        <Text>Welcome to WaaV! This wizard will help you configure your voice AI gateway.</Text>

        <Box marginTop={1} flexDirection="column">
          <Text bold>Setup Steps:</Text>
          <Box marginLeft={2} flexDirection="column">
            <Text dimColor>1. Configure gateway connection</Text>
            <Text dimColor>2. Set up STT provider (Deepgram, Whisper, etc.)</Text>
            <Text dimColor>3. Set up TTS provider (ElevenLabs, etc.)</Text>
            <Text dimColor>4. Test audio devices</Text>
            <Text dimColor>5. Configure voice mode preferences</Text>
          </Box>
        </Box>

        <Box marginTop={1}>
          <Text dimColor>Full setup wizard available via: waav init</Text>
        </Box>
      </Box>

      <Box marginTop={1}>
        <Text dimColor>[ESC] Back  [Q] Quit</Text>
      </Box>
    </Box>
  );
};

export default SetupWizardScreen;
