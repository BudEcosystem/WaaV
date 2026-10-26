/**
 * Interaction mode (wire protocol 1.1) on `/ws`.
 *
 * A session asks for the mode with `interaction: true` (the defaults) or an object of settings
 * (`gateway/docs/interaction_mode.md`, "Session Configuration Reference"). When the gateway runs
 * it, `ready` carries `protocol_version: "1.1"` and the resolved settings under `interaction`,
 * and the session emits one `interaction` event per frame below. Events and settings keep the
 * gateway's snake_case wire names (the settings are sent as-is: a tool's `parameters` is a JSON
 * Schema whose keys must not be renamed).
 *
 * Source of truth: `gateway/src/core/interaction/events.rs` (`WireEvent`) and
 * `gateway/src/handlers/ws/messages.rs` (the client messages).
 */

/** The `protocol_version` of a session that runs interaction mode. */
export const INTERACTION_PROTOCOL_VERSION = '1.1';

// ============================================================================
// Session settings (`config.interaction`)
// ============================================================================

/** A tool the background LLM may call (an HTTP address the gateway calls). */
export interface InteractionTool {
  name: string;
  url: string;
  description?: string;
  /** JSON Schema of the arguments. */
  parameters?: Record<string, unknown>;
  /** The call changes something: the caller confirms it first. */
  side_effects?: boolean;
  /** Implies `side_effects`; only a clear "yes" confirms it. */
  irreversible?: boolean;
  confirm_phrase?: string;
  confirm_template?: string;
  progress_phrase?: string;
}

/**
 * The `interaction` object of the `config` message. Every key is optional; the gateway warns
 * (`config_warning`) about a value it ignores. Keys this type does not list are passed on too.
 */
export interface InteractionConfig {
  front?: {
    profile?: 'auto' | 'scripted' | 'model';
    /** A persona id (a session may only name one). */
    persona?: string;
    act_budget_ms?: number;
    total_budget_ms?: number;
    preferred_name?: string | null;
    caller?: {
      preferred_name?: string | null;
      verbosity?: 'brief' | 'normal' | 'detailed';
      pace?: 'slower' | 'normal' | 'faster';
      identity_token?: string;
    };
    [key: string]: unknown;
  };
  tasks?: {
    /** The background LLM, in the shape of `conversation_config` (wire names). */
    llm?: Record<string, unknown>;
    /** A backend of the server's `interaction.task_backends` (needs `allow_client_backends`). */
    default_backend?: string;
    /** Needs the server's `allow_client_tools`. */
    tools?: InteractionTool[];
    speculative_start?: 'auto' | 'on' | 'off';
    reconnect_grace_s?: number;
    /** The model that plans the tasks: a model name, or an LLM in the shape of `llm`. */
    orchestrator?: string | Record<string, unknown>;
    /** Entries of `interaction.task_backends` the orchestrator may start tasks on. */
    agents?: string[];
    /** No limit by default; work beyond one waits in a queue. */
    limits?: { max_live?: number };
    /** How a task's update reaches the caller while the conversation goes on. */
    announce?: 'ask' | 'tell' | 'screen';
    [key: string]: unknown;
  };
  /** Timing of acknowledgements, progress lines and questions, in ms (`policy.*`). */
  policy?: Record<string, number>;
  duplex?: {
    barge_in?: boolean;
    duck_db?: number;
    eager?: boolean;
    echo?: { correlation_gate?: boolean; aec?: 'auto' | 'on' | 'off' };
    backchannel?: boolean | { enabled: 'auto' | boolean };
    [key: string]: unknown;
  };
  events?: { floor?: boolean; segments?: boolean };
  hitl?: { channels?: Array<'voice' | 'client' | 'dtmf'> };
  [key: string]: unknown;
}

/** `ready.interaction`: the settings the session runs with. Tool URLs and keys are never echoed. */
export interface InteractionEcho {
  enabled: boolean;
  protocol_version: string;
  regime: string;
  front: Record<string, unknown>;
  tasks: {
    /** `llm`, `agent:<name>`, `dag` or `<type>:<name>`. */
    backend: string;
    speculative_start: string;
    /** Tool names only. */
    tools: string[];
    reconnect_grace_s: number;
    /** Whether a push address is set. */
    push: boolean;
  };
  policy: Record<string, number>;
  duplex: Record<string, unknown>;
  events: { floor: boolean; segments: boolean };
  hitl: { channels: string[] };
  /** Warning codes that changed behaviour (for example `front_model_scripted`). */
  degraded: string[];
  [key: string]: unknown;
}

// ============================================================================
// Events (server -> client)
// ============================================================================

/** `submitted`, `working`, `input_required`, `completed`, `failed`, `cancelled`, `rejected`, `unknown`. */
export type TaskState =
  | 'submitted'
  | 'working'
  | 'input_required'
  | 'completed'
  | 'failed'
  | 'cancelled'
  | 'rejected'
  | 'unknown';

/** `"none_committed"`, `"unknown"`, or the labels of what was done. */
export type TaskEffects = 'none_committed' | 'unknown' | string[];

interface TaskEventBase {
  task_id: string;
  task_ref: string;
  state: TaskState;
  cancelling: boolean;
  /** The task whose orchestrator started this one; `null` for a caller's request. */
  parent_id?: string | null;
  /** The catalog entry that runs it; `null` on the session's own background. */
  agent?: string | null;
  /** One per caller's request, shared by the tasks it starts (W3C trace id). */
  trace_id?: string;
  /** Milliseconds since the session started. */
  t_ms: number;
}

/** A request became a task. */
export interface TaskCreatedEvent extends TaskEventBase {
  type: 'task_created';
  revision: number;
  title: string;
  kind: 'lookup' | 'action';
  side_effects: boolean;
  speculative: boolean;
  /** `front`, or `provider_tool` on a speech-to-speech session. */
  origin: string;
  /** `llm`, `agent:<name>`, `dag` or `<type>:<name>`. */
  background: string;
}

/** The task moved: started, revised, progressed, changed effects, answer stopped, cancel asked. */
export interface TaskProgressEvent extends TaskEventBase {
  type: 'task_progress';
  revision: number;
  seq: number;
  text: string | null;
  fraction: number | null;
  spoken: boolean;
  effects: TaskEffects | null;
  answer_stopped: { reason: string; released_sentences: number } | null;
}

/** The task needs the caller: answer it with `answerTask(request_id, ...)`. */
export interface TaskQuestionEvent extends TaskEventBase {
  type: 'task_question';
  request_id: string;
  kind: 'confirm' | 'choose' | 'value' | 'form' | 'auth_url';
  prompt: string;
  options: string[] | null;
  /** A form's JSON Schema. */
  schema: Record<string, unknown> | null;
  /** The sign-in link of an `auth_url` question. */
  url?: string;
  sensitive: boolean;
  irreversible: boolean;
  /** Where it may be answered: `['client']` means on the screen only. */
  channels: string[];
  expires_at_ms: number;
  spoken: boolean;
  heard: string;
}

/** A question closed. */
export interface TaskAnswerReceivedEvent extends TaskEventBase {
  type: 'task_answer_received';
  request_id: string;
  outcome: 'accept' | 'decline' | 'cancel' | 'timeout' | 'superseded';
  reason: string | null;
  correction: string | null;
  /** `voice`, `client`, `dtmf`, `timeout`, `session_end` or `policy`. */
  by: string;
  corroborated: boolean;
  /** `null` for a sensitive question (`value_redacted`). */
  value: unknown;
  value_redacted: boolean;
}

/** The task ended (or a replaced revision's call went through: `superseded`). */
export interface TaskResultEvent extends TaskEventBase {
  type: 'task_result';
  revision: number;
  /** The same value as `state`. */
  status: TaskState;
  /** The answer of a completed task, otherwise what went wrong. */
  summary: string;
  effects: TaskEffects;
  spoken: 'full' | 'partial' | 'none';
  streamed: boolean;
  superseded: boolean;
}

/** A task completed: its whole result, published at once, before anything is said of it. */
export interface TaskOutputEvent extends TaskEventBase {
  type: 'task_output';
  revision: number;
  /** The answer as the background wrote it. */
  text: string;
  /** The backend's structured result, or `null`. */
  data: unknown;
  /** Links and files the backend returned. */
  artifacts: unknown[];
  effects: TaskEffects;
}

/** A cancel was confirmed. */
export interface TaskCancelledEvent extends TaskEventBase {
  type: 'task_cancelled';
  revision: number;
  by: string;
  reason: string;
  effects: TaskEffects;
}

/** Who is speaking changed (`events.floor`). */
export interface FloorStateEvent {
  type: 'floor_state';
  holder: 'user' | 'assistant' | 'both' | 'none';
  lane: string | null;
  reason: string;
  t_ms: number;
}

/** An utterance's audio starts or ends in the stream (`events.segments`). */
export interface AudioSegmentEvent {
  type: 'audio_segment';
  phase: 'start' | 'end';
  utterance_id: string;
  /** `front`, `main` or `backchannel`. */
  lane: string;
  kind: string;
  task_id: string | null;
  t_ms: number;
}

/** Agent audio was cut: drop what is still queued for playback. */
export interface AgentAudioStoppedEvent {
  type: 'agent_audio_stopped';
  utterance_ids: string[];
  reason: 'barge_in' | 'client_clear' | 'policy' | 'provider';
  delivered_ms: number[];
  t_ms: number;
}

/** The line the caller hears because a service failed ("When Services Fail"). */
export interface FailureNote {
  /** The line's id in the pack, for example `stall-1`. */
  line: string;
  kind: 'stall' | 'hearing' | 'exit' | 'exit_follow_up' | 'back';
  service: 'recogniser' | 'front' | 'background' | 'voice';
  text: string;
}

/** The reply to `updateInteraction`, a capability was lost (`degraded`), or a failure line plays (`failure`). */
export interface InteractionUpdatedEvent {
  type: 'interaction_updated';
  applied: string[];
  refused: Array<{ path: string; reason: string }>;
  degraded?: string[];
  failure?: FailureNote;
}

/** A task that was live when the session failed. */
export interface PendingTask {
  task_id: string;
  task_ref: string;
  title: string;
  state: string;
}

/**
 * A service the call needs stayed down: the exit line was said, and the gateway closes the
 * socket (close code 1011, reason `session_failed`).
 */
export interface SessionFailedEvent {
  type: 'session_failed';
  service: 'recogniser' | 'front' | 'background' | 'voice';
  reason: string;
  pending: PendingTask[];
  /** The exit line promised to get back to the caller. */
  follow_up: boolean;
  t_ms: number;
}

/** The agent said a line; an empty `delta` closes it and says how much was `heard`. */
export interface AssistantTranscriptEvent {
  type: 'assistant_transcript';
  turn_index: number;
  delta: string;
  source: 'front' | 'background' | 'gateway';
  /** `ack`, `relay`, `confirm`, `question`, `progress`, ... */
  kind: string;
  verbatim: boolean;
  act?: string;
  task_id?: string;
  utterance_id: string;
  sentence?: { attempt: number; seq: number };
  heard: 'full' | 'partial' | 'none';
}

/** Speech-to-speech sessions (`/realtime`): a claim of the provider's that no task supports. */
export interface UnverifiedClaimEvent {
  type: 'unverified_claim';
  kind: string;
  rule: string;
  text: string;
  corrected: boolean;
  output_stopped: boolean;
  t_ms: number;
}

/** One interaction-mode event; switch on `type`. */
export type InteractionEvent =
  | TaskCreatedEvent
  | TaskProgressEvent
  | TaskQuestionEvent
  | TaskAnswerReceivedEvent
  | TaskResultEvent
  | TaskOutputEvent
  | TaskCancelledEvent
  | FloorStateEvent
  | AudioSegmentEvent
  | AgentAudioStoppedEvent
  | InteractionUpdatedEvent
  | SessionFailedEvent
  | AssistantTranscriptEvent
  | UnverifiedClaimEvent;

export type InteractionEventType = InteractionEvent['type'];

/** Every interaction event's wire `type`. */
export const INTERACTION_EVENT_TYPES: ReadonlySet<string> = new Set<InteractionEventType>([
  'task_created',
  'task_progress',
  'task_question',
  'task_answer_received',
  'task_result',
  'task_output',
  'task_cancelled',
  'floor_state',
  'audio_segment',
  'agent_audio_stopped',
  'interaction_updated',
  'session_failed',
  'assistant_transcript',
  'unverified_claim',
]);

/**
 * Whether a server frame is an interaction event. An `assistant_transcript` without the 1.1
 * field `source` is a plain voice-agent line (the mode is off) and is not one.
 */
export function isInteractionEvent(message: { type?: unknown } & Record<string, unknown>): message is InteractionEvent & Record<string, unknown> {
  if (typeof message.type !== 'string' || !INTERACTION_EVENT_TYPES.has(message.type)) return false;
  return message.type !== 'assistant_transcript' || 'source' in message;
}

// ============================================================================
// Messages (client -> server)
// ============================================================================

/** What a client may answer a `task_question` with. */
export type TaskAnswerOutcome = 'accept' | 'decline' | 'cancel';

export interface TaskAnswerMessage {
  type: 'task_answer';
  request_id: string;
  outcome: TaskAnswerOutcome;
  /** With `accept`: the chosen option, the value, or a form's fields. */
  value?: unknown;
  /** With `decline`: what to change, in the caller's words. */
  correction?: string;
}

export interface TaskCancelMessage {
  type: 'task_cancel';
  task_id: string;
}

/** The caller read the task's result on the screen: a spoken update of it is dropped. */
export interface TaskSeenMessage {
  type: 'task_seen';
  task_id: string;
}

export interface InteractionUpdateMessage {
  type: 'interaction_update';
  /** The settings to change, in the shape of `config.interaction`. */
  patch: InteractionConfig;
}

export interface PlaybackPositionMessage {
  type: 'playback_position';
  utterance_id: string;
  played_ms: number;
}

export type InteractionClientMessage =
  | TaskAnswerMessage
  | TaskCancelMessage
  | TaskSeenMessage
  | InteractionUpdateMessage
  | PlaybackPositionMessage;

// ============================================================================
// The task list (GET /v1/tasks)
// ============================================================================

/** A task as `GET /v1/tasks` and `GET /v1/tasks/{task_id}` show it. */
export interface TaskView {
  id: string;
  task_ref: string;
  revision: number;
  title: string;
  state: TaskState;
  cancelling: boolean;
  effects: TaskEffects;
  /** The `request_id` of the open question, if any. */
  open_question: string | null;
  side_effects: boolean;
  started_at: number;
  /** Only from `getTask`. */
  stream_id?: string;
  /** Only from `getTask`: `null` while the task runs. */
  result?: { state: TaskState; summary: string; effects: TaskEffects } | null;
}
