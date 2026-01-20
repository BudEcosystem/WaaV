/**
 * WaaV CLI Server Start Command
 *
 * Starts the WaaV Gateway server process with proper
 * lifecycle management, logging, and health checks.
 */

import React, { useState, useEffect } from 'react';
import { render, Text, Box, useApp } from 'ink';
import { spawn, ChildProcess } from 'node:child_process';
import { createWriteStream } from 'node:fs';

import { configManager } from '../../core/config/manager.js';
import { GatewayClient } from '../../core/gateway/client.js';
import { logger } from '../../utils/logger.js';
import {
  findGatewayBinary,
  getRecommendedRunMethod,
} from '../../utils/binary-finder.js';
import {
  writePidFile,
  readPidFile,
  isProcessRunning,
  getLogPath,
} from '../../utils/pid-manager.js';
import {
  Spinner,
  SuccessIndicator,
  ErrorIndicator,
  WarningIndicator,
} from '../../components/common/index.js';
import { GRADIENTS } from '../../components/branding/index.js';
import type { GlobalOptions } from '../../cli.js';

interface ServerStartOptions extends GlobalOptions {
  detach?: boolean;
  port?: number;
}

interface StartState {
  phase: 'checking' | 'starting' | 'waiting' | 'healthy' | 'error' | 'already_running' | 'no_binary';
  message: string;
  details?: string;
  pid?: number;
  port?: number;
  error?: string;
  binaryPath?: string;
}

/**
 * Server Start Component (Interactive UI)
 */
const ServerStart: React.FC<{
  options: ServerStartOptions;
  onComplete: (success: boolean) => void;
}> = ({ options, onComplete }) => {
  useApp();  // For Ink lifecycle
  const [state, setState] = useState<StartState>({
    phase: 'checking',
    message: 'Checking for existing gateway process...',
  });

  useEffect(() => {
    let mounted = true;
    let childProcess: ChildProcess | null = null;

    const startServer = async () => {
      try {
        // Initialize config
        await configManager.initialize({
          configPath: options.config,
          profile: options.profile,
        });

        const config = configManager.get();
        const gatewayUrl = options.gateway ?? config.gateway.url;
        const port = options.port ?? parseInt(new URL(gatewayUrl).port || '3001', 10);

        // Check if already running
        const pidData = readPidFile();
        if (pidData) {
          const isRunning = await isProcessRunning(pidData.pid);
          if (isRunning) {
            if (mounted) {
              setState({
                phase: 'already_running',
                message: 'Gateway is already running',
                pid: pidData.pid,
                port: pidData.port,
                binaryPath: pidData.binaryPath,
              });
            }
            setTimeout(() => onComplete(true), 1500);
            return;
          }
        }

        // Find binary
        if (mounted) {
          setState({
            phase: 'checking',
            message: 'Looking for gateway binary...',
          });
        }

        const binaryResult = await findGatewayBinary();

        if (!binaryResult) {
          const recommended = await getRecommendedRunMethod();
          if (mounted) {
            setState({
              phase: 'no_binary',
              message: 'Gateway binary not found',
              details: recommended.method === 'none'
                ? 'Please build or install the WaaV Gateway first'
                : `Recommendation: ${recommended.command}`,
              error: recommended.description,
            });
          }
          setTimeout(() => onComplete(false), 3000);
          return;
        }

        // Start the process
        if (mounted) {
          setState({
            phase: 'starting',
            message: 'Starting gateway server...',
            binaryPath: binaryResult.path,
            port,
          });
        }

        // Build command arguments
        const args: string[] = [];
        if (options.config) {
          args.push('-c', options.config);
        }

        // Set up log file for detached mode
        const logPath = getLogPath();
        const logStream = options.detach ? createWriteStream(logPath, { flags: 'a' }) : null;

        // Spawn the process
        childProcess = spawn(binaryResult.path, args, {
          detached: options.detach ?? false,
          stdio: options.detach
            ? ['ignore', logStream ? 'pipe' : 'ignore', logStream ? 'pipe' : 'ignore']
            : ['ignore', 'pipe', 'pipe'],
          env: {
            ...process.env,
            RUST_LOG: process.env['RUST_LOG'] ?? 'info',
          },
        });

        const pid = childProcess.pid;
        if (!pid) {
          throw new Error('Failed to get process PID');
        }

        // Pipe output to log file in detached mode
        if (options.detach && logStream) {
          childProcess.stdout?.pipe(logStream);
          childProcess.stderr?.pipe(logStream);
        }

        // Write PID file
        writePidFile({
          pid,
          startTime: Date.now(),
          binaryPath: binaryResult.path,
          configPath: options.config,
          port,
        });

        // Unref for detached mode
        if (options.detach) {
          childProcess.unref();
        }

        // Wait for server to become healthy
        if (mounted) {
          setState({
            phase: 'waiting',
            message: 'Waiting for server to become healthy...',
            pid,
            port,
            binaryPath: binaryResult.path,
          });
        }

        // Health check loop
        const client = new GatewayClient({ baseUrl: gatewayUrl });
        const maxAttempts = 30;
        const delayMs = 500;
        let healthy = false;

        for (let i = 0; i < maxAttempts; i++) {
          await new Promise(resolve => setTimeout(resolve, delayMs));

          // Check if process died
          const stillRunning = await isProcessRunning(pid);
          if (!stillRunning) {
            throw new Error('Server process exited unexpectedly');
          }

          // Try health check
          try {
            const status = await client.getStatus();
            if (status) {
              healthy = true;
              break;
            }
          } catch {
            // Not ready yet
          }
        }

        if (!healthy) {
          throw new Error('Server failed to become healthy within timeout');
        }

        // Success
        if (mounted) {
          setState({
            phase: 'healthy',
            message: 'Gateway server started successfully!',
            pid,
            port,
            binaryPath: binaryResult.path,
          });
        }

        setTimeout(() => onComplete(true), 1500);
      } catch (error) {
        const errMsg = error instanceof Error ? error.message : String(error);
        if (mounted) {
          setState({
            phase: 'error',
            message: 'Failed to start gateway',
            error: errMsg,
          });
        }
        setTimeout(() => onComplete(false), 3000);
      }
    };

    startServer();

    return () => {
      mounted = false;
    };
  }, [options, onComplete]);

  // Render based on state
  const renderContent = () => {
    switch (state.phase) {
      case 'checking':
      case 'starting':
      case 'waiting':
        return (
          <Box flexDirection="column">
            <Spinner label={state.message} color="cyan" />
            {state.binaryPath && (
              <Box marginTop={1}>
                <Text dimColor>Binary: {state.binaryPath}</Text>
              </Box>
            )}
            {state.port && (
              <Box>
                <Text dimColor>Port: {state.port}</Text>
              </Box>
            )}
          </Box>
        );

      case 'healthy':
        return (
          <Box flexDirection="column">
            <SuccessIndicator label={state.message} />
            <Box marginTop={1} flexDirection="column">
              <Box>
                <Text dimColor>PID: </Text>
                <Text>{state.pid}</Text>
              </Box>
              <Box>
                <Text dimColor>Port: </Text>
                <Text>{state.port}</Text>
              </Box>
              <Box>
                <Text dimColor>Binary: </Text>
                <Text>{state.binaryPath}</Text>
              </Box>
            </Box>
            <Box marginTop={1}>
              <Text dimColor>Use "waav server status" to check status</Text>
            </Box>
          </Box>
        );

      case 'already_running':
        return (
          <Box flexDirection="column">
            <WarningIndicator label={state.message} />
            <Box marginTop={1} flexDirection="column">
              <Box>
                <Text dimColor>PID: </Text>
                <Text>{state.pid}</Text>
              </Box>
              <Box>
                <Text dimColor>Port: </Text>
                <Text>{state.port}</Text>
              </Box>
            </Box>
            <Box marginTop={1}>
              <Text dimColor>Use "waav server stop" to stop it first</Text>
            </Box>
          </Box>
        );

      case 'no_binary':
        return (
          <Box flexDirection="column">
            <ErrorIndicator label={state.message} />
            <Box marginTop={1}>
              <Text dimColor>{state.details}</Text>
            </Box>
            <Box marginTop={1}>
              <Text>To build the gateway:</Text>
            </Box>
            <Box marginLeft={2} flexDirection="column">
              <Text dimColor>cd /path/to/waav/gateway</Text>
              <Text dimColor>cargo build --release</Text>
            </Box>
          </Box>
        );

      case 'error':
        return (
          <Box flexDirection="column">
            <ErrorIndicator label={state.message} />
            <Box marginTop={1}>
              <Text color="red">{state.error}</Text>
            </Box>
            <Box marginTop={1}>
              <Text dimColor>Check logs at: {getLogPath()}</Text>
            </Box>
          </Box>
        );
    }
  };

  return (
    <Box flexDirection="column" paddingY={1}>
      <Box marginBottom={1}>
        <Text bold>{GRADIENTS.waav('WaaV Gateway')}</Text>
        <Text> - Server Start</Text>
      </Box>
      {renderContent()}
    </Box>
  );
};

/**
 * Server start command handler
 */
export async function serverStartCommand(options: ServerStartOptions): Promise<void> {
  // Quick mode for scripting (no interactive UI)
  if (options.quiet || options.json) {
    await startServerQuiet(options);
    return;
  }

  // Interactive mode
  return new Promise((resolve) => {
    const { waitUntilExit } = render(
      <ServerStart
        options={options}
        onComplete={(success) => {
          if (!success) {
            process.exitCode = 1;
          }
          setTimeout(() => resolve(), 100);
        }}
      />
    );
    waitUntilExit().then(() => resolve());
  });
}

/**
 * Non-interactive server start
 */
async function startServerQuiet(options: ServerStartOptions): Promise<void> {
  try {
    await configManager.initialize({
      configPath: options.config,
      profile: options.profile,
    });

    const config = configManager.get();
    const gatewayUrl = options.gateway ?? config.gateway.url;
    const port = options.port ?? parseInt(new URL(gatewayUrl).port || '3001', 10);

    // Check if already running
    const pidData = readPidFile();
    if (pidData) {
      const isRunning = await isProcessRunning(pidData.pid);
      if (isRunning) {
        if (options.json) {
          console.log(JSON.stringify({ status: 'already_running', pid: pidData.pid, port: pidData.port }));
        } else {
          logger.warn(`Gateway already running (PID: ${pidData.pid})`);
        }
        return;
      }
    }

    // Find binary
    const binaryResult = await findGatewayBinary();
    if (!binaryResult) {
      if (options.json) {
        console.log(JSON.stringify({ status: 'error', message: 'Binary not found' }));
      } else {
        logger.error('Gateway binary not found');
      }
      process.exitCode = 1;
      return;
    }

    // Start process
    const args: string[] = [];
    if (options.config) {
      args.push('-c', options.config);
    }

    const logPath = getLogPath();
    const logStream = createWriteStream(logPath, { flags: 'a' });

    const child = spawn(binaryResult.path, args, {
      detached: true,
      stdio: ['ignore', 'pipe', 'pipe'],
      env: {
        ...process.env,
        RUST_LOG: process.env['RUST_LOG'] ?? 'info',
      },
    });

    const pid = child.pid;
    if (!pid) {
      throw new Error('Failed to get process PID');
    }

    child.stdout?.pipe(logStream);
    child.stderr?.pipe(logStream);
    child.unref();

    writePidFile({
      pid,
      startTime: Date.now(),
      binaryPath: binaryResult.path,
      configPath: options.config,
      port,
    });

    // Wait for health
    const client = new GatewayClient({ baseUrl: gatewayUrl });
    let healthy = false;

    for (let i = 0; i < 30; i++) {
      await new Promise(resolve => setTimeout(resolve, 500));
      try {
        const status = await client.getStatus();
        if (status) {
          healthy = true;
          break;
        }
      } catch {
        // Not ready yet
      }
    }

    if (options.json) {
      console.log(JSON.stringify({
        status: healthy ? 'started' : 'starting',
        pid,
        port,
        binary: binaryResult.path,
      }));
    } else {
      if (healthy) {
        logger.success(`Gateway started (PID: ${pid}, Port: ${port})`);
      } else {
        logger.warn(`Gateway starting... (PID: ${pid})`);
      }
    }
  } catch (error) {
    const errMsg = error instanceof Error ? error.message : String(error);
    if (options.json) {
      console.log(JSON.stringify({ status: 'error', message: errMsg }));
    } else {
      logger.error(`Failed to start: ${errMsg}`);
    }
    process.exitCode = 1;
  }
}

export default serverStartCommand;
