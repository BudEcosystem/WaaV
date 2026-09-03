# 04 — WaaV Infer SERVER crate: brutal integration / load-resilience / observability review

Scope: `crates/waav-infer-server/src/` — `engine.rs`, `ws.rs`, `lib.rs` (REST),
`codec_ar_batcher.rs`, `codec_ar_admission.rs`, `stt_coalescer.rs`, `tts_coalescer.rs`,
`cascade.rs`, `control.rs`, `ingress.rs`, `torch_sidecar.rs`, `calib.rs`, `otel.rs`, `bin/`.

Method: **static read + grep only** (no build/test/run). Confirmed = traced in code with
file:line. Suspected = inferred but a dynamic test would settle it.

Bar: super-scalable, ultra-low-latency, chaotic enterprise scale; hostile/buggy clients,
slow consumers, overload bursts, mid-stream failures.

## Counts

| Severity | Count |
|---|---|
| CRITICAL | 4 |
| HIGH | 7 |
| MED | 6 |
| LOW | 4 |
| **Total** | **21** |

**Headline verdict:** the *load-resilience* core (codec-AR admission gate, bounded
channels, watchdog spine, sidecar reaper, coalescers) is genuinely wired onto the live
path and is well-built. But the **control plane is almost entirely shelf-ware** (drain
does not drain, reconnect-storm cap never runs, slot/VRAM/canary/LoRA/migration surface
unreachable), the **live codec-AR batcher regresses TTFA to full-synthesis** and has a
**single-slow-consumer-stalls-all-streams head-of-line block**, and **observability of the
entire load-resilience layer is zero** (no 429/queue/slot/VRAM metrics; WS emits no error
metrics at all). The S2S surface is a stub. These are the production-blocking gaps.

---

## Integration-completeness table

`on-live-path?` = reachable from a real client request (REST `/v1/audio/*`, WS `/v1/ws`,
or control endpoints). `gated/shadowed?` = behind a disabled flag, duplicated, or shadowed
by an older path. `live-test?` = exercised by a non-`#[ignore]` test through the real seam.

| Component | on-live-path? | gated/shadowed? | live-test? |
|---|---|---|---|
| REST `/v1/audio/speech` (TTS) | **yes** | — | yes (server_live) |
| REST `/v1/audio/transcriptions` (STT) | **yes** | — | yes |
| WS `/v1/ws` TTS speak | **yes** | — | yes (ws_live) |
| WS `/v1/ws` STT finalize | **yes** | — | yes |
| WS S2S session | **NO** | hard-`not_implemented` (ws.rs:118) | n/a (stub) |
| `codec_ar_batcher` (live lockstep) | **yes** | only iff TTS model is codec-AR | only `#[ignore]` GPU gates + 1 stress test |
| `CodecArAdmission` (GATE #9) | **yes** (via batcher) | — | yes (unit + stress) |
| `stt_coalescer` | **yes** (every `transcribe`) | — | yes |
| `tts_coalescer` | **yes** (every one-shot `synthesize`) | — | yes |
| `serve_codec_ar_stream` (Engine, per-chunk TTFA) | **NO** (shadowed by batcher) | dead on live path when batcher present | only deterministic double tests |
| `serve_codec_ar_streams` / `_guarded` (Engine) | **NO** | not called by any handler | `#[ignore]` GPU + double |
| `ProdSpine` frame_watchdog + leak_watchdog | **yes** | — | yes (spawned-watchdog test) |
| `spawn_watchdog` thread | **yes** (bin boot) | — | yes |
| `VramAccountant` (graph-pool boot reserve) | **yes** (engine boot) | — | yes |
| `VramAccountant` (codec-AR per-stream admit) | **yes** (gate) | — | yes (stress) |
| `admit_bandwidth` (DutyLedger → admit) | **yes** (try_admit) | no-op unless DCGM present | yes (unit) |
| Calibration stamp gate (readyz/admit) | **yes** | — | yes |
| NaN/finiteness reject (H1) | partial (runtime egress only) | not at server ingress | indirect |
| barge-in (WS) | **yes** (codec-AR only) | one-shot path can't barge-in | double test |
| delta-streaming egress | **yes** structurally / **regressed** TTFA | see CRIT-2 | double test |
| Torch sidecar (Path-B) | **yes** (if `waav.json runtime=torch`) | — | yes (sh-process tests) |
| cascade STT→LLM→TTS | **CLI only** (`run-dag`) | not on any HTTP/WS route | unit + `#[ignore]` live |
| `control.apply(Drain)` | **yes** (endpoint) | **disconnected from /readyz + admission** (CRIT-1) | unit only |
| `control.apply(Load/Unload)` | **yes** (endpoint) | informational; no real cudaMalloc | unit only |
| `control.apply(SetAdmitPolicy)` canary | **yes** (endpoint) | **route_session never consulted** → no effect | unit only |
| `control.on_admit` (drain refcount) | **NO** | never called on admit | unit only |
| `control.admit_ok` (lifecycle gate) | **NO** | never gates real admission | unit only |
| `control.admit_reconnect` (J19 storm cap) | **NO** | never called on WS connect | unit only |
| `control.admit_slot` / `effective_slot_cap` (VRAM slot cap) | **NO** | unreachable | unit only |
| `control` LoRA adapters (register/bind/route) | **NO** | unreachable | unit only |
| `control.migrate` (fault/spill migration) | **NO** | unreachable | unit only |
| GW-17 trace span (`otel::turn_span`) | **yes** (WS) | **not on REST** | unit |

**Shelf-ware tally:** ~17 of `control.rs`'s 27 public methods are never reached from the
live path; the cascade, the per-chunk-TTFA single-stream codec path, and the whole S2S
surface are not reachable from any HTTP/WS route.

---

## CRITICAL

### [CRITICAL] `POST /v1/control/drain` does not actually drain — `/readyz` stays ready and admission keeps accepting
`lib.rs:618-626` (`control_drain`) · `lib.rs:253-270` (`try_admit`) · `control.rs:453-547` (`apply`)
- **What:** the drain endpoint calls only `s.control().apply(Command::Drain{..})`, which
  flips the `LifecycleFsm` to `Draining` and republishes it on a `watch` channel. It does
  **not** call `engine.begin_drain()`. `/readyz` (`health_ready`, lib.rs:416) consults
  `engine.is_ready()` (engine.rs:686) which reads `self.draining` + the calib stamp — never
  the control-plane FSM. And `try_admit` (the gate for *all* REST + WS traffic) consults
  `admit_calibrated()` + `admit_bandwidth()` + the raw semaphore — never `control().admit_ok()`.
- **Why at scale:** the orchestrator's primary lifecycle lever is broken. An operator issues
  a drain to take a replica out of rotation for a deploy/rollback; the endpoint returns
  `200 DrainStarted`, but the box keeps reporting `/readyz=200` and keeps admitting new
  streams. Rolling deploys, canary rollback, and node cordon all silently fail to stop
  traffic. The only working drain path is SIGTERM (bin/waav_infer.rs:555, which *does* call
  `begin_drain()`), i.e. you must kill the process to drain it.
- **Fix:** in `control_drain`, on a successful `DrainStarted` also call `engine.begin_drain()`
  + `mark_draining(&engine)`. Better: make `try_admit` consult `control().admit_ok()` and make
  `engine.is_ready()` honor `control().lifecycle() == Draining`, so the FSM is the single
  source of truth instead of two disconnected drain flags.

### [CRITICAL] One slow/wedged WS consumer freezes the entire shared codec-AR lockstep loop (head-of-line block, up to 10 s/chunk × all chunks)
`codec_ar_batcher.rs:358-375` (`bounded_send`) · `codec_ar_batcher.rs:253-275` (sink) · `serve.rs:810-870` (`drain_finished_stream`, single mux thread)
- **What:** the live batcher runs **one** `serve_codec_ar_multiplexed_bounded` loop on **one**
  OS thread (`codec-ar-mux`). When a slot finishes, `drain_finished_stream` decodes its whole
  body and pushes every chunk to that stream's sink synchronously *inside that single thread*.
  The sink is `bounded_send`, which on a full egress channel does `std::thread::sleep(2ms)` in
  a loop for up to `EGRESS_BACKPRESSURE_BUDGET = 10s`. During that sleep the mux thread is
  blocked, so **no other slot advances** — every other concurrent stream stalls.
- **Why at scale:** the documented guarantee is "a slow consumer… every other stream is
  untouched" (codec_ar_batcher.rs:24, :273). That is **false**. A single buggy/slow client (or
  TCP backpressure on one socket) inflicts up to 10 s × (number of chunks) of stall on *all 24
  concurrent slots*. With a worst-case wedged consumer this is a near-total throughput
  collapse triggered by one hostile peer — exactly the chaotic-enterprise failure the gate
  claims to prevent. The 10 s budget is also far too long to hold a shared thread.
- **Fix:** the per-stream egress drain must not run on the shared lockstep thread. Either
  (a) have `drain_finished_stream` enqueue the decoded PCM to a *non-blocking* bounded channel
  and let a per-stream forwarder task move it to the socket (drop/`SlowConsumer` immediately on
  full, never `sleep` on the mux thread), or (b) make `bounded_send` non-blocking (`try_send`
  → on full, mark `hung_up` and stop *this* stream) so a slow consumer is shed in O(1) without
  blocking peers. The 10 s park belongs nowhere near the shared loop.

### [CRITICAL] The live codec-AR batcher emits audio only at slot completion — TTFA regresses to full synthesis time (the "incremental TTFA" claim is false on the live path)
`serve.rs:771-781` (drain only when `s.accum.done`) · `serve.rs:810-870` (`drain_finished_stream`) · `ws.rs:300-302` / `codec_ar_batcher.rs:50` (claims)
- **What:** in the multiplexed loop, a slot's codes accumulate in `s.accum.body` across ticks;
  audio is decoded and pushed to the sink **only** in step 5 once `s.accum.done` (eos /
  truncation / cancel). There is **no mid-utterance emit for an active slot** in the mux path
  (grep: the only sink calls on the active path are inside `drain_finished_stream`). So the
  first audio byte for a batched codec-AR `speak` lands only after the *entire* utterance is
  synthesized. ws.rs:301 states "Each audio delta is forwarded to the wire AS IT IS PRODUCED
  (incremental TTFA, not the whole-utterance buffer)" — that is true of the unused
  `Engine::serve_codec_ar_stream` single-stream path, but the WS/REST handlers route through
  the batcher, which does not stream incrementally.
- **Why at scale:** TTFA is the headline latency metric for voice. Routing every codec-AR
  request through the batcher to win throughput **sacrifices the very TTFA the project is
  optimizing for** — a 1.5B AR-TTS utterance now has TTFA = full decode (hundreds of ms to
  seconds), not first-chunk. Barge-in latency is also degraded (no audio to interrupt until
  the end). The chunking that *would* give low TTFA exists in `serve_codec_ar_stream` but is
  shadowed.
- **Fix:** stream per-tick from the mux loop: after each batched `step`, decode/emit the newly
  available frames for each active slot (the incremental seam `decode_audio_stream` already
  exists and is used by the single-stream path), rather than buffering the whole body to
  `drain_finished_stream`. Or document honestly that the batcher trades TTFA for throughput and
  route latency-critical first-utterances to the single-stream path.

### [CRITICAL] WS handshake (`session.config`) text frame is bounded only by `max_binary_frame_bytes` (1 MiB), and there is no per-session text-rate / total-text bound — hostile-client memory + parse amplification
`ws.rs:42-45` (cap) · `ws.rs:67-80` (handshake) · `ws.rs:287-293` (per-`speak` cap only) · `lib.rs:60-63`
- **What:** `ws.max_message_size(max_frame)` with `max_frame = max_binary_frame_bytes` (1 MiB
  default) caps a single WS message (text or binary). The first text frame must be
  `session.config`; a client can send up to ~1 MiB of JSON, which is `serde_json::from_str`'d
  in full (ws.rs:68). For TTS, only individual `speak.text` frames are bounded
  (`max_text_bytes`=16 KiB, ws.rs:287). For STT, binary audio is bounded by
  `max_session_audio_bytes`. But there is **no bound on the number / rate of control frames**
  a session may send: a client can stream unbounded 1 MiB `session.update` / `keepalive` /
  malformed text frames, each fully buffered + JSON-parsed, resetting the idle timer forever
  (slowloris). The connection cap (64) does not help — 64 such peers is enough.
- **Why at scale:** a handful of hostile peers can pin CPU on JSON parsing of megabyte frames
  and hold connections open indefinitely (idle timer resets on every frame arrival, ws.rs:266),
  exhausting the 64-session pool with near-zero useful work. No per-session frame-rate limit,
  no total-bytes-per-session control budget, no max-control-frames.
- **Fix:** set a *separate, small* `max_message_size` for the text/control channel (e.g. 64 KiB)
  distinct from the binary audio cap; add a per-session control-frame rate limit and a total
  control-bytes budget; cap `session.config` JSON size explicitly before parsing.

---

## HIGH

### [HIGH] Reconnect-storm cap (`admit_reconnect`, J19) is never invoked on WS connect — the reconnect-flood defense is shelf-ware
`control.rs:417-424` (`admit_reconnect`) · `ws.rs:59-64` (session entry) — grep: zero callers on the live path
- **What:** `ControlPlane::admit_reconnect` (the per-replica token-bucket that stops a
  reconnect herd from re-knocking-down a recovering replica) is fully built but **never called**
  by `ws::session` or anywhere in `lib.rs`. The only WS-connect gate is `try_ws_session` (a raw
  64-permit semaphore, ws.rs:61).
- **Why at scale:** after a blip, thousands of clients reconnect simultaneously. The semaphore
  caps *concurrent* sessions but does nothing to de-correlate the *arrival storm* — every
  rejected connection retries immediately (no Retry-After de-correlation), and accepted ones
  immediately re-run the full handshake + warmup-cold path. The J19 storm control that exists
  to prevent exactly this is dead code.
- **Fix:** call `control().admit_reconnect()` at the top of `ws::session` (and ideally a
  connection-level middleware) before `try_ws_session`; on `ReconnectRateCapped`, close with the
  typed 429 + Retry-After.

### [HIGH] Drain "completion" is meaningless — `on_admit` is never called, so the FSM refcount is always 0
`control.rs:407-409` (`on_admit`) · `control.rs:457-467` (Drain reports `refcount()`) — grep: no live caller
- **What:** the FSM's drain-exits-at-refcount-0 contract depends on `on_admit()` being called
  per admitted stream and `StreamEnded` per completion. Neither is wired on the live path. So
  `refcount()` is permanently 0; `DrainStarted` always reports `live_streams: 0`, and a drain is
  considered "done" immediately even with N streams mid-synthesis.
- **Why at scale:** graceful drain that waits for in-flight work to finish does not work via the
  control plane. Combined with CRIT-1 (drain doesn't stop new traffic anyway), the entire
  drain/cordon story is non-functional through the documented control surface.
- **Fix:** call `control().on_admit()` when a stream is admitted and signal `StreamEnded` on
  completion (REST handlers + WS speak/finalize + the batcher's RAII ticket drop).

### [HIGH] Canary/rainbow `SetAdmitPolicy` has no effect — `route_session` is never consulted at admission
`lib.rs:663-671` (`control_set_policy`) · `control.rs:688-695` (`route_session`) — grep: no live caller
- **What:** `POST /v1/control/set-policy` sets `rainbow.set_fraction(..)`, but `route_session`
  (the per-new-session lane decision the fraction drives) is never called from any admission
  path. Setting a canary fraction changes nothing about which weights a session uses.
- **Why at scale:** rainbow/canary deploys (the whole point of the policy endpoint) are
  inert. An operator dials 10% canary; 0% of sessions actually route to canary weights. Silent
  no-op of a deploy-safety primitive.
- **Fix:** call `route_session` once per new session at admission and thread the resulting
  `DeployLane` to model selection; until then, the endpoint should not advertise success.

### [HIGH] The codec-AR admission gate, batcher, and both coalescers emit ZERO metrics — the entire load-resilience layer is invisible in production
`codec_ar_admission.rs` (no `metrics::`) · `codec_ar_batcher.rs` (no `metrics::`) · `stt_coalescer.rs` / `tts_coalescer.rs` (no `metrics::`)
- **What:** grep confirms zero `metrics::` calls in `codec_ar_admission.rs`,
  `codec_ar_batcher.rs`, `stt_coalescer.rs`, `tts_coalescer.rs`. There is no counter for
  load-shed/429 events, no gauge for in-flight admissions, reserved VRAM, submission-channel
  depth, egress backpressure events, `SlowConsumer` stops, or coalescer cohort sizes.
- **Why at scale:** when the server starts shedding under load you cannot see it. There is no
  signal for "we are at the concurrency cap", "VRAM admission is binding", "the queue is
  deepening", or "consumers are slow". The 429 rate — the single most important overload
  signal — is uncounted on the WS path entirely (see next finding) and on the batcher path
  always. Operators are blind to the exact failure modes GATE #9 exists to handle.
- **Fix:** instrument the gate (`admission_shed_total{reason}`, `inflight`, `vram_reserved`,
  `queue_depth`), the batcher (`submission_channel_full_total`, `egress_backpressure_total`,
  `slow_consumer_stop_total`), and the coalescers (`coalescer_cohort_size` histogram,
  `coalescer_batches_total`).

### [HIGH] The WS surface emits no error / latency / shed metrics — `waav_infer_errors_total` (the alert key) only counts REST errors
`ws.rs:157` (only `ws_sessions_total`) · `ws.rs:502` (`send_err` — no metric) · `lib.rs:1056-1058` (`err()` counts, REST only)
- **What:** `waav_infer_errors_total` is incremented only inside `err()` (the REST response
  helper, lib.rs:1058). WS errors go through `send_err`/`close_err`, which emit no metrics. The
  WS path records no TTFA, no RTF, no STT latency, no admission-reject counter — only
  `ws_sessions_total`.
- **Why at scale:** the §15.4 alert pack keys on `waav_infer_errors_total` and the latency
  histograms. A WS-only deployment (the realtime path) is effectively unmonitored: stalls,
  bad-config storms, admission rejections, and idle-timeout closes are all invisible. You cannot
  alert on WS error rate.
- **Fix:** count errors + record latency on the WS path (reuse the taxonomy label); add a WS
  TTFA / RTF histogram and a WS admission-reject counter.

### [HIGH] WS codec-AR `speak` silently drops non-barge-in control frames mid-stream (loses `clear`/`finalize`/`session.update`/`keepalive`)
`ws.rs:339-351` (the `incoming = socket.recv()` arm)
- **What:** during a codec-AR `speak`, the `select!` reads frames off the socket to detect
  `barge_in`. Any non-`barge_in` text frame (`Clear`, `Flush`, `SessionUpdate`, another `Speak`,
  `Keepalive`) hits the `_ => {}` arm or the inner non-barge-in branch and is **consumed and
  discarded** — never acked, never applied. A `Clear { context_id }` mid-speak is dropped; the
  client waits forever for `Cleared`.
- **Why at scale:** half-duplex realtime clients send `clear`/`flush`/`session.update` during
  playback. These are silently lost only while a codec-AR speak is in flight (the one-shot path
  doesn't read the socket at all, so it doesn't lose them — but also can't barge-in). The
  behavior is inconsistent and a correctness trap: a dropped `Clear` desyncs context state.
- **Fix:** route non-`barge_in` control frames to `handle_text` (or at least ack the
  barrier-style ones) instead of discarding; or document that the session is strictly one
  in-flight op and reject (not drop) frames arriving mid-op with a typed "busy" error.

### [HIGH] Double admission gating on the WS/REST codec-AR path holds two independent permits; the outer semaphore permit is held across the whole stream
`ws.rs:295-298` (outer `try_admit` permit) · `ws.rs:307-387` (batcher `submit` also admits + holds ticket) · `lib.rs:741-744` + `lib.rs:759`
- **What:** for a codec-AR `speak`/`speech`, the handler first acquires `try_admit()` (the
  `max_concurrency`=4 semaphore permit) **and** the batcher's `submit` independently runs the
  GATE #9 admission (concurrency + VRAM + deadline). The outer permit is held for the *entire*
  stream duration (dropped at ws.rs:386 / after drain in lib.rs). So effective concurrency for
  codec-AR is `min(4, MAX_SLOTS=24, gate.max_inflight)` = **4** — the small REST semaphore, not
  the 24-slot batcher, caps the live codec-AR throughput.
- **Why at scale:** the whole point of the batcher is ≥16 concurrent streams; the outer
  semaphore (default 4) silently throttles it to 4, and the two gates can disagree (outer admits,
  inner sheds, or vice-versa) producing confusing mixed signals. Two RAII lifetimes for one
  stream is also a footgun.
- **Fix:** for the codec-AR path, do **not** take the outer `try_admit` permit — let the
  batcher's GATE #9 be the single gate (it is strictly richer). Keep the outer semaphore only for
  the one-shot/STT paths. Or size the semaphore to the batcher cap.

---

## MED

### [MED] Cascade (STT→LLM→TTS) is CLI-only and has no admission gate, no per-stage deadline, and a fixed 120 s egress timeout — not production-reachable, not load-safe if wired
`cascade.rs:96-190` · `bin/waav_infer.rs:399-493` (`run_dag_once`)
- **What:** `run_cascade` is reachable only from the `run-dag` CLI subcommand; no HTTP/WS route
  exposes it. It spawns three OS threads per call, has a single fixed `recv(120s)` egress
  deadline (cascade.rs:171), no per-stage deadline, and no admission/concurrency bound. The LLM
  stage is a hardcoded deterministic `llm_reply` stub (cascade.rs:74) — there is no real LLM
  dependency, error propagation across a real LLM, or timeout per stage.
- **Why at scale:** the heterogeneous cascade is presented as a delivered capability but is not
  a serving feature. If it were exposed without an admission gate, each request would spawn 3
  threads with a 120 s blocking deadline — trivially DoS-able. The "G11 accept" is a CLI
  benchmark, not a live endpoint.
- **Fix:** if cascade is meant to be live, add an HTTP/WS route behind `try_admit`, per-stage
  deadlines, and bounded thread/worker reuse; otherwise document it as a CLI/bench-only path.

### [MED] `admit_bandwidth` (the measured DutyLedger → admission) is a no-op on every box without a DCGM exporter, including all current deployments
`engine.rs:708-729` · `engine.rs:786-837` (calibrate) · `lib.rs:262`
- **What:** `admit_bandwidth` returns `Ok(())` whenever `bandwidth_profile == None`, which is
  the case unless `dcgmi dmon -e 1005` succeeds (DRAM_ACTIVE is off by default on most boxes).
  So the deadline-graded bus-saturation shed — sold as the GATE #9/#10 latency-explosion guard —
  is inert in practice and falls back to "the conservative roofline" which is *also* not wired
  into the live admit (only the codec-AR gate's own deadline/VRAM bounds remain).
- **Why at scale:** the headline "deadline-graded bandwidth admission" protection is effectively
  absent on real hardware until someone enables the DCGM profiling counter. The codec-AR gate's
  fixed `RATED_STREAM_SERVE_SECS = 0.5` deadline projection (codec_ar_admission.rs:47) is the
  *only* real latency-shed, and it is a hardcoded guess, not measured.
- **Fix:** make DCGM enablement part of boot (or fail readiness if the bus profile cannot be
  measured and the operator requested bandwidth admission); document that without it the latency
  guard is the fixed-constant projection only.

### [MED] `slot_kv` admission (`admit_slot` / `effective_slot_cap`, M4.5-T5) is never used — codec-AR concurrency is not VRAM-capacity-bounded against the box weights
`control.rs:584-621` · `lib.rs:184-188` (control plane built with `bytes_per_slot = 0`)
- **What:** `AppState::new` constructs the control plane via `new_ready` (lib.rs:184), which sets
  `bytes_per_slot = 0`, so `effective_slot_cap`/`admit_slot`/`vram_backed_slots` are all no-ops
  (`u32::MAX`). The richer per-slot-KV VRAM admission is unreachable. The codec-AR gate's own
  VRAM check uses a *separate* `CodecArAdmission` budget (codec_ar_admission.rs:36,
  `DEFAULT_BYTES_PER_STREAM = 256 MiB`) that is **independent of the control plane's box ledger**
  and of the resident weights.
- **Why at scale:** two separate VRAM accountants (the control-plane box ledger + the codec-AR
  gate's own `vram_reserved`) track overlapping budgets that never reconcile. The gate can admit
  up to `max_inflight × 256 MiB` regardless of what the resident weights + graph pool already
  consumed from the box budget — a path to over-commit on a multi-model box.
- **Fix:** have the codec-AR gate reserve against the *same* box `VramAccountant` the control
  plane and engine graph-pool use (one ledger), or wire the control plane with a real
  `bytes_per_slot` and route admission through `admit_slot`.

### [MED] STT/TTS coalescers use **unbounded** mpsc queues — no load-shed, no backpressure (asymmetric with the codec-AR gate)
`stt_coalescer.rs:46,65` (`mpsc::unbounded_channel`) · `tts_coalescer.rs:51,70` (`unbounded_channel`)
- **What:** both coalescers submit jobs over `mpsc::unbounded_channel`. The cohort *width* is
  bounded (`MAX_BATCH = 24`), but the *queue* of pending jobs is unbounded. Under a burst of
  concurrent `transcribe`/`synthesize` (one-shot models, or STT), the queue grows without limit;
  there is no 429 shed and no deadline projection — unlike the codec-AR gate.
- **Why at scale:** for one-shot TTS models (kokoro/melo/supertonic) and *all* STT, the GATE #9
  protections do not apply — only the outer `max_concurrency`=4 semaphore (which is held across
  the whole call) gates them. A flood of STT finalizes or one-shot TTS speaks past the semaphore
  wait will pile unbounded jobs in the coalescer queue (each holding a `Vec<f32>` clip / text),
  bounded in practice only by the 4-permit semaphore + 2 s admission wait. The asymmetry means
  "load resilience" is real only for codec-AR.
- **Fix:** bound the coalescer submit channels and shed (or rely on the outer semaphore being the
  true bound and document it); add a queue-depth metric so the unboundedness is at least visible.

### [MED] Per-model latency / RTF metrics hardcode `"model" => "kokoro"` / `"whisper"` and the codec-AR batched path records none
`lib.rs:817-821` (`"model" => "kokoro"` literal) · `lib.rs:971` (`"whisper"` literal) · batcher path records no histogram
- **What:** the REST `speech` handler records `waav_infer_ttfa_seconds` / `waav_infer_stream_rtf`
  with a hardcoded `"model" => "kokoro"` label regardless of the actually-loaded TTS arch
  (chatterbox, supertonic, …). `transcriptions` hardcodes `"whisper"`. The codec-AR batched
  drain path in `speech` (lib.rs:751-793) does record TTFA/RTF — but still mislabeled "kokoro" —
  and the *WS* codec-AR path records nothing.
- **Why at scale:** per-model latency dashboards are wrong (every model's latency attributed to
  "kokoro"/"whisper"). Mixed-model or non-kokoro deployments have no correct per-model latency
  signal; the codec-AR WS path has none at all.
- **Fix:** label with `s.engine.model_id()` / `stt_model_id()`; record TTFA/RTF on the WS path.

### [MED] `record_audio_seconds` and the WS/batcher meta use `duration_ms = 0`, and `audio_seconds_total` double-counts micros under concurrency-rounding
`codec_ar_batcher.rs:259` (`ChunkMeta::pcm16(.., 0, ..)`) · `lib.rs:991-1001` (`record_audio_seconds`)
- **What:** the batcher stamps every chunk meta with `duration_ms = 0` (codec_ar_batcher.rs:259);
  the actual duration is recomputed downstream, but a consumer reading `chunk_meta.duration_ms`
  gets 0. Separately, `record_audio_seconds` does a non-atomic read-modify (fetch_add then
  recompute whole-second boundary) that is correct for a single counter but the `prev` used for
  the boundary calc is the pre-add value from *this* thread's fetch_add — under heavy concurrency
  the whole-second attribution can drift (minor; the total is preserved by the atomic add).
- **Why at scale:** clients that pace playback off `duration_ms` get 0 from the batched path
  (forcing them to recompute); accounting/billing reading `audio_seconds_total` is approximately
  but not exactly partitioned across concurrent requests. Low blast radius but a correctness
  smell on a billing-adjacent counter.
- **Fix:** stamp the real `duration_ms` in the batcher sink (it has `sr` + sample count); the
  audio-seconds rounding is acceptable but worth a comment that the whole-second split is
  best-effort under concurrency.

---

## LOW

### [LOW] GW-17 distributed-trace span is opened only on the WS path; REST requests get no per-turn trace
`ws.rs:145` (`turn_span`) · `lib.rs` (no `turn_span` in `speech`/`transcriptions`) · `otel.rs:26`
- **What:** `otel::turn_span` (the engine side of the gateway trace) is created only in
  `ws::session`. REST `/v1/audio/speech` and `/transcriptions` rely on `TraceLayer` (HTTP span)
  but never parent under a propagated gateway `trace_id` (there is no field to carry it in the
  REST request types). So REST turns are not grouped under the gateway trace.
- **Fix:** accept a trace header on REST and open a `turn_span` there too, or document that
  distributed tracing is WS-only.

### [LOW] `transcriptions` multipart parses fields with no per-field cap and `field.text()`/`field.bytes()` swallow errors via `unwrap_or_default()`
`lib.rs:888-919`
- **What:** only the 100 MiB `DefaultBodyLimit` bounds the whole multipart; individual non-file
  fields (`response_format`, `model`, drained `_`) are read with `field.text()/.bytes()` and
  `unwrap_or_default()` on error (lib.rs:903,906), silently treating a read error as empty. A
  malicious form with many small fields is bounded only by the 100 MiB total.
- **Fix:** cap individual field sizes; surface field-read errors as bad_config rather than
  defaulting.

### [LOW] `ingress::resample_linear` builds the full output `Vec` for a 100 MiB upload before transcription — large transient allocation on the decode thread
`ingress.rs:126-148` · `lib.rs:947-951` (decode on `spawn_blocking`)
- **What:** decode + resample of a 100 MiB upload materializes the full mono f32 buffer
  (`mono.push` per sample) and then a full resampled `Vec` — a ~hundreds-of-MB transient on the
  blocking pool. The admission permit is held across it (good), but there is no streaming/chunked
  decode, so peak memory per STT request is large.
- **Fix:** chunked/streaming decode for large uploads; or lower `max_upload_bytes` default if
  100 MiB transients are unacceptable on a shared-memory box.

### [LOW] `hv()` swallows invalid header values to the literal string `"invalid"` — a malformed model id / content-type silently corrupts response headers
`lib.rs:1013-1015`
- **What:** `hv` returns `HeaderValue::from_static("invalid")` on any non-ASCII/control value
  rather than failing or sanitizing. A model id or sample-rate string that isn't a valid header
  value yields a header literally reading `invalid`.
- **Fix:** sanitize/percent-encode, or omit the header rather than emit a misleading sentinel.

---

## Notes on what is solid (so the fixes don't break it)

- **Codec-AR admission gate** (`codec_ar_admission.rs`) is correct: O(1) CAS-loop concurrency
  bound, VRAM headroom check, deadline projection, RAII ticket, shed-consumes-nothing — and the
  400-request spike stress test (`codec_ar_batcher.rs:983`) proves bounded counters + RSS +
  bit-identity of admitted streams. No lock held across an await; no deadlock in the gate itself.
- **Bounded submission channel** (`SUBMISSION_CHANNEL_DEPTH=256`, `try_send`→shed) and the
  in-loop `max_pending` bound (`serve_codec_ar_multiplexed_bounded`) genuinely close the
  unbounded-admit trap (modulo CRIT-2/3 on the egress side).
- **Torch sidecar** (`torch_sidecar.rs`) is robust: `authorize_serve` gate before any pipe touch,
  the bounded-read reaper thread (`read_frame_bounded`) returns within the deadline *unconditionally*
  (does not depend on the kill unblocking the read), mark-dead fan-out, RAII child reap on Drop,
  bounded boot read. The real `/bin/sh` wedge tests exercise the actual hang surface. A sidecar
  crash fails the request typed; it does not hang or take down the worker.
- **Poison recovery** is consistent: every `Mutex` lock uses `unwrap_or_else(|p| p.into_inner())`,
  so one panicked request cannot permanently brick synthesis/transcription.
- **Calibration stamp gate** (`calib.rs`) is sound: monotonic-time-in, fail-closed on backwards
  clock, content-addressed cache hit, type-level `WarmupComplete` witness gating the readiness
  flip — the ready-then-crash-loop is genuinely unrepresentable.
- **Watchdog spine** is installed at boot (`bin/waav_infer.rs:540`) and the serve loop registers
  every slot with both watchdogs (`serve.rs:663-700`); the out-of-band thread sheds silently-hung
  slots and publishes the leak gauge.
- The REST error envelope (`err()`, typed code→HTTP status + Retry-After), `ApiJson` typed
  rejection, per-route body caps, and the non-loopback-without-auth boot refusal are all correct.

## Top systemic theme

The **data-plane load-resilience** (codec-AR gate, bounded channels, watchdogs, sidecar reaper)
is real and well-tested. The **control-plane and observability** are the rot: an enormous,
carefully-built control surface (`control.rs`, 27 methods) is wired to HTTP for 4 commands and
otherwise **never touched by the live path** — drain doesn't drain, reconnect-storm cap never
runs, canary routing has no effect, slot/VRAM/LoRA/migration are unreachable — and the load-
resilience layer it *does* run is **completely uninstrumented** (no 429/queue/slot/VRAM metrics,
no WS error metrics). Combined with the batched-path TTFA regression and the single-consumer
head-of-line block, the system is throughput-correct under load but **operationally blind and
not actually drainable/cordonable** at the chaotic-enterprise scale it targets.
