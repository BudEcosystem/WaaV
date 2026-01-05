/**
 * Base Error Classes for @bud-foundry/sdk
 */

/**
 * Error codes for categorization
 */
export enum BudErrorCode {
  // Connection errors (1xx)
  CONNECTION_FAILED = 100,
  CONNECTION_TIMEOUT = 101,
  CONNECTION_CLOSED = 102,
  RECONNECT_FAILED = 103,
  RECONNECT_MAX_ATTEMPTS = 104,

  // API errors (2xx)
  API_ERROR = 200,
  API_UNAUTHORIZED = 201,
  API_FORBIDDEN = 202,
  API_NOT_FOUND = 203,
  API_RATE_LIMITED = 204,
  API_SERVER_ERROR = 205,

  // Configuration errors (3xx)
  CONFIG_INVALID = 300,
  CONFIG_MISSING_REQUIRED = 301,
  CONFIG_UNSUPPORTED_PROVIDER = 302,

  // STT errors (4xx)
  STT_ERROR = 400,
  STT_PROVIDER_ERROR = 401,
  STT_TRANSCRIPTION_FAILED = 402,
  STT_AUDIO_FORMAT_UNSUPPORTED = 403,

  // TTS errors (5xx)
  TTS_ERROR = 500,
  TTS_PROVIDER_ERROR = 501,
  TTS_SYNTHESIS_FAILED = 502,
  TTS_VOICE_NOT_FOUND = 503,

  // WebSocket errors (6xx)
  WS_ERROR = 600,
  WS_MESSAGE_PARSE_ERROR = 601,
  WS_SEND_FAILED = 602,
  WS_INVALID_STATE = 603,

  // Audio errors (7xx)
  AUDIO_ERROR = 700,
  AUDIO_PLAYBACK_ERROR = 701,
  AUDIO_RECORDING_ERROR = 702,
  AUDIO_PROCESSING_ERROR = 703,

  // LiveKit errors (8xx)
  LIVEKIT_ERROR = 800,
  LIVEKIT_TOKEN_ERROR = 801,
  LIVEKIT_ROOM_ERROR = 802,

  // SIP errors (9xx)
  SIP_ERROR = 900,
  SIP_TRANSFER_ERROR = 901,
  SIP_INVALID_NUMBER = 902,

  // Unknown
  UNKNOWN = 999,
}

/**
 * Base error class for all Bud SDK errors
 */
export class BudError extends Error {
  /** Error code for categorization */
  readonly code: BudErrorCode;
  /** Original error that caused this error */
  readonly cause?: Error;
  /** Additional context data */
  readonly context?: Record<string, unknown>;
  /** Timestamp when error occurred */
  readonly timestamp: number;

  constructor(
    message: string,
    code: BudErrorCode = BudErrorCode.UNKNOWN,
    options?: {
      cause?: Error;
      context?: Record<string, unknown>;
    }
  ) {
    super(message);
    this.name = 'BudError';
    this.code = code;
    this.cause = options?.cause;
    this.context = options?.context;
    this.timestamp = Date.now();

    // Maintain proper stack trace in V8
    if (Error.captureStackTrace) {
      Error.captureStackTrace(this, this.constructor);
    }
  }

  /**
   * Check if error is of a specific code
   */
  is(code: BudErrorCode): boolean {
    return this.code === code;
  }

  /**
   * Check if error is in a category (e.g., all 4xx are STT errors)
   */
  isCategory(category: number): boolean {
    return Math.floor(this.code / 100) === Math.floor(category / 100);
  }

  /**
   * Convert to JSON for logging/serialization
   */
  toJSON(): Record<string, unknown> {
    return {
      name: this.name,
      message: this.message,
      code: this.code,
      context: this.context,
      timestamp: this.timestamp,
      cause: this.cause
        ? {
            name: this.cause.name,
            message: this.cause.message,
          }
        : undefined,
      stack: this.stack,
    };
  }

  /**
   * Create error from unknown caught value
   */
  static from(err: unknown, defaultCode: BudErrorCode = BudErrorCode.UNKNOWN): BudError {
    if (err instanceof BudError) {
      return err;
    }

    if (err instanceof Error) {
      return new BudError(err.message, defaultCode, { cause: err });
    }

    if (typeof err === 'string') {
      return new BudError(err, defaultCode);
    }

    return new BudError('Unknown error occurred', defaultCode, {
      context: { originalError: err },
    });
  }
}

/**
 * Check if a value is a BudError
 */
export function isBudError(err: unknown): err is BudError {
  return err instanceof BudError;
}

/**
 * Get error code name for display
 */
export function getErrorCodeName(code: BudErrorCode): string {
  return BudErrorCode[code] ?? 'UNKNOWN';
}
