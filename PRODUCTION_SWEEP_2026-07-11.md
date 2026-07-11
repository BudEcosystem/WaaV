# Full-Stack Production-Readiness Sweep — 2026-07-11

Five parallel domain sweeps (gateway core, provider fleet, infer serving, infer backends, SDKs)
cross-checked against BRUTAL_REVIEW / AUDIT_REPORT / PRODUCTION_PLAN / PERF_RESILIENCE_PLAN /
inferv2 SYNTHESIS, followed by a prioritized fix loop. Suites re-run green after every fix.

## Headline

The stack is far healthier than the last audits recorded: **every previously-documented P0 across
all five domains was verified FIXED in the current tree** (gateway: DAG split/join, speech-final
race, monotonic time, rate-limit bypass, SSRF, reconfigure leak, L2 placeholder-keys, S3 format
trio; infer: slow-consumer HOL, TTFA, coalescer bounds, drain gating, ORT deadlock class, disconnect
pinning; SDK: the whole PERF_RESILIENCE Tier-1 — zombie watchdog, player, worklet, reconnect breaker).
The sweeps found **0 new P0 runtime defects** — but the tree itself didn't compile (stale WIP), and
the SDK↔gateway wire had 2 P0 contract breaks. All fixed below.

## Fixed this session (all verified by build + suite)

### Build integrity (P0)
- Gateway tree had 3 never-compiled edits in the 412-file uncommitted delta: `messages.rs`
  missing `Ok(header)`, `smallest/provider.rs` `from_base(&config)`, bench missing new
  `OutgoingMessage` fields. Workspace now checks clean `--all-targets`.
- ARM64+CUDA build failure (`webrtc-sys` x86-only NVIDIA implib asm, auto-enabled by
  `/usr/local/cuda` detection) — root-caused; the documented `CUDA_HOME` mask (BUILD.md Gotcha 2)
  verified working.

### SDK ↔ gateway wire contracts (P0/P1)
- **TS `speak()`**: sent a flat body; gateway requires nested `tts_config` → every one-shot TTS 422'd. Fixed.
- **Python `create_livekit_token()`**: sent `identity`/`name`; gateway requires
  `participant_identity`/`participant_name` → always 422'd. Fixed.
- **SIP hooks (both SDKs)**: flat body deserialized into an EMPTY `hooks[]` (silent destructive
  no-op) → wrapped in the envelope; TS `deleteSIPHook` hit a non-existent path-param route → now
  `DELETE /sip/hooks {hosts}`. Fixed both.
- **Default port drift**: TS SDK + widget defaulted to the dev machine's `:3009`; gateway default is
  `:3001` → out-of-box connection refused for every customer. Fixed everywhere.
- **TS REST retry parity**: TS threw on the first 429/503 while Python rode out blips → TS now
  retries with Retry-After-aware jittered backoff (max 3, `retries` option), + 4 new tests.
- Python dead endpoints (5): `get_recording` re-pointed at the SERVED `/recording/{id}` route;
  `get/delete_cloned_voice`, `list/delete_recordings` now fail fast with typed 501s instead of
  confusing network 404s. `get_metrics` docstring/type corrected (the gateway DOES serve
  Prometheus text). `py.typed` marker added (PEP 561).
- Widget: loud console warning when `data-llm-api-key`/`data-llm-reasoning-api-key` embed raw
  provider keys in the DOM (server-side alias is the safe path).
- Dashboard: puppeteer e2e split to opt-in `test:e2e` (default `npm test` green again).

### Gateway (P1/P2)
- **WS connect timeouts (fleet)**: new canonical `resilience::connect` (15 s dial / 30 s factory
  bounds) wired into the realtime scaffold (all 13 S2S providers) + the 10 bare-dial STT clients —
  a blackholed handshake previously pinned the reconnect supervisor forever, invisible to the
  circuit breaker. 738 provider tests green.
- **LiveKit token least-privilege (S2.5)**: end-user tokens no longer carry `room_record`/
  `room_list`/`room_create`/admin (leaked-token blast radius); TTLs added (user 1 h, agent 6 h);
  behavior locked by tests.
- `create_room` hardcoded `max_participants: 3` → named default + `create_room_with_capacity`.
- Baidu STT: silent transcript drop under backpressure now logged (the N1/N2 straggler).
- `RealtimeProvider` enum extended 2→12 to mirror the string registry (parse silently rejected ten
  valid providers), with a registry↔enum lockstep test.

### Infer (P1)
- **Admission portability**: the GB10-measured DRAM peak (198.5e9) hardcoded into the live
  bandwidth-duty gate → `WAAV_RATED_DRAM_BYTES_PER_S` env-derivable, resolved value + source logged.
- **Flat `try_admit` observability**: the sole backpressure gate for REST STT/one-shot-TTS/WS paths
  emitted ZERO metrics → `waav_infer_flat_admission_shed_total{reason}` (draining/uncalibrated/
  bandwidth/concurrency_timeout) + available-permits gauge.
- **ORT silent CPU degradation**: `auto`→CPU fallback and EP registration failure now warn +
  `waav_degraded_total` (previously debug-log only — a fleet-wide latency collapse with no alert).
- **Inert TRT knobs**: default builds compile the TRT path OUT while 25 `WAAV_*TRT*` knobs read
  inside it → one-shot boot warning when any is set on a non-TRT build.
- **RSB unwired model**: the complete 1.3k-LoC speech-enhancement model had no reachable entry
  point → wired into CLI `enhance` (dir → TorchRsb, SDE/3-step verified recipe), live-verified
  RTF 0.236 on CUDA.

### Round 2 (post-checkpoint)
- **Test-suite stability**: rotating SSRF-test flakes root-caused to the process-global
  `WAAV_ALLOW_LOOPBACK_ENDPOINTS` env var read by the validator while ~50 setter tests mutated it
  under module-local locks → single crate-global `net::ssrf_env_lock()`; 128 rejection tests locked
  across 109 files. Full suite now deterministic: 6712/0 twice, then 6700/0 and 6867/0 (featured).
- **GoogleTTS validation ordering**: `new()` built the google-cloud-auth token cache BEFORE
  validating sample_rate → a bad config panicked ("no reactor") in sync contexts instead of a typed
  error. Validation now precedes auth construction.
- **DAG routing scope (G7)**: arrays + nested objects were silently DROPPED from the Rhai scope
  ("for now") — `user.tier == "pro"` / `tags.contains("vip")` mis-routed with no diagnostic. Now
  bound as Rhai maps/arrays (`data_user.tier`, `data_tags[0]`) + recursive underscore flattening,
  with tests.
- **Dead `LiveKitManager` (G8)**: 492-line unwired module carrying an unbounded audio channel
  (a backpressure trap for any future caller) — removed; module docs point at the real
  `LiveKitClient`/`LiveKitRoomHandler` path.

### Round 3 (autonomous loop)
- **G3 WS first-message JWT auth**: JWT-only deployments couldn't authenticate over WS except via
  the log-leaking `?token=` query param — the first-message path hard-required API-secret mode.
  Now routes first-message tokens through the same external auth service the HTTP middleware uses.
- **TS↔Python REST parity closed**: TS gained `sipTransfer`, `remove/muteLiveKitParticipant`,
  DAG template/validate methods, `getMetrics`, `getLanguageCapabilities` (all wire shapes verified
  against gateway handler structs; 12 new tests). Bonus P0-class find: **Python `sip_transfer`
  sent `{stream_id,…}` — never a gateway field, every call 422'd** — fixed. Python gained
  `fetch_language_capabilities` (live). TS 311/0, Py 415/0.
- **Infer consolidation (top-2)**: the drifted TF32 FFI block (11 copies, 3 distinct behaviors)
  → one parameterized `nn::tf32::set_tf32/enable/disable` with per-model values preserved exactly
  and scar rationale moved to call sites; `argmax_first` (6 identical host defs) → canonical
  `nn::sampling::argmax_first` with the tie-break contract documented. 783+242 tests green.

### Round 4 (autonomous loop)
- **WS-dial divergence fully zeroed**: 8 STT + 2 TTS local connect-timeout constants collapsed onto
  `resilience::connect` (per-provider values preserved exactly, deviations commented); the 7
  hand-rolled WS upgrade builders (the Sarvam-scar class) converted to `into_client_request()` with
  load-bearing Hosts pinned (azure/huawei override cases) and scar comments preserved. Zero
  hand-typed `Sec-WebSocket-Key` and zero local timeout consts remain. 6699/0 twice.
- **Diarize + enhance served over REST**: `POST /v1/audio/diarize` (speaker segments JSON) +
  `POST /v1/audio/enhance` (WAV out) with the full transcriptions-grade treatment (501-honesty,
  admission, bounded decode, timeout, metrics, capabilities rows); config/env resolution
  (`WAAV_DIARIZER_DIR`/`WAAV_ENHANCER_MODEL`) with warn-vs-fail semantics; live-verified on GB10
  (pyannote 2-segment diarize + dpdfnet2 enhance through the real router). **Bonus real bug**: the
  diarizer's embedding validator hardcoded 512-d and rejected the actual cached 256-d
  pyannote-community-1 model — the pre-existing CLI diarize path was broken on any real speech;
  fixed (dimension-agnostic, layout errors still typed). Server 244/0, components 98/0, core 343/0.

## Remaining (tracked, not yet fixed)
- Gateway P1: SIP dispatch `max_participants` TODO (upstream livekit-protocol gap).
- Gateway P2: 8 HTTP STT providers off the resilience trio (uniformity, bounded impact).
- Infer P2: `nn/` panic-wrapper discipline on decode loops (per-model guards exist, no shared
  invariant); `serve_codec_ar_stream` shelfware (deprecated, unbounded-egress hazard documented);
  consolidation remainder (Weights bag ×22, HF-snapshot resolver ×4, test wav-readers ×18, …).
- SDK P2: dashboard token in URL query (can now migrate to first-message auth — G3 landed).

## Verification matrix (post-fix)
| Suite | Result |
|---|---|
| Gateway workspace `--all-targets` check | clean |
| Gateway provider/realtime/livekit/baidu lib tests | 738 + 420 + 21 + 80 green |
| Gateway full lib suite | 6699/0 plain + 6867/0 featured; deterministic across 6+ runs |
| Python SDK | 415 passed |
| TypeScript SDK | 311 passed (+ tsc clean) |
| Widget | 60 passed |
| Dashboard | 245 passed (e2e opt-in) |
| Infer server/backends/components/core | 244 + 783 + 98 + 343 green |
