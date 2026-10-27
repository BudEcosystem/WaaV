# Pipecat → WaaV: Exhaustive Comparative Analysis & Adoption Report

**Date:** 2026-06-12
**Subject repo:** `pipecat-ai/pipecat` (cloned for research at `/home/bud/ditto/research/pipecat`, 528 Python files / ~142k LOC) + companion `pipecat-ai/pipecat-flows` (`/home/bud/ditto/research/pipecat-flows`, 27 files).
**Compared against:** WaaV (`/home/bud/ditto/waav/WaaV`), a production Rust voice gateway (axum/tokio) — `VoiceManager` + `ConversationOrchestrator` + DAG executor, 67 providers (31 STT + 36 TTS).
**Mandate:** line-by-line, multi-pass, "don't miss a single thing" — analyse core engine, streaming, best practices, optimisations, production-hardening, provider integrations; identify what WaaV could adopt (learnings, optimisations, systems, features).

> Every Pipecat claim below is cited `file:line` (paths relative to `research/pipecat/src/pipecat/` unless absolute). Every WaaV status is grounded against actual WaaV source (cited `gateway/src/...`). Where the first analysis pass made a claim that grounding **corrected**, it is flagged ⚠️.

---

## 0. Executive Summary

Pipecat and WaaV solve the same problem — low-latency `audio → STT → LLM → TTS → audio` with barge-in — but with **opposite core abstractions**:

- **Pipecat** is a **typed-frame pipeline**: ~120 `Frame` types flow through a chain of `FrameProcessor`s, each running two async tasks (a priority lane + a FIFO lane). Turn-taking, interruption, metrics, and even speech-to-speech models are all expressed as frames and processors. This buys *uniformity* (one interruption mechanism for every component) and *composability* (pluggable turn strategies, observers, filters) at the cost of indirection.
- **WaaV** is a **trait + callback core**: `VoiceManager` owns `BaseSTT`/`BaseTTS` provider trait objects and drives them via callbacks and a speech-final state machine, with a separate DAG executor for graph routing. This buys *directness* and *Rust-native performance/safety* (zero-copy, `catch_unwind` isolation, bounded queues) at the cost of some hardcoded logic that Pipecat has factored into swappable strategies.

**Where WaaV is already at parity or ahead** (do **not** rebuild these): smart-turn v3 (identical model), reconnect backoff + circuit-breaker + storm control, LiveKit buffer-clear on barge-in, silero VAD, SIGTERM drain, per-turn latency profiler, bounded WS backpressure (Pipecat's queues are *unbounded*), per-session panic isolation, and a 160-char sentence safety-cap Pipecat lacks.

**The five highest-value things WaaV can take from Pipecat**, in priority order:

| # | Adoption | Type | Why it matters |
|---|----------|------|----------------|
| 1 | **Pluggable turn-taking strategy abstraction** (start / stop / mute) | System | WaaV's turn logic is hardcoded across `stt_result.rs` + `turn_decision/engine.rs` + `conversation/mod.rs`. Pipecat factors it into composable strategy objects → min-words barge-in gate, dual-timer end-of-turn with STT-finalization short-circuit, wake-phrase, mute-during-greeting — all for free. Directly lowers false barge-ins and end-of-turn latency. |
| 2 | **Universal LLM context + per-vendor adapter** | System | WaaV has only an OpenAI-compatible `LlmClient`. Pipecat keeps one OpenAI-shaped context and translates *just-in-time* to Anthropic / Gemini wire formats. The blueprint to support Anthropic Messages + Gemini natively (not via shims). |
| 3 | **Sentence-aggregation lookahead + unify WaaV's two divergent splitters** | Optimisation | WaaV has *two* sentence splitters (DAG vs conversation), neither with lookahead or decimal/abbreviation disambiguation → `"$29."`, `"3.14"`, `"Mr."` flush to TTS prematurely. Cheap fix, real prosody win. |
| 4 | **Structured conversation flows** (`pipecat-flows`) | Feature | `FlowManager` / `NodeConfig` / `ContextStrategy` = guided multi-step agents (IVR, forms, intake) with per-node context reset/summary. WaaV's DAG is *data-flow*, not *conversation-state* — a genuine capability gap. |
| 5 | **S2S as a service** (realtime LLM = `LLMService` subclass emitting the same downstream frames) | System | WaaV has a *separate* realtime handler (`core/realtime/`). Pipecat makes OpenAI Realtime / Gemini Live / Nova Sonic drop-in replacements for the cascade. Squarely on WaaV's recorded realtime roadmap. |

Plus a long tail of **production-hardening techniques** (bounded forced-shutdown, fatal/non-fatal error tiers, observer fan-out isolation, quick-failure circuit-breaker special-case, VAD-aware deferred reconnect with audio replay) and **features** (voicemail detection, IVR/DTMF, call recording, eval harness with LLM-as-judge, idle re-engagement). Full ledger in §12.

---

## 1. Methodology

Per the "review multiple times / don't miss a single thing" mandate, coverage was built in layers:

1. **Module inventory** of all 528 files; identified the load-bearing subtrees (`frames/`, `processors/`, `pipeline/`, `services/`, `turns/`, `audio/`, `transports/`, `bus/`, `observers/`, `adapters/`, `utils/`).
2. **Five parallel deep agents**, each owning a domain and armed with WaaV's architecture: (a) core engine, (b) streaming/turn-taking, (c) services/providers, (d) production-hardening/observability, (e) optimisations/aggregation. Each returned a `file:line`-cited report with per-finding WaaV verdicts.
3. **First-hand reads** of the cruxes: `frames/frames.py` (taxonomy), `processors/frame_processor.py` (two-task model + interruption), `utils/string.py` (`match_endofsentence`), `pipeline/worker.py` (heartbeat), and the entire `pipecat-flows` package.
4. **Two completeness-critic passes** over the lower-coverage subtrees: (a) `extensions/` + full `processors/` tree + `evals/`; (b) `transports/` provider-specifics (esp. LiveKit) + `serializers/`. Reported **only net-new** findings.
5. **WaaV grounding pass** — verified every top finding against WaaV's actual source before assigning a status, which corrected several first-pass assumptions (see ⚠️ flags).

**Corrections grounding produced (important — these are *not* gaps):**
- ⚠️ Smart-turn: WaaV is **already on v3** — it loads the identical `smart-turn-v3.2-cpu.onnx` from pipecat-ai's HF repo (`gateway/src/core/smart_turn/detector.rs:30-33`) with a Whisper-mel front-end (`whisper_mel.rs`). The "WaaV is on v2-era ONNX" note was stale.
- ⚠️ LiveKit barge-in flush: WaaV **already** clears the sink (`gateway/src/livekit/client/audio.rs:192,216` → `audio_source.clear_buffer()`). PARITY, not a gap.
- ⚠️ Exponential backoff: WaaV **already** has it (`gateway/src/core/websocket/reconnection.rs` `calculate_delay`, + `core/resilience/{reconnect_governor,circuit_breaker}.rs`). PARITY.
- ⚠️ Multilingual sentence splitting: WaaV's DAG path already handles the Devanagari danda (`dag/nodes/llm.rs:33`, tested at `:481`); the conversation path covers western+CJK+Devanagari (`core/conversation/mod.rs:321`). So this is *partial parity* — the real gap is lookahead + decimal disambiguation + unification, not multilingual coverage wholesale.

---

## 2. Architectural Comparison

| Dimension | Pipecat | WaaV |
|---|---|---|
| **Core abstraction** | Typed `Frame` pipeline; `FrameProcessor` chain (`frames/frames.py`, `processors/frame_processor.py`) | `VoiceManager` owning `BaseSTT`/`BaseTTS` trait objects + callbacks (`gateway/src/core/voice_manager/manager.rs`) |
| **Concurrency unit** | 2 tasks per processor: priority input queue + FIFO process queue (`frame_processor.py:996,1024`) | Tokio tasks per provider stream + per-turn `CancellationToken` |
| **Graph routing** | `Pipeline` / `ParallelPipeline` / `SyncParallelPipeline` of processors | Dedicated DAG executor (`gateway/src/dag/executor.rs`) |
| **Interruption** | One broadcast `InterruptionFrame`; every processor cancels+recreates its task (`frame_processor.py:718,842`) | Centralised `VoiceManager.interruption_state` + `clear_tts()` + per-turn cancel |
| **Turn-taking** | Pluggable strategy stack (`turns/user_start`, `user_stop`, `user_mute`) | Hardcoded speech-final state machine (`stt_result.rs`) + `turn_decision/engine.rs` |
| **Backpressure** | **Unbounded** `asyncio.Queue` everywhere; cooperative one-frame-at-a-time | **Bounded** per-class WS queues (WaaV ahead) |
| **Memory safety** | Python GC; `deque(maxlen=N)` dedup caps; dangling-task warnings | Rust ownership; `catch_unwind` per session (WaaV ahead) |
| **S2S** | `LLMService` subclass emitting normal `TTSAudioRawFrame` (drop-in for cascade) | Separate realtime handler (`core/realtime/`) |
| **Context** | Universal `LLMContext` + per-vendor adapters | OpenAI-compatible `LlmClient` only |

The single most consequential difference: **Pipecat's typed frame bus makes cross-cutting behaviour uniform** (interruption, metrics, observability, lifecycle all ride frames), whereas **WaaV wires each cross-cutting concern explicitly**. WaaV should *not* rewrite to a frame bus — that's a multi-month rearchitecture with no clear ROI given the DAG already exists — but it should adopt the *factored abstractions* the frame bus enabled (turn strategies, observers, universal context), implemented as Rust traits.

---

## 3. Core Engine (goal: "Core engine")

### 3.1 Three-tier frame taxonomy with a priority lane — the interruption keystone
`frames/frames.py`: `Frame:58` → `SystemFrame:99` (priority/immediate, interruption-immune), `DataFrame:110` (ordered/interruptible), `ControlFrame:122`. Plus the `UninterruptibleFrame` mixin (`:141`) — `EndFrame`, `FunctionCallResultFrame` survive barge-in. The processor's input queue is a `PriorityQueue` that gives `SystemFrame`s `HIGH_PRIORITY` (`frame_processor.py:119-167,996`), so a `CancelFrame`/`InterruptionFrame` jumps ahead of any backlog at *every* processor.

**WaaV verdict — Learning (not a port):** WaaV's interruption is centralised and already correct, but the *concept* worth adopting is the **`UninterruptibleFrame` distinction**: WaaV's blanket `clear_tts()` can't express "this TTS chunk (a function-call result, a critical disclaimer) must finish even on barge-in." Add an `interruptible: bool` to the TTS playback unit so specific utterances survive a clear.

### 3.2 FrameProcessor two-task model + cancel-and-recreate
`frame_processor.py`: each processor runs `__input_frame_task_handler:996` (drains priority queue, handles system frames inline, shunts data frames to `__process_queue`) and `__process_frame_task_handler:1024` (FIFO, one frame fully processed before the next). On interruption, `_start_interruption:842` **cancels and recreates** the process task — which kills any in-flight streaming coroutine (LLM token generation, TTS synthesis) instantly and for free.

**WaaV verdict — PARITY (different mechanism):** WaaV achieves the same in-flight cancellation via `CancellationToken` checked in the LLM/TTS loops (`core/llm/mod.rs:870`). No change needed; the Pipecat model is just the frame-native expression of what WaaV does with tokens.

### 3.3 Pipeline composition + ServiceSwitcher (warm-standby failover)
`ParallelPipeline` fans out to sub-pipelines with fan-in dedup + lifecycle barriers; `ServiceSwitcher`/`LLMSwitcher` hold warm-standby services and hot-swap on failure with **zero teardown** (the standby is already `StartFrame`-initialised).

**WaaV verdict — GAP (medium):** WaaV recovers from provider failure via *reconnect* (`core/resilience/`), which has a cold-start cost. A warm-standby pattern — a second STT/TTS provider pre-initialised and swapped on circuit-breaker trip — would cut failover latency to near-zero. Aligns with the standing "standardised across all providers" instruction: implement as a generic `WarmStandby<P: BaseSTT>` / `WarmStandby<P: BaseTTS>` wrapper, available to all 67 providers.

### 3.4 Determinism primitives: StartFrame barrier, Flush probe, heartbeat, SystemClock
- `StartFrame` init barrier — no data flows until every processor is initialised (`frame_processor.py:805`).
- `PipelineFlushFrame` round-trips source→sink→source to *confirm drainage* (`pipeline/worker.py:746`, 5s timeout).
- Per-worker monotonic `SystemClock`.

**WaaV verdict — PARITY/Learning:** WaaV has `now_monotonic_ns()` and readiness gating (`core/readiness.rs`). The **Flush probe** ("inject a sentinel, confirm it round-trips before declaring drained") is a clean testing/shutdown primitive WaaV could add for its DAG executor.

---

## 4. Streaming System (goal: "Streaming system")

### 4.1 Output pacing — the `MediaSender` (the audio-pacing answer)
`transports/base_output.py`: one `MediaSender` per destination. Output is sliced into **10ms × `audio_out_10ms_chunks` (default 4 = 40ms)** chunks "to help with interruption handling" (`:131-135,580-588,82-85`). Three pacing models:
1. **Device/SDK backpressure** (local/Daily) — `write_audio_frame` awaits the device.
2. **WebSocket software clock** — sends then sleeps half the chunk duration → 2× realtime to keep a small lead (`websocket/server.py:314,397`).
3. **WebRTC pull clock** — 10ms sub-chunks pulled at codec cadence; emits silence when empty (`smallwebrtc/transport.py:101,134`).

Because only 1–4 chunks (10–40ms) are ever buffered ahead, **barge-in residual audio is bounded to ~one chunk** regardless of TTS frame size.

**WaaV verdict — STEAL THIS (medium):** the **fixed ≤40ms output chunking** is the load-bearing trick for tight barge-in. WaaV should confirm its egress chunks to LiveKit/WS are bounded to ≤40ms so a clear truncates within one chunk. (WaaV already does `clear_buffer()`; small chunks make that clear *audibly instant*.)

### 4.2 VAD — silero dual-gate + idle failsafe
`audio/vad/vad_analyzer.py`: 4-state machine (QUIET→STARTING→SPEAKING→STOPPING) with **confidence AND volume** gate (`speaking = confidence ≥ 0.7 AND volume ≥ 0.6`, `:206`), volume via EBU-R128 loudness (`pyloudnorm`) with exponential smoothing. Silero resets LSTM state every 5s to bound memory (`silero.py:215`). **Audio-idle failsafe**: if no audio for 1.0s while SPEAKING (mic mute mid-speech), force speech-stop (`vad_controller.py:101,194`).

**WaaV status:** WaaV has silero VAD (`core/silero_vad/`, `core/audio/vad.rs`). Three cheap wins it likely lacks:
- **STEAL (low):** the **volume-AND-confidence dual gate** (reduces false triggers from low-level noise that clears the NN threshold).
- **STEAL (low):** the **5s state-reset** discipline.
- **STEAL (low):** the **audio-idle forced-stop** — a dropped/muted stream mid-utterance currently can hang WaaV's turn.

### 4.3 Smart-turn — ⚠️ PARITY (WaaV already on v3)
`audio/turn/smart_turn/local_smart_turn_v3.py`: ONNX `smart-turn-v3.2-cpu.onnx`, resample→16kHz soxr HQ → truncate/pad to **exactly 8s keeping the tail** → Whisper log-mel (vendored numpy, no torch) → sigmoid, **threshold 0.5**.

**WaaV verdict — ⚠️ PARITY:** `gateway/src/core/smart_turn/detector.rs:30-33` loads the **identical model** from the same HF repo; `whisper_mel.rs` is WaaV's Whisper-mel front-end. **Do not treat as a gap.** One nuance *might* be portable: Pipecat re-adds the VAD-swallowed onset to the model input by extending the segment back by `pre_speech_ms + vad_start_secs` (`base_smart_turn.py:169,193`). Worth a one-line check that WaaV's segment extraction (`detector.rs:~708`) isn't clipping the onset; if it is, extending the window back by the VAD `start_secs` is a free accuracy bump.

### 4.4 Pluggable turn-taking strategy stack — **the headline GAP**
This is the single biggest structural difference. Pipecat's turn brain is **not** in the transport or STT — it's a stack of composable strategy objects consumed by the LLM user aggregator (`turns/` + `processors/aggregators/llm_response_universal.py`). Three orthogonal families:

**Start strategies** (`turns/user_start/`, when does a turn / barge-in begin) — default `[VAD, Transcription]`:
- `MinWordsUserTurnStartStrategy` (`min_words_user_turn_start_strategy.py:105`): **`min_words = self._min_words if bot_speaking else 1`** — require N words to interrupt while the bot speaks, 1 word when silent. *The* production answer to "don't let a cough/'uh-huh' interrupt the bot." Insufficient words → `trigger_reset_aggregation()` discards the partial.
- `TranscriptionUserTurnStartStrategy` — fallback start on any interim transcript even if VAD missed soft speech.
- `WakePhraseUserTurnStartStrategy` — wake-word FSM, strips pre-wake speech from context.
- `ExternalUserTurnStartStrategy` — server-driven.

**Stop strategies** (`turns/user_stop/`, when is the turn over) — default `[TurnAnalyzer(SmartTurnV3)]`:
- `SpeechTimeoutUserTurnStopStrategy` (`speech_timeout_user_turn_stop_strategy.py`) — **the right version of WaaV's 600ms timer**: two *independent* timers — `user_speech_timeout` (0.6s policy floor, same as WaaV) **and** `stt_timeout` (= STT TTFS-p99 − VAD stop_secs), the latter **short-circuited when STT emits `finalized=True`**. Turn ends only when both elapse AND ≥1 transcript arrived. WaaV's single 600ms timer can fire *before* a slow STT returns its final → truncated last words.
- `TurnAnalyzerUserTurnStopStrategy` — smart-turn model gated on transcript arrival; `wait_for_transcript=False` mode takes transcripts off the critical path for S2S.
- `LLMTurnCompletionUserTurnStopStrategy` — semantic end-of-turn via an LLM marker protocol (the model prefixes `✓`/`○`/`◐`; `✓` finalizes, `○`/`◐` re-prompts and keeps the turn open).
- `DeferredUserTurnStopStrategy` — wraps a detector so it triggers *speculative inference* while a *different* strategy owns *finalization* (the composability primitive behind predictive turn-taking).

**Mute strategies** (`turns/user_mute/`, suppress user input) — `Always`, `FirstSpeech`, `MuteUntilFirstBotComplete`, `FunctionCall`. Solve "don't let the user interrupt the greeting / the disclaimer / a tool call."

**WaaV verdict — GAP (very high):** WaaV's turn logic is hardcoded in `stt_result.rs` + `turn_decision/engine.rs` + `conversation/mod.rs`; there is no `*Strategy` trait (grep-confirmed). Adopt a Rust trait set:
```
trait UserTurnStartStrategy { fn on_signal(&mut self, sig: TurnSignal) -> StartVerdict; }
trait UserTurnStopStrategy  { fn on_signal(&mut self, sig: TurnSignal) -> StopVerdict;  }
trait UserMuteStrategy      { fn is_muted(&self, ctx: &TurnCtx) -> bool; }
```
First two concrete impls to ship: **`MinWords` start-gate** (kill false barge-ins) and **dual-timer `SpeechTimeout` stop** (add the STT-p99 safety net WaaV lacks). Per the standing instruction, the abstraction is provider-agnostic — every STT feeds the same `TurnSignal` stream. This also cleanly houses WaaV's existing smart-turn + eager-EoT as strategies rather than hardcoded branches.

### 4.5 Interruption end-to-end
`broadcast_interruption()` (`frame_processor.py:718`) sends paired `InterruptionFrame`s upstream **and** downstream; every processor flushes itself via task cancel (`_start_interruption:842`), except `UninterruptibleFrame`s (drained selectively via `FrameQueue.reset()`). LLM token generation stops because it lives inside the cancelled task; TTS `_handle_interruption` (`services/tts_service.py:914`) resets aggregators + drops queued audio; output `MediaSender.handle_interruptions` clears the partial buffer.

**WaaV verdict — PARITY + one Learning:** WaaV's centralised `interruption_state` + `clear_tts()` + `CancellationToken` is equivalent and arguably simpler for a single-session gateway. The Learning is the `UninterruptibleFrame` concept (§3.1).

### 4.6 Eager / predictive turn — two mechanisms
(A) **Inference-triggered speculation** (`llm_response_universal.py:1186`): a stop strategy can fire `on_user_turn_inference_triggered` *before* finalization → writes the user message to context → kicks the LLM while the turn is still open; multiple inferences accumulate into `_full_user_turn_aggregation` so no segment is lost. The `deferred()` wrapper separates *speculation* from *finalization*.
(B) **LLM-as-judge marker** (`turns/user_turn_completion_mixin.py:69`) — the ✓/○/◐ protocol above.

**WaaV verdict — GAP (medium):** WaaV's eager-EoT (speculative LLM on smart-turn prediction, `core/conversation/mod.rs`) ≈ mechanism (A) but **without** the inference-triggered-vs-finalized split or multi-segment accumulation. Adopting that distinction lets a wrong speculation be cleanly superseded rather than committed. Mechanism (B) is a whole capability WaaV lacks.

---

## 5. Provider Integrations (goal: "provider integrations")

### 5.1 Service abstraction — everything is a FrameProcessor
`services/ai_service.py:31` `AIService` → `STTService:47` / `TTSService:107` / `LLMService:255`, with `WebsocketService` (`services/websocket_service.py:23`) a **standalone mixin** (connect/reconnect/receive-loop) mixed into all three. A provider implements *only* the wire protocol (`run_stt`/`run_tts`/`_process_context`); the base owns aggregation, metrics, interruption, lifecycle frames, and audio resampling.

**WaaV verdict — PARITY (design):** WaaV's `BaseSTT`/`BaseTTS` traits + `from_standard` config already embody "provider implements wire protocol, base owns the rest." The reuse-once-inherit-everywhere reconnect mixin maps to WaaV's `ReconnectableStream`.

### 5.2 Settings layer — the NOT_GIVEN delta/store pattern
`services/settings.py`: one dataclass per service type, doubling as a **store** (full state, `None`=unsupported) and a **delta** (sparse update, unset=`NOT_GIVEN` sentinel, `:57-117`). `apply_update(delta)` merges + returns changed fields; `validate_complete()` asserts no `NOT_GIVEN` leaked. Aliases + an `extra` overflow dict (`:194,446`) so provider-specific knobs don't force schema changes. Runtime `*UpdateSettingsFrame(delta=...)` flows as a frame → mid-call voice/model/language change *without teardown*; `_update_settings` reconnects *only if* a connection-relevant field changed (`stt_service.py:643`).

**WaaV verdict — STEAL THIS (medium):** WaaV's `StandardSTTConfig`/`from_standard` covers the standard→provider mapping, but likely lacks (a) **mid-call reconfiguration** (change voice/model/language live) and (b) the **`extra` overflow** for provider-specific knobs. Add a delta type + `update_settings(delta)` to `BaseSTT`/`BaseTTS` (standardised across all providers), reconnecting only on connection-relevant changes.

### 5.3 STT-specific reconnect resilience — VAD-aware deferral + audio replay
`services/stt_service.py`: defers reconnect while the user is speaking (`_can_reconnect=False` on `VADUserStartedSpeaking`, fires on `UserStopped`, `:643,532`); **buffers incoming audio during the reconnect gap and replays it** after the new connection is up (`:361,605`) → no speech lost mid-utterance. Keepalive via 100ms silence (`:657`). Finalize handshake (`request_finalize`/`confirm_finalize` → next transcript `finalized=True`).

**WaaV verdict — STEAL THIS (high):** WaaV has reconnect + "featured restore" (per memory) but the concrete **buffer-audio-during-reconnect-and-replay + defer-until-user-stops-speaking** pair is a robustness win over a bare circuit-breaker. Implement in `ReconnectableStream` so all STT providers get it.

### 5.4 The WebsocketService reconnect engine + quick-failure breaker
`services/websocket_service.py`: `_try_reconnect:83` (≤3 retries, exponential backoff, single-flight guard, ping-verify before declaring success). **Quick-failure circuit-breaker** (`:142-201`): tracks `_MIN_STABLE_CONNECTION_DURATION=5.0s` / `_MAX_CONSECUTIVE_QUICK_FAILURES=3` — if the handshake keeps succeeding but the connection dies <5s each time (bad API key, policy reject), backoff won't help, so after 3 quick failures it **stops and reports fatal**. `WebsocketLLMService` raises `WebsocketReconnectedError` so the caller **restarts the inference** (server-side state is gone).

**WaaV verdict — PARITY + STEAL (low):** WaaV has backoff + `CircuitBreaker` + `ReconnectGovernor` (`core/resilience/`, `core/websocket/reconnection.rs`) — PARITY. The one cheap addition: the **"<5s-stable × 3 = give up fatal" special-case** so WaaV's breaker doesn't backoff-loop forever on a permanently-bad credential (distinct from normal backoff). And the `WebsocketReconnectedError`→restart-inference signal for any request/response WS provider.

### 5.5 Per-provider wire protocols (reference confirmations)
- **Deepgram Aura WS TTS** (`services/deepgram/tts.py`): `{"type":"Speak"}` + `Flush`/`Clear`/`Close`, `push_start_frame=True, push_stop_frames=False` — ⚠️ **exactly** what WaaV just implemented (`gateway/src/core/tts/deepgram_aura.rs`). PARITY confirmed.
- **The universal interruption seam** (`tts_service.py:1551`): every WS TTS provider overrides `on_audio_context_interrupted(context_id)` (send cancel/clear) and `on_audio_context_completed` (close server context). The base calls them from the audio-context drain loop.
- **ElevenLabs 10s keepalive** (`elevenlabs/tts.py:931`) — its socket dies at 5 min.

**WaaV verdict — STEAL THIS (medium):** the **`on_audio_context_interrupted`/`completed` override seam** is a clean, standardised barge-in/close hook to add to WaaV's `BaseTTS` (`core/tts/base.rs`) so all 36 TTS providers express cancel/close uniformly instead of ad-hoc per provider. Plus ElevenLabs' keepalive.

### 5.6 ⭐ Universal LLM context + adapter — **top strategic GAP**
`processors/aggregators/llm_context.py` + `adapters/`: `LLMContext:95` holds an **OpenAI-shaped** message list + `ToolsSchema` + `tool_choice`, treated as its *own* boundary type. It is never converted eagerly — each provider's `BaseLLMAdapter` (`adapters/base_llm_adapter.py:33`, generic) translates the *same* context to its wire format just-in-time:

| | system msg | tool def | tool result |
|---|---|---|---|
| OpenAI | inline | `{"type":"function",...}` | `{"role":"tool",...}` |
| Anthropic | **extracted to `system` param** | `{name,description,input_schema}` | `{"role":"user",[{"type":"tool_result",...}]}`; +prompt-cache markers |
| Gemini | separate `system_instruction` | `function_declarations` (strip `additionalProperties`) | `Part(function_response=...)`; +thought-signatures |

Escape hatches: `LLMSpecificMessage` (`:80`, carry provider-native messages alongside universal ones, filtered per provider) and `ToolsSchema.custom_tools` (per-adapter overflow). `LLMService` is generic over the adapter (`llm_service.py:255`, default `OpenAILLMAdapter`).

**WaaV verdict — GAP (top priority):** WaaV's `core/llm/mod.rs` is OpenAI-compat only. This is the blueprint to target Anthropic Messages + Gemini generateContent *natively*. The two non-obvious must-haves: (a) **system-prompt placement differs** (inline vs separate param — encapsulate in a `resolve_system_instruction` helper), (b) **tool-result shape differs wildly** per vendor. Implement as a `LlmAdapter` trait with `OpenAi`/`Anthropic`/`Gemini` impls; keep WaaV's one OpenAI-shaped context as the universal type. Standardised: every LLM provider plugs in via the same trait.

### 5.7 ⭐ S2S as an `LLMService` subclass — GAP (on roadmap)
OpenAI Realtime, Gemini Live, AWS Nova Sonic are **all `LLMService` subclasses** marked by a `RealtimeServiceInfo` class var (`llm_service.py:109,300`), carrying `emits_user_turn_frames` (whether the model supplies its own turn signals). They emit the **same downstream frames** as a cascade (`TTSAudioRawFrame`/`TTSStartedFrame`/`LLMTextFrame`) and go through the **same `run_function_calls`** — so a single model drops in where a cascade was with no downstream changes. Barge-in = server-item truncate + `response.cancel` + **replay a short audio preroll** (so the first phoneme after interruption isn't lost). Socket-lifetime handled via **session-resumption** (Gemini) / **transparent handoff** (Nova Sonic pre-creates the next session before the ~8-min timeout).

**WaaV verdict — GAP (high, roadmap-aligned):** WaaV has a *separate* realtime handler (`core/realtime/{base,openai,hume}`). Refactor S2S to a `BaseLLM`-shaped service (or a DAG S2S node) that emits WaaV's normal TTS-audio callbacks, so realtime is a drop-in for the cascade — exactly the "S2S DAG node" in WaaV's realtime roadmap. Steal specifically: (1) the **`emits_user_turn_frames` capability flag** driving whether the pipeline supplies local VAD/turn detection, (2) the **truncate+cancel+preroll-replay** barge-in, (3) **session resumption/handoff** for max-socket-lifetime.

### 5.8 Function/tool orchestration
`llm_service.py`: `register_function` + `register_direct_function` (auto-schema from a typed fn, auto-registered/unregistered as the advertised tool set changes, `:820,960`). `run_function_calls:1131` runs parallel (default) or sequential; **`group_id` re-triggers the LLM exactly once after the last call in a batch** (`:1152`). **Async tools** (`cancel_on_interruption=False`) let the LLM continue immediately, result injected later as a developer message, with a built-in `cancel_async_tool_call` tool the model can invoke (`:1406`).

**WaaV verdict — GAP (medium, if native tools wanted):** standout ideas — **direct-function auto-schema**, **group-id "trigger LLM once after the batch"** (no duplicate responses on parallel tool calls), and **async non-blocking tools with a cancellation tool**.

### 5.9 TTS word-timing playback queue + the STTMetadata broadcast
- Word timestamps are first-class in `TTSService` now (`add_word_timestamps:1197`): word entries routed into the **per-context audio queue** so they're processed in strict playback order alongside audio, converted to `TTSTextFrame`s with a PTS baseline. Enables (a) accurate "what was *actually spoken*" context capture on interruption, (b) precise barge-in truncation. The load-bearing idea is the **per-context queue interleaving audio + word timestamps in playback order**, more than the timestamps themselves.
- **`STTMetadataFrame(ttfs_p99_latency)`** broadcast at pipeline start (`stt_service.py:484`) feeds downstream turn strategies the STT latency they need to tune end-of-turn timing.

**WaaV verdict:** word-timing = **GAP (medium)** — per memory WaaV's TTS word-timestamps are "mostly unused"; this is the machinery to make "add only the spoken prefix to context on interruption" correct. STTMetadata ttfs_p99 = **STEAL (low)** — directly feeds the dual-timer stop strategy (§4.4) and WaaV's latency profiler.

---

## 6. Production Hardening (goal: "production-hardened logic/methodology/algorithms")

> Notable finding: this Pipecat fork **removed** the generic per-task watchdog. Its hardening is narrower than expected, and WaaV is at-or-ahead on most shared essentials.

### 6.1 Heartbeat (advisory liveness) + idle monitor
`pipeline/worker.py`: opt-in heartbeat (`HEARTBEAT_SECS=1.0`, `HEARTBEAT_MONITOR_SECS=10.0`, `:88`) injects a `HeartbeatFrame` at the pipeline head, measures **end-to-end traversal latency** at the sink (`:1226`), and **only WARNs** on timeout — it does **not** kill anything. Separately, an idle-frame monitor (`IDLE_TIMEOUT_SECS=300`, default-on) tears the pipeline down after 300s with no activity frames (`:1234,1250`) — an *activity-gap* detector (minutes), not a hung-task detector.

**WaaV verdict — STEAL THIS (medium):** the **pipeline-traversal heartbeat as a liveness probe** is complementary to WaaV's per-turn `LatencyProfiler` (which only measures *real* turns) — inject a sentinel through STT→LLM→TTS when *no* turn is happening to detect a wedged path. Note even Pipecat only warns; neither system auto-kills. WaaV's idle handling gap is better filled by the `UserIdleController` feature (§8).

### 6.2 Task supervision — bounded forced-shutdown + dangling-task audit
- **EndFrame = graceful drain** (no timeout); **CancelFrame = forced**, bounded at `CANCEL_TIMEOUT_SECS=20.0` — on timeout it logs "being blocked somewhere?" and proceeds anyway (`worker.py:961`), so shutdown never hangs.
- Cancel-safety: `INPUT_TASK_CANCEL_TIMEOUT_SECS=3` + 1.0s default `cancel_task` timeout, with the explicit rationale "if a library swallows `asyncio.CancelledError`, detect via timeout and log instead of hanging" (`frame_processor.py:928`, `base_object.py:143`).
- `_print_dangling_tasks()` WARNs the names of any tasks still alive at shutdown (`:1290`) — a leak detector.

**WaaV verdict — STEAL THIS (medium):** WaaV's `catch_unwind` isolates panics but doesn't enumerate leaked Tokio tasks. Adopt: (a) a **bounded forced-shutdown** with a "blocked somewhere?" log rather than an unbounded await, (b) the **"swallowed-cancel → timeout, log, don't hang"** guard specifically on **FFI/native awaits** (ONNX/ORT inference, codec calls) where a Tokio cancel might not land, (c) a **dangling-task warning** at session teardown.

### 6.3 Error tiers — fatal vs non-fatal
`ErrorFrame` (`frames.py:905`) carries `fatal: bool`. Surfaced **upstream** with file:line from the traceback (`frame_processor.py:644`); at the source, **non-fatal = log + continue** (one processor's hiccup), **fatal = inject CancelFrame = whole pipeline down** (`worker.py:1145`). A single processor error never kills the pipeline unless marked fatal. No frame-layer retry — retry lives only at the connection layer (§5.4).

**WaaV verdict — GAP (medium):** WaaV's `catch_unwind` is coarser (panic = whole session) with no "non-fatal, log-and-continue per-stage" tier. Porting the **fatal/non-fatal distinction** lets a transient provider error degrade one DAG stage / one turn instead of dropping the call.

### 6.4 Observability — observer-without-processor + fan-out isolation + metrics-as-frames
- `BaseObserver` (`observers/base_observer.py`) sees **every** frame transfer without being in the chain (`on_push_frame`/`on_process_frame`).
- **`WorkerObserver`** (`pipeline/worker_observer.py`) gives **one queue + one task per registered observer** (`:22,153`), so a slow observer (e.g. a remote-export sink) **can't block the pipeline or sibling observers** — it only backs up its own queue.
- **Metrics are normal frames in-band**: `MetricsFrame` (TTFB, processing, LLM token usage incl. cache/reasoning, TTS chars, text-aggregation) flows through the pipeline and is consumed by any observer (Prometheus, Sentry, OTel, latency-breakdown) without bespoke wiring (`processors/metrics/frame_processor_metrics.py`, `metrics/metrics.py`).
- Built-ins: `TurnTrackingObserver`, `UserBotLatencyObserver` (per-cycle latency breakdown — a near-exact analog of WaaV's `LatencyProfiler`), `StartupTimingObserver`.

**WaaV verdict:** WaaV already has `VoiceObserver`/`ObserverRegistry` + `LatencyProfiler` + Prometheus `waav_turn_*` (`gateway/src/core/observability/`) — architecturally the same idea. Two techniques to steal:
- **STEAL (medium):** the **per-observer queue+task fan-out** so any slow sink stays off the hot path — verify WaaV's `ObserverRegistry` dispatch is similarly isolated/bounded (it currently appears synchronous).
- **STEAL (low):** richer **metrics taxonomy** — `TextAggregationMetricsData` (time first-token→first-sentence, directly relevant to WaaV's sentence-cap tuning), `TTSUsageMetricsData` (chars/utterance for billing), cache/reasoning token counts. Add to `LatencyProfiler`.

### 6.5 Reconnect/backoff/storm-control + the event bus
Reconnect engine covered in §5.4 (PARITY). The `bus/` package is a genuine pub/sub bus (system-vs-data priority lanes, Redis/PGMQ transports) for **cross-worker** coordination.

**WaaV verdict — mostly N/A:** WaaV is a single-process linear gateway; the bus, multi-worker runner, and job-RPC layers are **out of scope, not gaps**. Two latent ideas: the **system-vs-data two-tier priority queue** (a cancel/control lane that preempts a backed-up data lane — the principle behind WaaV's per-class WS backpressure), and the bus shape *if* WaaV ever runs sidecar processes (separate turn-detector, TTS fan-out).

---

## 7. Optimisations (goal: "optimisations used")

### 7.1 ⭐ Sentence aggregation — lookahead before the boundary decision
`utils/text/simple_text_aggregator.py:78-121`: accumulates char-by-char; when the just-added char is sentence-ending punctuation it **does not split yet** — it sets `_needs_lookahead=True` and only confirms the boundary when a subsequent **non-whitespace** char arrives (`:99,117`). Rationale (`:84`): disambiguate `"$29."` (incomplete, end of a streamed token) vs `"$29. Next"` (real boundary). The boundary detector `utils/string.py:125 match_endofsentence` runs NLTK `sent_tokenize` for Latin (handles `Mr.`, `3.14`, `e.g.`) with a **multi-script fallback** (`:158-168`) for CJK/Hindi/Arabic/etc. via an UNAMBIGUOUS punctuation set that bypasses NLTK. **Pipecat has no max-length cap** — it can stall on unpunctuated text.

**WaaV status (grounded):** WaaV has **two divergent splitters, neither with lookahead or decimal disambiguation**:
- DAG: `dag/nodes/llm.rs:32 drain_complete_sentences` — terminators `.!?।\n` only (no CJK/Arabic), drains to last terminator.
- Conversation: `core/conversation/mod.rs:316-363` — broader set `.!?\n।。！？…` + `MAX_HOLD_CHARS=160` cap, but boundary checks **the last char only** (`:362`), so when a streamed delta ends in `"3."` or `"Mr."`, `at_boundary=true` → **premature flush** to TTS.

**WaaV verdict — GAP (high) + a standardisation fix:**
1. **Add lookahead** — defer the flush one non-whitespace char after a boundary punct. Kills the `"$29."`/`"3.14"`/`"Mr."` mid-number/abbreviation flushes. Small code, real prosody win.
2. **Add decimal/abbreviation disambiguation** — a curated abbreviation list + "digit-dot-digit" guard (full NLTK is a heavy dep; a Rust port or curated list suffices).
3. **Unify the two splitters into one shared util** (per the standing "standardised API" instruction) — the DAG and conversation paths should not diverge; one `sentence_aggregator` module feeds both, with the full punctuation set + lookahead + WaaV's existing 160-cap (which Pipecat *lacks* and is a genuine WaaV advantage — keep it as the safety valve).

### 7.2 Space-aware concatenation
`utils/string.py:223-303` `concatenate_aggregated_text` + `TextPartForConcatenation`: each text part carries `includes_inter_part_spaces: bool`; runs of "spaces included" concatenate directly, "no spaces" runs get a space inserted, transitions insert a space only if neither boundary char is whitespace. Correctly reassembles STT tokens (no spaces) vs LLM tokens (embedded spaces) without double/missing spaces.

**WaaV verdict — STEAL THIS (medium):** WaaV's context/transcript concatenation likely uses naïve string concat → wrong spacing for some providers. Track a per-segment "includes spaces" flag and join accordingly.

### 7.3 Markdown stripping for TTS
`utils/text/markdown_text_filter.py:23-273`: MD→HTML→strip tags, but **preserves leading/trailing spaces and list markers with sentinel placeholders** (`§`, `§NUM§`) before conversion (critical for word-by-word streaming, `:90,135`), strips repeated-char runs (`"aaaaaa"`), unwraps inline code, removes `**`/`*`, makes links readable (strips `https://`).

**WaaV verdict — GAP (medium):** WaaV has no markdown stripping → spoken asterisks/URLs/backticks. Adopt; the **sentinel-space trick** is the key detail for streaming.

### 7.4 soxr streaming resampler
`audio/resamplers/soxr_stream_resampler.py`: `soxr.ResampleStream` keeps internal filter history across chunks (**no chunk-boundary clicks**, `:36`), **lazy init** on first resample (`:75`), **auto-clears stale state after 0.2s gap** (`CLEAR_STREAM_AFTER_SECS`, `:27,66`) so a new utterance doesn't inherit the previous filter tail. SOXR `VHQ` default; `resampy` deprecated (quality won over speed).

**WaaV verdict — STEAL THIS (high):** per memory WaaV resampling is "ad hoc." Use a soxr binding with **stream/history + lazy init + stale-clear (0.2s) + skip-when-in==out**. Higher quality, no clicks, no per-call init cost. Standardise one resampler used by every audio path (ingress decode + TTS egress).

### 7.5 FrameQueue O(1) uninterruptible counter
`utils/frame_queue.py:16-94`: an `asyncio.Queue` subclass with an O(1) `has_uninterruptible` counter maintained in put/get (`:71`) so interrupt handling decides cancel-vs-drain **without scanning the queue**; `reset()` drains interruptible frames while preserving uninterruptible ones.

**WaaV verdict — STEAL THIS (low-medium):** if WaaV adds the `UninterruptibleFrame` concept (§3.1), maintain a counter so the barge-in path never scans the playback queue.

### 7.6 GatedLLMContextAggregator + producer/consumer (defer/low)
`processors/aggregators/gated_llm_context.py` holds context frames until a notifier fires, keeping only the latest (coalescing bursts) — used to delay inference until a classifier verdict resolves (the voicemail/IVR pattern, §8). `WordCompletionTracker` (`utils/context/word_completion_tracker.py`) tracks exact spoken position via normalized char counts — **DEFER** (only for word-level interruption).

---

## 8. Features Missing in WaaV That Could Add Value (goal: explicit)

These are end-user capabilities, not just techniques.

### 8.1 ⭐ Structured conversation flows — `pipecat-flows` (top feature gap)
The companion package (`research/pipecat-flows/src/pipecat_flows/`): `FlowManager` (`manager.py:77`) drives a state machine of `NodeConfig`s (`types.py:182`) — each node = a conversation state with its own messages, available functions (function-call-driven transitions), pre/post actions (`ActionManager`), and a **`ContextStrategy`** (`types.py:134`): `APPEND` / `RESET` / `RESET_WITH_SUMMARY`. Supports static (predefined) and dynamic (LLM-generated) flows.

**WaaV verdict — GAP (high):** WaaV's DAG is a **data-flow** graph (audio→STT→LLM→TTS); it cannot express "if the user says X, transition to node B with a reset+summarised context." This is the machinery for **guided agents** — IVR menus, form-filling, appointment intake, multi-step troubleshooting. Implement as a conversation-flow layer atop the existing LLM client; `RESET_WITH_SUMMARY` alone is a valuable long-call context optimisation WaaV lacks.

### 8.2 Voicemail detection — parallel-pipeline gate coordination
`extensions/voicemail/voicemail_detector.py`: a parallel pipeline (conversation branch + classification-LLM branch) with three coordinating gates (Classifier/Conversation/TTS) and an event-notifier; **buffers TTS until classification completes** so the bot never speaks before knowing human-vs-machine (`:397`), then releases buffered audio in order (human) or clears it (voicemail).

**WaaV verdict — GAP (high for outbound calling):** the gate-coordination architecture generalises beyond voicemail to any state-driven routing (hold music, transfer, language detection). WaaV's `GatedLLMContextAggregator` analog (§7.6) is the primitive.

### 8.3 IVR navigation + DTMF
`extensions/ivr/ivr_navigator.py`: classifies prompts as IVR-vs-human, generates DTMF (`<dtmf>1</dtmf>`), bidirectional mode-switching with context preservation across switches, IVR-specific VAD timeouts. Plus `processors/aggregators/dtmf_aggregator.py` (collects keypad input, timeout/terminator flush, → `TranscriptionFrame` prefixed `"DTMF: "` for unified LLM context).

**WaaV verdict — GAP (high for outbound/telephony):** directly applicable if WaaV drives outbound calls or terminates SIP/telephony.

### 8.4 Call recording — multi-stream sync
`processors/audio/audio_buffer_processor.py`: buffers user + bot audio with **wall-clock silence injection** on gaps and **buffer synchronization** (pads bot buffer to user position before appending) so mono-mix or stereo (user-left/bot-right) tracks stay time-aligned; emits per-turn and threshold-based audio events.

**WaaV verdict — GAP (medium):** the reference pattern for compliance/QA call recording and dual-track transcription.

### 8.5 Eval harness + LLM-as-judge — testing methodology
`evals/`: YAML scenario runner (`harness.py`, `scenario.py`) drives a running bot over RTVI, asserting per-turn expectations with **latency budgets** (`within_ms`); `EvalJudge` (`judge.py`) runs natural-language assertions ("the response mentions three features") out-of-pipeline, **treating homophones gracefully** ("for"/"four"), cached by criterion+conversation hash; `EvalSuite` runs multiple bots in parallel.

**WaaV verdict — STEAL THIS (methodology, medium):** WaaV's tests are Rust unit/integration. A **YAML scenario runner + LLM-as-judge** would enable conversation-level regression testing, latency-SLO assertions, and post-call quality scoring without golden transcripts.

### 8.6 Idle re-engagement
`turns/user_idle_controller.py`: timer starts on `BotStoppedSpeaking` (only if not mid-turn and no function calls pending), fires `on_user_turn_idle` after a timeout → "are you still there?" re-engagement. Carefully guards interruption/function-call ordering races.

**WaaV verdict — GAP (medium):** WaaV has no idle re-engagement (grep-confirmed). Cheap, improves UX on silent callers. Pairs with the idle-frame liveness probe (§6.1).

### 8.7 Producer/consumer fan-out + async-generator export
`processors/producer_processor.py` + `consumer_processor.py`: filter+transform frames, fan out to multiple **independently-queued** consumers (slow consumer doesn't block the pipeline). `processors/async_generator.py` exposes serialized frames via an async generator (gRPC streaming / external subscribers).

**WaaV verdict — STEAL THIS (low):** useful for non-blocking metrics/transcription/eval fan-out and a future gRPC frame-subscribe endpoint.

---

## 9. Transport & Serializer Layer

### 9.1 LiveKit transport (WaaV uses LiveKit — highest relevance)
`transports/livekit/transport.py`:
- **`@retry` exponential backoff on connect** (`:51`) — ⚠️ WaaV has equivalent backoff machinery (`reconnection.rs`); verify the LiveKit connect path actually uses it. (PARITY-ish.)
- **Per-participant stream cleanup on resubscribe** (`:511-518,524-534`): on mute/unmute re-subscribe, the prior `(stream, task)` is explicitly closed + cancelled before registering the new one. **STEAL (medium)** — prevents resource leaks / frame ghosting; verify WaaV's LiveKit track handling does this.
- **`_audio_source.clear_queue()` on InterruptionFrame** (`:895`) — ⚠️ WaaV **already** does `clear_buffer()` (`livekit/client/audio.rs:192,216`). PARITY.
- **11 event handlers** (`:1043-1076`: `on_first_participant_joined`, `on_audio_track_subscribed`, etc.) — **STEAL (medium)** a richer callback surface enables flow control (e.g. delay TTS greeting until first participant joins) without pipeline hacks.
- **Video-in gating** (`:528`) — N/A (WaaV has no video).

### 9.2 TransportParams configurability surface
`transports/base_transport.py:25-90` — the full knob set: `audio_out_10ms_chunks`, `audio_out_mixer` (single/per-destination), `audio_out_destinations` (multi-cast), `audio_out_auto_silence`, `audio_out_end_silence_secs`, `audio_in_passthrough`, `audio_in_filter` (Krisp), `audio_in_stream_on_start`, + video params.

**WaaV verdict — Learning:** WaaV covers enable/sample-rate/channels/filters. The gaps worth considering: **adjustable output chunk size** (§4.1 barge-in tightness), **per-destination mixing/multicast** (multi-party), **auto-silence on empty queue** (prevents WebRTC underrun clicks).

### 9.3 WebSocket transports
- **Fixed-size audio packetization** (`websocket/fastapi.py:62,512`): buffer outgoing PCM until a full protocol-required packet (e.g. 20ms/640 bytes) can be emitted. **STEAL (medium)** if WaaV ever speaks a strict-frame-size protocol (Genesys/Vonage).
- **Origin validation at init** (`:576`) — **STEAL (low)**, CSRF gate on the ws:// endpoint.

### 9.4 Telephony serializers (relevant if WaaV terminates telephony)
`serializers/` — one small class per carrier behind a 2-method `FrameSerializer` trait. Per-carrier specifics worth noting:

| Carrier | Encoding | Interruption signal | DTMF | Notable |
|---|---|---|---|---|
| Twilio | μ-law/8k | `{"event":"clear"}` | RFC4733 | REST hangup |
| Plivo | μ-law/8k | `{"event":"clearAudio"}` | KeypadEntry | DELETE hangup |
| **Telnyx** | μ-law **or a-law**/8k | `{"event":"clear"}` | KeypadEntry | **dual codec PCMU/PCMA** (`:153`) |
| Exotel | raw PCM/8k | `{"event":"clear"}` | KeypadEntry | no μ-law |
| Vonage | PCM 8/16/24k | `{"action":"clear"}` | JSON digit | flexible rate |
| **Genesys** | μ-law/L16 | **`barge_in` event** (`:468`) | parameters.digit | **stateful handshake** (open/opened/ping/pong/pause/resume), output-variables pass-through |

**WaaV verdict — PARTIAL (telephony-dependent):** the serializer-per-carrier pattern mirrors WaaV's `BaseSTT`/`BaseTTS` trait approach. **STEAL** Telnyx dual-codec PCMU/PCMA (global carriers) and Genesys's stateful handshake + barge-in event (robust call-center integration) — *if/when* WaaV terminates telephony.

---

## 10. Where WaaV Is Already At Parity or Ahead

Do **not** rebuild these:

| Area | Status | Evidence |
|---|---|---|
| Smart-turn v3 | ⚠️ PARITY (identical model) | `core/smart_turn/detector.rs:30-33` = pipecat-ai/smart-turn-v3.2-cpu.onnx |
| LiveKit barge-in flush | ⚠️ PARITY | `livekit/client/audio.rs:192,216` `clear_buffer()` |
| Reconnect backoff + circuit-breaker + storm control | ⚠️ PARITY | `core/resilience/{reconnect_governor,circuit_breaker}.rs`, `core/websocket/reconnection.rs` |
| Silero VAD | PARITY | `core/silero_vad/`, `core/audio/vad.rs` |
| Per-turn latency profiling | PARITY (WaaV instruments call-sites; Pipecat observes frames) | `core/observability/{profiler,turn_profile}.rs` |
| **Bounded backpressure** | **AHEAD** | WaaV has per-class WS queue bounds; **Pipecat's queues are unbounded** |
| **Sentence 160-char safety cap** | **AHEAD** | `core/conversation/mod.rs:316`; Pipecat has no max cap (can stall on unpunctuated text) |
| **Per-session panic isolation** | **AHEAD** | `catch_unwind`; Pipecat bots are fire-and-forget asyncio tasks |
| **SIGTERM graceful drain** | **AHEAD** | WaaV has it; Pipecat's runner defaults SIGTERM **OFF** (`runner.py:111`) |
| Zero-copy audio buffers | AHEAD (native) | Rust slices vs numpy views |
| Deepgram Aura WS TTS | PARITY (just built) | `core/tts/deepgram_aura.rs` ≡ `deepgram/tts.py` |

---

## 11. Out of Scope / N/A for WaaV

- **Event bus / multi-worker runner / job-RPC** (`bus/`, `workers/`, `registry/`) — WaaV is single-process linear; adopting a bus would be over-engineering (revisit only for sidecar/multi-process).
- **GStreamer media encoding** (`processors/gstreamer/`) — WaaV delegates media to SFUs.
- **LangChain / Strands framework processors** — Python/framework-specific.
- **WebRTC avatar transports** (tavus/simli/heygen) — no video in WaaV.
- **Full NLTK dependency** — too heavy for Rust; use a curated abbreviation list + lookahead instead (§7.1).
- **`WordCompletionTracker`** — defer until WaaV does word-level interruption.

---

## 12. Consolidated Adoption Roadmap

Priority key: **P0** = highest leverage / on critical path; **P3** = nice-to-have. All recommendations honour the standing instruction — implement as a **standardised API across all relevant providers**, not per-provider.

### P0 — Strategic systems (highest leverage)

| Item | WaaV target | Pipecat ref | Effort | Benefit |
|---|---|---|---|---|
| **Pluggable turn-strategy abstraction** (start/stop/mute traits; ship MinWords + dual-timer SpeechTimeout first) | new `core/turn_strategy/`; refactor `stt_result.rs` + `turn_decision/engine.rs` to consume it | `turns/user_{start,stop,mute}/*` | High | Fewer false barge-ins; STT-p99 safety net removes truncated-last-word bug; houses smart-turn + eager-EoT as strategies |
| **Universal LLM context + per-vendor adapter** (`LlmAdapter` trait: OpenAI/Anthropic/Gemini) | `core/llm/` | `adapters/base_llm_adapter.py` + `adapters/services/*` | High | Native Anthropic/Gemini (not OpenAI-compat shims); system-placement + tool-result shape handled per vendor |
| **Sentence-aggregation lookahead + unify the two splitters** (one shared util, +decimal/abbrev disambig, keep 160-cap) | new `core/text/sentence.rs`; feed both `dag/nodes/llm.rs` + `core/conversation/mod.rs` | `utils/text/simple_text_aggregator.py:78`, `utils/string.py:125` | Low | Stops premature `"$29."`/`"3.14"`/`"Mr."` TTS flushes; removes DAG/conversation divergence |

### P1 — High-value systems & features

| Item | WaaV target | Pipecat ref | Effort | Benefit |
|---|---|---|---|---|
| **S2S as a service** (realtime LLM emits normal TTS-audio callbacks; `emits_user_turn_frames` flag; truncate+cancel+preroll barge-in; session resumption) | `core/realtime/` → `BaseLLM`-shaped / DAG S2S node | `services/openai/realtime/llm.py`, `services/google/gemini_live/llm.py` | High | Realtime drop-in for cascade; matches recorded roadmap |
| **Structured conversation flows** (FlowManager/NodeConfig/ContextStrategy) | new conversation-flow layer atop `core/llm` | `research/pipecat-flows/src/pipecat_flows/*` | High | Guided agents (IVR/forms/intake); `RESET_WITH_SUMMARY` long-call context optimisation |
| **soxr streaming resampler** (filter-history + lazy init + stale-clear + skip-when-equal) | standardise one resampler across ingress + TTS egress | `audio/resamplers/soxr_stream_resampler.py` | Medium | Higher quality, no boundary clicks, no per-call init |
| **VAD-aware deferred reconnect + audio replay** | `core/resilience/` / `ReconnectableStream` | `services/stt_service.py:605,643` | Medium | No dropped words on mid-utterance STT reconnect |
| **Voicemail / call-state detection** (gate-coordination) | new `core/extensions/` | `extensions/voicemail/voicemail_detector.py` | Medium | Outbound-calling capability; generalises to routing |

### P2 — Hardening & optimisation

| Item | WaaV target | Pipecat ref | Effort | Benefit |
|---|---|---|---|---|
| **Fatal/non-fatal error tier** (degrade one stage vs drop call) | `core/errors`, DAG executor | `frames/frames.py:905`, `worker.py:1145` | Medium | Transient provider error doesn't kill the call |
| **Bounded forced-shutdown + dangling-task audit + swallowed-cancel timeout on FFI** | `main.rs` drain, ONNX/codec await sites | `worker.py:961`, `frame_processor.py:928` | Medium | Shutdown never hangs; leak detection; ORT cancel safety |
| **Observer fan-out isolation** (per-observer queue+task) | `core/observability/observer.rs` | `pipeline/worker_observer.py:153` | Medium | Slow metrics sink can't add hot-path latency |
| **`UninterruptibleFrame` concept** (utterance survives barge-in) + TTS `on_audio_context_interrupted/completed` seam | `core/tts/base.rs`, playback unit | `frames.py:141`, `tts_service.py:1551` | Medium | Protect tool-result/disclaimer audio; uniform cancel/close across 36 TTS |
| **Mid-call settings delta + `extra` overflow** | `BaseSTT`/`BaseTTS` config | `services/settings.py:57` | Medium | Change voice/model/language live, no teardown |
| **TTS word-timing playback queue** | `core/tts/` | `tts_service.py:1197` | Medium | Correct "spoken-prefix" context on interruption |
| **Space-aware concatenation** | transcript/context join | `utils/string.py:223` | Low | No double/missing spaces across providers |
| **Markdown stripping for TTS** (sentinel-space trick) | TTS pre-filter | `utils/text/markdown_text_filter.py` | Low | No spoken asterisks/URLs/backticks |
| **VAD dual-gate + 5s reset + audio-idle forced-stop** | `core/silero_vad/`, `core/audio/vad.rs` | `audio/vad/vad_analyzer.py:206`, `vad_controller.py:194` | Low | Fewer false triggers; no hung turn on muted stream |
| **Pipeline-traversal heartbeat (liveness probe)** | `core/observability/` | `pipeline/worker.py:1205` | Low | Detect wedged path when no turn is active |
| **Quick-failure breaker special-case** (<5s-stable × 3 = fatal) | `core/resilience/circuit_breaker.rs` | `websocket_service.py:142` | Low | Stop backoff-looping on permanently-bad credential |
| **STTMetadata ttfs_p99 broadcast** | STT → turn strategy + profiler | `stt_service.py:484` | Low | Feeds dual-timer stop strategy |
| **Richer metrics taxonomy** (text-agg latency, TTS chars, cache/reasoning tokens) | `LatencyProfiler` | `metrics/metrics.py` | Low | Sentence-cap tuning, billing, cost visibility |

### P3 — Features / methodology (as needed)

| Item | Pipecat ref | Benefit |
|---|---|---|
| Eval harness + LLM-as-judge (YAML scenarios, latency SLOs) | `evals/harness.py`, `evals/judge.py` | Conversation-level regression + post-call QA |
| IVR navigation + DTMF aggregator | `extensions/ivr/`, `aggregators/dtmf_aggregator.py` | Outbound/telephony automation |
| Idle re-engagement ("are you still there?") | `turns/user_idle_controller.py` | UX on silent callers |
| Call recording (multi-stream sync) | `processors/audio/audio_buffer_processor.py` | Compliance/QA recording |
| Producer/consumer fan-out + async-generator export | `processors/{producer,consumer,async_generator}.py` | Non-blocking metrics/transcription; gRPC frame subscribe |
| Warm-standby failover | `ServiceSwitcher` | Near-zero provider failover vs reconnect |
| Telephony serializers (Telnyx dual-codec, Genesys handshake) | `serializers/*` | If terminating telephony |
| LiveKit per-participant stream cleanup + richer event callbacks | `transports/livekit/transport.py:511,1043` | Leak prevention; flow control |

---

## 13. Closing Assessment

WaaV is **not behind Pipecat on fundamentals** — on several production-hardening axes (bounded backpressure, panic isolation, SIGTERM drain, sentence safety-cap) it is *ahead*, and on the headline realtime primitive (smart-turn v3) it is at exact parity using the identical model. The value Pipecat offers WaaV is **factored abstractions and breadth of features**, not core competence:

1. **Abstractions** that WaaV currently hardcodes — pluggable turn strategies, a universal LLM context adapter, S2S-as-a-service. These are the P0/P1 systems; they unlock capability (Anthropic/Gemini native, composable barge-in policy, drop-in realtime) WaaV would otherwise build ad-hoc.
2. **Optimisation refinements** — sentence-aggregation lookahead, soxr streaming resampler, space-aware concatenation, VAD-aware reconnect — small, high-ROI, and each implementable as a standardised primitive across all providers.
3. **Features** WaaV simply lacks — structured conversation flows, voicemail/IVR, call recording, eval harness, idle re-engagement — that expand WaaV's addressable use-cases (especially outbound/telephony and guided agents).

The recommended sequence: land the three **P0** systems (turn strategies → universal LLM adapter → sentence lookahead/unification) first, as they sit on the conversational critical path and align with WaaV's existing realtime and standardisation goals; then the **P1** systems as roadmap items; absorb **P2** hardening/optimisations opportunistically alongside related work; pull **P3** features as product demand dictates.

*All Pipecat paths cited relative to `research/pipecat/src/pipecat/` (or `research/pipecat-flows/`); all WaaV paths relative to `gateway/src/`. Research clones retained under `/home/bud/ditto/research/` for follow-up reference.*
