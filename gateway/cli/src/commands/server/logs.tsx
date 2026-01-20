/**
 * WaaV CLI Server Logs Command (Enhanced)
 *
 * htop-style log viewer with:
 * - Interactive filter input
 * - Provider-specific filtering
 * - Error/warning counters
 * - Jump to first error
 * - Export filtered logs
 * - Timestamp relative/absolute toggle
 * - Vim-style navigation via ScrollableList
 */

import React, { useState, useEffect, useCallback, useRef, useMemo } from 'react';
import { render, Text, Box, useInput, useApp, useStdout } from 'ink';
import { createReadStream, existsSync, statSync, readFileSync, watchFile, unwatchFile, writeFileSync } from 'node:fs';
import { createInterface } from 'node:readline';
import { join } from 'node:path';
import { homedir } from 'node:os';

import { configManager } from '../../core/config/manager.js';
import { logger } from '../../utils/logger.js';
import { getLogPath, readPidFile, isProcessRunning } from '../../utils/pid-manager.js';
import {
  Spinner,
  ErrorIndicator,
  StatusBadge,
} from '../../components/common/index.js';
import { ScrollableList, type ScrollableListItem } from '../../components/common/ScrollableList.js';
import { GRADIENTS } from '../../components/branding/index.js';
import type { GlobalOptions } from '../../cli.js';

interface ServerLogsOptions extends GlobalOptions {
  follow?: boolean;
  lines?: string;
  level?: string;
  filter?: string;
  provider?: string;
}

type LogLevel = 'error' | 'warn' | 'info' | 'debug' | 'trace';

interface LogLine {
  id: string;
  text: string;
  level: LogLevel | null;
  timestamp: Date;
  provider?: string;
  rawLine: string;
}

/**
 * Parse log level from a line
 */
function parseLogLevel(line: string): LogLevel | null {
  const lowerLine = line.toLowerCase();
  if (lowerLine.includes('error') || lowerLine.includes('err]')) {
    return 'error';
  }
  if (lowerLine.includes('warn') || lowerLine.includes('warning')) {
    return 'warn';
  }
  if (lowerLine.includes('info') || lowerLine.includes('inf]')) {
    return 'info';
  }
  if (lowerLine.includes('debug') || lowerLine.includes('dbg]')) {
    return 'debug';
  }
  if (lowerLine.includes('trace') || lowerLine.includes('trc]')) {
    return 'trace';
  }
  return null;
}

/**
 * Parse provider from a log line
 */
function parseProvider(line: string): string | undefined {
  // Match patterns like [deepgram], [elevenlabs], [voice_mgr], etc.
  const providerMatch = line.match(/\[([a-z_]+)\]/i);
  if (providerMatch && providerMatch[1]) {
    return providerMatch[1].toLowerCase();
  }
  return undefined;
}

/**
 * Parse timestamp from log line
 */
function parseTimestamp(line: string): Date {
  // Try to parse ISO timestamp or common log formats
  const isoMatch = line.match(/(\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}(?:\.\d+)?)/);
  if (isoMatch && isoMatch[1]) {
    return new Date(isoMatch[1]);
  }

  // Try HH:MM:SS.mmm format
  const timeMatch = line.match(/(\d{2}:\d{2}:\d{2}(?:\.\d+)?)/);
  if (timeMatch && timeMatch[1]) {
    const today = new Date();
    const parts = timeMatch[1].split(':');
    const hours = parts[0] ?? '00';
    const minutes = parts[1] ?? '00';
    const rest = parts[2] ?? '00';
    const secParts = rest.split('.');
    const seconds = secParts[0] ?? '00';
    const ms = secParts[1] ?? '0';
    today.setHours(parseInt(hours, 10));
    today.setMinutes(parseInt(minutes, 10));
    today.setSeconds(parseInt(seconds, 10));
    today.setMilliseconds(parseInt(ms, 10));
    return today;
  }

  return new Date();
}

/**
 * Format timestamp for display
 */
function formatTimestamp(date: Date, relative: boolean): string {
  if (relative) {
    const now = new Date();
    const diff = now.getTime() - date.getTime();

    if (diff < 1000) return 'just now';
    if (diff < 60000) return `${Math.floor(diff / 1000)}s ago`;
    if (diff < 3600000) return `${Math.floor(diff / 60000)}m ago`;
    if (diff < 86400000) return `${Math.floor(diff / 3600000)}h ago`;
    return `${Math.floor(diff / 86400000)}d ago`;
  }

  const hours = date.getHours().toString().padStart(2, '0');
  const minutes = date.getMinutes().toString().padStart(2, '0');
  const seconds = date.getSeconds().toString().padStart(2, '0');
  const ms = date.getMilliseconds().toString().padStart(3, '0');
  return `${hours}:${minutes}:${seconds}.${ms}`;
}

/**
 * Get color for log level
 */
function getLevelColor(level: LogLevel | null): string | undefined {
  switch (level) {
    case 'error':
      return 'red';
    case 'warn':
      return 'yellow';
    case 'info':
      return 'cyan';
    case 'debug':
      return 'gray';
    case 'trace':
      return 'gray';
    default:
      return undefined;
  }
}

/**
 * Should include log line based on level filter
 */
function shouldIncludeLevel(lineLevel: LogLevel | null, filterLevel: string | undefined): boolean {
  if (!filterLevel) {
    return true;
  }

  const levels: LogLevel[] = ['error', 'warn', 'info', 'debug', 'trace'];
  const filterIndex = levels.indexOf(filterLevel.toLowerCase() as LogLevel);
  const lineIndex = lineLevel ? levels.indexOf(lineLevel) : 2; // Default to info

  if (filterIndex === -1) {
    return true;
  }

  return lineIndex <= filterIndex;
}

/**
 * Known providers for filtering
 */
const KNOWN_PROVIDERS = [
  'deepgram',
  'elevenlabs',
  'cartesia',
  'google',
  'azure',
  'voice_mgr',
  'ws_handler',
  'livekit',
  'auth',
  'server',
] as const;

/**
 * Enhanced Server Logs Component with htop-style UI
 */
const ServerLogs: React.FC<{
  options: ServerLogsOptions;
  onComplete: () => void;
}> = ({ options, onComplete }) => {
  useApp();
  const { stdout } = useStdout();

  // State
  const [logs, setLogs] = useState<LogLine[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [following, setFollowing] = useState(options.follow ?? false);
  const [serverRunning, setServerRunning] = useState(false);
  const [logPath, setLogPath] = useState<string>('');

  // Filter state
  const [filterMode, setFilterMode] = useState(false);
  const [textFilter, setTextFilter] = useState(options.filter ?? '');
  const [levelFilter, setLevelFilter] = useState<LogLevel | ''>(
    (options.level as LogLevel) ?? ''
  );
  const [providerFilter, setProviderFilter] = useState<string>(
    options.provider ?? ''
  );

  // Display state
  const [relativeTime, setRelativeTime] = useState(false);
  const [selectedIndex, setSelectedIndex] = useState(0);

  // Refs
  const lastSizeRef = useRef<number>(0);
  const logIdCounter = useRef(0);

  // Dimensions
  const maxLines = parseInt(options.lines ?? '500', 10);
  const terminalHeight = stdout?.rows ?? 24;
  const listHeight = Math.max(10, terminalHeight - 12);

  // Parse and add a log line
  const createLogLine = useCallback((text: string): LogLine | null => {
    const trimmed = text.trim();
    if (!trimmed) {
      return null;
    }

    return {
      id: `log-${logIdCounter.current++}`,
      text: trimmed,
      level: parseLogLevel(trimmed),
      timestamp: parseTimestamp(trimmed),
      provider: parseProvider(trimmed),
      rawLine: trimmed,
    };
  }, []);

  const addLogLine = useCallback((text: string) => {
    const logLine = createLogLine(text);
    if (!logLine) return;

    setLogs((prev) => {
      const newLogs = [...prev, logLine];
      if (newLogs.length > maxLines) {
        return newLogs.slice(-maxLines);
      }
      return newLogs;
    });
  }, [createLogLine, maxLines]);

  // Filter logs
  const filteredLogs = useMemo(() => {
    return logs.filter((log) => {
      // Level filter
      if (levelFilter && !shouldIncludeLevel(log.level, levelFilter)) {
        return false;
      }

      // Text filter
      if (textFilter && !log.text.toLowerCase().includes(textFilter.toLowerCase())) {
        return false;
      }

      // Provider filter
      if (providerFilter && log.provider !== providerFilter.toLowerCase()) {
        return false;
      }

      return true;
    });
  }, [logs, levelFilter, textFilter, providerFilter]);

  // Count errors/warnings
  const errorCount = useMemo(() =>
    filteredLogs.filter((l) => l.level === 'error').length
  , [filteredLogs]);

  const warnCount = useMemo(() =>
    filteredLogs.filter((l) => l.level === 'warn').length
  , [filteredLogs]);

  // Convert to ScrollableListItems
  const listItems: ScrollableListItem[] = useMemo(() => {
    return filteredLogs.map((log) => ({
      id: log.id,
      content: log.text,
      level: log.level ?? undefined,
      timestamp: log.timestamp,
      color: getLevelColor(log.level),
      meta: { provider: log.provider, rawLine: log.rawLine },
    }));
  }, [filteredLogs]);

  // Find first error index
  const firstErrorIndex = useMemo(() => {
    return filteredLogs.findIndex((l) => l.level === 'error');
  }, [filteredLogs]);

  // Export logs to file
  const exportLogs = useCallback(() => {
    const exportPath = join(homedir(), `waav-logs-${Date.now()}.txt`);
    const content = filteredLogs.map((l) => l.rawLine).join('\n');
    writeFileSync(exportPath, content);
    logger.info(`Logs exported to: ${exportPath}`);
  }, [filteredLogs]);

  // Initialize
  useEffect(() => {
    const init = async () => {
      try {
        await configManager.initialize({
          configPath: options.config,
          profile: options.profile,
        });

        const path = getLogPath();
        setLogPath(path);

        // Check if server is running
        const pidData = readPidFile();
        if (pidData) {
          const running = await isProcessRunning(pidData.pid);
          setServerRunning(running);
        }

        // Check if log file exists
        if (!existsSync(path)) {
          setError(`Log file not found: ${path}`);
          setLoading(false);
          return;
        }

        // Read last N lines
        const content = readFileSync(path, 'utf-8');
        const lines = content.split('\n').filter((l) => l.trim());
        const lastLines = lines.slice(-maxLines);

        const initialLogs: LogLine[] = [];
        for (const line of lastLines) {
          const logLine = createLogLine(line);
          if (logLine) {
            initialLogs.push(logLine);
          }
        }
        setLogs(initialLogs);

        lastSizeRef.current = statSync(path).size;
        setLoading(false);

        // Set up file watching if following
        if (options.follow) {
          watchFile(path, { interval: 100 }, (curr, prev) => {
            if (curr.size > prev.size) {
              const stream = createReadStream(path, {
                start: lastSizeRef.current,
                end: curr.size,
              });

              const rl = createInterface({
                input: stream,
                crlfDelay: Infinity,
              });

              rl.on('line', (line) => {
                addLogLine(line);
              });

              lastSizeRef.current = curr.size;
            }
          });
        }
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
        setLoading(false);
      }
    };

    init();

    return () => {
      if (logPath && options.follow) {
        unwatchFile(logPath);
      }
    };
  }, [options, createLogLine, addLogLine, maxLines, logPath]);

  // Handle keyboard input
  useInput((input, key) => {
    // Filter mode input
    if (filterMode) {
      if (key.escape || key.return) {
        setFilterMode(false);
        return;
      }
      if (key.backspace || key.delete) {
        setTextFilter((prev) => prev.slice(0, -1));
        return;
      }
      if (input && !key.ctrl && !key.meta) {
        setTextFilter((prev) => prev + input);
      }
      return;
    }

    // Global shortcuts
    if (key.escape || input === 'q' || (key.ctrl && input === 'c')) {
      if (logPath && options.follow) {
        unwatchFile(logPath);
      }
      onComplete();
      return;
    }

    // Toggle follow mode
    if (input === 'f' || input === 'F') {
      setFollowing((prev) => !prev);
      return;
    }

    // Clear logs
    if (input === 'c' || input === 'C') {
      setLogs([]);
      return;
    }

    // Enter filter mode
    if (input === '/') {
      setFilterMode(true);
      return;
    }

    // Clear filter
    if (key.escape) {
      setTextFilter('');
      setProviderFilter('');
      setLevelFilter('');
      return;
    }

    // Cycle level filter
    if (input === 'l' || input === 'L') {
      const levels: (LogLevel | '')[] = ['', 'error', 'warn', 'info', 'debug', 'trace'];
      const currentIndex = levels.indexOf(levelFilter);
      const nextIndex = (currentIndex + 1) % levels.length;
      const nextLevel = levels[nextIndex] ?? '';
      setLevelFilter(nextLevel);
      return;
    }

    // Cycle provider filter
    if (input === 'p' || input === 'P') {
      const providers: string[] = ['', ...KNOWN_PROVIDERS];
      const currentIndex = providers.indexOf(providerFilter);
      const nextIndex = (currentIndex + 1) % providers.length;
      const nextProvider = providers[nextIndex] ?? '';
      setProviderFilter(nextProvider);
      return;
    }

    // Toggle relative/absolute time
    if (input === 't' || input === 'T') {
      setRelativeTime((prev) => !prev);
      return;
    }

    // Jump to first error
    if (input === 'E' && key.shift) {
      if (firstErrorIndex >= 0) {
        setSelectedIndex(firstErrorIndex);
      }
      return;
    }

    // Export logs
    if (input === 'e') {
      exportLogs();
      return;
    }
  }, { isActive: !loading && !error });

  if (loading) {
    return (
      <Box flexDirection="column" paddingY={1}>
        <Box marginBottom={1}>
          <Text bold>{GRADIENTS.waav('WaaV Gateway')}</Text>
          <Text> - Server Logs</Text>
        </Box>
        <Spinner label="Loading logs..." color="cyan" />
      </Box>
    );
  }

  if (error) {
    return (
      <Box flexDirection="column" paddingY={1}>
        <Box marginBottom={1}>
          <Text bold>{GRADIENTS.waav('WaaV Gateway')}</Text>
          <Text> - Server Logs</Text>
        </Box>
        <ErrorIndicator label="Error loading logs" />
        <Box marginTop={1}>
          <Text color="red">{error}</Text>
        </Box>
        <Box marginTop={1}>
          <Text dimColor>Start the server first: waav server start</Text>
        </Box>
      </Box>
    );
  }

  return (
    <Box flexDirection="column">
      {/* Header with title and status */}
      <Box
        borderStyle="single"
        borderColor="cyan"
        paddingX={1}
        marginBottom={0}
      >
        <Box flexGrow={1}>
          <Text bold>{GRADIENTS.waav('WaaV Gateway')}</Text>
          <Text bold> Logs</Text>
        </Box>
        <Box>
          <StatusBadge status={serverRunning ? 'online' : 'offline'} compact />
          {following && (
            <Box marginLeft={1}>
              <Text color="green" bold>[FOLLOW]</Text>
            </Box>
          )}
        </Box>
      </Box>

      {/* Filter bar */}
      <Box
        borderStyle="single"
        borderColor="gray"
        borderTop={false}
        paddingX={1}
      >
        <Box width={20}>
          <Text dimColor>Filter: </Text>
          {filterMode ? (
            <Box>
              <Text color="yellow">{textFilter}</Text>
              <Text dimColor>▌</Text>
            </Box>
          ) : (
            <Text color={textFilter ? 'yellow' : 'gray'}>
              {textFilter || '(press / to filter)'}
            </Text>
          )}
        </Box>
        <Box marginLeft={2} width={15}>
          <Text dimColor>Level: </Text>
          <Text color={levelFilter ? 'cyan' : 'gray'}>
            [{levelFilter ? levelFilter.toUpperCase() : 'ALL'}]
          </Text>
        </Box>
        <Box marginLeft={2} width={20}>
          <Text dimColor>Provider: </Text>
          <Text color={providerFilter ? 'magenta' : 'gray'}>
            [{providerFilter || 'all'}]
          </Text>
        </Box>
        <Box marginLeft={2}>
          <Text dimColor>Time: </Text>
          <Text color="gray">[{relativeTime ? 'relative' : 'absolute'}]</Text>
        </Box>
      </Box>

      {/* Custom log item renderer */}
      <Box flexDirection="column" height={listHeight}>
        {listItems.length === 0 ? (
          <Box paddingX={1} paddingY={1}>
            <Text dimColor>No logs to display</Text>
          </Box>
        ) : (
          <ScrollableList
            items={listItems}
            height={listHeight}
            showLineNumbers
            lineNumberWidth={5}
            enableSearch={false}
            selectedIndex={selectedIndex}
            onSelectionChange={setSelectedIndex}
            follow={following}
            onFollowChange={setFollowing}
            showStatusBar={false}
            vimNavigation
            renderItem={(item, _index, isSelected) => {
              const log = filteredLogs.find((l) => l.id === item.id);
              const ts = log?.timestamp ?? new Date();
              const levelColor = getLevelColor(log?.level ?? null);
              const levelBadge = log?.level
                ? `[${log.level.toUpperCase().padEnd(5)}]`
                : '[     ]';

              return (
                <Box key={item.id}>
                  {/* Timestamp */}
                  <Box width={14}>
                    <Text dimColor>{formatTimestamp(ts, relativeTime)}</Text>
                  </Box>

                  {/* Level badge */}
                  <Box width={8} marginLeft={1}>
                    <Text color={levelColor as Parameters<typeof Text>[0]['color']}>
                      {levelBadge}
                    </Text>
                  </Box>

                  {/* Provider tag */}
                  {log?.provider && (
                    <Box width={12} marginLeft={1}>
                      <Text color="magenta">[{log.provider.slice(0, 10).padEnd(10)}]</Text>
                    </Box>
                  )}

                  {/* Message content */}
                  <Box marginLeft={1} flexShrink={1}>
                    <Text
                      color={levelColor as Parameters<typeof Text>[0]['color']}
                      inverse={isSelected}
                      wrap="truncate"
                    >
                      {item.content.replace(/^\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}(?:\.\d+)?\s*/, '')}
                    </Text>
                  </Box>
                </Box>
              );
            }}
          />
        )}
      </Box>

      {/* Status bar */}
      <Box
        borderStyle="single"
        borderColor="gray"
        paddingX={1}
        justifyContent="space-between"
      >
        <Box>
          <Text dimColor>Lines: </Text>
          <Text>{filteredLogs.length}</Text>
          <Text dimColor>/{logs.length}</Text>

          {errorCount > 0 && (
            <>
              <Text dimColor> | </Text>
              <Text color="red">Errors: {errorCount}</Text>
            </>
          )}

          {warnCount > 0 && (
            <>
              <Text dimColor> | </Text>
              <Text color="yellow">Warnings: {warnCount}</Text>
            </>
          )}
        </Box>

        <Box>
          <Text dimColor>
            {selectedIndex + 1}/{filteredLogs.length}
            {' '}
            ({Math.round(((selectedIndex + 1) / Math.max(filteredLogs.length, 1)) * 100)}%)
          </Text>
        </Box>
      </Box>

      {/* Help bar */}
      <Box paddingX={1}>
        <Text dimColor>
          [Q]uit [/]Filter [L]evel [P]rovider [T]ime [F]ollow [E]xport [Shift+E]FirstError [↑↓/jk]Nav [C]lear
        </Text>
      </Box>

      {/* Log file path */}
      <Box paddingX={1}>
        <Text dimColor>Log: {logPath}</Text>
      </Box>
    </Box>
  );
};

/**
 * Server logs command handler
 */
export async function serverLogsCommand(options: ServerLogsOptions): Promise<void> {
  // Non-interactive mode
  if (options.quiet || options.json) {
    await logsQuiet(options);
    return;
  }

  // Interactive mode
  return new Promise((resolve) => {
    const { waitUntilExit } = render(
      <ServerLogs options={options} onComplete={() => resolve()} />
    );
    waitUntilExit().then(() => resolve());
  });
}

/**
 * Non-interactive logs output
 */
async function logsQuiet(options: ServerLogsOptions): Promise<void> {
  try {
    await configManager.initialize({
      configPath: options.config,
      profile: options.profile,
    });

    const path = getLogPath();

    if (!existsSync(path)) {
      if (options.json) {
        console.log(JSON.stringify({ status: 'error', message: 'Log file not found' }));
      } else {
        logger.error(`Log file not found: ${path}`);
      }
      return;
    }

    const content = readFileSync(path, 'utf-8');
    const lines = content.split('\n').filter((l) => l.trim());
    const maxLines = parseInt(options.lines ?? '50', 10);

    let filteredLines = lines;

    // Apply level filter
    if (options.level) {
      filteredLines = filteredLines.filter((line) => {
        const level = parseLogLevel(line);
        return shouldIncludeLevel(level, options.level);
      });
    }

    // Apply text filter
    if (options.filter) {
      filteredLines = filteredLines.filter((line) =>
        line.toLowerCase().includes(options.filter!.toLowerCase())
      );
    }

    // Apply provider filter
    if (options.provider) {
      filteredLines = filteredLines.filter((line) => {
        const provider = parseProvider(line);
        return provider === options.provider?.toLowerCase();
      });
    }

    const lastLines = filteredLines.slice(-maxLines);

    if (options.json) {
      const errorCount = lastLines.filter((l) => parseLogLevel(l) === 'error').length;
      const warnCount = lastLines.filter((l) => parseLogLevel(l) === 'warn').length;

      console.log(JSON.stringify({
        status: 'success',
        path,
        lines: lastLines,
        total: lines.length,
        filtered: lastLines.length,
        errors: errorCount,
        warnings: warnCount,
      }));
    } else {
      for (const line of lastLines) {
        console.log(line);
      }
    }

    // Follow mode for quiet (simple tail -f style)
    if (options.follow && !options.json) {
      let lastSize = statSync(path).size;

      const checkNewContent = () => {
        const currentSize = statSync(path).size;
        if (currentSize > lastSize) {
          const stream = createReadStream(path, {
            start: lastSize,
            end: currentSize,
          });

          const rl = createInterface({
            input: stream,
            crlfDelay: Infinity,
          });

          rl.on('line', (line) => {
            const trimmed = line.trim();
            if (!trimmed) {
              return;
            }

            // Apply filters
            if (options.filter && !trimmed.toLowerCase().includes(options.filter.toLowerCase())) {
              return;
            }

            const level = parseLogLevel(trimmed);
            if (!shouldIncludeLevel(level, options.level)) {
              return;
            }

            if (options.provider) {
              const provider = parseProvider(trimmed);
              if (provider !== options.provider.toLowerCase()) {
                return;
              }
            }

            console.log(trimmed);
          });

          lastSize = currentSize;
        }
      };

      // Check every 100ms
      const interval = setInterval(checkNewContent, 100);

      // Handle Ctrl+C
      process.on('SIGINT', () => {
        clearInterval(interval);
        process.exit(0);
      });

      // Keep process running
      await new Promise(() => {
        // Never resolves, wait for SIGINT
      });
    }
  } catch (error) {
    const errMsg = error instanceof Error ? error.message : String(error);
    if (options.json) {
      console.log(JSON.stringify({ status: 'error', message: errMsg }));
    } else {
      logger.error(errMsg);
    }
    process.exitCode = 1;
  }
}

export default serverLogsCommand;
