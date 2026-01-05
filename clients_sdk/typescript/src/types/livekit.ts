/**
 * LiveKit Types
 */

/**
 * LiveKit token request
 */
export interface LiveKitTokenRequest {
  /** Room name to join */
  roomName: string;
  /** Participant identity */
  identity: string;
  /** Participant display name */
  name?: string;
  /** Token validity in seconds (default: 3600) */
  ttl?: number;
  /** Participant metadata (JSON string) */
  metadata?: string;
  /** Room creation options */
  roomOptions?: {
    /** Auto-create room if it doesn't exist */
    autoCreate?: boolean;
    /** Room empty timeout in seconds */
    emptyTimeout?: number;
    /** Maximum participants allowed */
    maxParticipants?: number;
  };
  /** Participant permissions */
  permissions?: {
    /** Can publish audio */
    canPublish?: boolean;
    /** Can subscribe to tracks */
    canSubscribe?: boolean;
    /** Can publish data */
    canPublishData?: boolean;
    /** Allowed sources to publish */
    canPublishSources?: string[];
    /** Hidden participant */
    hidden?: boolean;
    /** Recorder participant */
    recorder?: boolean;
  };
}

/**
 * LiveKit token response
 */
export interface LiveKitTokenResponse {
  /** JWT token for connecting to LiveKit */
  token: string;
  /** Room name */
  roomName: string;
  /** Participant identity */
  identity: string;
  /** LiveKit server URL */
  url?: string;
}

/**
 * LiveKit room information
 */
export interface RoomInfo {
  /** Room name */
  name: string;
  /** Room SID (unique identifier) */
  sid: string;
  /** Number of participants */
  numParticipants: number;
  /** Maximum participants allowed */
  maxParticipants: number;
  /** Room creation timestamp */
  createdAt: number;
  /** Active recording status */
  activeRecording: boolean;
  /** Room metadata */
  metadata?: string;
}

/**
 * LiveKit participant information
 */
export interface ParticipantInfo {
  /** Participant SID */
  sid: string;
  /** Participant identity */
  identity: string;
  /** Display name */
  name: string;
  /** Participant state */
  state: 'joining' | 'joined' | 'active' | 'disconnected';
  /** Published tracks */
  tracks: TrackInfo[];
  /** Participant metadata */
  metadata?: string;
  /** Join timestamp */
  joinedAt: number;
  /** Is speaker (audio detected) */
  isSpeaking: boolean;
  /** Audio level (0-1) */
  audioLevel: number;
}

/**
 * LiveKit track information
 */
export interface TrackInfo {
  /** Track SID */
  sid: string;
  /** Track type */
  type: 'audio' | 'video' | 'data';
  /** Track source */
  source: 'camera' | 'microphone' | 'screen_share' | 'screen_share_audio' | 'unknown';
  /** Track name */
  name: string;
  /** Is muted */
  muted: boolean;
  /** Simulcast layers (video only) */
  layers?: Array<{
    quality: 'low' | 'medium' | 'high';
    width: number;
    height: number;
    bitrate: number;
  }>;
}

/**
 * LiveKit room list response
 */
export interface RoomListResponse {
  /** List of active rooms */
  rooms: RoomInfo[];
}

/**
 * LiveKit connection options
 */
export interface LiveKitConnectOptions {
  /** LiveKit server URL */
  url: string;
  /** JWT token */
  token: string;
  /** Auto-subscribe to tracks */
  autoSubscribe?: boolean;
  /** Adaptive stream configuration */
  adaptiveStream?: boolean;
  /** Dynacast for bandwidth optimization */
  dynacast?: boolean;
}
