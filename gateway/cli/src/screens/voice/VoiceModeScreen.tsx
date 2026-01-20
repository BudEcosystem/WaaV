/**
 * WaaV CLI Voice Mode Screen
 *
 * Real-time voice interaction mode.
 */

import React from 'react';
import { Text, Box, useInput, useApp } from 'ink';
import { useNavigation } from '../../hooks/useNavigation.js';
import { useGatewayStatus, useConfiguredProviders } from '../../hooks/useAppState.js';
import type { ScreenProps } from '../../types/navigation.js';

export const VoiceModeScreen: React.FC<ScreenProps> = () => {
  const { exit } = useApp();
  const { goBack, canGoBack, goHome, navigate } = useNavigation();
  const { isConnected } = useGatewayStatus();
  const { stt, tts } = useConfiguredProviders();

  useInput((input, key) => {
    if (key.escape) {
      canGoBack ? goBack() : goHome();
    }
    if (input === 'q') exit();
    if (input === 't') navigate('voice_test');
    if (input === 'c') navigate('voice_config');
  });

  return (
    <Box flexDirection="column" paddingY={1}>
      <Text bold color="cyan">Voice Mode</Text>

      <Box marginTop={1} flexDirection="column">
        <Box>
          <Text dimColor>Gateway: </Text>
          <Text color={isConnected ? 'green' : 'red'}>{isConnected ? 'Connected' : 'Disconnected'}</Text>
        </Box>
        <Box>
          <Text dimColor>STT Provider: </Text>
          <Text>{stt || 'Not configured'}</Text>
        </Box>
        <Box>
          <Text dimColor>TTS Provider: </Text>
          <Text>{tts || 'Not configured'}</Text>
        </Box>
      </Box>

      {!isConnected && (
        <Box marginTop={1}>
          <Text color="yellow">Gateway must be connected to use voice mode.</Text>
        </Box>
      )}

      <Box marginTop={2}>
        <Text dimColor>Voice interaction will be available here.</Text>
      </Box>

      <Box marginTop={1}>
        <Text dimColor>[T] Test Audio  [C] Configure  [ESC] Back  [Q] Quit</Text>
      </Box>
    </Box>
  );
};

export default VoiceModeScreen;
