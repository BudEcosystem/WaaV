/**
 * SIP (Session Initiation Protocol) Types
 */

/**
 * SIP webhook hook configuration
 */
export interface SIPHook {
  /** SIP domain to match (case-insensitive) */
  host: string;
  /** Webhook URL to forward events to */
  url: string;
  /** Optional per-hook secret (overrides global) */
  secret?: string;
}

/**
 * SIP hook list response
 */
export interface SIPHookListResponse {
  /** List of configured hooks */
  hooks: SIPHook[];
}

/**
 * SIP hook create request
 */
export interface SIPHookCreateRequest {
  /** SIP domain to match */
  host: string;
  /** Webhook URL */
  url: string;
}

/**
 * SIP hook create response
 */
export interface SIPHookCreateResponse {
  /** Created hook */
  hook: SIPHook;
  /** Whether hook was created (true) or updated (false) */
  created: boolean;
}

/**
 * SIP transfer request
 */
export interface SIPTransferRequest {
  /** The destination phone number to transfer the call to */
  transferTo: string;
}

/**
 * SIP transfer result
 */
export interface SIPTransferResult {
  /** Whether transfer was initiated successfully */
  success: boolean;
  /** Error message if transfer failed */
  error?: string;
}

/**
 * SIP call information (from LiveKit SIP participant)
 */
export interface SIPCallInfo {
  /** Call SID */
  callSid: string;
  /** Caller phone number */
  from: string;
  /** Called phone number */
  to: string;
  /** Call direction */
  direction: 'inbound' | 'outbound';
  /** Call status */
  status: 'ringing' | 'in-progress' | 'completed' | 'busy' | 'failed' | 'no-answer';
  /** Call start timestamp */
  startedAt?: number;
  /** Call answer timestamp */
  answeredAt?: number;
  /** Call end timestamp */
  endedAt?: number;
  /** Call duration in seconds */
  duration?: number;
}

/**
 * SIP participant webhook event
 */
export interface SIPWebhookEvent {
  /** Event type */
  type: 'participant_joined' | 'participant_left' | 'call_ended';
  /** Room name */
  room: string;
  /** Participant identity */
  identity: string;
  /** Participant name */
  name?: string;
  /** SIP call info */
  sipInfo?: SIPCallInfo;
  /** Timestamp */
  timestamp: number;
}

/**
 * Validate phone number format
 */
export function isValidPhoneNumber(phone: string): boolean {
  // Remove whitespace
  const cleaned = phone.trim();
  // Basic validation: should contain only digits, +, -, (, ), and spaces
  // And should have at least 3 digits
  const digitCount = (cleaned.match(/\d/g) ?? []).length;
  const validChars = /^[+\d\s\-()]+$/.test(cleaned);
  return validChars && digitCount >= 3;
}

/**
 * Normalize phone number to E.164 format
 */
export function normalizePhoneNumber(phone: string): string {
  // Remove all non-digit characters except leading +
  const hasPlus = phone.trim().startsWith('+');
  const digits = phone.replace(/\D/g, '');
  return hasPlus ? `+${digits}` : digits;
}
