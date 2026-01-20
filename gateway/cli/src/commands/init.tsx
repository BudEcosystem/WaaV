/**
 * WaaV CLI Init Command
 *
 * First-time setup wizard for configuring the CLI
 */

import React, { useState, useEffect, useCallback } from 'react';
import { render, Text, Box, useApp, useInput } from 'ink';
import { configManager } from '../core/config/manager.js';
import { GatewayClient } from '../core/gateway/client.js';
import { logger } from '../utils/logger.js';
import { checkDependencies, getInstallInstructions, installDependencies, detectPackageManager } from '../utils/platform.js';
import { WelcomeMessage } from '../components/branding/index.js';
import {
  Spinner,
  MultiStepLoader,
  Select,
  TextInput,
  SuccessIndicator,
  ErrorIndicator,
  type SelectItem,
} from '../components/common/index.js';
import { PROVIDER_ENV_VARS } from '../core/config/defaults.js';
import type { GlobalOptions } from '../cli.js';

/**
 * Setup wizard steps
 */
type SetupStep =
  | 'welcome'
  | 'check_deps'
  | 'gateway_url'
  | 'select_stt'
  | 'configure_stt'
  | 'select_tts'
  | 'configure_tts'
  | 'test_connection'
  | 'complete';

/**
 * Setup Wizard Component
 */
const SetupWizard: React.FC<{ options: GlobalOptions }> = ({ options }) => {
  const { exit } = useApp();
  const [step, setStep] = useState<SetupStep>('welcome');
  const [gatewayUrl, setGatewayUrl] = useState('http://localhost:3001');
  const [sttProvider, setSttProvider] = useState('deepgram');
  const [sttApiKey, setSttApiKey] = useState('');
  const [ttsProvider, setTtsProvider] = useState('elevenlabs');
  const [ttsApiKey, setTtsApiKey] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [depStatus, setDepStatus] = useState<Array<{
    id: string;
    label: string;
    status: 'pending' | 'loading' | 'complete' | 'error';
  }>>([]);

  // Initialize config manager
  useEffect(() => {
    configManager.initialize({
      configPath: options.config,
      profile: options.profile,
    }).catch(err => {
      logger.debug('Config init error (expected for first run)', err);
    });
  }, [options]);

  // Handle key input
  useInput((_input, key) => {
    if (key.escape) {
      exit();
    }
  });

  // Check dependencies
  const runDependencyCheck = useCallback(async () => {
    setStep('check_deps');
    setDepStatus([
      { id: 'node', label: 'Node.js version', status: 'pending' },
      { id: 'sox', label: 'Audio tools (sox)', status: 'pending' },
      { id: 'audio', label: 'Audio system', status: 'pending' },
    ]);

    try {
      let deps = await checkDependencies();

      setDepStatus(deps.map(dep => ({
        id: dep.name.toLowerCase().replace(/[^a-z0-9]/g, '_'),
        label: dep.name,
        status: dep.installed ? 'complete' : 'error',
      })));

      // Check if all required deps are installed
      let allInstalled = deps.every(d => !d.required || d.installed);

      if (!allInstalled) {
        // Try to auto-install missing dependencies
        const missing = deps.filter(d => d.required && !d.installed);
        const packageManager = detectPackageManager();

        setDepStatus(prev => prev.map(dep => {
          const isMissing = missing.some(m => m.name.toLowerCase().replace(/[^a-z0-9]/g, '_') === dep.id);
          return isMissing ? { ...dep, status: 'loading', label: `Installing ${dep.label}...` } : dep;
        }));

        logger.info(`Auto-installing dependencies using ${packageManager}...`);

        const installResult = await installDependencies({
          onProgress: (msg) => {
            logger.debug(msg);
          },
        });

        if (installResult.success) {
          // Re-check after installation
          deps = await checkDependencies();
          allInstalled = deps.every(d => !d.required || d.installed);

          setDepStatus(deps.map(dep => ({
            id: dep.name.toLowerCase().replace(/[^a-z0-9]/g, '_'),
            label: dep.name,
            status: dep.installed ? 'complete' : 'error',
          })));
        } else {
          logger.error(`Auto-install failed: ${installResult.error || installResult.message}`);
        }
      }

      setTimeout(() => {
        if (allInstalled) {
          setStep('gateway_url');
        } else {
          setError('Some required dependencies are missing. Please install them manually.');
        }
      }, 1000);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, []);

  // Test gateway connection
  const testGatewayConnection = useCallback(async () => {
    setStep('test_connection');
    try {
      const client = new GatewayClient({ baseUrl: gatewayUrl });
      const reachable = await client.ping();

      if (reachable) {
        // Save configuration
        configManager.setGatewayUrl(gatewayUrl);
        configManager.setDefaultSTT(sttProvider);
        configManager.setDefaultTTS(ttsProvider);

        if (sttApiKey) {
          configManager.setProvider(sttProvider, { api_key: sttApiKey });
        }
        if (ttsApiKey) {
          configManager.setProvider(ttsProvider, { api_key: ttsApiKey });
        }

        await configManager.save();
        configManager.completeFirstRun();

        setStep('complete');
      } else {
        setError('Could not connect to gateway. Please check the URL and try again.');
        setStep('gateway_url');
      }
    } catch {
      // Gateway not running is OK - we can still save config
      configManager.setGatewayUrl(gatewayUrl);
      configManager.setDefaultSTT(sttProvider);
      configManager.setDefaultTTS(ttsProvider);

      if (sttApiKey) {
        configManager.setProvider(sttProvider, { api_key: sttApiKey });
      }
      if (ttsApiKey) {
        configManager.setProvider(ttsProvider, { api_key: ttsApiKey });
      }

      await configManager.save();
      configManager.completeFirstRun();

      setStep('complete');
    }
  }, [gatewayUrl, sttProvider, sttApiKey, ttsProvider, ttsApiKey]);

  // STT provider options
  const sttOptions: SelectItem[] = [
    { value: 'deepgram', label: 'Deepgram', description: 'Nova-2, real-time streaming, best accuracy' },
    { value: 'openai', label: 'OpenAI Whisper', description: 'GPT-4o audio, multilingual' },
    { value: 'google', label: 'Google Cloud', description: 'Chirp 3, 125+ languages' },
    { value: 'azure', label: 'Azure Speech', description: 'Custom models, enterprise' },
    { value: 'assemblyai', label: 'AssemblyAI', description: 'Speaker diarization, sentiment' },
    { value: 'groq', label: 'Groq Whisper', description: 'Fast inference, open-source' },
  ];

  // TTS provider options
  const ttsOptions: SelectItem[] = [
    { value: 'elevenlabs', label: 'ElevenLabs', description: 'Voice cloning, most natural' },
    { value: 'cartesia', label: 'Cartesia', description: 'Sonic, ultra-low latency' },
    { value: 'openai', label: 'OpenAI TTS', description: 'HD voices, streaming' },
    { value: 'play_ht', label: 'PlayHT', description: 'Voice emotions, custom' },
    { value: 'deepgram', label: 'Deepgram Aura', description: 'Fast, conversational' },
    { value: 'azure', label: 'Azure Speech', description: 'Neural voices, enterprise' },
  ];

  // Render based on current step
  switch (step) {
    case 'welcome':
      return <WelcomeMessage onContinue={() => runDependencyCheck()} />;

    case 'check_deps':
      return (
        <Box flexDirection="column" paddingY={2}>
          <MultiStepLoader
            title="Checking dependencies..."
            steps={depStatus}
          />
          {error && (
            <Box marginTop={2} flexDirection="column">
              <ErrorIndicator label={error} />
              <Box marginTop={1}>
                <Text dimColor>{getInstallInstructions()}</Text>
              </Box>
            </Box>
          )}
        </Box>
      );

    case 'gateway_url':
      return (
        <Box flexDirection="column" paddingY={2}>
          <Text bold color="cyan">Step 1/4: Gateway URL</Text>
          <Text dimColor>Enter the WaaV Gateway URL (default: http://localhost:3001)</Text>
          <Box marginTop={1}>
            <TextInput
              value={gatewayUrl}
              placeholder="http://localhost:3001"
              onChange={setGatewayUrl}
              onSubmit={() => setStep('select_stt')}
              prefix="URL: "
            />
          </Box>
          {error && (
            <Box marginTop={1}>
              <ErrorIndicator label={error} />
            </Box>
          )}
        </Box>
      );

    case 'select_stt':
      return (
        <Box flexDirection="column" paddingY={2}>
          <Text bold color="cyan">Step 2/4: Select STT Provider</Text>
          <Text dimColor>Choose your preferred Speech-to-Text provider</Text>
          <Box marginTop={1}>
            <Select
              items={sttOptions}
              onSelect={(item) => {
                setSttProvider(item.value);
                setStep('configure_stt');
              }}
            />
          </Box>
        </Box>
      );

    case 'configure_stt': {
      const sttEnvVar = PROVIDER_ENV_VARS[sttProvider]?.api_key;
      const existingSttKey = process.env[sttEnvVar ?? ''];

      return (
        <Box flexDirection="column" paddingY={2}>
          <Text bold color="cyan">Step 2/4: Configure {sttProvider}</Text>
          <Text dimColor>
            Enter your API key (or set {sttEnvVar} environment variable)
          </Text>
          {existingSttKey && (
            <Box marginTop={1}>
              <Text color="green">Found existing key in environment</Text>
            </Box>
          )}
          <Box marginTop={1}>
            <TextInput
              value={sttApiKey}
              placeholder={existingSttKey ? '(using env var)' : 'sk-...'}
              onChange={setSttApiKey}
              onSubmit={() => setStep('select_tts')}
              mask
              prefix="API Key: "
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Press Enter to continue (leave blank to use env var)</Text>
          </Box>
        </Box>
      );
    }

    case 'select_tts':
      return (
        <Box flexDirection="column" paddingY={2}>
          <Text bold color="cyan">Step 3/4: Select TTS Provider</Text>
          <Text dimColor>Choose your preferred Text-to-Speech provider</Text>
          <Box marginTop={1}>
            <Select
              items={ttsOptions}
              onSelect={(item) => {
                setTtsProvider(item.value);
                setStep('configure_tts');
              }}
            />
          </Box>
        </Box>
      );

    case 'configure_tts': {
      const ttsEnvVar = PROVIDER_ENV_VARS[ttsProvider]?.api_key;
      const existingTtsKey = process.env[ttsEnvVar ?? ''];

      return (
        <Box flexDirection="column" paddingY={2}>
          <Text bold color="cyan">Step 3/4: Configure {ttsProvider}</Text>
          <Text dimColor>
            Enter your API key (or set {ttsEnvVar} environment variable)
          </Text>
          {existingTtsKey && (
            <Box marginTop={1}>
              <Text color="green">Found existing key in environment</Text>
            </Box>
          )}
          <Box marginTop={1}>
            <TextInput
              value={ttsApiKey}
              placeholder={existingTtsKey ? '(using env var)' : 'sk-...'}
              onChange={setTtsApiKey}
              onSubmit={() => testGatewayConnection()}
              mask
              prefix="API Key: "
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Press Enter to continue (leave blank to use env var)</Text>
          </Box>
        </Box>
      );
    }

    case 'test_connection':
      return (
        <Box flexDirection="column" paddingY={2}>
          <Spinner label="Testing gateway connection..." />
        </Box>
      );

    case 'complete':
      return (
        <Box flexDirection="column" paddingY={2}>
          <SuccessIndicator label="Setup complete!" />
          <Box marginTop={1} flexDirection="column">
            <Text>Configuration saved to: {configManager.getConfigPath()}</Text>
            <Box marginTop={1}>
              <Text color="cyan">Quick Start Commands:</Text>
            </Box>
            <Text>  waav server status    - Check gateway status</Text>
            <Text>  waav voice test       - Test microphone/speakers</Text>
            <Text>  waav voice start      - Start voice conversation</Text>
            <Text>  waav provider browse  - Browse all providers</Text>
          </Box>
          <Box marginTop={2}>
            <Text dimColor>Press any key to exit...</Text>
          </Box>
        </Box>
      );

    default:
      return <Text>Unknown step</Text>;
  }
};

/**
 * Init command handler
 */
export async function initCommand(options: GlobalOptions): Promise<void> {
  // Check if already configured
  const isFirstRun = configManager.isFirstRun();

  if (!isFirstRun && !options.verbose) {
    logger.info('WaaV CLI is already configured.');
    logger.info(`Config file: ${configManager.getConfigPath()}`);
    logger.info('Run with --verbose to reconfigure.');
    return;
  }

  // Render setup wizard
  const { waitUntilExit } = render(<SetupWizard options={options} />);
  await waitUntilExit();
}

export default initCommand;
