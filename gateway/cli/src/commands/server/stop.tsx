/**
 * WaaV CLI Server Stop Command
 *
 * Stops the WaaV Gateway server process gracefully
 * with timeout-based fallback to force kill.
 */

import React, { useState, useEffect } from 'react';
import { render, Text, Box, useApp, useInput } from 'ink';

import { configManager } from '../../core/config/manager.js';
import { logger } from '../../utils/logger.js';
import {
  readPidFile,
  isProcessRunning,
  stopGatewayProcess,
  removePidFile,
  type PidData,
} from '../../utils/pid-manager.js';
import {
  Spinner,
  SuccessIndicator,
  ErrorIndicator,
  WarningIndicator,
  Confirm,
} from '../../components/common/index.js';
import { GRADIENTS } from '../../components/branding/index.js';
import type { GlobalOptions } from '../../cli.js';

interface ServerStopOptions extends GlobalOptions {
  force?: boolean;
  timeout?: number;
}

interface StopState {
  phase: 'checking' | 'confirming' | 'stopping' | 'stopped' | 'not_running' | 'error';
  message: string;
  pid?: number;
  error?: string;
}

/**
 * Server Stop Component (Interactive UI)
 */
const ServerStop: React.FC<{
  options: ServerStopOptions;
  onComplete: (success: boolean) => void;
}> = ({ options, onComplete }) => {
  useApp();  // For Ink lifecycle
  const [state, setState] = useState<StopState>({
    phase: 'checking',
    message: 'Checking gateway process status...',
  });
  const [pidData, setPidData] = useState<PidData | null>(null);
  const [showConfirm, setShowConfirm] = useState(false);

  // Initial check
  useEffect(() => {
    const checkProcess = async () => {
      try {
        await configManager.initialize({
          configPath: options.config,
          profile: options.profile,
        });

        const data = readPidFile();

        if (!data) {
          setState({
            phase: 'not_running',
            message: 'No gateway process found',
          });
          setTimeout(() => onComplete(true), 1500);
          return;
        }

        const running = await isProcessRunning(data.pid);

        if (!running) {
          // Stale PID file
          removePidFile();
          setState({
            phase: 'not_running',
            message: 'Gateway not running (cleaned up stale PID file)',
          });
          setTimeout(() => onComplete(true), 1500);
          return;
        }

        setPidData(data);

        // If force flag, skip confirmation
        if (options.force) {
          await performStop(data.pid);
        } else {
          setState({
            phase: 'confirming',
            message: `Gateway is running (PID: ${data.pid})`,
            pid: data.pid,
          });
          setShowConfirm(true);
        }
      } catch (error) {
        const errMsg = error instanceof Error ? error.message : String(error);
        setState({
          phase: 'error',
          message: 'Failed to check gateway status',
          error: errMsg,
        });
        setTimeout(() => onComplete(false), 3000);
      }
    };

    checkProcess();
  }, [options]);

  // Perform the stop
  const performStop = async (pid?: number) => {
    const targetPid = pid ?? pidData?.pid;
    if (!targetPid) {
      return;
    }

    setShowConfirm(false);
    setState({
      phase: 'stopping',
      message: 'Stopping gateway process...',
      pid: targetPid,
    });

    try {
      const timeoutMs = (options.timeout ?? 10) * 1000;
      const result = await stopGatewayProcess(timeoutMs);

      if (result.success) {
        setState({
          phase: 'stopped',
          message: result.message,
          pid: targetPid,
        });
        setTimeout(() => onComplete(true), 1500);
      } else {
        setState({
          phase: 'error',
          message: 'Failed to stop gateway',
          error: result.message,
          pid: targetPid,
        });
        setTimeout(() => onComplete(false), 3000);
      }
    } catch (error) {
      const errMsg = error instanceof Error ? error.message : String(error);
      setState({
        phase: 'error',
        message: 'Error stopping gateway',
        error: errMsg,
        pid: targetPid,
      });
      setTimeout(() => onComplete(false), 3000);
    }
  };

  // Handle confirmation
  const handleConfirm = (confirmed: boolean) => {
    if (confirmed) {
      performStop();
    } else {
      setState({
        phase: 'not_running',
        message: 'Stop cancelled',
      });
      setTimeout(() => onComplete(true), 1000);
    }
  };

  // Allow ESC to cancel
  useInput((input, key) => {
    if (key.escape && showConfirm) {
      handleConfirm(false);
    }
  });

  // Render based on state
  const renderContent = () => {
    switch (state.phase) {
      case 'checking':
        return <Spinner label={state.message} color="cyan" />;

      case 'confirming':
        return (
          <Box flexDirection="column">
            <WarningIndicator label={state.message} />
            {showConfirm && (
              <Box marginTop={1}>
                <Confirm
                  message="Stop the gateway server?"
                  onConfirm={handleConfirm}
                />
              </Box>
            )}
          </Box>
        );

      case 'stopping':
        return (
          <Box flexDirection="column">
            <Spinner label={state.message} color="yellow" />
            {state.pid && (
              <Box marginTop={1}>
                <Text dimColor>PID: {state.pid}</Text>
              </Box>
            )}
          </Box>
        );

      case 'stopped':
        return (
          <Box flexDirection="column">
            <SuccessIndicator label={state.message} />
            {state.pid && (
              <Box marginTop={1}>
                <Text dimColor>PID {state.pid} terminated</Text>
              </Box>
            )}
          </Box>
        );

      case 'not_running':
        return <WarningIndicator label={state.message} />;

      case 'error':
        return (
          <Box flexDirection="column">
            <ErrorIndicator label={state.message} />
            {state.error && (
              <Box marginTop={1}>
                <Text color="red">{state.error}</Text>
              </Box>
            )}
            <Box marginTop={1}>
              <Text dimColor>Try "waav server stop --force" to force kill</Text>
            </Box>
          </Box>
        );
    }
  };

  return (
    <Box flexDirection="column" paddingY={1}>
      <Box marginBottom={1}>
        <Text bold>{GRADIENTS.waav('WaaV Gateway')}</Text>
        <Text> - Server Stop</Text>
      </Box>
      {renderContent()}
    </Box>
  );
};

/**
 * Server stop command handler
 */
export async function serverStopCommand(options: ServerStopOptions): Promise<void> {
  // Quick mode for scripting (no interactive UI)
  if (options.quiet || options.json) {
    await stopServerQuiet(options);
    return;
  }

  // Interactive mode
  return new Promise((resolve) => {
    const { waitUntilExit } = render(
      <ServerStop
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
 * Non-interactive server stop
 */
async function stopServerQuiet(options: ServerStopOptions): Promise<void> {
  try {
    await configManager.initialize({
      configPath: options.config,
      profile: options.profile,
    });

    const pidData = readPidFile();

    if (!pidData) {
      if (options.json) {
        console.log(JSON.stringify({ status: 'not_running', message: 'No gateway process found' }));
      } else {
        logger.warn('No gateway process found');
      }
      return;
    }

    const running = await isProcessRunning(pidData.pid);

    if (!running) {
      removePidFile();
      if (options.json) {
        console.log(JSON.stringify({ status: 'not_running', message: 'Gateway not running' }));
      } else {
        logger.warn('Gateway not running (removed stale PID file)');
      }
      return;
    }

    const timeoutMs = (options.timeout ?? 10) * 1000;
    const result = await stopGatewayProcess(timeoutMs);

    if (options.json) {
      console.log(JSON.stringify({
        status: result.success ? 'stopped' : 'error',
        pid: pidData.pid,
        message: result.message,
      }));
    } else {
      if (result.success) {
        logger.success(result.message);
      } else {
        logger.error(result.message);
        process.exitCode = 1;
      }
    }
  } catch (error) {
    const errMsg = error instanceof Error ? error.message : String(error);
    if (options.json) {
      console.log(JSON.stringify({ status: 'error', message: errMsg }));
    } else {
      logger.error(`Failed to stop: ${errMsg}`);
    }
    process.exitCode = 1;
  }
}

export default serverStopCommand;
