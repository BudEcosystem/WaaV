/**
 * TTS (Text-to-Speech) error classes
 */

import { BudError, BudErrorCode } from './base.js';

/**
 * Base error for TTS operations
 */
export class TTSError extends BudError {
  /** Provider that caused the error */
  readonly provider?: string;

  constructor(
    message: string,
    code: BudErrorCode = BudErrorCode.TTS_ERROR,
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
    this.name = 'TTSError';
    this.provider = options?.provider;
  }
}

/**
 * Error thrown when TTS provider returns an error
 */
export class TTSProviderError extends TTSError {
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
    super(message, BudErrorCode.TTS_PROVIDER_ERROR, {
      ...options,
      context: {
        ...options?.context,
        providerErrorCode: options?.providerErrorCode,
        providerErrorMessage: options?.providerErrorMessage,
      },
    });
    this.name = 'TTSProviderError';
    this.providerErrorCode = options?.providerErrorCode;
    this.providerErrorMessage = options?.providerErrorMessage;
  }
}

/**
 * Error thrown when TTS synthesis fails
 */
export class SynthesisError extends TTSError {
  /** Text that failed to synthesize */
  readonly text?: string;
  /** Voice ID that was used */
  readonly voiceId?: string;

  constructor(
    message: string,
    options?: {
      provider?: string;
      text?: string;
      voiceId?: string;
      cause?: Error;
      context?: Record<string, unknown>;
    }
  ) {
    super(message, BudErrorCode.TTS_SYNTHESIS_FAILED, {
      ...options,
      context: {
        ...options?.context,
        text: options?.text,
        voiceId: options?.voiceId,
      },
    });
    this.name = 'SynthesisError';
    this.text = options?.text;
    this.voiceId = options?.voiceId;
  }
}

/**
 * Error thrown when voice is not found
 */
export class VoiceNotFoundError extends TTSError {
  /** Voice ID that was not found */
  readonly voiceId: string;
  /** Available voices (if known) */
  readonly availableVoices?: string[];

  constructor(
    voiceId: string,
    options?: {
      provider?: string;
      availableVoices?: string[];
      cause?: Error;
      context?: Record<string, unknown>;
    }
  ) {
    super(`Voice not found: ${voiceId}`, BudErrorCode.TTS_VOICE_NOT_FOUND, {
      ...options,
      context: {
        ...options?.context,
        voiceId,
        availableVoices: options?.availableVoices,
      },
    });
    this.name = 'VoiceNotFoundError';
    this.voiceId = voiceId;
    this.availableVoices = options?.availableVoices;
  }
}
