# WaaV Full-Codebase Brutal Audit — June 11, 2026

**Mandate:** line-by-line audit of the entire gateway, live testing with all components up against real providers, verification that every worthwhile optimization is integrated into the core path, and a catalog of gaps/bugs/non-production code.
**Method:** 7 parallel line-by-line subsystem audits (~276k LoC, 468 src files) cross-checked against the June-1 `BRUTAL_REVIEW.md` + `FIXES_APPLIED.md`; the full gateway booted and driven over its real WS protocol with **live Deepgram STT + live Sarvam LLM + live Deepgram TTS**; integrations implemented and re-verified this session. Companions: `LATENCY_ANALYSIS.md`, `REALTIME_ROADMAP.md`.

> **Verdict:** WaaV has moved from "NOT production-ready" (June 1) to **"production-capable with named, bounded gaps."** Every June-1 CRITICAL is verified fixed in code (S6 split/join, S7 rate-limit, Azure/Cartesia/Tencent/Tinkoff auth, panic isolation, model-hash fail-closed). The **full live multi-provider conversation loop now works end-to-end through the production binary**, and the latency profiler built this engagement is **integrated and measuring it live**. What remains is a specific, prioritized list: zero true streaming TTS, 3 audio-corrupting TTS format mismatches, config-DX traps found by live testing, per-session lifecycle gaps, and the per-provider feature long-tail.

---

## 1. Live system test — full loop with real providers (NEW evidence)

The gateway was built, booted (`WAAV_DEBUG_PROFILE=1`), and driven over `ws://…/ws` with the real protocol:

```
config(deepgram STT + deepgram TTS + sarvam-30b conversation, streaming)
→ stream real synthesized speech at 1× → silence
→ stt_result(final, speech_final)            +0.76 s after end-of-speech
→ [conversation turn runs live]
→ FIRST BINARY RESPONSE AUDIO                +8.2 s after end-of-speech
→ in-session direct `speak` control          ✓ 167 KB audio
```

**The integrated live profiler measured the production turn** (from `/debug/profile` + `/metrics` + the `waav::turn` tracing event, all emitted by the live binary):

| stage | live measurement |
|---|---|
| `waav_turn_response_latency_ms` | **7478 ms** (completed turn) |
| stt (partial→final) | 10 ms |
| stt_to_llm (WaaV glue) | **10 ms** — matches the harness's 12 ms |
| **llm_ttft** | **5905 ms** ← tagged bottleneck (`sarvam-30b` reasoning burn, measured live) |
| llm_sentence | 62 ms |
| tts_queue | 0 ms |
| tts_ttfb (Aura HTTP) | 1509 ms |
| egress | 0 ms |
| frames / skips | 1843 / 0 |

WaaV's own contribution to a 7.5-second turn: **~20 ms (0.3%)**. The providers (reasoning LLM + batch TTS) are 99.7% — confirming `LATENCY_ANALYSIS.md` end-to-end *through the production stack*, and proving the profiler attributes bottlenecks correctly (`bottleneck="llm_ttft"`).

### Bugs found *by* live testing (all real, none visible to static review)
| # | Finding | Status |
|---|---|---|
| L1 | **Empty `model` builds a broken Deepgram URL** (`model=` empty param → Deepgram rejects the WS handshake with a misleading 401), violating the documented "empty model → provider default" contract | **FIXED this session** (empty → `nova-2`) |
| L2 | **`config.yaml` placeholder beats env var**: `deepgram_api_key: "your-deepgram-api-key"` is returned verbatim by `get_api_key` even with `DEEPGRAM_API_KEY` set, despite the `# ENV:` comment claiming env binding. Quickstart configs silently authenticate with the placeholder string | **OPEN — HIGH.** Fix: env-first precedence (or placeholder detection `your-*` → unset) |
| L3 | **Provider auth errors don't reach the client**: Deepgram's 401 surfaced as `"Connection failed: Connection channel closed"`; the real cause only in server logs ("no error callback registered" — callbacks register *after* `start()` fails) | OPEN — MED (ordering: register error callback before connect) |
| L4 | **Unknown config fields silently ignored**: `"conversation"` vs `conversation_config` typo yields a half-configured session with zero feedback (no `deny_unknown_fields`/warning) | OPEN — MED (DX trap) |
| L5 | `tts_config.model` is required with no serde default while `stt_config.model` defaults — inconsistent contract | OPEN — LOW |

---

## 2. June-1 systemic issues (S1–S10): verified current status

| ID | June-1 finding | Verified now |
|---|---|---|
| S1 flat-config strands features | CRITICAL | **FIXED (keystone)** — all 67 providers have `from_standard`; the live WS path builds `StandardSTTConfig`/`StandardTTSConfig` (`config_handler` → `create_*_standard`). Flat `create_*_provider` path remains feature-limited (legacy callers only) |
| S2 default build ships VAD/turn stubs | CRITICAL | **UNCHANGED by design** (`default = []`) — operational footgun documented; silent-degradation alert still missing (see C6) |
| S3 TTS codec mismatch corrupts audio | CRITICAL | **PARTIALLY FIXED** — fleet Class-C audit fixed commercial providers; **Speechify / UnrealSpeech / WellSaid still hardcode MP3/WAV chunked as PCM → corrupted audio** (top remaining CRITICAL) |
| S4 no reconnection | HIGH | **FIXED for the streaming fleet** — `ReconnectableStream` + breaker + governor wired for **21 of 30** streaming STT providers; 9 stragglers (incl. AssemblyAI on the old manager) |
| S5 emotion_config dead | HIGH | **PARTIALLY FIXED** — reachable via the standardized path (Cartesia full; Hume via `from_standard`); dead on the legacy flat path |
| S6 DAG split/join broken, "lock-free" fictional | CRITICAL | **FIXED & verified** — split/join correct, per-node timeout honored, credentials resolve, streaming executor production-ready with full fan-out; DAG executor overhead ~0.1 ms. Two MED remain: router pattern-match nondeterminism (HashMap iteration), streaming task leak on forwarder panic |
| S7 rate-limit bypass + silent disable | CRITICAL | **FIXED & verified** — `PeerIpKeyExtractor` (no client-header trust); disable is explicit (`rps == 0` + loud warn) |
| S8 panic=abort + unwraps | HIGH | **FIXED** — `panic="unwind"` + per-session `catch_unwind` ("process unaffected"); residual unwraps: 2 serialization unwraps in the LLM hot path (H6) |
| S9 no Cargo.lock | MED | **FIXED** — lockfile committed (aws-smithy pins in git log) |
| S10 fake model-hash verification | HIGH | **FIXED for turn_detect** (pinned SHA-256, fail-closed); **silero-vad had NO check — FIXED this session** (pinned hash + `WAAV_SKIP_MODEL_HASH_CHECK` escape); smart-turn download path: verify when enabled |

---

## 3. Optimization-integration audit ("is everything beneficial actually wired?")

Verdicts from tracing every candidate from definition → live serving path:

**WIRED-LIVE (verified):** standardized rich configs (the S1 keystone) on the live WS path · resilience trio (governor + breakers + `ReconnectableStream`, 21 STT providers; breaker state on `/readyz` + `/metrics`) · streaming DAG executor driven from `speech_final` · conversation loop streaming-by-default · TTS audio cache + JWT cache · provider metrics (`waav_provider_*`) · readiness probes · smart-turn processing on the audio hot path (when features enabled) · `endpoint_override` operator surface.

**WAS DORMANT — INTEGRATED THIS SESSION:**
1. **The entire live latency-profiling stack** (the headline integration): per-session `ObserverRegistry` + `TurnProfiler` + `FrameProfiler` + the long-dormant `UserBotLatencyObserver`, attached in `config_handler`; `VoiceManager` observer hooks on the frame/STT/TTS paths; LLM-stage anchors in the conversation loop; `waav_turn_*`/`waav_frame_*` Prometheus series; `waav::turn` structured tracing; **`/debug/profile` (snapshot) + `/debug/profile/stream` (SSE)** mounted behind auth + `WAAV_DEBUG_PROFILE`. *Proven live (§1).* Overhead when disabled: one read per hook site; zero registries allocated.
2. **Sentence aggregation in the conversation pump** (was: `speak(token, flush=true)` per token — one HTTP synthesis per token on batch TTS, ruined prosody on WS TTS; comment claimed buffering that didn't exist). Now: boundary-aware aggregation (western/CJK/Devanagari terminators + 160-char latency cap + tail flush). Verified: conversation tests 4/4; harness A1 overhead unchanged (~12 ms p50); A2 overlap preserved (~146 ms saving with whole-sentence first-audio).
3. **Reasoning-model empty-content guard**: streaming turns that produce no speakable tokens now fall back to final `content`, and a *loud* warning replaces silent muteness when content is empty (the live-measured `sarvam-30b` failure mode).

**STILL DORMANT / PARTIAL (ranked by benefit × ease):**
| Gap | Detail | Benefit |
|---|---|---|
| **Zero true streaming TTS** | All 36/37 TTS providers are HTTP-request-per-`speak()` — even Cartesia (the base-trait docs describing "WebSocket providers (Cartesia, ElevenLabs streaming)" are aspirational). Roadmap P0's biggest lever | first-audio 700–1500 ms → <200 ms |
| Smart-turn not surfaced | runs per-frame when compiled+configured, but not parseable from the WS config; silent fallback to 1.8 s/2.5 s timers when init fails (C6) | 300–400 ms/turn |
| Phase-4 DAG turn profiling | DAG path turns not opened/closed in the profiler (node durations exist in `ctx.timing`; `waav_dag_node_ms` emit helper ready) | DAG parity for §1 evidence |
| 9 STT providers on legacy reconnect | assemblyai, openai, groq, bhashini, fpt_ai, naver_clova, nectec, sberdevices, yandex | uniform resilience |
| 24/35 TTS without `ReqManager` pooling | fresh reqwest clients per call | 50–100 ms TTFB |
| Speech-final defaults 4–7× too slow | `stt_speech_final_wait_ms=1800`, `hard=2500`, `turn_detection_timeout=500` vs measured needs (~420/54 ms) | 1–2 s/turn on the timer path |
| `WAAV_EAGER_WARMUP` off | cold-start spikes measured at 5–6 s | first-call latency |

---

## 4. New findings by subsystem (beyond §1–§3; severity-ranked highlights)

**Core engine (6 CRITICAL / 8 HIGH found; 3 fixed this session):**
- C1 token-per-speak — **FIXED** (§3.2) · C3 reasoning-empty — **FIXED** (§3.3) · C2 observers dormant — **FIXED** (§3.1)
- **C4 OPEN (HIGH):** `speech_final` duplicate-fire race — check-then-update of `turn_detection_last_fired_ms` isn't atomic across the timer/provider paths → double LLM turns under unlucky timing; compounded by **H5**: the hard-timeout task isn't cancelled on same-segment finals (two timers can both force-fire with different buffered text)
- **C5 OPEN (HIGH):** speech-final timing uses wall-clock `SystemTime` (`unwrap_or_default` → epoch 0 on clock skew) — NTP/VM-restore breaks turn timing; should be monotonic `Instant`
- **C6 OPEN (HIGH):** smart-turn/turn-detector init failure degrades silently to 2.5 s timers — no warn, no metric (the single biggest *silent* latency regression a deployment can hit)
- H6 OPEN: two `serde_json::to_string().unwrap()` in the LLM request path · MED: `speak_with_interruption` duration math assumes mono 16-bit PCM (wrong for compressed formats) · MED: TTS provider dispatch swallows send errors (`let _ =`) — audio can vanish without an error (mitigated by `tts_playback_complete` arriving only on success)

**STT fleet (32 providers):** all June-1 BROKEN providers verified fixed (Azure USP framing, Cartesia version, Tencent HMAC, Tinkoff JWT, Sarvam schema, Deepgram keyterm/utterance_end). Phonexia correctly **fail-closed** (honest stub). OPEN: **N1/N2** bounded-channel `try_send` can drop final transcripts/errors under load (Deepgram warns; AssemblyAI error-drop now logged — fixed this session); **N3** ElevenLabs hardcodes `is_final=false` on interims (breaks downstream turn logic); **N6** Google maps `is_final`→`is_speech_final` without `SpeechActivityEnd`; **N10** `is_final`/`is_speech_final` semantics undocumented and inconsistent across the fleet (portable turn-taking hazard); OpenAI realtime WS exists upstream but only batch is wired.

**TTS fleet (37 providers):** S3 trio still corrupting (above); Hume `description` clamped at one entry-point but not `from_standard`; Cartesia `speed` schema version-fragile + language hardcoded `"en"`; IBM `instance_id` env-only through the flat path; ElevenLabs default model is the non-realtime `eleven_v3`. Chunker frames by *declared* config format with no magic-byte/Content-Type validation — the enabling defect for S3.

**DAG:** S6 fixed (above). OPEN-MED: router HashMap iteration → nondeterministic pattern routing (sort patterns); streaming forwarder panic can leak tasks. SSRF validator solid (metadata/loopback/private/v6 + resolve-then-validate); residual LOW: decimal-IP literal and multicast corner cases.

**Transport/security:** S7/S8 fixed (above). JWT sound (no alg confusion). OPEN: graceful shutdown doesn't drain in-flight voice sessions (30 s axum drain only); WS `message_tx` backpressure can stall a session on a slow client (no send timeout); LiveKit operation queue lacks a shutdown handshake + depth metrics (the profiler's `observe_queue_depth` is ready to wire); double-`config` replaces the VoiceManager without tearing the old one down (leak); `/metrics` is public with no scrape limit.

**Infra/ML:** circuit breaker half-open CAS correct (verified concurrent-probe test); cache TTL clock-safe; `waav_dag_node_ms` label now length-clamped (this session). The "deadlocked test suite" was **diagnosed and fixed**: a test held a `parking_lot` lock across a second `record_turn` → silent non-reentrant self-deadlock (test bug, not production); plus two pre-existing test-fixture bugs the hang had masked. **Observability suite now 87/87.**

---

## 5. What changed this session (all verified by build + tests + live run)

1. Conversation pump: sentence aggregation + reasoning-empty guard + LLM stage anchors (`conversation/mod.rs`)
2. VoiceManager observer wiring: `observers` field/accessors + frame/smart-turn/skip/STT/TTS-request/first-chunk/audio-out hooks (`voice_manager/manager.rs`)
3. Per-session profiler registration on the live WS path (`config_handler.rs`)
4. `/debug/profile` + `/debug/profile/stream` handlers + auth-gated mount (`handlers/debug_profile.rs`, `main.rs`)
5. Deepgram empty-model URL contract fix (`stt/deepgram.rs`)
6. Silero-VAD pinned-hash verification, fail-closed (`silero_vad/detector.rs`)
7. DAG metric label cardinality clamp (`metrics/bridge.rs`); AssemblyAI error-drop logging (`stt/assemblyai/client.rs`)
8. Test-deadlock fix + 2 test-fixture fixes (`turn_profile.rs`, `profiler.rs`)

**Verification:** observability/bridge 87/87 · conversation_loop 4/4 · latency_harness A1/A2 green (overhead p50 11.97 ms) · gateway binary builds + boots · live multi-provider loop end-to-end (§1) · no API keys written to any file (env/inline only).

## 6. Prioritized remaining work
**P0 (correctness):** S3 TTS format trio (chunker must validate actual bytes) · L2 placeholder-beats-env key resolution · C4/H5 speech-final races · C5 monotonic time
**P1 (realtime, = roadmap P0/P1):** streaming-TTS abstraction + first WS provider · smart-turn surfaced in WS config + loud degradation warning + tuned speech-final defaults · L3 error-callback ordering
**P2 (robustness):** session drain on SIGTERM · WS backpressure timeout · double-config teardown · 9 STT reconnect stragglers · TTS `ReqManager` adoption · Phase-4 DAG turn profiling · L4 unknown-field warnings
**P3 (fleet long-tail):** ElevenLabs realtime default + interim semantics · `is_final` semantics doc + fleet conformance · Cartesia speed/language · IBM instance_id · Hume clamp parity

---
*Continuously verifiable: every latency claim in §1 is reproducible from `/debug/profile`, `/metrics` (`waav_turn_*`, `waav_frame_*`), and `RUST_LOG=waav::turn=info` on any live session — that instrumentation is now part of the product.*
