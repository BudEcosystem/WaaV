/**
 * STT (Speech-to-Text) error classes
 */

import { BudError, BudErrorCode } from './base.js';

/**
 * Base error for STT operations
 */
export class STTError extends BudError {
  /** Provider that caused the error */
  readonly provider?: string;

  constructor(
    message: string,
    code: BudErrorCode = BudErrorCode.STT_ERROR,
    options?: {
      provider?: string;
      cause?: Error;
      context?: Record<string, unknown>;
    }
  ) {
    super(message, code, {
      ...options,
      context: {
        ...options?.context,
        provider: options?.provider,
      },
    });
    this.name = 'STTError';
    this.provider = options?.provider;
  }
}

/**
 * Error thrown when STT provider returns an error
 */
export class STTProviderError extends STTError {
  /** Provider-specific error code */
  readonly providerErrorCode?: string;
  /** Provider-specific error message */
  readonly providerErrorMessage?: string;

  constructor(
    message: string,
    options?: {
      provider?: string;
      providerErrorCode?: string;
      providerErrorMessage?: string;
      cause?: Error;
      context?: Record<string, unknown>;
    }
  ) {
    super(message, BudErrorCode.STT_PROVIDER_ERROR, {
      ...options,
      context: {
        ...options?.context,
        providerErrorCode: options?.providerErrorCode,
        providerErrorMessage: options?.providerErrorMessage,
      },
    });
    this.name = 'STTProviderError';
    this.providerErrorCode = options?.providerErrorCode;
    this.providerErrorMessage = options?.providerErrorMessage;
  }
}

/**
 * Error thrown when transcription fails
 */
export class TranscriptionError extends STTError {
  /** Audio duration that failed to transcribe */
  readonly audioDuration?: number;
  /** Language code */
  readonly language?: string;

  constructor(
    message: string,
    options?: {
      provider?: string;
      audioDuration?: number;
      language?: string;
      cause?: Error;
      context?: Record<string, unknown>;
    }
  ) {
    super(message, BudErrorCode.STT_TRANSCRIPTION_FAILED, {
      ...options,
      context: {
        ...options?.context,
        audioDuration: options?.audioDuration,
        language: options?.language,
      },
    });
    this.name = 'TranscriptionError';
    this.audioDuration = options?.audioDuration;
    this.language = options?.language;
  }
}

/**
 * Error thrown when audio format is not supported
 */
export class AudioFormatError extends STTError {
  /** Format that was provided */
  readonly providedFormat?: string;
  /** Formats that are supported */
  readonly supportedFormats?: string[];

  constructor(
    message: string,
    options?: {
      provider?: string;
      providedFormat?: string;
      supportedFormats?: string[];
      cause?: Error;
      context?: Record<string, unknown>;
    }
  ) {
    super(message, BudErrorCode.STT_AUDIO_FORMAT_UNSUPPORTED, {
      ...options,
      context: {
        ...options?.context,
        providedFormat: options?.providedFormat,
        supportedFormats: options?.supportedFormats,
      },
    });
    this.name = 'AudioFormatError';
    this.providedFormat = options?.providedFormat;
    this.supportedFormats = options?.supportedFormats;
  }
}
