/**
 * LiveKit Types
 */

/**
 * LiveKit token request
 */
export interface LiveKitTokenRequest {
  /** Room name to join */
  roomName: string;
  /**
   * Participant identity. Sent on the wire as `participant_identity`
   * (the gateway's TokenRequest field name).
   */
  participantIdentity: string;
  /**
   * Participant display name. Sent on the wire as `participant_name`
   * (the gateway's TokenRequest field name).
   */
  participantName: string;
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
  /** Room name (gateway `room_name`) */
  roomName: string;
  /** Participant identity (gateway `participant_identity`) */
  participantIdentity: string;
  /** LiveKit server URL (gateway `livekit_url`) */
  livekitUrl: string;
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
 * Response for `DELETE /livekit/participant` (gateway `RemoveParticipantResponse`,
 * handlers/livekit/participants.rs).
 */
export interface RemoveParticipantResponse {
  /** Status of the removal operation, e.g. `"removed"` (wire: `status`). */
  status: string;
  /** The tenant-normalized room name, e.g. `project1_room-123` (wire: `room_name`). */
  roomName: string;
  /** The identity of the removed participant (wire: `participant_identity`). */
  participantIdentity: string;
}

/**
 * Response for `POST /livekit/participant/mute` (gateway `MuteParticipantResponse`,
 * handlers/livekit/participants.rs).
 */
export interface MuteParticipantResponse {
  /** The tenant-normalized room name (wire: `room_name`). */
  roomName: string;
  /** The identity of the participant (wire: `participant_identity`). */
  participantIdentity: string;
  /** The session ID of the track (wire: `track_sid`). */
  trackSid: string;
  /** Current muted state after the operation (wire: `muted`). */
  muted: boolean;
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
