/**
 * API-related error classes
 */

import { BudError, BudErrorCode } from './base.js';

/**
 * Error thrown when API request fails
 */
export class APIError extends BudError {
  /** HTTP status code */
  readonly statusCode: number;
  /** Response body */
  readonly responseBody?: unknown;
  /** Request URL */
  readonly url?: string;
  /** HTTP method */
  readonly method?: string;

  constructor(
    message: string,
    statusCode: number,
    options?: {
      responseBody?: unknown;
      url?: string;
      method?: string;
      cause?: Error;
      context?: Record<string, unknown>;
    }
  ) {
    const code = APIError.statusToErrorCode(statusCode);
    super(message, code, {
      ...options,
      context: {
        ...options?.context,
        statusCode,
        url: options?.url,
        method: options?.method,
      },
    });
    this.name = 'APIError';
    this.statusCode = statusCode;
    this.responseBody = options?.responseBody;
    this.url = options?.url;
    this.method = options?.method;
  }

  /**
   * Convert HTTP status code to BudErrorCode
   */
  static statusToErrorCode(statusCode: number): BudErrorCode {
    if (statusCode === 401) return BudErrorCode.API_UNAUTHORIZED;
    if (statusCode === 403) return BudErrorCode.API_FORBIDDEN;
    if (statusCode === 404) return BudErrorCode.API_NOT_FOUND;
    if (statusCode === 429) return BudErrorCode.API_RATE_LIMITED;
    if (statusCode >= 500) return BudErrorCode.API_SERVER_ERROR;
    return BudErrorCode.API_ERROR;
  }

  /**
   * Check if error is due to authentication failure
   */
  isAuthError(): boolean {
    return this.statusCode === 401 || this.statusCode === 403;
  }

  /**
   * Check if error is due to rate limiting
   */
  isRateLimited(): boolean {
    return this.statusCode === 429;
  }

  /**
   * Check if error is a server error (5xx)
   */
  isServerError(): boolean {
    return this.statusCode >= 500;
  }

  /**
   * Check if error is a client error (4xx)
   */
  isClientError(): boolean {
    return this.statusCode >= 400 && this.statusCode < 500;
  }

  /**
   * Check if error might be retryable
   */
  isRetryable(): boolean {
    // Rate limits and server errors might be retryable
    return this.isRateLimited() || this.isServerError();
  }

  /**
   * Create from fetch Response
   */
  static async fromResponse(
    response: Response,
    options?: { method?: string }
  ): Promise<APIError> {
    let responseBody: unknown;
    try {
      const text = await response.text();
      try {
        responseBody = JSON.parse(text);
      } catch {
        responseBody = text;
      }
    } catch {
      responseBody = undefined;
    }

    const message =
      typeof responseBody === 'object' &&
      responseBody !== null &&
      'message' in responseBody
        ? String((responseBody as Record<string, unknown>).message)
        : `API request failed with status ${response.status}`;

    return new APIError(message, response.status, {
      responseBody,
      url: response.url,
      method: options?.method,
    });
  }
}

/**
 * Error thrown when API configuration is invalid
 */
export class ConfigurationError extends BudError {
  /** Field that is invalid or missing */
  readonly field?: string;
  /** Expected value or type */
  readonly expected?: string;
  /** Actual value received */
  readonly actual?: unknown;

  constructor(
    message: string,
    options?: {
      field?: string;
      expected?: string;
      actual?: unknown;
      cause?: Error;
      context?: Record<string, unknown>;
    }
  ) {
    const code = options?.field
      ? BudErrorCode.CONFIG_MISSING_REQUIRED
      : BudErrorCode.CONFIG_INVALID;

    super(message, code, {
      ...options,
      context: {
        ...options?.context,
        field: options?.field,
        expected: options?.expected,
        actual: options?.actual,
      },
    });
    this.name = 'ConfigurationError';
    this.field = options?.field;
    this.expected = options?.expected;
    this.actual = options?.actual;
  }
}
