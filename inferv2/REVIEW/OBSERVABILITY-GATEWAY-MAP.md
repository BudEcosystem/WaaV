# WaaV Gateway Observability / Profiling — Read-Only Reconnaissance Map

Scope: map the WaaV **Gateway**'s existing observability so it can be extended into a unified
end-to-end (gateway → Infer) perf-observability picture. Read-only; **no code changed**.

Root: `/home/bud/ditto/waav/WaaV/gateway`. Infer side cross-referenced from
`/home/bud/ditto/waav/waav-infer`.

TL;DR:
- The gateway has a **mature, multi-surface latency-profiling system** (per-turn timeline →
  rolling percentiles + bottleneck tally + Prometheus + structured tracing event + live SSE +
  JSON snapshot). It is **both human- and machine-readable today**.
- The gateway↔Infer distributed-trace seam (GW-17) is **a one-sided gap**: the Infer **engine**
  fully implements the *receiving* half (parse `traceparent` → parent its turn span), but the
  **live gateway binary never mints a trace_id and never injects a traceparent** into the Infer
  handshake or WS headers. A single turn's trace does **NOT** span both halves today.
- There is no aggregation across the gateway-observed turn and the Infer-observed turn — no shared
  correlation id reaches Infer, and no consumer joins the two.

---

## (A) What the gateway observability captures + its output format

### A0. Module layout
`src/core/observability/mod.rs` re-exports the system (lines 26–47):
`latency`, `async_observer`, `heartbeat`, `observer`, `profiler`, `speaking`, `task_tracker`,
`turn_profile`. The metrics/Prometheus side lives under `src/core/metrics/` (`mod.rs`, `bridge.rs`,
`provider.rs`) and `src/dag/metrics.rs`.

The whole thing is driven by a **`VoiceObserver` trait** (`src/core/observability/observer.rs`) with
a per-session `ObserverRegistry` that fans out lifecycle hooks (`on_audio_in`, `on_stt_partial`,
`on_stt_result`, `on_llm_request`, `on_llm_first_token`, `on_llm_first_sentence`, `on_tts_request`,
`on_tts_chunk`, `on_audio_out`, `on_smart_turn`, `on_frame_skipped`, `on_frame_stage`, …). Two tiers:
a **sync** tier (the profilers, exact-ordering, inline) and an **async** tier
(`src/core/observability/async_observer.rs`, lines 60–108) that wraps heavier observers behind a
bounded `try_send`-or-drop queue so observability never blocks the audio hot path.

### A1. The turn profiler — per-stage latency timeline
**`src/core/observability/turn_profile.rs`** is the core. A "turn" = one user utterance → bot
response.

- **Stages captured** (`Stage` enum, `turn_profile.rs:72–138`) — a monotonic-ns timeline of
  anchors and the deltas between them:
  | Stage | delta measured | anchors (turn_profile.rs:221–236) |
  |---|---|---|
  | `stt` | last user audio frame → end-of-speech (STT finalize; *pre-response*, excluded from headline) | `audio_in_ns → stt_final_ns` |
  | `stt_to_llm` | end-of-speech → LLM request dispatched | `stt_final_ns → llm_request_ns` |
  | `llm_ttft` | LLM request → first token (TTFT) | `llm_request_ns → llm_first_token_ns` |
  | `llm_sentence` | first token → first complete sentence | `llm_first_token_ns → llm_first_sentence_ns` |
  | `tts_queue` | first sentence → TTS request issued | `llm_first_sentence_ns → tts_request_ns` |
  | `tts_ttfb` | TTS request → first synthesized audio (TTS TTFB) | `tts_request_ns → tts_first_audio_ns` |
  | `egress` | first TTS audio → first audio leaves the gateway | `tts_first_audio_ns → audio_out_ns` |

- **Handshake** is NOT a turn stage. The turn anchors at *end-of-speech* (`stt_final`), not at
  session/WS connect. Connection-level timing lives in the Prometheus provider TTFB + reconnect
  series (A4), not in the turn timeline.

- **Headline metric** = "response latency" = end-of-speech → first audio out =
  `audio_out_ns − stt_final_ns` (`response_latency_ns`, `turn_profile.rs:213–219`). This is the
  user-perceived figure.

- **Pre-EOS anchors** (`audio_in`, `smart_turn`, `stt_partial`) are written lock-free into relaxed
  atomics on the audio hot path; the turn opens on `stt_final` and snapshots them
  (`turn_profile.rs:370–385`). Post-EOS anchors take an uncontended per-session mutex (~6/turn).
  Turn closes on first `audio_out` (`turn_profile.rs:489–504`); a barge-in/overlap closes the prior
  turn as `Aborted` (`open_turn` 370–385, `abort_current` 395–400).

- **Smart-turn (turn-detection) inference cost** rides separately: `smart_turn_inference_us` per
  turn + a dedicated rolling window in the hub (it's a per-frame hot-path cost, not a response
  stage).

- **DAG per-node breakdown**: for the DAG path, `node_durations_us: Vec<(Arc<str>, u64)>`
  (`turn_profile.rs:178`) carries per-node wall times.

- **Path + outcome**: `TurnPath` ∈ {`conversation`, `dag`} (turn_profile.rs:34–48);
  `TurnOutcome` ∈ {`completed`, `aborted`} (51–65); `streaming_path: bool` (DAG streaming executor
  vs batch fallback).

- **Bottleneck**: `compute_bottleneck()` (`turn_profile.rs:247–256`) = the dominant **response**
  stage (max delta among the 6 response stages; `stt` excluded), set at close.

### A2. Assembly + emission — how a turn's profile is produced
The per-session `TurnProfiler` (`turn_profile.rs:337`) is a `VoiceObserver`; on turn close it hands
the finished `TurnTrace` to a `TurnSink` — the process-wide `LatencyProfiler`
(`src/core/observability/profiler.rs:189`), one per `CoreState`. `LatencyProfiler::record_turn`
(`profiler.rs:361–439`) emits **four ways simultaneously**:

1. **(a) Structured tracing event** (`profiler.rs:372–383`): `tracing::info!(target: "waav::turn",
   turn_id, session, path, outcome, streaming, response_latency_ms, smart_turn_inference_ms,
   bottleneck, "turn_complete")`. Always-on; surfaced via `RUST_LOG=waav::turn=info`. **This is the
   only per-turn surface carrying `session_id`/`turn_id`** (deliberately kept out of Prometheus
   labels — bridge.rs:76–79).
2. **(b) Prometheus** (`profiler.rs:385–405`): per-turn counters + histograms (see A4).
3. **(c) Rolling aggregates** (`profiler.rs:407–420`): headline + per-stage `RollingWindow`s
   (1024-sample bounded windows, `profiler.rs:36–106`, computing avg/p50/p90/p99/min/max).
4. **(d) recent-slow ring + gated SSE broadcast** (`profiler.rs:422–438`): turns ≥ `SLOW_THRESHOLD_MS`
   (1000ms, profiler.rs:27) kept in a 32-deep ring; broadcast over a `tokio::broadcast` channel only
   when there is ≥1 SSE subscriber (zero-cost when nobody's listening).

So a turn's profile is **a struct** (`TurnTrace`/`TurnSummary`) that is emitted as **a log event AND
Prometheus samples AND an in-memory rolling aggregate AND a JSON SSE/snapshot payload** — all three
of log / metric / JSON.

### A3. Output formats / surfaces (human + machine readable)
1. **`GET /debug/profile`** — JSON snapshot (`src/handlers/debug_profile.rs:30–32` →
   `LatencyProfiler::snapshot`, profiler.rs:298–358). `ProfileSnapshot` (Serialize, profiler.rs:490)
   contains: `enabled`, `sample_count`, `headline` (WindowStats), per-`stages` p50/p90/p99,
   `bottleneck_histogram`, `current_bottleneck`, `recent_slow_turns` (per-turn `TurnSummary`s), and
   the `realtime_blockers` block. **Machine-readable JSON + human-skimmable.**
2. **`GET /debug/profile/stream`** — SSE, one `turn` event per completed turn (sampled by
   `WAAV_DEBUG_PROFILE_SAMPLE_N`); lagging client gets a `lagged` event, not a kill
   (`debug_profile.rs:35–63`). Each event is `TurnSummary` JSON.
   Both `/debug/profile*` routes mount **only when `WAAV_DEBUG_PROFILE=1`** and sit behind
   `auth_middleware` (`src/main.rs:214–231` — double lock, never public).
3. **`GET /metrics`** — Prometheus text exposition (A4). Public, no-auth operability route
   (`src/main.rs:235–255`); handler `src/handlers/api.rs:124`.
4. **`waav::turn` tracing log** — the per-turn structured event (A2-a), the only `session_id`-bearing
   surface.

`TurnSummary` (turn_profile.rs:307–320) and `ProfileSnapshot` are `#[derive(Serialize)]` → clean
JSON; `Stage`/`TurnPath`/`TurnOutcome` serialize `snake_case`. **Conclusion: both human-readable
(log line, /metrics text, JSON skim) AND machine-readable (JSON snapshot, SSE, Prometheus).**

### A4. The `/metrics` endpoint — exported Prometheus series
Bridge: `src/core/metrics/bridge.rs` installs a one-shot process-global Prometheus recorder
(`metrics_handle`, bridge.rs:160–204) and renders via `render()` (207–209). Series (names are the
W-C1/E13 contract — dashboards depend on them):

**Provider / resilience** (bridge.rs:38–48):
- `waav_provider_requests_total` (counter; labels `provider,channel,outcome`)
- `waav_provider_ttfb_ms` (histogram; `provider,channel`) — the per-provider TTFB
- `waav_provider_errors_total` (counter; `provider,channel,kind`)
- `waav_circuit_breaker_state` (gauge 0/1/2; `provider`)
- `waav_reconnects_total` (counter; `provider,outcome`)

**Turn-level errors / lifecycle / usage** (bridge.rs:52–72):
- `waav_turn_errors_total` (`class` recoverable|fatal)
- `waav_session_teardown_timeouts_total`, `waav_session_dangling_tasks_total`
- `waav_pipeline_heartbeat_ms` (histogram), `waav_pipeline_heartbeat_misses_total`
- `waav_tts_chars_total` (`provider`), `waav_llm_tokens_total`
  (`provider,kind` ∈ prompt|completion|cache_read|cache_creation|reasoning) — cost proxies

**Live latency profiling** (bridge.rs:83–112) — fed from `LatencyProfiler`/`FrameProfiler`:
- `waav_turn_response_latency_ms` (histogram; `path`) — the headline
- `waav_turn_stage_ms` (histogram; `stage,path`) — per-stage delta
- `waav_smart_turn_inference_ms` (histogram), `waav_smart_turn_frame_skips_total`
- `waav_llm_ttft_ms` (`path`), `waav_tts_ttfb_ms` (`path`)
- `waav_turns_total` (`path,outcome`), `waav_turn_bottleneck_total` (`stage`)
- `waav_dag_node_ms` (histogram; `node`, id clamped to 48 chars, bridge.rs:521–528)
- `waav_frame_stage_ms` (`stage` receive|decode|vad|smart_turn|stt_send), `waav_frame_total`
- `waav_queue_depth` (gauge; `queue` ws_msg|livekit_op), `waav_queue_latency_ms` (gauge; `queue`)
- `waav_tts_format_mismatch_total` (`provider,source`), `waav_degraded_total`
  (`component,reason`), `waav_async_tool_total` (`outcome`)

Histogram bucket sets are voice-tuned (bridge.rs:117–146). **Cardinality rule (bridge.rs:76–79):
NEVER `session_id`/`turn_id` in labels** — those ride the `waav::turn` tracing event only. The
endpoint test `tests/metrics_endpoint.rs` drives one real `/speak` through an OpenAI mock and asserts
`waav_provider_ttfb_ms` + `waav_provider_requests_total` + the `openai` label appear.

A second, **separate** metrics struct exists: `src/dag/metrics.rs` `DAGMetrics` — atomic counters +
an 8-bucket latency histogram with its own `latency_percentiles()` (p50/p90/p99/p999) and
per-node/per-endpoint snapshots. This is an **in-memory struct API (not exported to Prometheus)**;
the Prometheus DAG view is the separate `waav_dag_node_ms` series fed from the turn trace.

Also note `src/core/metrics/provider.rs` `ProviderMetrics` keeps lock-free atomic TTFB
(min/max/sum/count) + request/error/timeout/rate_limit counters and a `snapshot()` (in-memory);
every `record_*` also feeds the Prometheus bridge.

### A5. Derived / analysis metrics already computed
Yes — the gateway already computes derived perf analytics, not just raw counters:
- **Rolling percentiles** p50/p90/p99 + avg/min/max per stage and for the headline
  (`RollingWindow::stats`, profiler.rs:79–105) over a 1024-sample window.
- **Bottleneck tagging**: per-turn dominant stage (`compute_bottleneck`) + a process-wide
  `bottleneck_histogram` and `current_bottleneck` (the mode, profiler.rs:304–315). `llm_ttft` is a
  first-class stage and a first-class bottleneck bucket.
- **`realtime_blockers`** (profiler.rs:341–356, struct 515–529): the actionable optimization signals —
  `smart_turn_inference_ms_p99`, `frame_skip_rate` (+totals), `llm_ttft_p50_ms`,
  `llm_sentence_p50_ms`, `tts_ttfb_p50_ms`, `streaming_path_used_ratio`, `ws_queue_depth_max`,
  `lk_queue_depth_max`.
- **recent-slow ring**: the 32 most recent ≥1s turns, full timeline retained for inspection.

### A6. Latency benchmarks (the three test files)
- **`tests/e2e_latency_benchmark.rs`** — wiremock-mocked providers. Measures **HTTP endpoint latency
  + concurrency throughput**, computes p50/p90/p99/avg, and asserts **gateway overhead < 50ms** over
  the simulated provider latency (e2e_latency_benchmark.rs:118–126). NB: it hits the **mock
  directly**, not through gateway turn stages — it bounds gateway HTTP overhead, not the turn
  timeline.
- **`tests/turn_detect_latency.rs`** — real ONNX on the audio hot path. Times **smart-turn
  inference** (mel extraction + ONNX, p50/p90/p99 in µs) and **silero-VAD per-frame** inference. This
  is the budget that *precedes* `stt_final` (the `Stage::Stt` / smart-turn region). Feature-gated
  (`smart-turn`,`silero-vad`), reads cached models.
- **`tests/livekit_audio_latency_tests.rs`** — the LiveKit audio **egress/queue** hot path. Asserts
  queue ops stay sub-ms / no sleep, bounded queue (`MAX_AUDIO_QUEUE_SIZE=100`) FIFO-drops, p99 < 1ms,
  no retry loops. This is the `egress` / `waav_queue_*` budget.

Together they cover three of the turn stages (turn-detect → `stt`; HTTP provider → `*_ttfb`;
egress/queue → `egress`) but as **isolated micro-benchmarks**, not one assembled end-to-end turn.

---

## (B) The gateway ↔ Infer trace/observability seam — **GAP (one-sided)**

The intended contract is **GW-17** (INFER_GATEWAY_INTEGRATION §13): the gateway mints a W3C
`traceparent`, forwards it on the Infer session handshake, and the Infer engine parents its per-turn
/ per-stage spans under it — so one turn = one distributed trace spanning gateway + Infer.

### B1. Infer (receiving) side — FULLY IMPLEMENTED
- `waav-infer/crates/waav-infer-protocol/src/trace.rs` — `TraceContext`: the on-wire W3C carrier
  (`parse`, `child`, `traceparent()`, `trace_id_hex`, `sampled`); rejects malformed/all-zero with a
  typed error, never panics.
- `waav-infer/crates/waav-infer-protocol/src/session.rs:103` —
  `SessionConfig.trace: Option<TraceContext>` — the handshake field the gateway is meant to populate
  (serialized only when present; `None` ⇒ byte-unchanged untraced handshake).
- `waav-infer/crates/waav-infer-server/src/ws.rs:166` — engine reads `cfg.trace` →
  `otel::turn_span(...)` and runs the whole session loop `.instrument(turn_span)` (ws.rs:236).
- `waav-infer/crates/waav-infer-server/src/otel.rs:26` — `turn_span()` builds `infer.turn` span with
  `trace_id` / `parent_span` / `gw_sampled` structured fields; degrades to a root span when no trace.
- `waav-infer/crates/waav-gateway-provider-api/src/config.rs:42,62,86,105` — the **GW-1 provider
  seam** `STTConfig`/`TTSConfig` already carry `trace: Option<TraceContext>` + `with_trace()` to
  forward onto `SessionConfig::trace`.

### B2. Gateway (injecting) side — ABSENT
The **live gateway binary** (`/home/bud/ditto/waav/WaaV/gateway`) never produces or forwards a trace:
- **No OTel, no trace_id minting.** `src/main.rs:74` is `tracing_subscriber::fmt::init()` (plain
  fmt). Grep across `src/`: **no `opentelemetry`, no `tracing-opentelemetry`, no `otel`, no
  `TraceContext`** (the only "trace" hits are unrelated provider `trace_id` fields and a test
  fixture). The gateway has nothing to inject.
- **No dependency on the Infer wire crate.** `Cargo.toml` has **no `waav-infer-*` dependency**, so
  `TraceContext`/`SessionConfig.trace`/`STTConfig::with_trace` are **not even in scope** for the
  gateway. The forwarding seam in `waav-gateway-provider-api` lives in the *waav-infer* workspace and
  is **not wired into the production gateway**.
- **Realtime S2S adapter injects nothing.** `src/core/realtime/infer/protocol.rs`:
  - `session_config()` (lines 74–89) builds the `session.config{task:s2s}` JSON with
    `type/task/model/audio/conditioning` — **no `trace` key**.
  - `connect_spec()` (lines 134–152) returns `ConnectSpec::WebSocket { url, headers: vec![] }` —
    **no `traceparent` header**, with the explicit comment that auth/headers are "not modeled in the
    open seam" (protocol.rs:149–151).
- **Cascade STT adapter has no transport yet.** `src/core/stt/infer.rs` `connect()` returns a typed
  `ConnectionFailed` ("native WS v1 transport not yet wired", infer.rs:59–65) — so there is **no
  injection point at all** on the cascade path today.
- **Inbound traceparent is parsed but never propagated outward.** `src/middleware/request_id.rs`
  derives a `request_id` *string* from an inbound `traceparent` (request_id.rs:56–72) and enters a
  `tracing` span with it (95) + echoes `x-request-id` back (100–104). But this id is **never injected
  on any outbound provider/Infer call** — grep for outbound `.header("x-request-id"/"traceparent")`
  injection returns **nothing**. The middleware docstring says handlers "can forward" it; no handler
  actually does.

### B3. Verdict
**The seam is a gap, not a connection.** A single turn's trace does **not** span both gateway and
Infer today:
- Infer can *receive and honor* a gateway trace, but the gateway never sends one (no traceparent on
  the WS handshake, no header, no trace_id minted).
- Even the coarse `request_id` correlation never crosses to Infer.
- The Infer engine therefore always takes its **no-trace root-span** branch (otel.rs:35–42) when
  driven by the live gateway — its `infer.turn` spans are a *separate* trace with no parent.

Closing GW-17 from the gateway requires: (1) an OTel/trace layer (or at least a minted W3C
`traceparent` from the per-request span the `request_id` middleware already opens), (2) a dependency
on `waav-infer-protocol::TraceContext` (or an equivalent local serializer), and (3) populating the
`trace` field of the realtime `session.config` frame + the cascade handshake (and/or a `traceparent`
WS header in `connect_spec`).

---

## (C) What's missing for a unified end-to-end (gateway → Infer) perf view

1. **No trace propagation across the hop (the headline gap — see B).** No shared
   trace_id/traceparent reaches Infer, so gateway turn spans and Infer `infer.turn` spans cannot be
   joined into one trace/turn.

2. **No shared correlation key in the data, either.** The gateway's per-turn surface keys on
   `session_id`/`turn_id` (`waav::turn` log); Infer keys on its own `session_id` + (would-be)
   `trace_id`. Nothing guarantees the gateway `session_id` equals the Infer `session_id`, and
   `turn_id` is gateway-local. There is no agreed join key today.

3. **The gateway turn timeline stops at its own egress.** Stages end at `egress` =
   "first audio leaves the gateway" (turn_profile.rs:88,229). When STT/LLM/TTS is served *by Infer*,
   the gateway sees only the provider-boundary `*_ttfb` numbers; the *intra-Infer* breakdown (model
   queue / prefill / decode / vocoder, the kind of split in `waav-infer`'s own profiling under
   `WaaV/inferv2/REVIEW/06-model-profiling-plan.md`) is invisible to the gateway view. A unified view
   needs the gateway stage deltas **and** the Infer per-stage spans under one trace.

4. **No exporter wiring even on the Infer side.** Infer's GW-17 is realized as `tracing` span
   *fields* (`trace_id`/`parent_span`), explicitly "not a full OTel SDK" (otel.rs:7–10); grouping
   "the exporter wiring is deployment config" (trace.rs / otel.rs). So even if the gateway injected a
   trace, **nothing currently collects + joins both sides** into one viewable trace. A unified view
   needs a collector (OTLP/Jaeger/Tempo or a custom scraper) consuming both halves.

5. **Two disjoint metric namespaces, no cross-walk.** Gateway exports `waav_*` Prometheus series;
   Infer has its own profiling/metrics (separate process, separate registry). There is no unified
   exposition, no label convention linking `waav_turn_stage_ms{stage="llm_ttft"}` to an Infer-side
   model-stage histogram for the same turn.

6. **The `/debug/profile` analysis is gateway-local.** `realtime_blockers`, the bottleneck
   histogram, and the recent-slow ring only know the gateway's 7 stages. When the bottleneck is
   `llm_ttft`/`tts_ttfb` and Infer serves it, the snapshot can say *which gateway stage* is slow but
   not *why inside Infer*. Unified end-to-end requires the snapshot (or a successor) to fold in
   Infer's per-stage numbers for the same trace.

7. **Cascade-over-Infer path is unmeasured because it's unwired.** `InferSTT::connect` is a stub
   (B2); until the native WS v1 transport lands there is no STT-over-Infer timing to unify, and the
   realtime S2S path bypasses the gateway's cascade stages entirely (`emits_user_turn_frames=true`,
   protocol.rs:118–132) so its `TurnProfiler` LLM/TTS stages won't even populate for native S2S
   turns — the unified view for S2S must come from the Infer span tree, making (1)+(4) mandatory.

### Smallest path to "connected" (for planning, not prescriptive)
- Gateway: mint a W3C `traceparent` from the existing per-request `tracing` span
  (request_id.rs:95) — or add a minimal OTel layer — and inject it as (a) the `trace` field of the
  realtime `session.config` frame (protocol.rs:74–89 / build_session_config) and (b) a `traceparent`
  WS header in `connect_spec` (protocol.rs:151, currently `headers: vec![]`); and when the cascade
  transport lands, via `STTConfig::with_trace`.
- Collector: stand up an OTLP/tracing exporter consuming both the gateway `waav::turn` events / spans
  and the Infer `infer.turn` spans, joined on `trace_id`.
- Unified analysis: extend `ProfileSnapshot`/`realtime_blockers` (or a new endpoint) to attribute the
  `llm_ttft`/`tts_ttfb`/`stt` budgets to Infer's per-stage spans for the same trace.

---

## Human + AI readability assessment (today)
**Good, on the gateway side.** Structured + machine-readable already exists: JSON snapshot
(`/debug/profile`), JSON SSE stream, Prometheus text, and a single structured `waav::turn` log event
per turn with named fields (response_latency_ms, bottleneck, path, outcome). `ProfileSnapshot` even
pre-computes the "what's slow" summary (`current_bottleneck`, `realtime_blockers`) — directly
consumable by an agent without re-deriving percentiles.

**Gaps for "easy for humans AND AI agents, end-to-end":**
- It is **gateway-scoped** — no single artifact shows the gateway *and* Infer view of one turn
  (needs B/C-1,4).
- There is **no rendered summary table / report** that fuses headline + per-stage p50/p95 + bottleneck
  + the Infer breakdown; an agent must still call `/debug/profile`, read `waav_*` Prometheus, *and*
  (today, impossible) the Infer trace, then join them by hand.
- Percentiles are **p50/p90/p99** in the snapshot (profiler.rs:92–104); the request mentions p95 —
  p95 is **not** currently computed (would be a one-line addition to `WindowStats`).

---

## Key file:line index
- Turn timeline + stages: `src/core/observability/turn_profile.rs` (Stage 72–138; anchors 221–236;
  headline 213–219; bottleneck 247–256; TurnSummary 307–320; assembly 337–505).
- Hub + percentiles + bottleneck + SSE + snapshot: `src/core/observability/profiler.rs`
  (RollingWindow 36–106; record_turn 4-way emit 361–439; snapshot 298–358; realtime_blockers 515–529).
- Async observer tier: `src/core/observability/async_observer.rs:60–135`.
- User↔bot latency (p50/p99): `src/core/observability/latency.rs:51–169`.
- Prometheus bridge + series contract: `src/core/metrics/bridge.rs` (series 38–112; recorder
  160–204; emit helpers 359–548).
- Provider in-memory metrics: `src/core/metrics/provider.rs`; registry `src/core/metrics/mod.rs`.
- DAG metrics (in-memory, not exported): `src/dag/metrics.rs`.
- Debug endpoints: `src/handlers/debug_profile.rs`; `/metrics` handler `src/handlers/api.rs:124`;
  route mounting + auth `src/main.rs:214–255`.
- `/metrics` test: `tests/metrics_endpoint.rs`. Benchmarks: `tests/e2e_latency_benchmark.rs`,
  `tests/turn_detect_latency.rs`, `tests/livekit_audio_latency_tests.rs`.
- Inbound trace-context middleware (parse-only, no outbound propagation):
  `src/middleware/request_id.rs:30,54–79,95–104`.
- Gateway Infer realtime adapter (no trace injected): `src/core/realtime/infer/protocol.rs:74–89,
  134–152`; `src/core/realtime/infer/mod.rs`.
- Gateway Infer cascade STT (transport stubbed): `src/core/stt/infer.rs:59–65`.
- Infer (receiving) GW-17: `waav-infer/crates/waav-infer-protocol/src/trace.rs`;
  `.../src/session.rs:103`; `waav-infer/crates/waav-infer-server/src/otel.rs:26`;
  `.../src/ws.rs:166,236`; `waav-infer/crates/waav-gateway-provider-api/src/config.rs:42,62,86,105`.
</content>
</invoke>
