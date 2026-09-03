# WaaV Master Fix & Realtime Plan — Extreme-TDD, Live-Gated

**Date:** 2026-06-11 · **Inputs:** `AUDIT_REPORT.md` (full issue register), `LATENCY_ANALYSIS.md` (measured budgets), `REALTIME_ROADMAP.md` (SOTA targets), three deep code-grounded integration analyses (streaming-TTS abstraction; speech-final concurrency RCA; config/chaos/hardware), and web research (Deepgram WS TTS protocol, ORT execution providers, pipecat/livekit abstraction patterns, audio sniffing, voice-AI chaos patterns).
**Mandate:** fix all issues, close all gaps → fully functional · standardized (one API per provider) · super-realtime · high accuracy · end-to-end working · hardware-optimized-but-portable · chaos-proof production-ready. Every change lands extreme-TDD with a live gate.

**Where we verifiably stand (measured, live):** the full multi-provider loop works end-to-end through the production binary; gateway glue is ~10–12 ms of a 7.5 s turn; the integrated profiler attributes the rest (llm_ttft 5905 ms, tts_ttfb 1509 ms). The plan's job is to make the *system* match the gateway: realtime, correct, and unkillable.

---

## 1. Root-cause taxonomy — eight classes explain every finding

Every fix below cites its class; fixing the class (not the instance) is what prevents recurrence.

| RC | Root cause | Instances (from the audit) |
|---|---|---|
| **RC1** | **Trust-by-declaration** — data trusted from config/declaration, never inspected at the boundary | S3 codec trio (chunker frames by *declared* format); L2 placeholder keys (present ⇒ assumed real); empty-model URLs; silently-ignored unknown WS fields |
| **RC2** | **Two sources of truth, no precedence contract** | YAML-beats-env in `merge.rs` macros; client vs server keys; flat vs standard config; `is_final`/`is_speech_final` per-provider divergence |
| **RC3** | **Wall-clock time in timing logic** | speech-final deadlines, duplicate window, interruption windows on `SystemTime` (`unwrap_or_default` → epoch-0 on skew) |
| **RC4** | **Check-then-act across concurrency boundaries** | duplicate-window TOCTOU; double hard-timeout; barge-in double-check; subscribe/guard ordering |
| **RC5** | **Silent degradation** | smart-turn init failure → quiet 2.5 s timers; `let _ = send` drops; server-only error detail; typo'd config fields ignored; stub features in default builds with no signal |
| **RC6** | **Lifecycle gaps** | callbacks registered *after* connect; no SIGTERM drain; double-config no teardown; forwarder task leaks; LiveKit queue no shutdown handshake |
| **RC7** | **Transport monoculture** | TTS is HTTP-only (36/37) though WS streaming APIs exist (Deepgram Aura WS confirmed); OpenAI realtime STT unwired |
| **RC8** | **Unbounded resources** | metric label cardinality; turn text_buffer; un-throttled `/metrics`; bounded channels "handled" by silent drops |

**Class-level policies adopted by this plan:** (RC1) *sniff at every media/config boundary*; (RC2) *one documented precedence: explicit-client > env > file > default, placeholders are never credentials*; (RC3) *monotonic `now_monotonic_ns()` for all arithmetic, wall-clock for display only*; (RC4) *single-owner tasks for multi-writer state machines*; (RC5) *degradation must emit: log + metric + (where a client exists) protocol message*; (RC6) *register-before-connect, teardown-before-replace, drain-before-exit*; (RC7) *capability-flagged transports behind the same provider API*; (RC8) *every queue/buffer/label set has a bound and a drop policy*.

---

## 2. Methodology — extreme TDD with live gates (applies to every work item)

1. **RED:** failing unit/property tests first — including *deterministic interleaving tests* (`tokio::time::pause()`/`advance`) for anything concurrent, and wire-mock e2e (the `tests/mock_endpoint_e2e.rs` + `tests/mock_providers/websocket_mock.rs` patterns; the TTS-WS mock already exists at `websocket_mock.rs:182-258`).
2. **GREEN:** minimal implementation.
3. **LIVE GATE:** env-keyed real-provider run (Deepgram STT/TTS-WS live key works; Sarvam LLM), asserted **through the integrated profiler** — `/debug/profile` stage numbers and `waav_turn_*` series are the oracle, not eyeballs.
4. **CHAOS GATE:** existing `chaos_reconnect`/`chaos_storm` plus the new chaos suite (§7): provider-kill mid-turn, slow-client, SIGTERM-mid-turn, clock-skew, breaker-trip storm.
5. **REGRESSION FLOOR:** full lib suite (5.7k+ tests) + `conversation_loop` + `latency_harness` with an asserted overhead budget: **stt_final→audio_out glue p99 ≤ 15 ms** (currently 13 ms) — any phase that regresses this fails its gate.

---

## 3. P0 — Correctness (nothing above matters if these fire)

### P0.1 Codec truth at the chunker boundary (S3 trio; RC1) — CRITICAL
**RCA:** `provider.rs` chunker frames bytes by the *declared* `audio_format`; Speechify (`wav_48000` hardcoded), UnrealSpeech/WellSaid (MP3) return containers that get sliced as PCM → clicks/noise.
**Design (researched, critique-amended):** a zero-dep ~30-line sniffer (`RIFF….WAVE` → wav; `ID3`/`0xFFEx` framesync → mp3; `OggS` → ogg; else trust declaration) applied at **all three byte-ingestion points**: (1) HTTP response → chunker, (2) **cache-store** (never persist a mismatched blob), and (3) **cache-hit** (12-byte check; on mismatch evict the entry and fall through to HTTP — heals caches poisoned before this fix). Policy: sniffed ≠ declared and declared is PCM-family → *do not chunk as PCM*; deliver one labeled blob with the **sniffed** format + `error!` + `waav_tts_format_mismatch_total{provider,source=http|cache}`. (b) fix the three providers' configs to reconcile `audio_format` with what they actually request; (c) optional decode-to-PCM via `symphonia` later (off by default). No consumer API changes — `AudioData.format` becomes truthful.
**Blast radius:** chunker only + 3 provider configs; all other providers sniff-clean.
**TDD:** RED: feed WAV/MP3/OGG bytes with `linear16` declared → assert no PCM-chunking, format corrected, counter incremented; per-provider mock e2e returns real container bytes. LIVE: Speechify/UnrealSpeech/WellSaid keys absent — gate on wire-mocks (registered as the audit's credential-free pattern); Deepgram linear16 live run must stay sniff-clean.

### P0.2 Credential precedence + placeholder rejection (L2; RC1+RC2) — HIGH
**RCA (exact):** `src/config/merge.rs:25-45` macros do `$yaml_value.or_else(|| env::var($env_var).ok())` — **YAML beats env**, opposite of the `# ENV:` comments; `get_api_key` (`config/mod.rs:493-601`) returns whatever is stored, so the shipped placeholder `"your-deepgram-api-key"` is sent to Deepgram.
**Design (critique-amended — no precedence flip):** the adversarial review found `test_from_file_yaml_overrides_env` (`config/mod.rs:1943-1974`) *encodes* YAML-over-env as intended behavior, so a blanket flip is a silent breaking change. The elegant fix: **filter placeholders at merge time** — a value matching `^your[-_]`/`changeme`/`x{3,}`/empty is treated as **`None` before the `or_else` chain runs**, so the *existing* `yaml.or_else(env)` macro falls through to the env var. Placeholder-shipped configs get env keys (the live-found bug fixed); real YAML values keep today's precedence (zero test breakage). Plus: (b) `get_api_key` guard — a surviving placeholder is "not configured" (clear error, never sent to a provider); (c) startup `warn!` per placeholder field. The full 12-factor precedence flip is logged as an explicit v2-config decision (requires the test + release note), not smuggled in here.
**Blast radius:** all ~20 key fields; only placeholder values change behavior.
**TDD:** RED: `yaml_placeholder_falls_through_to_env`, `placeholder_alone_is_not_configured`, `yaml_real_value_still_overrides_env` (pins the existing contract); the `test_from_file_*`/`test_from_env_*` suites are the floor. LIVE: boot with **stock** `config.yaml` + env keys → live e2e passes without client-supplied keys.

### P0.3 Speech-final state machine: single-owner actor (C4+H5+HIGH-8; RC4) — HIGH
**RCA (one sentence each, verified):** C4 — duplicate check at `stt_result.rs:189-207` is read-outside-lock TOCTOU, two final paths can both pass before either writes; H5 — an already-woken hard-timeout that read `waiting_for_speech_final` before the new final's `Release` store still force-fires with different buffered text (`stt_result.rs:170-172/218-274`); HIGH-8 — interruption checked independently in `manager.rs:765` and the conversation layer across an await.
**Design (twice-revised — the adversarial review killed the v1 actor):** v1 proposed a single-owner actor task; the critique showed it **changes who fires the user callback** — finals would fire from the actor task while interims fire from the STT path, breaking interim/final ordering and the synchronous `process_result → caller-fires-callback` contract that `config_handler`'s transcript forwarding depends on. **v2 design — narrow critical section + generation counter, callbacks stay with the initiating task:**
- A single `parking_lot::Mutex<FireState>` (dedup timestamp, generation, segment buffer handle, timer deadlines) whose critical section contains **no awaits and no inference** — microseconds, taken only by final-ish events (~a few per turn), never by the per-frame audio path.
- Every fire path (provider-final, turn-detect confirm, hard-timeout) executes: lock → re-check {dedup window, generation, waiting flag} → claim (bump generation, take buffer, clear deadline) → unlock → fire the callback **from its own task** (contract preserved). Turn-detect inference runs *outside* the lock; on completion it re-locks and re-checks generation (ABA-safe) before claiming.
- **Hard-timeout lifecycle:** one **persistent monitor task** per session that sleeps toward a deadline atomic and **re-reads it on wake** — a moved/cleared deadline means no fire; kills the orphan-timer class without spawn/abort churn.
- Interruption: the claim inside the lock is the single authoritative can-fire decision (closes HIGH-8's double-check window for turn-firing; conversation barge-in keeps its idempotent clear).
Rejected: actor (callback-ordering breakage, above); bare CAS counter (multi-field state is not point-wise atomic).
**Blast radius:** `stt_result.rs` (~250-line refactor — smaller than v1), `state.rs` fields; `manager.rs` wrapper unchanged in shape; user-visible callback contract **bit-identical**. Profiler anchors unchanged.
**TDD:** RED first: `racing_provider_final_vs_hard_timeout`, `racing_turn_detect_vs_hard_timeout`, `duplicate_window_atomicity` (loom-style interleavings via paused time + yield points), `hard_timeout_rearm_no_double_fire`, `barge_in_toctou`; the four existing hard-timeout tests (`stt_result.rs:512-767`) are the floor. LIVE: the committed live e2e asserts **exactly one** `waav_turns_total{outcome="completed"}` increment per utterance across 10 runs.

### P0.4 Monotonic time everywhere timing decides (C5; RC3) — HIGH
**Design:** migrate `segment_start_ms` / `hard_timeout_deadline_ms` / `turn_detection_last_fired_ms` / `non_interruptible_until_ms` to `u64` **monotonic ns** via the existing `core::observability::now_monotonic_ns()` (`latency.rs:38` — process-relative, cross-task-safe, already battle-tested by the profiler); sleeps become `sleep(Duration::from_nanos(deadline.saturating_sub(now)))`. Wall-clock remains only in logs/protocol timestamps. Folded into the P0.3 rewrite (same files) so the migration is one reviewed change.
**TDD:** unit: deadline math under simulated backward jumps (inject a `Clock` trait in tests OR assert no `SystemTime` remains in the decision path via a grep-lint test); paused-time determinism tests double as skew immunity.

### P0.5 Error truth to the caller (L3 + H6; RC5+RC6)
**Design (critique-amended — the buffer is real work, not a code move):** (a) reorder `config_handler.rs:581-617`: register STT/TTS **error** callbacks *before* `voice_manager.start()`; **plus** a `last_error: Mutex<Option<STTError>>` buffer in the provider base written by any pre-registration failure and **drained on callback registration** — covers both orderings forever (the critique verified neither buffer nor drain exists today; both are scoped here with tests). Client then receives `401 Unauthorized` instead of "Connection channel closed". (b) Replace the two `serde_json::to_string().unwrap()`s in `llm/mod.rs:1115/1124` with error-mapped returns. (c) Adopt the RC5 policy: every degradation path (smart-turn init failure, VAD stub build, format mismatch, dropped frames) emits log + metric (`waav_degraded_total{component,reason}`) — `/readyz` already reports per-provider state; extend its JSON with `features: {vad: real|stub|failed, …}`.
**TDD:** mock provider whose connect 401s → client receives the auth message (wire e2e); unwrap-removal covered by serialization-failure unit tests. LIVE: invalid key run asserts the client-visible error class.

---

## 4. P1 — Super-realtime (the measured 7.5 s → sub-second path)

### P1.1 Streaming TTS: `WebSocketTtsProvider` + Deepgram Aura WS first (RC7) — THE lever
**Why first:** profiler-measured tts_ttfb 1509 ms is pure batch-HTTP cost; Deepgram's WS (`wss://api.deepgram.com/v1/speak`, `Speak`/`Flush`/`Clear` text frames in, binary audio + `Flushed{sequence_id}`/`Cleared` events out) maps **1:1 onto the existing `BaseTTS` contract** (`speak(flush=false)` buffer → `Flush` on boundary → `clear()` ↔ `Clear`), and **we hold a working key** → live-gateable today.
**Architecture (from the integration analysis, critique-amended):**
- **No breaking trait change:** the critique counted **38 `impl BaseTTS` sites and ~135 `.speak(` call sites** (not "~6"), so the context id lands as a defaulted method — `async fn speak_with_context(&mut self, text, flush, context_id: Option<&str>) { self.speak(text, flush).await }` — zero edits to existing providers/callers; the WS provider overrides it; `VoiceManager::speak` calls the new method.
- **Lock-safety invariant (critique-found deadlock risk):** `VoiceManager` holds `tts: Arc<RwLock<Box<dyn BaseTTS>>>` and takes a **write** lock per `speak()` — therefore the WS provider's recv task must hold **no outer lock**: it is spawned detached at `connect()`, owns the socket read-half outright, and communicates via channels + a small internal map mutex; `speak_with_context()` only channel-sends. A bench asserts `speak()` ≤ 5 ms with an active recv task.
- New `core/tts/websocket.rs`: generic `WebSocketTtsProvider` — persistent socket, per-utterance `PendingUtterance{cancel_token, sender, request_ts_ns}` map, internal `flush=false` text buffer, TTFB stamped on first binary frame (feeds the existing `notify_tts_chunk(_, Some(ttfb))`), **reuses** the breaker/governor/ReconnectionManager (reconnect logic 100% reusable; the STT supervision loop is not — TTS has its own send/recv halves), and **reuses the existing dispatcher** for ordered delivery.
- Selection: `TtsFeatures.streaming: Some(true)` in the **standard** config (the one-API path) — `create_tts_standard("deepgram", …)` dispatches Aura-WS vs HTTP; absent/unsupported → HTTP fallback with a `warn!` (RC5). No parallel provider names.
- Cache: skipped when streaming (per-session voice state; correctness over hit-rate).
- **SSRF (critique-found gap):** any client-supplied `endpoint_override` for the WS TTS URL passes the same validator the DAG uses (`validate_url_for_ssrf`), loopback only under `WAAV_ALLOW_LOOPBACK_ENDPOINTS` — a client must not be able to point the gateway's socket at internal services.
- **Consumer impact: none** — interruption-window math already accrues per `on_audio` chunk; conversation pump unchanged (sentence-`flush=true` is right for WS prosody); barge-in `clear_tts()` now actually cancels in-flight synthesis via the context map.
- Phase 2 of this item: pump sends sub-sentence deltas with `flush=false` + boundary `flush=true` (restores the full ~276 ms overlap with clean prosody); Cartesia/ElevenLabs WS variants when keys exist (same base class, ~200 LOC each).
**Effort (revised up per critique):** ~1.5–2 k LOC incl. mock + tests, ~5 days. **Blast radius:** one defaulted trait method, `standard.rs` dispatch, zero handler/DAG changes (callback contract stable).
**TDD:** the agent-specified 8-file suite — unit (context/cancel/buffer), wire-mock e2e (**TTFB < 300 ms asserted against the mock**, ordering, cancellation-mid-stream, aggregation), chaos (socket drop mid-synthesis → reconnect; breaker trip), VoiceManager integration (interruption windows from chunks). **LIVE GATE:** real Aura-WS run through the full gateway; `/debug/profile` must show **tts_ttfb p50 < 300 ms** (vs 1509 measured) and headline drop accordingly.

### P1.2 Turn detection: Smart Turn v3 + surfaced config + tuned defaults (RC5; roadmap P1)
**Facts:** v3.0 ONNX is public (8.7 MB, verified fetchable; v3.1/3.2 variants are gated) — 12 ms-class CPU inference vs our measured 54 ms v2-era cost, and it ingests raw audio (drops the 26 ms MEL stage). The audit found smart-turn runs on the live frame path but is **un-surfaceable** from the WS config and **silently absent** when init fails, while timer defaults (`1800/2500/500 ms`) cost 1.4–2 s per turn on the fallback path.
**Design:** (a) model upgrade behind the existing `SmartTurnDetectorConfig` (new model path + pinned SHA-256 + the same input contract — keep v2 fallback flag); (b) add `turn_detection` block to `STTWebSocketConfig` (enable + threshold + eager flag) flowing into `VoiceManagerConfig` (the field exists; the parsing doesn't); (c) RC5 treatment for init failure: log + `waav_degraded_total{component="smart_turn"}` + `ready` protocol message gains `"turn_detection": "ml"|"timer"`; (d) defaults tuned to measured reality: `stt_speech_final_wait_ms` 1800→**600**, `hard` 2500→**1500**, inference timeout 500→**100** (54 ms measured; 12 ms after upgrade) — config-overridable, release-noted; (e) **eager end-of-turn** (Flux-style, roadmap-validated): on smart-turn `probability ≥ eager_threshold` *before* provider endpointing, open the LLM turn speculatively; provider-final confirms (commit), new user speech cancels via the existing turn `CancellationToken`. **Critique-found BLOCKER, fixed here: history staging.** `LlmClient::prepare_messages` (`llm/mod.rs:648-660`) appends the user message at *request* time, so a cancelled speculative turn would orphan a partial-utterance user message in history (the next, fuller utterance then duplicates it → context corruption/hallucinations). Fix: speculative turns build `history + input` **without mutating** stored history; user+assistant messages are committed **together on confirmation** (`record_user_and_assistant` at the existing `record_assistant` point). Normal turns keep today's semantics. Test: `speculative_cancelled_leaves_history_untouched`, `speculative_confirmed_commits_once`. Eager is opt-in (`eager: true`) — it raises LLM call volume 50–70% (researched Flux trade-off).
**TDD:** v3 accuracy gate on the labeled set (the `real_dataset_accuracy` harness exists — `acc > 0.65` floor, expect ~0.85+); inference-latency bench (target ≤ 15 ms p99 on this host, `turn_detect_latency.rs` extended); eager-EoT unit tests (speculative-start→cancel-on-resume; speculative-start→commit) with the paused-clock harness; **LIVE:** profiler `stt` stage (EOS→turn-open) p50 target **< 300 ms** (from ~760 ms measured), zero duplicate turns across the 10-run gate.

### P1.3 DAG path parity (profiler Phase 4)
Open/close `TurnTrace` around `register_dag_stream_driver`'s speech-final gate → `execute_streaming_from` completion; map `ctx.timing.node_durations` → `node_durations_us` → the (label-clamped) `waav_dag_node_ms`. Small (~150 LOC), the design was already specified in the original profiler plan; streaming-path ratio then becomes a real signal (`streaming_path_used_ratio` currently 0.0).
**TDD:** mock-node DAG turn → trace with node breakdown; LIVE: the multivendor DAG e2e (`--ignored`, keys) asserts `waav_turn_response_latency_ms{path="dag"}` populated.

---

## 5. P2 — Chaos-proof lifecycle (the "doesn't crash in any real-world usage" layer)

| Item | Design (agent-verified locations) | Gate |
|---|---|---|
| **SIGTERM drain** (RC6) | `CancellationToken` on `AppState`; `main.rs` cancels before axum's 30 s `graceful_shutdown`; the WS session loop (`handler.rs:150-340`) selects on it: stop intake → flush in-flight TTS → close providers → goodbye frame | chaos: SIGTERM mid-turn ⇒ in-flight audio reaches the client, exit 0, no orphan tasks (`tokio::runtime` metrics assert) |
| **Per-class send policy** (RC8) | `send_with_policy(msg_class)`: audio = `try_send` (drop, count `waav_ws_dropped_frames_total`), transcripts = 500 ms timeout, errors/control = await; replaces bare `.send()` on `message_tx` (~15 sites) | chaos: slow-client test (reader sleeps 5 s) ⇒ session survives, transcripts+errors delivered, drop counter > 0, **no stall of the STT path** |
| **Double-config** (RC6) | reject with protocol error once `config_received` (state flag); replace-with-teardown deferred (reconfigure-in-place is a feature, not a bug fix) | unit + wire e2e: second config → error message, original session intact |
| **LiveKit op-queue** (RC6/RC8) | enqueue `Shutdown` sentinel on drain; worker exports `observe_queue_depth/latency_ms("livekit_op", …)` every N ops (the profiler API already exists and is wired to `/metrics`) | unit: drain completes; metrics visible in snapshot `realtime_blockers.lk_queue_depth_max` |
| **Reconnect stragglers** | migrate the 9 legacy-manager STT providers (assemblyai first — it's tier-1) onto `ReconnectableStream`; batch providers documented N/A | existing chaos_reconnect matrix extended per provider (wire-mock kill/restore) |
| **TTS pooling** | inject `ReqManager` into the 24 non-pooled TTS providers via the existing `set_req_manager` hook | bench: first-call TTFB delta on wire-mock; no behavior change otherwise |
| **Streaming-forwarder leak + router determinism** | abort children on forwarder error (`executor.rs:235-262`); sort router patterns longest-first (`compiler.rs:614-618`) | unit: leak test via task-count; routing property test |
| **`/metrics` guard** (RC8) | cache rendered exposition ~1 s + optional bearer (`WAAV_METRICS_TOKEN`) | unit + load test |

**Protocol hardening (L4/L5; RC1/RC5):** **warn-on-unknown, not deny** — `deny_unknown_fields` breaks old-server/new-client forward compat (researched trade-off); a `serde_json::Value` pre-pass diffs received keys vs schema and returns a `warning` protocol message listing unknown fields (the `conversation` typo becomes instantly visible, nothing breaks). Unify `model` defaulting (STT-style `#[serde(default)]` + per-provider mapping — the Deepgram empty-model fix generalizes to a fleet conformance test).

---

## 6. P3 — Standardization & accuracy (one API, every provider, trustworthy semantics)

1. **One API:** `StandardSTTConfig`/`StandardTTSConfig` + `create_*_standard` is **the** documented contract (it already carries features/extras/endpoint_override and is what the live WS path uses). Flat `create_*_provider` becomes a deprecated shim (`#[deprecated]`, mapped through `from_base`) — no removal (663 call sites), no behavioral change, but every new feature lands only on the standard path. The WS config already maps 1:1; SDKs follow.
2. **Turn-taking semantics contract (accuracy-critical):** document the canonical meaning — `is_final` = transcript immutable; `is_speech_final` = end-of-utterance signal — then a **fleet conformance test**: every streaming STT mock e2e asserts the mapping (catches ElevenLabs's hardcoded `is_final=false` interims, Google's missing `SpeechActivityEnd` gating, and future drift). Fix the two known offenders.
3. **Channel-truth policy (RC8):** finals/errors must never silently drop — bounded-channel sends on result/error paths get warn+counter (`waav_stt_dropped_results_total`); Deepgram done, AssemblyAI done, sweep the remaining fleet.
4. **Provider long-tail** (each small, each TDD'd against its wire-mock): ElevenLabs realtime default model + WS variant; Cartesia speed-schema versioning + language passthrough; IBM `instance_id` via extras; Hume description clamp at `from_standard`; OpenAI realtime STT WS (behind `features.streaming`, same pattern as P1.1).
5. **Accuracy levers surfaced:** keyterms/diarization/redaction already reachable via the standard path (W1) — add per-provider conformance tests that the features actually serialize onto the wire (the audit's "stranded features" class, now test-enforced); noise-filter exposed for WS sessions (currently LiveKit-only) behind a config flag.

---

## 7. Hardware optimization with portability (H)

**Design (grounded in ort 2.0.0-rc.10's API — EP modules for cuda/tensorrt/coreml/xnnpack/directml/openvino… with `is_available()` probing):**
- `WAAV_ORT_EP=auto|cpu|cuda|tensorrt|coreml|directml|xnnpack` (default `auto` = probe in platform order, **CPU always the final fallback** — ORT falls back per-op anyway; we make it explicit, logged, and metric'd: `waav_ort_ep{ep}` gauge).
- One policy point: a small `core/onnx/ep.rs` helper used by all three model loaders (turn_detect `model_manager.rs:54-69`, smart_turn, silero) — today each builds sessions independently with no EP registration.
- Per-model thread tuning stays as-is for tiny models (sequential, 1 intra-op — the Smart Turn v3 recommendation); big-model knobs (TensorRT workspace, FP16) exposed via extras, never required.
- Model variants: ship hash-pinned entries for the int8 CPU model (v3.0 now; the gated v3.1/3.2 cpu/gpu variants slot in when accessible). Feature-gated builds (S2) keep `default = []` but the stub state becomes **loud** (readyz `features` block + startup warn) per RC5.
- Portability invariant: **no EP-specific code outside `ep.rs`**; CI gate runs the model suite with `WAAV_ORT_EP=cpu` (always) and auto (opportunistic).
**TDD:** unit — EP request honored/probed/fallback-logged (mock availability); bench — `turn_detect_latency.rs` records per-EP numbers into the doc; LIVE — this host (aarch64 + CUDA-capable) runs cpu vs auto and asserts ≤ baseline.

---

## 8. Sequencing, dependencies, effort

```
(S ≤1d, M 2-4d, L ~1wk)                       — critique-revised order: concurrency
1. P0.2 keys (S) ─┐                              correctness lands BEFORE new concurrency
2. P0.5 errors(M)─┼─► live-e2e-without-client-keys gate
3. P0.1 codec (M)─┘
4. P0.3+P0.4 speech-final critical-section + monotonic (L)   ← gates everything below
5. P1.2a smart-turn v3 + surfacing + defaults (M)
6. P1.2b eager EoT w/ history staging (M)        ← needs 4 (single-fire) + LlmClient staging
7. P1.1 streaming TTS (L, 5d)                    ← needs 4 (else double-final ⇒ double-synthesis
                                                    gets blamed on the new transport)
8. pump flush=false deltas (S) ──► sub-second LIVE gate
P1.3 DAG parity (S) · P2 chaos items (S/M each) · P3 fleet (S each) · H hardware (M)
                — all parallelizable once 4 lands; P2's chaos suite is the standing gate.
```
Critical path to **sub-second live turns**: 4 → 5 → 7 → 8 plus a non-reasoning LLM (`REALTIME_ROADMAP.md`; Sarvam-30b stays the reasoning-model regression fixture). Projected live profile after P0+P1 with a fast LLM: EOS→turn-open <300 ms · glue ~10 ms · llm_ttft ~80–250 ms · tts_ttfb <300 ms ⇒ **headline ~400–700 ms**, with P3 colocation pushing toward the roadmap's ~250 ms.

## 9. Acceptance criteria (measurable, live)
1. **Correctness (turn-identity defined per critique):** a *turn* = one `turn_id` allocated at speech-final (or eager-open) and closed at audio-out/abort. 10 consecutive live e2e runs — per utterance, **exactly one `turn_id` with `outcome="completed"`**; speculative cancels appear only as `outcome="aborted",reason="eager_resumed"` (new label) and never double-complete; sniffer mismatch counter = 0 on PCM providers.
2. **Realtime:** `/debug/profile` on live runs: tts_ttfb p50 < 300 ms (WS), EOS→turn-open p50 < 300 ms, glue p99 ≤ 15 ms; headline < 1 s with a non-reasoning LLM.
3. **Chaos:** the §5 suite green: SIGTERM-mid-turn, provider-kill-mid-turn (breaker visible in `/readyz`, session survives via reconnect), slow-client, clock-skew, double-config, storm (chaos_storm).
4. **Standardization:** every provider constructible via the standard API; fleet conformance suites (turn-semantics, feature-serialization, format-truth) green; flat path deprecated-but-working.
5. **Hardware:** model suite green on `cpu` EP everywhere; `auto` improves or matches latency on accelerated hosts; zero EP-specific code outside the policy module.
6. **No silent failure:** grep-lint test forbids new bare `let _ = *_tx.send` / `SystemTime::now()` in decision paths; `waav_degraded_total` covers every fallback.

---

## 10. Critique log (multi-pass reflection)

Three passes: (1) plan v1 synthesized from the code-grounded analyses + research; (2) an independent adversarial review attacked v1 against the source (every objection evidence-cited); (3) a critique-of-the-critique accepted, improved, or bounded each objection. Decisions that changed:

| # | Objection (severity) | Disposition in v2 |
|---|---|---|
| 1 | **Actor rewrite breaks the caller-fires-callback contract / interim-final ordering (BLOCKER)** — `process_result` returns synchronously; an actor would fire finals from its own task | **Accepted, design replaced** — v2 uses a narrow no-await critical section + generation counter; callbacks stay with the initiating task; smaller diff (~250 vs ~400 lines). Stronger than the critic's own amendment (event-drain), which kept the actor's complexity |
| 2 | **Trait change blast radius (38 impls, ~135 call sites) + WS recv-task vs `RwLock` deadlock (MAJOR)** | **Accepted** — defaulted `speak_with_context` method (zero provider edits); recv task owns no outer lock (channel-only); ≤5 ms `speak()` bench added |
| 3 | **Sniffer misses the cache paths — poisoned entries replay forever (MAJOR)** | **Accepted** — sniff at store + hit; evict-and-refetch heals pre-fix caches |
| 4 | **Precedence flip breaks `test_from_file_yaml_overrides_env` (MAJOR)** | **Accepted, improved** — placeholder-filter *before* the `or_else` chain delivers env-fallback-for-placeholders with zero precedence change and zero test breakage; full 12-factor flip deferred to an explicit v2-config decision |
| 5 | **Eager EoT corrupts LLM history — user msg appended at request time (BLOCKER)** | **Accepted** — history staging: speculative turns read-only; commit user+assistant together on confirmation. Noted: normal cancelled turns keep append-at-request (arguably correct — the user did speak); only *speculative* turns stage, because their text is a prefix of the still-running utterance |
| 6 | **P1.1-before-P0.3 compounds double-final into double-synthesis (MAJOR)** | **Accepted** — resequenced: P0.3 gates P1.1/P1.2; added the combined chaos scenario |
| 7 | **P0.5 "reorder" hides real work (error buffer + drain) (MAJOR)** | **Accepted** — buffer+drain explicitly scoped with tests |
| 8 | **"Exactly one completed turn" ambiguous under eager (MAJOR)** | **Accepted** — turn-identity defined by `turn_id` grouping; `aborted/eager_resumed` label |
| 9 | **WS TTS endpoint_override lacks SSRF validation (MINOR)** | **Accepted** — reuse the DAG validator |
| 10 | **P1.1 effort underestimated (MINOR)** | **Accepted** — 1.5–2 k LOC / 5 d |

Held against critique unchanged: P0.4 monotonic migration, P1.3 DAG parity, P2 chaos designs, P3 fleet items, H hardware policy (critic verdict: sound).
