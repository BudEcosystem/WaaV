/**
 * WaaV CLI Server Status Screen
 *
 * Display gateway server status.
 */

import React from 'react';
import { Text, Box, useInput, useApp } from 'ink';
import { useNavigation } from '../../hooks/useNavigation.js';
import { useGatewayStatus, useConfig } from '../../hooks/useAppState.js';
import { Badge } from '../../components/common/index.js';
import type { ScreenProps } from '../../types/navigation.js';

export const ServerStatusScreen: React.FC<ScreenProps> = () => {
  const { exit } = useApp();
  const { goBack, canGoBack, goHome, navigate } = useNavigation();
  const { status, isConnected, refresh } = useGatewayStatus();
  const config = useConfig();

  useInput((input, key) => {
    if (key.escape) {
      canGoBack ? goBack() : goHome();
    }
    if (input === 'q') exit();
    if (input === 'r') refresh();
    if (input === 'l') navigate('server_logs');
  });

  return (
    <Box flexDirection="column" paddingY={1}>
      <Text bold color="cyan">Server Status</Text>

      <Box marginTop={1} flexDirection="column" borderStyle="single" borderColor="gray" paddingX={2} paddingY={1}>
        <Box>
          <Box width={20}><Text dimColor>Status:</Text></Box>
          {isConnected ? (
            <Badge label="Online" variant="success" />
          ) : (
            <Badge label="Offline" variant="error" />
          )}
        </Box>
        <Box>
          <Box width={20}><Text dimColor>Gateway URL:</Text></Box>
          <Text>{config.gateway?.url || 'Not configured'}</Text>
        </Box>
        {status && (
          <>
            <Box>
              <Box width={20}><Text dimColor>Version:</Text></Box>
              <Text>{status.version}</Text>
            </Box>
            <Box>
              <Box width={20}><Text dimColor>Uptime:</Text></Box>
              <Text>{Math.floor(status.uptime_seconds / 60)}m {status.uptime_seconds % 60}s</Text>
            </Box>
            <Box>
              <Box width={20}><Text dimColor>Connections:</Text></Box>
              <Text>{status.connections}</Text>
            </Box>
          </>
        )}
      </Box>

      <Box marginTop={1}>
        <Text dimColor>[R] Refresh  [L] View Logs  [ESC] Back  [Q] Quit</Text>
      </Box>
    </Box>
  );
};

export default ServerStatusScreen;
