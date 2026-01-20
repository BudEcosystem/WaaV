/**
 * WaaV CLI Doctor Command
 *
 * Diagnose issues with the CLI setup
 */

import React, { useState, useEffect } from 'react';
import { render, Text, Box } from 'ink';
import { configManager } from '../core/config/manager.js';
import { GatewayClient } from '../core/gateway/client.js';
import {
  checkDependencies,
  getAudioSystemInfo,
  getPlatformInfo,
  getInstallInstructions,
  formatBytes,
  installDependencies,
  detectPackageManager,
} from '../utils/platform.js';
import { SuccessIndicator } from '../components/common/index.js';
import { GRADIENTS } from '../components/branding/index.js';
import type { GlobalOptions } from '../cli.js';

/**
 * Diagnostic check result
 */
interface DiagnosticCheck {
  id: string;
  label: string;
  status: 'pending' | 'loading' | 'complete' | 'error' | 'warning';
  message?: string;
  details?: string[];
}

/**
 * Doctor display component
 */
const DoctorDisplay: React.FC<{ options: GlobalOptions }> = ({ options }) => {
  const [checks, setChecks] = useState<DiagnosticCheck[]>([
    { id: 'platform', label: 'Platform', status: 'pending' },
    { id: 'node', label: 'Node.js', status: 'pending' },
    { id: 'dependencies', label: 'Dependencies', status: 'pending' },
    { id: 'audio', label: 'Audio System', status: 'pending' },
    { id: 'config', label: 'Configuration', status: 'pending' },
    { id: 'gateway', label: 'Gateway Connection', status: 'pending' },
    { id: 'providers', label: 'Provider Configuration', status: 'pending' },
  ]);
  const [done, setDone] = useState(false);
  const [summary, setSummary] = useState<{ passed: number; warnings: number; failed: number }>({
    passed: 0,
    warnings: 0,
    failed: 0,
  });

  // Update a check
  const updateCheck = (id: string, update: Partial<DiagnosticCheck>) => {
    setChecks(prev =>
      prev.map(check =>
        check.id === id ? { ...check, ...update } : check
      )
    );
  };

  // Run diagnostics
  useEffect(() => {
    const runDiagnostics = async () => {
      // Platform check
      updateCheck('platform', { status: 'loading' });
      try {
        const platform = getPlatformInfo();
        updateCheck('platform', {
          status: 'complete',
          message: `${platform.platform} ${platform.arch}`,
          details: [
            `Release: ${platform.release}`,
            `CPU Cores: ${platform.cpuCores}`,
            `Memory: ${formatBytes(platform.freeMemory)} free / ${formatBytes(platform.totalMemory)} total`,
          ],
        });
      } catch {
        updateCheck('platform', {
          status: 'error',
          message: 'Failed to check platform',
        });
      }
      await new Promise(r => setTimeout(r, 200));

      // Node.js check
      updateCheck('node', { status: 'loading' });
      const nodeVersion = process.version;
      const nodeMajor = parseInt(nodeVersion.slice(1).split('.')[0] ?? '0', 10);
      if (nodeMajor >= 18) {
        updateCheck('node', {
          status: 'complete',
          message: nodeVersion,
        });
      } else {
        updateCheck('node', {
          status: 'error',
          message: `${nodeVersion} (requires Node.js 18+)`,
        });
      }
      await new Promise(r => setTimeout(r, 200));
      
      // Dependencies check
      updateCheck('dependencies', { status: 'loading' });
      try {
        let deps = await checkDependencies();
        let missing = deps.filter(d => d.required && !d.installed);

        if (missing.length === 0) {
          updateCheck('dependencies', {
            status: 'complete',
            message: 'All dependencies installed',
            details: deps.map(d => `${d.name}: ${d.installed ? '✓' : '✗'} ${d.version || ''}`),
          });
        } else {
          // Try to auto-install missing dependencies
          updateCheck('dependencies', {
            status: 'loading',
            message: `Installing: ${missing.map(d => d.name).join(', ')}`,
            details: [`Using package manager: ${detectPackageManager()}`],
          });

          const installResult = await installDependencies({
            onProgress: (msg) => {
              updateCheck('dependencies', {
                status: 'loading',
                message: msg,
              });
            },
          });

          if (installResult.success) {
            // Re-check after installation
            deps = await checkDependencies();
            missing = deps.filter(d => d.required && !d.installed);

            if (missing.length === 0) {
              updateCheck('dependencies', {
                status: 'complete',
                message: 'Dependencies installed successfully',
                details: deps.map(d => `${d.name}: ${d.installed ? '✓' : '✗'} ${d.version || ''}`),
              });
            } else {
              updateCheck('dependencies', {
                status: 'warning',
                message: 'Some dependencies still missing',
                details: missing.map(d => d.suggestion || `Install ${d.name}`),
              });
            }
          } else {
            updateCheck('dependencies', {
              status: 'error',
              message: `Auto-install failed: ${installResult.message}`,
              details: [
                installResult.error || 'Unknown error',
                getInstallInstructions(),
              ],
            });
          }
        }
      } catch {
        updateCheck('dependencies', {
          status: 'warning',
          message: 'Could not check all dependencies',
        });
      }
      await new Promise(r => setTimeout(r, 200));
      
      // Audio check
      updateCheck('audio', { status: 'loading' });
      try {
        const audio = await getAudioSystemInfo();

        if (audio.soxInstalled) {
          updateCheck('audio', {
            status: 'complete',
            message: `${audio.backend} (sox ${audio.soxVersion})`,
            details: [
              `Input devices: ${audio.inputDevices.length}`,
              `Output devices: ${audio.outputDevices.length}`,
              audio.defaultInput ? `Default input: ${audio.defaultInput.name}` : 'No default input',
              audio.defaultOutput ? `Default output: ${audio.defaultOutput.name}` : 'No default output',
            ],
          });
        } else {
          updateCheck('audio', {
            status: 'error',
            message: 'sox not installed',
            details: ['sox is required for audio recording', getInstallInstructions()],
          });
        }
      } catch {
        updateCheck('audio', {
          status: 'warning',
          message: 'Could not detect audio system',
        });
      }
      await new Promise(r => setTimeout(r, 200));
      
      // Config check
      updateCheck('config', { status: 'loading' });
      try {
        await configManager.initialize({
          configPath: options.config,
          profile: options.profile,
        });

        const validation = configManager.validate();
        const configPath = configManager.getConfigPath();

        if (validation.valid) {
          updateCheck('config', {
            status: 'complete',
            message: configPath,
          });
        } else {
          updateCheck('config', {
            status: 'warning',
            message: 'Configuration has issues',
            details: validation.errors,
          });
        }
      } catch {
        updateCheck('config', {
          status: 'warning',
          message: 'No configuration found',
          details: ['Run "waav init" to create configuration'],
        });
      }
      await new Promise(r => setTimeout(r, 200));
      
      // Gateway check
      updateCheck('gateway', { status: 'loading' });
      try {
        const config = configManager.get();
        const gatewayUrl = options.gateway ?? config.gateway.url;
        const client = new GatewayClient({ baseUrl: gatewayUrl });

        const reachable = await client.ping();

        if (reachable) {
          const status = await client.getStatus();
          updateCheck('gateway', {
            status: 'complete',
            message: `Connected to ${gatewayUrl}`,
            details: [
              `Version: ${status.version}`,
              `Uptime: ${Math.floor(status.uptime_seconds / 60)} minutes`,
              `Connections: ${status.connections}`,
            ],
          });
        } else {
          updateCheck('gateway', {
            status: 'error',
            message: `Cannot reach ${gatewayUrl}`,
            details: [
              'Gateway server is not running or not reachable',
              'Start with: waav server start',
            ],
          });
        }
      } catch {
        updateCheck('gateway', {
          status: 'error',
          message: 'Gateway connection failed',
          details: ['Could not connect to the gateway server'],
        });
      }
      await new Promise(r => setTimeout(r, 200));
      
      // Provider check
      updateCheck('providers', { status: 'loading' });
      try {
        const config = configManager.get();
        const providers = Object.keys(config.providers);

        if (providers.length > 0) {
          const providersWithKeys = providers.filter(p => config.providers[p]?.api_key);
          updateCheck('providers', {
            status: 'complete',
            message: `${providers.length} providers configured`,
            details: [
              `Default STT: ${config.defaults.stt_provider}`,
              `Default TTS: ${config.defaults.tts_provider}`,
              `With API keys: ${providersWithKeys.join(', ') || 'None'}`,
            ],
          });
        } else {
          updateCheck('providers', {
            status: 'warning',
            message: 'No providers configured',
            details: ['Run "waav provider add <provider>" to add providers'],
          });
        }
      } catch {
        updateCheck('providers', {
          status: 'warning',
          message: 'Could not check providers',
        });
      }

      // Calculate summary
      const finalChecks = checks;
      const passed = finalChecks.filter(c => c.status === 'complete').length;
      const warnings = finalChecks.filter(c => c.status === 'warning').length;
      const failed = finalChecks.filter(c => c.status === 'error').length;

      setSummary({ passed, warnings, failed });
      setDone(true);
    };

    runDiagnostics();
  }, [options]);

  // Get status indicator
  const getIndicator = (status: DiagnosticCheck['status']) => {
    switch (status) {
      case 'complete':
        return <Text color="green">✓</Text>;
      case 'error':
        return <Text color="red">✗</Text>;
      case 'warning':
        return <Text color="yellow">⚠</Text>;
      case 'loading':
        return <Text color="cyan">◐</Text>;
      default:
        return <Text dimColor>○</Text>;
    }
  };

  return (
    <Box flexDirection="column" paddingY={1}>
      {/* Header */}
      <Box marginBottom={1}>
        <Text bold>{GRADIENTS.waav('WaaV CLI Diagnostics')}</Text>
      </Box>

      {/* Checks */}
      {checks.map(check => (
        <Box key={check.id} flexDirection="column" marginBottom={1}>
          <Box>
            {getIndicator(check.status)}
            <Text bold> {check.label}</Text>
            {check.message && (
              <Text dimColor={check.status === 'pending'}> - {check.message}</Text>
            )}
          </Box>
          {check.details && check.status !== 'pending' && (
            <Box marginLeft={3} flexDirection="column">
              {check.details.map((detail, i) => (
                <Text key={i} dimColor>{detail}</Text>
              ))}
            </Box>
          )}
        </Box>
      ))}

      {/* Summary */}
      {done && (
        <Box marginTop={1} flexDirection="column">
          <Text bold>Summary</Text>
          <Box marginLeft={2}>
            <Text color="green">{summary.passed} passed</Text>
            <Text dimColor> | </Text>
            <Text color="yellow">{summary.warnings} warnings</Text>
            <Text dimColor> | </Text>
            <Text color="red">{summary.failed} failed</Text>
          </Box>

          {summary.failed > 0 && (
            <Box marginTop={1}>
              <Text dimColor>
                Run "waav init" to fix configuration issues
              </Text>
            </Box>
          )}

          {summary.failed === 0 && summary.warnings === 0 && (
            <Box marginTop={1}>
              <SuccessIndicator label="All checks passed! WaaV CLI is ready to use." />
            </Box>
          )}
        </Box>
      )}
    </Box>
  );
};

/**
 * Doctor command handler
 */
export async function doctorCommand(options: GlobalOptions): Promise<void> {
  // JSON output
  if (options.json) {
    const results: Record<string, unknown> = {
      platform: getPlatformInfo(),
      node: process.version,
    };

    try {
      results['dependencies'] = await checkDependencies();
    } catch {
      results['dependencies'] = null;
    }

    try {
      results['audio'] = await getAudioSystemInfo();
    } catch {
      results['audio'] = null;
    }

    console.log(JSON.stringify(results, null, 2));
    return;
  }

  // Render doctor display
  const { waitUntilExit } = render(<DoctorDisplay options={options} />);
  await waitUntilExit();
}

export default doctorCommand;
