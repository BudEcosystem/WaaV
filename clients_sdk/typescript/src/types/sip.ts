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
 * SIP transfer request (REST `POST /sip/transfer`).
 *
 * Mirrors the gateway `SIPTransferRequest` (handlers/sip/transfer.rs) exactly:
 * `{ room_name, participant_identity, transfer_to }` — all three fields are
 * required (a body missing `room_name`/`participant_identity` 422s).
 */
export interface SIPTransferRequest {
  /** The LiveKit room name where the SIP participant is connected (wire: `room_name`). */
  roomName: string;
  /**
   * The identity of the SIP participant to transfer (wire: `participant_identity`).
   * Obtainable by listing participants in the room via the LiveKit API.
   */
  participantIdentity: string;
  /**
   * The destination to transfer the call to (wire: `transfer_to`). Supports
   * international format (`+1234567890`), national format (`07123456789`),
   * or internal extensions (`1234`).
   */
  transferTo: string;
}

/**
 * SIP transfer response (gateway `SIPTransferResponse`, handlers/sip/transfer.rs).
 *
 * A success indicates the transfer was accepted — `status` is `"completed"` or
 * `"initiated"` (the gateway returns `initiated` when the transfer request
 * timed out awaiting confirmation but likely succeeded).
 */
export interface SIPTransferResponse {
  /** `"initiated"` or `"completed"` (wire: `status`). */
  status: string;
  /** The (tenant-normalized) room name the transfer ran in (wire: `room_name`). */
  roomName: string;
  /** The identity of the participant transferred (wire: `participant_identity`). */
  participantIdentity: string;
  /** The normalized destination with `tel:` prefix (wire: `transfer_to`). */
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
