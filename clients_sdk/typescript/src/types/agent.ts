/**
 * A Bud voice agent for a `/ws` session (spec 025), sent as the config envelope's `agent`.
 *
 * The agent decides both speech legs (its STT and TTS deployments), the voice and how turns are
 * taken; budprompt answers one turn per utterance. A session with an agent sends no conversation
 * or DAG block, and its `stt`/`tts` carry only the audio format: the gateway refuses a `model` on
 * them (`agent_owns_legs`). Mirrors the gateway `AgentWebSocketConfig`.
 */
export interface VoiceAgentConfig {
  /**
   * The agent's name (what `prompt:<name>` names on `/v1/responses`), optionally pinned as
   * `name:v<n>`. A `prompt:` prefix is accepted.
   */
  id: string;
  /** Pin a version; otherwise the agent's default version, pinned for the session. */
  version?: number;
  /** Text replies only: answers arrive as `assistant_transcript` and nothing is spoken. */
  textOnly?: boolean;
  /** The agent's structured input, once per session. */
  variables?: Record<string, unknown>;
}

/** An agent by name, or its full config. */
export function toVoiceAgentConfig(agent: VoiceAgentConfig | string): VoiceAgentConfig {
  const config = typeof agent === 'string' ? { id: agent } : agent;
  if (!config.id || !config.id.trim()) {
    throw new Error('an agent needs its name');
  }
  return config;
}

/** The `agent` wire block (snake_case), with only the fields set. */
export function voiceAgentConfigToWire(config: VoiceAgentConfig): Record<string, unknown> {
  const wire: Record<string, unknown> = { id: config.id };
  if (config.version !== undefined) wire.version = config.version;
  if (config.textOnly !== undefined) wire.text_only = config.textOnly;
  if (config.variables !== undefined) wire.variables = config.variables;
  return wire;
}
