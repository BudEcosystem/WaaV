// =============================================================================
// Conversation / Agent-loop Types (built-in LLM loop + REALTIME_REASONING)
//
// 1:1 mirror of the gateway ConversationWebSocketConfig
// (gateway/src/handlers/ws/config.rs:53-217). When attached to a /ws session,
// the gateway wires up an automatic conversation loop: each finalized STT turn
// drives an OpenAI-compatible LLM and the reply is streamed to TTS, with
// per-session history, barge-in, a two-tier reasoning stack, and latency
// masking. Serialized into the `conversation_config` block of the `config`
// message. This is the flagship surface `bud.agent()` exposes.
// =============================================================================

/**
 * LLM vendor wire format (gateway `AdapterKind`, core/llm/adapter.rs:40).
 *
 * NOTE: this is NOT a registered openapi schema — `provider_kind` /
 * `reasoning_provider_kind` appear as a plain nullable `string` in the spec
 * (utoipa `schema(value_type = Option<String>)` override). The union is
 * hand-authored from adapter.rs. Omit => OpenAI-compatible with canonical-host
 * inference.
 */
export type AdapterKind = 'openai' | 'anthropic' | 'gemini';

/**
 * Reasoning / thinking-effort dial (REALTIME_REASONING D1; gateway
 * `ReasoningEffort`, adapter.rs:78). Mapped server-side to each vendor's native
 * thinking control and clamped to the model's floor. Keep a FAST, non-reasoning
 * model on the spoken path (`model`) for realtime latency.
 */
export type ReasoningEffort = 'off' | 'minimal' | 'low' | 'medium' | 'high';

/**
 * Latency-masking mode (REALTIME_REASONING D3; gateway `LatencyFiller`,
 * conversation/mod.rs:226, default `auto`). `auto` speaks ONE short action
 * phrase when first audio is slow, keeping the line alive while the real answer
 * streams in behind it.
 */
export type LatencyFiller = 'off' | 'auto' | 'aggressive';

/**
 * Two-tier reasoning routing (REALTIME_REASONING S2; gateway `RoutingMode`,
 * conversation/mod.rs:259, default `auto`).
 */
export type RoutingMode = 'auto' | 'always';

/**
 * User-mute strategy (conversation A-G5). While active, USER INPUT is
 * suppressed (bot/lifecycle signals always flow). Typed in the gateway as a
 * plain `Option<String>` (config.rs:125) — not an enum schema — so values are
 * a convention, not a hard contract.
 */
export type MuteStrategy =
  | 'always_while_bot_speaks'
  | 'until_first_bot_complete'
  | 'first_speech_only';

/**
 * Built-in conversation-loop configuration for the gateway
 * (`ConversationWebSocketConfig`, config.rs:53-217).
 *
 * `baseUrl` and `model` are the only required fields (mirrors the gateway);
 * every other field is optional and only sent when set, so the gateway applies
 * its own documented defaults.
 */
export interface ConversationConfig {
  // --- Core LLM ---
  /** OpenAI-compatible base URL for the LLM (e.g. 'https://api.openai.com/v1'). */
  baseUrl: string;
  /** Model identifier — keep this a FAST model (e.g. 'gpt-4o-mini', 'llama3.2:1b'). */
  model: string;
  /** Optional system prompt seeding the conversation. */
  systemPrompt?: string;
  /** API key (literal or '${ENV_VAR}'); falls back to OPENAI_API_KEY server-side. */
  apiKey?: string;
  /** Sampling temperature. */
  temperature?: number;
  /** Max tokens per completion. */
  maxTokens?: number;
  /** Stream tokens to TTS as they arrive (gateway default true). */
  streaming?: boolean;
  /** Max retained history messages (gateway default 20). */
  maxHistory?: number;
  /** Whether the bot's speech is interruptible / barge-in (gateway default true). */
  allowInterruption?: boolean;
  /** LLM vendor wire format: 'openai' (default) | 'anthropic' | 'gemini'. */
  providerKind?: AdapterKind;
  /** Strip markdown from LLM sentences before TTS (gateway default true). */
  stripMarkdown?: boolean;

  // --- Barge-in / mute (A-G group) ---
  /** Eager end-of-turn: start the LLM speculatively on a turn-complete prediction. */
  eagerEot?: boolean;
  /** While the bot speaks, require >= N words to interrupt it (values < 2 clamp to 2). */
  bargeInMinWords?: number;
  /** User-mute strategy (suppress user input while active). */
  muteStrategy?: MuteStrategy;
  /** Idle re-engagement after this many ms of silence (0/omitted = off). */
  userIdleTimeoutMs?: number;
  /** Token-aware context compaction threshold (0/omitted = off). */
  summarizeTargetTokens?: number;

  // --- Latency filler (D3) ---
  /** Latency-masking mode: off | auto | aggressive (gateway default auto). */
  latencyFiller?: LatencyFiller;
  /** Override the masking wait threshold in ms. */
  latencyFillerAfterMs?: number;
  /** Custom masking phrases (empty = built-in pool). */
  latencyFillerPhrases?: string[];

  // --- Reasoning tier (D1/S1/S2) ---
  /** Reasoning/thinking-effort dial: off | minimal | low | medium | high. */
  reasoningEffort?: ReasoningEffort;
  /** Optional slow REASONING tier model (e.g. 'o3', 'deepseek-r1'); turns two-tier on. */
  reasoningModel?: string;
  /** Reasoning-tier base URL (defaults to baseUrl). */
  reasoningBaseUrl?: string;
  /** Reasoning-tier API key (defaults to apiKey). */
  reasoningApiKey?: string;
  /** Reasoning-tier vendor wire format (defaults to providerKind). */
  reasoningProviderKind?: AdapterKind;
  /** Route turns between tiers: 'auto' (default) or 'always'. */
  reasoningRoute?: RoutingMode;
  /** Reasoning-tier max-silence-gap budget in ms (gateway default 15000; 0 disables). */
  reasoningBudgetMs?: number;
  /** Spoken line used when EVERY LLM tier fails. */
  degradationMessage?: string;
  /** Per-turn ceiling on LLM re-inference rounds (tool-call loop; gateway default 8). */
  maxLlmCallsPerTurn?: number;
  /** Hard ceiling on the reasoning tier's output tokens. */
  maxReasoningTokens?: number;
}

/**
 * Map a {@link ConversationConfig} (camelCase, SDK-facing) to the gateway
 * `conversation_config` wire block (snake_case). `base_url` + `model` are
 * always emitted; every other field is sent ONLY when explicitly set so the
 * gateway applies its own defaults (a literal `undefined` is never serialized).
 */
export function conversationConfigToWire(config: ConversationConfig): Record<string, unknown> {
  const wire: Record<string, unknown> = {
    base_url: config.baseUrl,
    model: config.model,
  };

  // [camelCase SDK field, snake_case wire key] pairs for the optional surface.
  const optional: Array<[keyof ConversationConfig, string]> = [
    ['systemPrompt', 'system_prompt'],
    ['apiKey', 'api_key'],
    ['temperature', 'temperature'],
    ['maxTokens', 'max_tokens'],
    ['streaming', 'streaming'],
    ['maxHistory', 'max_history'],
    ['allowInterruption', 'allow_interruption'],
    ['providerKind', 'provider_kind'],
    ['stripMarkdown', 'strip_markdown'],
    ['eagerEot', 'eager_eot'],
    ['bargeInMinWords', 'barge_in_min_words'],
    ['muteStrategy', 'mute_strategy'],
    ['userIdleTimeoutMs', 'user_idle_timeout_ms'],
    ['summarizeTargetTokens', 'summarize_target_tokens'],
    ['latencyFiller', 'latency_filler'],
    ['latencyFillerAfterMs', 'latency_filler_after_ms'],
    ['latencyFillerPhrases', 'latency_filler_phrases'],
    ['reasoningEffort', 'reasoning_effort'],
    ['reasoningModel', 'reasoning_model'],
    ['reasoningBaseUrl', 'reasoning_base_url'],
    ['reasoningApiKey', 'reasoning_api_key'],
    ['reasoningProviderKind', 'reasoning_provider_kind'],
    ['reasoningRoute', 'reasoning_route'],
    ['reasoningBudgetMs', 'reasoning_budget_ms'],
    ['degradationMessage', 'degradation_message'],
    ['maxLlmCallsPerTurn', 'max_llm_calls_per_turn'],
    ['maxReasoningTokens', 'max_reasoning_tokens'],
  ];

  for (const [field, key] of optional) {
    const value = config[field];
    if (value !== undefined) {
      wire[key] = value;
    }
  }

  return wire;
}
