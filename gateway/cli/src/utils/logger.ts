/**
 * WaaV CLI Logger
 *
 * Structured logging with levels, colors, and formatting
 */

import chalk from 'chalk';
import type { LogLevel } from '../types/index.js';

// Logger configuration
interface LoggerConfig {
  level: LogLevel;
  prefix: string;
  showTimestamp: boolean;
  noColor: boolean;
  quiet: boolean;
}

// Log level priorities
const LOG_LEVELS: Record<LogLevel, number> = {
  debug: 0,
  info: 1,
  warn: 2,
  error: 3,
};

// Level colors and icons
const LEVEL_STYLES: Record<LogLevel, { color: typeof chalk.blue; icon: string }> = {
  debug: { color: chalk.gray, icon: '🔍' },
  info: { color: chalk.blue, icon: 'ℹ️' },
  warn: { color: chalk.yellow, icon: '⚠️' },
  error: { color: chalk.red, icon: '❌' },
};

class Logger {
  private config: LoggerConfig = {
    level: 'info',
    prefix: 'waav',
    showTimestamp: false,
    noColor: false,
    quiet: false,
  };

  /**
   * Configure the logger
   */
  configure(config: Partial<LoggerConfig>): void {
    this.config = { ...this.config, ...config };
  }

  /**
   * Set log level
   */
  setLevel(level: LogLevel): void {
    this.config.level = level;
  }

  /**
   * Check if a level should be logged
   */
  private shouldLog(level: LogLevel): boolean {
    if (this.config.quiet && level !== 'error') {
      return false;
    }
    return LOG_LEVELS[level] >= LOG_LEVELS[this.config.level];
  }

  /**
   * Format a log message
   */
  private format(level: LogLevel, message: string, data?: unknown): string {
    const style = LEVEL_STYLES[level];
    const parts: string[] = [];

    // Timestamp
    if (this.config.showTimestamp) {
      const timestamp = new Date().toISOString().slice(11, 23);
      parts.push(this.config.noColor ? `[${timestamp}]` : chalk.gray(`[${timestamp}]`));
    }

    // Level icon and color
    if (!this.config.noColor) {
      parts.push(style.icon);
      parts.push(style.color(`[${level.toUpperCase()}]`));
    } else {
      parts.push(`[${level.toUpperCase()}]`);
    }

    // Prefix
    if (this.config.prefix) {
      parts.push(this.config.noColor ? `[${this.config.prefix}]` : chalk.cyan(`[${this.config.prefix}]`));
    }

    // Message
    parts.push(message);

    // Data
    if (data !== undefined) {
      const dataStr = typeof data === 'object' ? JSON.stringify(data, null, 2) : String(data);
      parts.push(this.config.noColor ? dataStr : chalk.gray(dataStr));
    }

    return parts.join(' ');
  }

  /**
   * Log a debug message
   */
  debug(message: string, data?: unknown): void {
    if (this.shouldLog('debug')) {
      console.log(this.format('debug', message, data));
    }
  }

  /**
   * Log an info message
   */
  info(message: string, data?: unknown): void {
    if (this.shouldLog('info')) {
      console.log(this.format('info', message, data));
    }
  }

  /**
   * Log a warning message
   */
  warn(message: string, data?: unknown): void {
    if (this.shouldLog('warn')) {
      console.warn(this.format('warn', message, data));
    }
  }

  /**
   * Log an error message
   */
  error(message: string, error?: unknown): void {
    if (this.shouldLog('error')) {
      console.error(this.format('error', message));
      if (error instanceof Error) {
        console.error(this.config.noColor ? error.stack : chalk.red(error.stack));
      } else if (error !== undefined) {
        console.error(this.config.noColor ? String(error) : chalk.red(String(error)));
      }
    }
  }

  /**
   * Log a success message (always shown unless quiet)
   */
  success(message: string, data?: unknown): void {
    if (!this.config.quiet) {
      const icon = '✅';
      const formatted = this.config.noColor
        ? `${icon} ${message}`
        : `${icon} ${chalk.green(message)}`;
      console.log(formatted);
      if (data !== undefined) {
        console.log(this.config.noColor ? JSON.stringify(data, null, 2) : chalk.gray(JSON.stringify(data, null, 2)));
      }
    }
  }

  /**
   * Log a plain message (no formatting, always shown unless quiet)
   */
  plain(message: string): void {
    if (!this.config.quiet) {
      console.log(message);
    }
  }

  /**
   * Log JSON output (for --json flag)
   */
  json(data: unknown): void {
    console.log(JSON.stringify(data, null, 2));
  }

  /**
   * Create a child logger with a different prefix
   */
  child(prefix: string): Logger {
    const child = new Logger();
    child.configure({
      ...this.config,
      prefix: `${this.config.prefix}:${prefix}`,
    });
    return child;
  }

  /**
   * Create a spinner-compatible message (for ora)
   */
  spinner(message: string): string {
    return this.config.noColor ? message : chalk.cyan(message);
  }

  /**
   * Format a value for display
   */
  value(val: unknown): string {
    if (typeof val === 'string') {
      return this.config.noColor ? `"${val}"` : chalk.green(`"${val}"`);
    }
    if (typeof val === 'number') {
      return this.config.noColor ? String(val) : chalk.yellow(String(val));
    }
    if (typeof val === 'boolean') {
      return this.config.noColor ? String(val) : chalk.magenta(String(val));
    }
    if (val === null || val === undefined) {
      return this.config.noColor ? 'null' : chalk.gray('null');
    }
    return this.config.noColor ? JSON.stringify(val) : chalk.cyan(JSON.stringify(val));
  }

  /**
   * Format a key-value pair
   */
  kv(key: string, value: unknown): string {
    return `${this.config.noColor ? key : chalk.bold(key)}: ${this.value(value)}`;
  }

  /**
   * Format a list of items
   */
  list(items: string[], bullet = '•'): string {
    return items.map(item => `  ${bullet} ${item}`).join('\n');
  }

  /**
   * Create a boxed message
   */
  box(message: string, title?: string): string {
    const lines = message.split('\n');
    const maxWidth = Math.max(...lines.map(l => l.length), title?.length ?? 0);
    const horizontal = '─'.repeat(maxWidth + 2);

    const parts: string[] = [];
    parts.push(`┌${horizontal}┐`);
    if (title) {
      parts.push(`│ ${title.padEnd(maxWidth)} │`);
      parts.push(`├${horizontal}┤`);
    }
    for (const line of lines) {
      parts.push(`│ ${line.padEnd(maxWidth)} │`);
    }
    parts.push(`└${horizontal}┘`);

    return this.config.noColor ? parts.join('\n') : chalk.cyan(parts.join('\n'));
  }
}

// Singleton instance
export const logger = new Logger();

// Export class for testing
export { Logger };
