# WaaV Infer — Production-Readiness Audit

**Scope:** `/home/bud/ditto/waav/waav-infer` workspace (`crates/waav-infer-*`), branch `waav-infer-v2-build`, HEAD `946996d`.
**Date:** 2026-06-28. **Method:** code-read of the serve hot path + deterministic build/clippy/test gates. **No GPU model tests run** (avoided GPU contention). **No code changes applied** (see "Why no auto-fix" at the end).

---

## 0. Deterministic gate status — ALL GREEN

| Gate | Command | Result |
|---|---|---|
| Build (default) | `cargo build --workspace` | **OK** (5.5s) |
| Build (torch) | `cargo build --workspace --features torch` | **OK** |
| Clippy (torch, all-targets, `-D warnings`) | `cargo clippy --workspace --all-targets --features torch -- -D warnings` | **CLEAN — 0 warnings** |
| Tests (deterministic lib) | `cargo test --workspace --lib -- --test-threads=1` | **1181 passed / 0 failed / 29 ignored** across 16 crates |

Per-crate test highlights: backend-torch 237, runtime 240, scheduler 145, core 94 (162s — heavy CPU numerics, not GPU), backend-api 87, server 69. The 29 `ignored` are the heavy/live GPU tests (correctly gated). No compile errors, no flaky failures at `--test-threads=1`.

**A green build is not production-ready** — the findings below are what the gates do not catch.

---

## Verdict in one paragraph

The **serve hot path is genuinely well-hardened**: the lockstep serve loop is fully typed-error + RAII-contained (no unguarded panics), the per-tick `step_batch` failure is contained per-stream (the known serve-loop-crash fix **holds** — `serve.rs:924-934`), admission is bounded on 4 legs with RAII tickets, the J16 out-of-band watchdog is live and logged, and model/tenant locks are poison-tolerant. The **real residual risk is memory accounting**: the admission gate that is supposed to prevent the unified-memory OOM (which has hard-crashed the box twice) does **not** model the two largest consumers of that memory (the ORT arena and per-request torch vocoder transients) — OOM-safety today is incidental (serialization + headroom), not enforced. That is the one P0. Everything else is observability polish and latent-trap hygiene.

---

## P0 — must fix before production

### P0-1 · The admission "VRAM headroom" gate does not account the dominant unified-memory consumers
**Files:** `crates/waav-infer-server/src/codec_ar_admission.rs:52` (`DEFAULT_BYTES_PER_STREAM = 256 MiB`), `:392-417` (the VRAM leg); `crates/waav-infer-backend-ort/src/ep.rs:25` (`GB10_ARENA_LIMIT_BYTES = 48 GiB`); `crates/waav-infer-backend-torch/src/device.rs:164` and peers (`free_mem: None`); vocoder decode paths `dots.rs` (BigVGAN whole-body), `indextts2_backhalf.rs`, `cfm/vocoder.rs`; budget source `control.rs:44-54` (`GB10_VRAM_CAP_BYTES = 96 GiB`, env `WAAV_VRAM_CAP_BYTES`).

**Risk.** The codec-AR gate's VRAM leg reserves only `256 MiB`/stream (a per-slot **KV proxy**) against a 96 GiB box cap, so 24 slots reserve ~6 GiB. But the two real consumers of the **shared 121 GB unified pool** are invisible to this accountant:
- the **ORT CUDA arena**, capped at **48 GiB** (`ep.rs:25`) — drawn from the same pool, never subtracted from the gate's `peak_cap`;
- **per-request torch vocoder transients** — the S3Gen / BigVGAN whole-body decode (the memory note records a ~**21.7 GiB** S3Gen transient as a "contained residual"), allocated by libtorch's caching allocator, with **no pre-allocation budget check** and **no free-memory query** anywhere (`free_mem: None` across `device.rs`/`dia2.rs`/`higgs.rs`/...).

OOM-safety today rests entirely on two **incidental** facts the gate does not enforce: (a) torch forwards are serialized (one shared `codec-ar-mux` thread + the model `Arc<Mutex>`), so only **one** transient exists at a time — the OOM survey's "16 concurrent vocoder decodes = 347 GiB" is **not** reachable on the live path; and (b) `96 GiB cap` under a `121 GB` pool leaves ~25 GiB OS headroom. Any one of these silently re-arms the box-crash: raising `WAAV_CODEC_AR_MAX_SLOTS`, raising `WAAV_VRAM_CAP_BYTES`, co-loading a second model, or onboarding a vocoder with a larger transient. Given the box has hard-crashed **twice** from exactly this class, an un-enforced invariant is a P0.

**Proposed fix (no behaviour change for the happy path):**
1. At model load, compute and subtract from the gate's effective `peak_cap`: resident weights (already gated by `VramAccountant` at load) **+ the 48 GiB ORT arena reservation + a measured per-model max-transient reserve**. Assert `weights + arena + max_transient ≤ box_cap` at boot and refuse to serve (typed) if it fails.
2. Wire a real free-memory query (`cudaMemGetInfo`) to replace `free_mem: None`, and add a **hard pre-decode guard** in `drain_finished_stream` / the vocoder seam: if the projected transient exceeds current free memory, shed the stream as a typed `StallTimeout`/`AdmissionRejected` instead of allocating.
3. Document the serialization invariant (one transient at a time) as a **load-resilience requirement**, so a future "parallel vocoder decode" optimization cannot land without re-introducing the budget.

---

## P1 — should fix

### P1-1 · Failure paths are metric-only — an operator tailing logs is blind to shedding/OOM-reject/quarantine
**Files (all metric-only, no `tracing`):** admission sheds `codec_ar_admission.rs:325` (concurrency), `:356` (tenant), `:381` (deadline), `:403` (vram); serve-deadline shed `runtime/serve.rs:823`; submit-queue shed `codec_ar_batcher.rs:264`; decode-crash quarantine report `codec_ar_batcher.rs:355`; global error counter `lib.rs:1280` (REST) & `ws.rs:769` (WS). Confirmed: `codec_ar_admission.rs` contains **zero** `tracing::` calls.

**Risk + nuance (harsh-auditor).** These are the exact events an operator needs at 3am, and there is **no log line** for any of them — only a Prometheus counter. *However*, per-event logs would **flood** under precisely the load you are shedding, so metric-only is *partly* the correct design. The real gap is the absence of an **edge/transition** log: nothing fires when the box first **enters** a shedding/OOM-reject/quarantine state. The well-instrumented rare events are the model to copy — `lib.rs:437` (watchdog trip), `:450` (leak), `:462` (sidecar death) each pair a counter with a `tracing::warn!`.

**Proposed fix.** Add an **edge-triggered** `tracing::warn!` (first shed after a clean window, or a once-per-N-seconds aggregate `"codec-AR gate shedding {n}/s reason={reason}"`) at the admission gate and the serve-deadline shed. Do **not** add a per-event log (that is why it isn't there). Promote the decode-crash quarantine report to `warn!` with the model + input signature (it is rare and security-relevant).

### P1-2 · CUDA-graph capture-fallback telemetry is collected but never exported
**File:** `crates/waav-infer-runtime/src/graph_fallback.rs` (`FallbackTelemetry` — counters per `NonCapturableReason`, distinct-pinned-key gauge).

**Risk.** Graph-capture failure silently pins a key to **eager** execution — a real, invisible perf cliff. The telemetry struct exists but is never published to `/metrics`, so an operator cannot see capture failing or how many keys fell back.

**Proposed fix.** Emit `waav_infer_graph_capture_failures_total{reason=…}` + `waav_infer_graph_fallback_pinned_keys` from the fallback recording site (the server already owns the Prometheus recorder; the runtime can use the `metrics` facade like `serve.rs` already does).

### P1-3 · Model-setup thread spawn panics instead of failing typed
**File:** `crates/waav-infer-server/src/codec_ar_batcher.rs:454` — `.expect("spawn codec-ar shared loop thread")`.

**Risk.** A `std::thread` spawn failure (thread/fd exhaustion under load) **panics** the model-setup task rather than returning a typed error — converting a recoverable resource-pressure condition into a crash of the setup path. It is one-time at model load (not per-request), which is why it is P1 not P0.

**Proposed fix.** Map the `Builder::spawn` `Result` to `InferError::internal("could not spawn codec-AR loop thread")` and fail the load cleanly.

---

## P2 — nice to have / latent traps

### P2-1 · `Engine::serve_codec_ar_stream` uses an UNBOUNDED per-request channel (latent OOM trap)
**File:** `crates/waav-infer-server/src/engine.rs:2177` (`tokio::sync::mpsc::unbounded_channel::<StreamItem>()`).
**Status corrected:** the OOM survey flagged this P0, but it is **test/bench-only** today — the live WS/REST codec-AR path uses the **bounded** batcher (`ws.rs:389-399 → batcher.submit`, whose `bounded_send` caps at `EGRESS_CHANNEL_DEPTH` and sheds `Full`). The only non-test callers are `engine.rs:2969/3006` (unit tests) and `perf_bench.rs`. **Risk:** it is `pub`; a future re-wire reintroduces the documented "unbounded_channel flood-to-OOM trap" (`codec_ar_batcher.rs:289` comment). **Fix:** gate it `#[cfg(test)]` or convert to a bounded channel for symmetry.

### P2-2 · `/livez` is a static 200 (does not reflect a wedged GPU/model)
**File:** `lib.rs:521`. By FR-O1 design `/livez` is process-liveness only; `/readyz` (`lib.rs:525-534`) is the **dynamic** gate (draining + calibration-stamp + admit). **Risk:** a load balancer probing `/livez` may route to an alive-but-unserviceable replica. **Fix:** none in code — document in the deploy runbook that **orchestrator routing must probe `/readyz`**.

### P2-3 · Dev-env path coupling (CI/deploy portability, not a binary defect)
**Files:** `gb10-env.sh` / `gb10-env-212.sh` hardcode `/home/bud/...` (`ORT_DYLIB_PATH`, the torch-2.12 venv); `sentencepiece.rs:363` hardcodes `/home/bud/.cache/...`.
**Status corrected:** the config survey flagged the sentencepiece path as a "serving-binary deployment blocker" — it is **not**; line 363 is inside `#[cfg(test)] mod tests` (test fixture). **There are no hardcoded `/home` paths in any serving binary** (verified). The env scripts are user-specific (dev only). **Fix:** derive paths from `$VIRTUAL_ENV`/`$CUDA_HOME`/the hf-hub cache for portable CI/deploy.

### P2-4 · Coalescer job channels are unbounded (bounded only indirectly)
**Files:** `tts_coalescer.rs:70`, `stt_coalescer.rs:65` (`mpsc::unbounded_channel::<Job>()`). Bounded **indirectly** by the outer `max_concurrency` admission semaphore (a submit holds a permit across its await), so in-flight jobs ≤ permits. Acceptable today; **fix:** make the bound explicit (bounded channel sized to `max_concurrency`) for defense-in-depth.

---

## What is genuinely production-grade (verified, not assumed)

- **Serve loop panic-safety.** No unguarded hot-path panics in `serve.rs` / `driver.rs` / `engine.rs` / `control.rs`. The three `.expect()` in the mux loop (`serve.rs:821/927/961`) are each guarded by a `matches!`/`Some(...)` on the **same cell in the same iteration** — unreachable. Driver-contract violations are typed errors, never panics (`serve.rs:333/909/1296`).
- **Tick-error containment holds.** A per-tick `step_batch`/device-KV failure closes only the in-flight cohort's streams (typed `Error` terminal) and `continue`s the shared loop (`serve.rs:924-934`) — the known "one bad tick kills the mux thread → every later request 500s" regression is fixed and the fix is intact.
- **Serve-loop no-wedge.** Per-slot serve deadline (`SERVE_DEADLINE_DEFAULT = 300s`, env `WAAV_CODEC_AR_SERVE_DEADLINE_SECS`) force-sheds a held slot between ticks (`serve.rs:802-844`); the in-loop `pending` queue is hard-bounded (`DEFAULT_MAX_PENDING_ADMISSIONS = 256`) with typed overflow shed.
- **Admission gate.** 4 legs (priority-banded concurrency, per-tenant fair-share, deadline-viability, VRAM-reserve) all CAS-bounded with **RAII tickets** that release on drop; every refusal is a typed `429/AdmissionRejected` with retry-after — never a hang or unbounded admit (`codec_ar_admission.rs:304-429`). (The VRAM **leg's accounting** is the P0 above; the *mechanism* is sound.)
- **J16 silent-hang defense.** `spawn_watchdog` (`lib.rs:421-479`) is a live out-of-band 1s poller: frame-progress trip + leak reconcile + sidecar reap + quarantine eviction, each with **both** a metric and a `tracing::warn!`.
- **Poison-tolerant locking.** The request-critical locks use `lock().unwrap_or_else(|p| p.into_inner())` (`engine.rs:2137/2182`, `codec_ar_admission.rs:351`) — a poisoned mutex does not cascade into a serve-path panic. (Bare `.lock().unwrap()` exists only in test instrumentation and a load-time engine-pool race.)
- **Graceful shutdown.** `with_graceful_shutdown` + SIGTERM/SIGINT flips readiness to 503 (`bin/waav_infer.rs:642-647`, `:674`); `process::exit(0)` teardown avoids the ORT/torch destructor hang.
- **ORT arena.** Capped (48 GiB GB10 / 90% elsewhere) with `ArenaExtendStrategy::SameAsRequested` anti-fragmentation + `WAAV_ORT_GPU_MEM_LIMIT_BYTES` override (`ep.rs:25-50,224-243`) — overrun fails as typed CUDA OOM, not a box crash.
- **KV bounds.** KV-length firewall (`scheduler/admission.rs`), fixed-size ring KV (`ring_kv.rs`), paged-KV long-form escape with exact sink-pinning (`runtime/paged_kv.rs`) — all bounded + typed-reject.
- **Config robustness.** Every `WAAV_*` env var (`CODEC_AR_MAX_SLOTS`, `CODEC_AR_SERVE_DEADLINE_SECS`, `VRAM_CAP_BYTES`, `PERF_MODE`, `*_PRECISION`, reconnect caps) parses with graceful fallback — invalid values default, never panic. **No hardcoded secrets** anywhere. TRT runtime self-load (`trt.rs:72`, `ensure_runtime_loaded`) and manifest load (`core/model.rs:112`, `waav.json`) both return typed errors (missing manifest → `Ok(default)`), never panic.

---

## Why no auto-fix was applied

The build, clippy (`-D warnings`), and the 1181-test suite are all green — there is **no lint or compile fix to make**. The one tempting "small safe fix" (add `tracing::warn!` to the admission sheds, P1-1) is **not** safe as written: a naive per-shed log floods under the exact overload it reports, so the correct fix is edge-triggered/aggregated logging — a design change, not a one-liner. Per the harsh-auditor mandate, that is **proposed, not applied**. All P0/P1/P2 items above are proposals for the owning effort.

---

## Top-3 to action

1. **P0-1** — make the codec-AR admission gate account the ORT arena + per-request vocoder transient (or add a hard pre-decode free-memory guard). This is the only thing standing between a config tweak and a third box hard-crash.
2. **P1-1** — add edge-triggered shed/OOM-reject/quarantine logs so an operator is not blind without a Prometheus scrape.
3. **P1-2** — export the CUDA-graph capture-fallback telemetry to `/metrics` (silent perf cliff today).
