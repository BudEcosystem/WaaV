/**
 * WaaV CLI Server Status Command
 *
 * Check gateway server status
 */

import React, { useState, useEffect } from 'react';
import { render, Text, Box } from 'ink';
import { configManager } from '../../core/config/manager.js';
import { GatewayClient } from '../../core/gateway/client.js';
import { formatDuration } from '../../utils/platform.js';
import { Spinner } from '../../components/common/index.js';
import { GRADIENTS } from '../../components/branding/index.js';
import type { GatewayStatus } from '../../types/index.js';
import type { GlobalOptions } from '../../cli.js';

/**
 * Status display component
 */
const StatusDisplay: React.FC<{
  gatewayUrl: string;
}> = ({ gatewayUrl }) => {
  const [loading, setLoading] = useState(true);
  const [status, setStatus] = useState<GatewayStatus | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const checkStatus = async () => {
      const client = new GatewayClient({ baseUrl: gatewayUrl });

      try {
        const reachable = await client.ping();

        if (reachable) {
          const gatewayStatus = await client.getStatus();
          setStatus(gatewayStatus);
        } else {
          setError('Gateway is not reachable');
        }
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      } finally {
        setLoading(false);
      }
    };

    checkStatus();
  }, [gatewayUrl]);

  if (loading) {
    return (
      <Box paddingY={1}>
        <Spinner label={`Checking gateway at ${gatewayUrl}...`} />
      </Box>
    );
  }

  if (error || !status) {
    return (
      <Box flexDirection="column" paddingY={1}>
        <Box>
          <Text color="red">●</Text>
          <Text bold> Gateway Status: </Text>
          <Text color="red">Offline</Text>
        </Box>
        <Box marginTop={1}>
          <Text dimColor>URL: {gatewayUrl}</Text>
        </Box>
        <Box marginTop={1}>
          <Text color="red">Error: {error || 'Could not connect to gateway'}</Text>
        </Box>
        <Box marginTop={1} flexDirection="column">
          <Text dimColor>Suggestions:</Text>
          <Text dimColor>  1. Start the gateway: waav server start</Text>
          <Text dimColor>  2. Check the gateway URL: waav config show</Text>
          <Text dimColor>  3. Verify network connectivity</Text>
        </Box>
      </Box>
    );
  }

  const uptimeStr = formatDuration(status.uptime_seconds * 1000);

  return (
    <Box flexDirection="column" paddingY={1}>
      {/* Header */}
      <Box marginBottom={1}>
        <Text bold>{GRADIENTS.waav('WaaV Gateway Status')}</Text>
      </Box>

      {/* Main status */}
      <Box marginBottom={1}>
        <Text color="green">●</Text>
        <Text bold> Status: </Text>
        <Text color="green">Online</Text>
      </Box>

      {/* Basic info */}
      <Box flexDirection="column" marginBottom={1}>
        <Box>
          <Text>URL: </Text>
          <Text color="cyan">{gatewayUrl}</Text>
        </Box>
        <Box>
          <Text>Version: </Text>
          <Text>{status.version}</Text>
        </Box>
        <Box>
          <Text>Uptime: </Text>
          <Text>{uptimeStr}</Text>
        </Box>
        <Box>
          <Text>Active Connections: </Text>
          <Text color="yellow">{status.connections}</Text>
        </Box>
      </Box>

      {/* STT Providers */}
      <Box flexDirection="column" marginBottom={1}>
        <Text bold color="cyan">STT Providers ({status.providers.stt.length})</Text>
        <Box marginLeft={2} flexWrap="wrap">
          {status.providers.stt.length > 0 ? (
            <Text>{status.providers.stt.join(', ')}</Text>
          ) : (
            <Text dimColor>None configured</Text>
          )}
        </Box>
      </Box>

      {/* TTS Providers */}
      <Box flexDirection="column" marginBottom={1}>
        <Text bold color="cyan">TTS Providers ({status.providers.tts.length})</Text>
        <Box marginLeft={2} flexWrap="wrap">
          {status.providers.tts.length > 0 ? (
            <Text>{status.providers.tts.join(', ')}</Text>
          ) : (
            <Text dimColor>None configured</Text>
          )}
        </Box>
      </Box>

      {/* Realtime Providers */}
      {status.providers.realtime.length > 0 && (
        <Box flexDirection="column" marginBottom={1}>
          <Text bold color="cyan">Realtime Providers ({status.providers.realtime.length})</Text>
          <Box marginLeft={2} flexWrap="wrap">
            <Text>{status.providers.realtime.join(', ')}</Text>
          </Box>
        </Box>
      )}

      {/* Features */}
      {status.features.length > 0 && (
        <Box flexDirection="column" marginBottom={1}>
          <Text bold color="cyan">Features</Text>
          <Box marginLeft={2} flexWrap="wrap">
            {status.features.map((feature, i) => (
              <Text key={feature}>
                <Text color="green">✓</Text> {feature}
                {i < status.features.length - 1 ? ', ' : ''}
              </Text>
            ))}
          </Box>
        </Box>
      )}

      {/* LiveKit status */}
      <Box marginBottom={1}>
        <Text>LiveKit: </Text>
        {status.livekit_enabled ? (
          <Text color="green">Enabled</Text>
        ) : (
          <Text dimColor>Disabled</Text>
        )}
      </Box>

      {/* Help */}
      <Box marginTop={1}>
        <Text dimColor>
          Use "waav server logs" to view gateway logs
        </Text>
      </Box>
    </Box>
  );
};

/**
 * Server status command handler
 */
export async function serverStatusCommand(options: GlobalOptions): Promise<void> {
  // Initialize config
  await configManager.initialize({
    configPath: options.config,
    profile: options.profile,
  });

  const config = configManager.get();
  const gatewayUrl = options.gateway ?? config.gateway.url;

  // JSON output
  if (options.json) {
    const client = new GatewayClient({ baseUrl: gatewayUrl });

    try {
      const status = await client.getStatus();
      console.log(JSON.stringify({
        success: true,
        online: true,
        ...status,
      }, null, 2));
    } catch (error) {
      console.log(JSON.stringify({
        success: false,
        online: false,
        error: error instanceof Error ? error.message : String(error),
      }, null, 2));
    }
    return;
  }

  // Render status display
  const { waitUntilExit } = render(<StatusDisplay gatewayUrl={gatewayUrl} />);
  await waitUntilExit();
}

export default serverStatusCommand;
