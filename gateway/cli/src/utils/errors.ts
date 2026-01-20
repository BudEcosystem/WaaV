/**
 * WaaV CLI Error Handling
 *
 * Custom error types and error handling utilities
 */

// Error codes
export enum ErrorCode {
  // Configuration errors (100-199)
  CONFIG_NOT_FOUND = 'CONFIG_NOT_FOUND',
  CONFIG_INVALID = 'CONFIG_INVALID',
  CONFIG_PARSE_ERROR = 'CONFIG_PARSE_ERROR',
  CONFIG_WRITE_ERROR = 'CONFIG_WRITE_ERROR',

  // Gateway errors (200-299)
  GATEWAY_UNREACHABLE = 'GATEWAY_UNREACHABLE',
  GATEWAY_AUTH_FAILED = 'GATEWAY_AUTH_FAILED',
  GATEWAY_TIMEOUT = 'GATEWAY_TIMEOUT',
  GATEWAY_ERROR = 'GATEWAY_ERROR',

  // WebSocket errors (300-399)
  WEBSOCKET_CONNECTION_FAILED = 'WEBSOCKET_CONNECTION_FAILED',
  WEBSOCKET_CLOSED = 'WEBSOCKET_CLOSED',
  WEBSOCKET_MESSAGE_ERROR = 'WEBSOCKET_MESSAGE_ERROR',
  WEBSOCKET_TIMEOUT = 'WEBSOCKET_TIMEOUT',

  // Provider errors (400-499)
  PROVIDER_NOT_FOUND = 'PROVIDER_NOT_FOUND',
  PROVIDER_NOT_CONFIGURED = 'PROVIDER_NOT_CONFIGURED',
  PROVIDER_AUTH_FAILED = 'PROVIDER_AUTH_FAILED',
  PROVIDER_ERROR = 'PROVIDER_ERROR',

  // Audio errors (500-599)
  AUDIO_DEVICE_NOT_FOUND = 'AUDIO_DEVICE_NOT_FOUND',
  AUDIO_RECORDING_FAILED = 'AUDIO_RECORDING_FAILED',
  AUDIO_PLAYBACK_FAILED = 'AUDIO_PLAYBACK_FAILED',
  AUDIO_SOX_NOT_INSTALLED = 'AUDIO_SOX_NOT_INSTALLED',

  // DAG errors (600-699)
  DAG_INVALID = 'DAG_INVALID',
  DAG_CYCLE_DETECTED = 'DAG_CYCLE_DETECTED',
  DAG_NODE_NOT_FOUND = 'DAG_NODE_NOT_FOUND',
  DAG_EXECUTION_ERROR = 'DAG_EXECUTION_ERROR',

  // General errors (900-999)
  UNKNOWN = 'UNKNOWN',
  CANCELLED = 'CANCELLED',
  PERMISSION_DENIED = 'PERMISSION_DENIED',
  INVALID_ARGUMENT = 'INVALID_ARGUMENT',
}

// Base CLI error class
export class CLIError extends Error {
  public readonly code: ErrorCode;
  public readonly recoverable: boolean;
  public readonly suggestion?: string;
  public readonly details?: Record<string, unknown>;
  public readonly cause?: Error;

  constructor(options: {
    code: ErrorCode;
    message: string;
    recoverable?: boolean;
    suggestion?: string;
    details?: Record<string, unknown>;
    cause?: Error;
  }) {
    super(options.message);
    this.name = 'CLIError';
    this.code = options.code;
    this.recoverable = options.recoverable ?? false;
    this.suggestion = options.suggestion;
    this.details = options.details;
    this.cause = options.cause;

    // Capture stack trace
    Error.captureStackTrace(this, CLIError);
  }

  /**
   * Convert to JSON for logging/serialization
   */
  toJSON(): Record<string, unknown> {
    return {
      name: this.name,
      code: this.code,
      message: this.message,
      recoverable: this.recoverable,
      suggestion: this.suggestion,
      details: this.details,
      stack: this.stack,
    };
  }

  /**
   * Format for display
   */
  format(verbose = false): string {
    const lines: string[] = [];
    lines.push(`Error [${this.code}]: ${this.message}`);

    if (this.suggestion) {
      lines.push(`\nSuggestion: ${this.suggestion}`);
    }

    if (verbose && this.details) {
      lines.push(`\nDetails: ${JSON.stringify(this.details, null, 2)}`);
    }

    if (verbose && this.stack) {
      lines.push(`\n${this.stack}`);
    }

    return lines.join('\n');
  }
}

// Configuration error
export class ConfigError extends CLIError {
  constructor(message: string, options?: Partial<ConstructorParameters<typeof CLIError>[0]>) {
    super({
      code: ErrorCode.CONFIG_INVALID,
      message,
      recoverable: true,
      suggestion: 'Run `waav init` to create a valid configuration',
      ...options,
    });
    this.name = 'ConfigError';
  }
}

// Gateway error
export class GatewayError extends CLIError {
  constructor(message: string, options?: Partial<ConstructorParameters<typeof CLIError>[0]>) {
    super({
      code: ErrorCode.GATEWAY_ERROR,
      message,
      recoverable: true,
      suggestion: 'Check that the gateway is running with `waav server status`',
      ...options,
    });
    this.name = 'GatewayError';
  }
}

// Provider error
export class ProviderError extends CLIError {
  public readonly provider: string;

  constructor(provider: string, message: string, options?: Partial<ConstructorParameters<typeof CLIError>[0]>) {
    super({
      code: ErrorCode.PROVIDER_ERROR,
      message: `[${provider}] ${message}`,
      recoverable: true,
      suggestion: `Check provider configuration with \`waav provider test ${provider}\``,
      ...options,
    });
    this.name = 'ProviderError';
    this.provider = provider;
  }
}

// Audio error
export class AudioError extends CLIError {
  constructor(message: string, options?: Partial<ConstructorParameters<typeof CLIError>[0]>) {
    super({
      code: ErrorCode.AUDIO_RECORDING_FAILED,
      message,
      recoverable: true,
      suggestion: 'Run `waav voice test` to diagnose audio issues',
      ...options,
    });
    this.name = 'AudioError';
  }
}

// WebSocket error
export class WebSocketError extends CLIError {
  constructor(message: string, options?: Partial<ConstructorParameters<typeof CLIError>[0]>) {
    super({
      code: ErrorCode.WEBSOCKET_CONNECTION_FAILED,
      message,
      recoverable: true,
      suggestion: 'Check network connection and gateway status',
      ...options,
    });
    this.name = 'WebSocketError';
  }
}

// DAG error
export class DAGError extends CLIError {
  constructor(message: string, options?: Partial<ConstructorParameters<typeof CLIError>[0]>) {
    super({
      code: ErrorCode.DAG_INVALID,
      message,
      recoverable: true,
      suggestion: 'Run `waav dag validate <name>` to check your pipeline',
      ...options,
    });
    this.name = 'DAGError';
  }
}

/**
 * Wrap an async function with error handling
 */
export function withErrorHandling<T extends (...args: never[]) => Promise<unknown>>(
  fn: T,
  context?: string
): T {
  return (async (...args: Parameters<T>) => {
    try {
      return await fn(...args);
    } catch (error) {
      if (error instanceof CLIError) {
        throw error;
      }

      const message = error instanceof Error ? error.message : String(error);
      throw new CLIError({
        code: ErrorCode.UNKNOWN,
        message: context ? `${context}: ${message}` : message,
        cause: error instanceof Error ? error : undefined,
      });
    }
  }) as T;
}

/**
 * Check if an error is a specific type
 */
export function isErrorCode(error: unknown, code: ErrorCode): error is CLIError {
  return error instanceof CLIError && error.code === code;
}

/**
 * Try an operation with retry
 */
export async function withRetry<T>(
  operation: () => Promise<T>,
  options: {
    maxAttempts?: number;
    delay?: number;
    backoffMultiplier?: number;
    shouldRetry?: (error: unknown) => boolean;
  } = {}
): Promise<T> {
  const {
    maxAttempts = 3,
    delay = 1000,
    backoffMultiplier = 2,
    shouldRetry = (e) => e instanceof CLIError && e.recoverable,
  } = options;

  let lastError: unknown;
  let currentDelay = delay;

  for (let attempt = 1; attempt <= maxAttempts; attempt++) {
    try {
      return await operation();
    } catch (error) {
      lastError = error;

      if (attempt === maxAttempts || !shouldRetry(error)) {
        throw error;
      }

      await new Promise(resolve => setTimeout(resolve, currentDelay));
      currentDelay *= backoffMultiplier;
    }
  }

  throw lastError;
}

/**
 * Format an error for display
 */
export function formatError(error: unknown, verbose = false): string {
  if (error instanceof CLIError) {
    return error.format(verbose);
  }

  if (error instanceof Error) {
    const lines = [`Error: ${error.message}`];
    if (verbose && error.stack) {
      lines.push(`\n${error.stack}`);
    }
    return lines.join('\n');
  }

  return `Error: ${String(error)}`;
}
