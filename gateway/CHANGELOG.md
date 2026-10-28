# Changelog

All notable changes to WaaV Gateway will be documented in this file.

## [Unreleased]

### Bug Fixes

- **A long voice session keeps its prompt's start, so the LLM server's prefix cache works.** Once a session held `max_history` messages (20 by default), every new message removed the oldest one, so the message after the system prompt changed on every turn and llama.cpp, vLLM or OpenAI could reuse only the system prompt: each turn re-read the whole conversation before the first word. History is now trimmed in blocks, down to about half the window, starting at one of the caller's turns, so the start holds for the next several turns. The current turn is never removed (a window of 0 or 1 used to send the system prompt without the caller's message), system messages are kept, and a tool result is never kept without its call. The same rule trims the conversation interaction mode gives a background run. Reported by the Gaia team.

### Features

- **Aliases set the history of the sessions they serve:** `llm.max_history` and `llm.summarize_target_tokens` (see `docs/aliases.md`, Conversation history). A client's own value still wins, including one that sends the default; `conversation_config.max_history` and `summarize_target_tokens` may now be omitted or `null` on the wire. The `ready` message's `resolved_alias.llm` shows the window the session keeps.

## [1.1.0] - 2026-10-27

Interaction mode is opt-in: it runs only when the server sets `interaction.enabled` and a session asks for it (or the operator sets a default block). Sessions without an `interaction` block, and servers without the switch, behave as in 1.0.0. The exceptions are the provider changes under Bug Fixes, which apply with interaction mode on or off; each realtime wire fix that is on by default has an escape flag that restores the 1.0.0 frames.

### Features

- **Interaction mode for `/ws` voice sessions** (wire protocol 1.1). A fast front model (Qwen3.5-2B on `llama-server`, set with `interaction.front_model`) decides every caller turn in context: a request, a correction, a control, an answer or a wait. It acknowledges at once while a slower background runs the request as a task, and the answer is relayed word for word, sentence by sentence. There is no rule-based fallback: without a ready front model a session that asks gets `config_warning` `interaction_refused` (reason `front_unavailable`) and runs without the mode. The gateway can run and restart the front server itself (`front_model.command`).
- **Tasks and agents.** A task runs on the session's LLM (`conversation_config` or `tasks.llm`), a Bud voice agent, a DAG that ends in text, or an operator backend listed in `interaction.task_backends`: a signed webhook, an A2A 1.0 agent (JSON-RPC, HTTP+JSON or gRPC, streaming and push notifications), an MCP 2026-07-28 task tool, or an OpenAI Responses model. An orchestrator model (`tasks.orchestrator`, for example `gpt-oss-20b`) hands requests to several agents (`tasks.agents`) as child tasks with `parent_id` and `trace_id`. Results are published at once (`task_output`) and spoken at a free moment of the conversation (`tasks.announce`). Follow-ups continue a finished task on its agent.
- **Tasks API.** `GET /v1/tasks`, `GET /v1/tasks/{task_id}`, `GET /v1/tasks/events` (server-sent events) and `POST /v1/tasks/{task_id}/callback` for webhook callbacks and A2A pushes. With `interaction.task_redis_url` several gateway processes share their tasks.
- **Duplex floor and barge-in.** The gateway tracks who holds the floor (`floor_state`), ducks the agent's voice when the caller starts speaking and cuts it on a real interruption (`agent_audio_stopped`), plays listening sounds, gates the agent's own echo with optional echo cancellation, and tracks what the caller actually heard (`audio_segment`, `playback_position`).
- **Conversation controls.** Stop, hold, resume, repeat, spell, rate, volume, verbosity, language, voice, updates, to-screen, end and hand-off to a person, asked by the caller (read by the front model), the client app (`control` message), the orchestrator (`conversation_control` tool) or an agent. Each answers with `control_applied`; the operator sets the bounds in `controls`.
- **Human in the loop.** Tools with `side_effects` are read back and confirmed before they run; `ask_caller` and agents' questions (`task_question`) are answered by voice, on the client's screen (forms, sign-in links, sensitive values) or on a phone keypad, with deadlines (`policy.hitl_answer_timeout_ms`).
- **Operator features.** Personas (packaged and per tenant), caller memory, language packs as data, recorded listening sounds and failure lines, failure handling with honest stall and exit lines (`session_failed`), tasks that run on after the call with reconnect and a signed push, and LiveKit rooms and SIP calls.
- **Interaction mode beyond plain `/ws` sessions.** It also runs on Bud voice-agent sessions on `/ws` and `/v1/realtime` (events as `bud.*`), and on speech-to-speech providers on `/realtime` and the `/v1/realtime` relay that the operator lists in `interaction.truth_ack`.
- **SDK types.** The Python (`bud_waav.interaction`) and TypeScript (`InteractionEvent`) SDKs type the interaction events and add `answer_task`/`answerTask`, `cancel_task`/`cancelTask`, `update_interaction`/`updateInteraction`, `report_playback`/`reportPlayback`, and the task list calls.

### Bug Fixes

- **Realtime provider wire fixes after a documentation audit.** Each fix is listed in `interaction.provider_fixes` (`WAAV_PROVIDER_FIXES`) and each has an escape flag in `interaction.realtime_legacy_wire` (`WAAV_REALTIME_LEGACY_WIRE`) that restores the 1.0.0 frames. On by default: Deepgram Voice Agent `deepgram_user_text` (typed text is the user's turn), `deepgram_tools` (the session's functions reach the agent) and `deepgram_update_prompt` (`UpdatePrompt`/`UpdateSpeak` instead of a second `Settings`), all probed live; Grok `grok_session`, `grok_events` and `grok_voice_default` (probed live); and, on the vendors' documentation, `azure_ga_url`, `openai_manual_turn_null`, `openai_replay_output_text`, the Hume EVI and the Inworld fixes. The other fixes stay off until their vendor is probed. A `/realtime` session may turn a fix off for itself with `provider_fixes`.
- **Deepgram.** Flux (`flux-general-en`) on `/v2/listen`, language codes each model accepts, file-only `whisper-*` models refused with a clear error, `Finalize` at the gateway's end of turn, and per-model circuit breakers for the Voice Agent.
- **Other providers.** `POST /speak` streams its audio; ElevenLabs streaming synthesis; vendor rate limits and refusals reach the client with their status; retiring OpenAI and Hume models get a warning before their shutdown date and are refused after it.

### Also in this release (merged to main after 1.0.0, #38-#44)

- **Voices.** `GET /voices` reads every vendor from one table, 11 more vendors are listed, and the vendors' list parsers are fixed (#44). A vendor whose list fails is named in the `x-voices-failed` header, and the answer is 502 when every configured vendor fails.
- **Smallest.ai** synthesizes with Lightning v3.1; every earlier model is retired by the vendor (410) (#43).
- **SDKs.** Browser WebSockets send the key as a subprotocol instead of `?token=`, and the SDKs reach every gateway config field (#42).
- **Dependencies.** RUSTSEC-2026-0395 to 0410 fixed (zstd-safe 5.0.2, zstd-sys 2.0.16); the libwebrtc download unpacks with deflate only (#39).
- **CI and tests.** Every network wait and job in CI is bounded (#40); live-vendor tests treat an empty key as unset (#38).

### Documentation

- `docs/interaction_mode.md`: the full reference of interaction mode, its settings, events, messages, provider fixes and known limits.
- `docs/tutorials/interaction_mode_tutorial.md`: a step-by-step tutorial for a first interaction-mode session.

## [1.0.0] - 2026-10-26

The first stable release. Its changes since 0.1.2 are listed in the GitHub release notes for `1.0.0`, `1.0.0-rc.1` and `v0.1.3` to `v0.1.8`.

## [0.1.2] - 2025-12-30

### Miscellaneous

- Release v0.1.1 by @github-actions[bot]


### Bug Fixes

- Adding Livekit Room Controls based on WaaV Gateway Token by @tigranbs in [#7](https://github.com/waav-gatewayai/waav-gateway/pull/7)

- Improved release workflow with manual version control by @tigranbs

- Add actions:write permission for workflow dispatch by @tigranbs

## [0.1.0] - 2025-12-21

### Bug Fixes

- Adding release process by @tigranbs in [#5](https://github.com/waav-gatewayai/waav-gateway/pull/5)

<!-- Generated by git-cliff -->
