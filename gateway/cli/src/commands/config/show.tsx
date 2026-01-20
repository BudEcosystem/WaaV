/**
 * WaaV CLI Config Show Command
 *
 * Display current configuration
 */

import React from 'react';
import { render, Text, Box } from 'ink';
import { configManager } from '../../core/config/manager.js';
import { maskSensitive } from '../../utils/helpers.js';
import { GRADIENTS } from '../../components/branding/index.js';
import type { GlobalOptions } from '../../cli.js';

/**
 * Config display component
 */
const ConfigDisplay: React.FC<{ options: GlobalOptions }> = ({ options }) => {
  const config = configManager.get();
  const configPath = configManager.getConfigPath();
  const profile = configManager.getProfile();

  // JSON output mode
  if (options.json) {
    return null; // Handled separately
  }

  return (
    <Box flexDirection="column" paddingY={1}>
      {/* Header */}
      <Box marginBottom={1}>
        <Text bold>{GRADIENTS.waav('WaaV CLI Configuration')}</Text>
      </Box>

      {/* File info */}
      <Box marginBottom={1}>
        <Text dimColor>File: {configPath}</Text>
      </Box>
      <Box marginBottom={1}>
        <Text dimColor>Profile: {profile}</Text>
      </Box>

      {/* Gateway section */}
      <Box flexDirection="column" marginBottom={1}>
        <Text bold color="cyan">Gateway</Text>
        <Box marginLeft={2} flexDirection="column">
          <Text>URL: <Text color="green">{config.gateway.url}</Text></Text>
          <Text>Auth: {config.gateway.auth?.api_key
            ? <Text color="green">{maskSensitive(config.gateway.auth.api_key)}</Text>
            : <Text dimColor>Not configured</Text>}
          </Text>
          <Text>Timeout: {config.gateway.timeout}ms</Text>
        </Box>
      </Box>

      {/* Defaults section */}
      <Box flexDirection="column" marginBottom={1}>
        <Text bold color="cyan">Default Providers</Text>
        <Box marginLeft={2} flexDirection="column">
          <Text>STT: <Text color="yellow">{config.defaults.stt_provider}</Text></Text>
          <Text>TTS: <Text color="yellow">{config.defaults.tts_provider}</Text></Text>
          {config.defaults.realtime_provider && (
            <Text>Realtime: <Text color="yellow">{config.defaults.realtime_provider}</Text></Text>
          )}
        </Box>
      </Box>

      {/* Configured providers */}
      <Box flexDirection="column" marginBottom={1}>
        <Text bold color="cyan">Configured Providers</Text>
        <Box marginLeft={2} flexDirection="column">
          {Object.keys(config.providers).length === 0 ? (
            <Text dimColor>No providers configured</Text>
          ) : (
            Object.entries(config.providers).map(([id, providerConfig]) => (
              <Box key={id}>
                <Text color="green">✓</Text>
                <Text> {id}</Text>
                {providerConfig.api_key && (
                  <Text dimColor> (API key: {maskSensitive(providerConfig.api_key)})</Text>
                )}
              </Box>
            ))
          )}
        </Box>
      </Box>

      {/* Audio section */}
      <Box flexDirection="column" marginBottom={1}>
        <Text bold color="cyan">Audio</Text>
        <Box marginLeft={2} flexDirection="column">
          <Text>Input Device: {config.audio.input_device}</Text>
          <Text>Output Device: {config.audio.output_device}</Text>
          <Text>Sample Rate: {config.audio.sample_rate} Hz</Text>
          <Text>VAD: {config.audio.vad.enabled
            ? <Text color="green">Enabled (threshold: {config.audio.vad.threshold})</Text>
            : <Text dimColor>Disabled</Text>}
          </Text>
        </Box>
      </Box>

      {/* Voice mode section */}
      <Box flexDirection="column" marginBottom={1}>
        <Text bold color="cyan">Voice Mode</Text>
        <Box marginLeft={2} flexDirection="column">
          <Text>Mode: {config.voice.mode}</Text>
          {config.voice.mode === 'push_to_talk' && (
            <Text>PTT Key: {config.voice.push_to_talk_key}</Text>
          )}
          <Text>Auto-play Response: {config.voice.auto_play_response ? 'Yes' : 'No'}</Text>
        </Box>
      </Box>

      {/* UI section */}
      <Box flexDirection="column" marginBottom={1}>
        <Text bold color="cyan">UI</Text>
        <Box marginLeft={2} flexDirection="column">
          <Text>Theme: {config.ui.theme}</Text>
          <Text>Animations: {config.ui.animations ? 'Yes' : 'No'}</Text>
          <Text>Verbose: {config.ui.verbose ? 'Yes' : 'No'}</Text>
        </Box>
      </Box>

      {/* Profiles */}
      {Object.keys(config.profiles).length > 0 && (
        <Box flexDirection="column" marginBottom={1}>
          <Text bold color="cyan">Available Profiles</Text>
          <Box marginLeft={2} flexDirection="column">
            <Text color={profile === 'default' ? 'cyan' : undefined}>
              {profile === 'default' ? '❯ ' : '  '}default
            </Text>
            {Object.keys(config.profiles).map(name => (
              <Text key={name} color={profile === name ? 'cyan' : undefined}>
                {profile === name ? '❯ ' : '  '}{name}
              </Text>
            ))}
          </Box>
        </Box>
      )}

      {/* Help */}
      <Box marginTop={1}>
        <Text dimColor>
          Use "waav config set {'<key>'} {'<value>'}" to modify configuration
        </Text>
      </Box>
    </Box>
  );
};

/**
 * Config show command handler
 */
export async function configShowCommand(options: GlobalOptions): Promise<void> {
  // Initialize config
  await configManager.initialize({
    configPath: options.config,
    profile: options.profile,
  });

  // JSON output
  if (options.json) {
    const config = configManager.get();
    // Mask sensitive values
    const safeConfig = JSON.parse(JSON.stringify(config));
    if (safeConfig.gateway?.auth?.api_key) {
      safeConfig.gateway.auth.api_key = maskSensitive(safeConfig.gateway.auth.api_key);
    }
    for (const [, provider] of Object.entries(safeConfig.providers || {})) {
      if ((provider as { api_key?: string }).api_key) {
        (provider as { api_key?: string }).api_key = maskSensitive((provider as { api_key?: string }).api_key!);
      }
    }
    console.log(JSON.stringify(safeConfig, null, 2));
    return;
  }

  // Render config display
  const { waitUntilExit } = render(<ConfigDisplay options={options} />);
  await waitUntilExit();
}

export default configShowCommand;
