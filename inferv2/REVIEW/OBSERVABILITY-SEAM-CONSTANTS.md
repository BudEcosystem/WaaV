# Observability: the Gateway→Infer trace seam + the magic-constant fixes

Two focused accuracy/integration fixes for the WaaV observability system (OBSERVABILITY-DESIGN.md §1
gaps 4 + 5). No stubs. Control-plane / telemetry only — the serve path's codec numerics are untouched
(no byte-identity impact).

---

## TASK A — the Gateway→Infer trace seam (the "end-to-end" link)

Before this change a turn's trace **never spanned both halves**: the Infer engine fully implemented the
*receiving* half (parse `traceparent` → parent the turn span), but the live gateway **injected nothing**,
and the engine's L2 stage spans (prefill|decode|codec) were created on a `spawn_blocking` worker that
loses the contextual span — so even an injected turn trace would not reach them.

### A.0 — wire format (foundational): `TraceContext` now serializes as its canonical W3C traceparent STRING
`waav-infer/crates/waav-infer-protocol/src/trace.rs` — replaced the derived `Serialize`/`Deserialize`
(which emitted an opaque `{trace_id:[..],span_id:[..],sampled}` byte-array object) with **hand-written
impls** that (de)serialize the canonical `00-<trace32>-<span16>-01` string. This:
- reconciles the code with its own module docs ("on the wire it is the canonical traceparent string"),
- lets the live gateway inject a **plain interoperable string** into `session.config` with **no
  cross-workspace dependency** on `waav-infer-protocol`,
- keeps both Infer-side consumers (engine + provider) consistent automatically (same struct),
- a malformed string is a typed serde error, never a panic (the gateway's bad forward degrades to
  no-trace). Test: `trace::tests::serde_round_trips_as_traceparent_string`.

### A.1 — Gateway INJECTING half (was absent)
| step | file:line | what |
|---|---|---|
| mint/propagate the W3C traceparent | `WaaV/gateway/src/middleware/request_id.rs` `mint_traceparent()` + `is_w3c_traceparent()` | reuse the inbound request's trace id when it is valid 32-hex non-zero (the middleware already derives it from an inbound `traceparent`), else mint a fresh 128-bit id; always a fresh non-zero 64-bit span id; validated shape |
| capture it once per connection | `WaaV/gateway/src/handlers/realtime/handler.rs:63` `realtime_handler` | extracts `Extension<RequestId>`, calls `mint_traceparent(inbound)`, threads the result into `handle_realtime_socket` |
| thread → the config builder | `handler.rs` `handle_realtime_socket` → `process_realtime_message` → `handle_realtime_incoming` → `handle_config` | a `trace_parent: &str` param down the realtime message chain |
| set it on the session config | `handler.rs` `handle_config` (right after the endpoint-override injection) | `realtime_config.trace = Some(trace_parent)` when well-formed |
| carry it on `RealtimeConfig` | `WaaV/gateway/src/core/realtime/base.rs` `RealtimeConfig.trace: Option<String>` | new server-set field (`skip_serializing_if=None`); other providers ignore it |
| **inject on the Infer handshake** | `WaaV/gateway/src/core/realtime/infer/protocol.rs` `session_config()` + `connect_spec()` | adds `sc["trace"] = <traceparent>` on the `session.config` frame (the engine's primary read) **and** a `traceparent` connect header; only a **validated** value is injected (a malformed one would fail the engine's typed `trace` deserialize); untraced ⇒ neither, byte-unchanged |

Proof (gateway half): `infer_protocol_injects_propagated_traceparent` — a propagated traceparent rides
**both** the `session.config` `trace` field (carrying trace id X) and the connect header; untraced and
malformed configs inject neither.

### A.2 — Infer plumb: `SessionConfig.trace` → `ServeSpine.trace` → the L2 stage spans
The profiler agent left `ServeSpine.trace = None` pending this plumb.
- `waav-infer/crates/waav-infer-server/src/engine.rs` `serve_codec_ar_streams_guarded(..)` gained a
  `trace: Option<TraceContext>` param and threads it into `ServeSpine.trace` (replacing the hardcoded
  `None`). The serve loop runs on a `spawn_blocking` worker that does **not** inherit the async task's
  contextual `otel::turn_span`, so this **explicit** trace is the only thing that carries the turn id into
  the `prefill`/`decode`/`codec` stage spans (`serve.rs::stage_span`, already wired by the profiler agent).
- The live admission gate already builds the gate per loaded model in `lib.rs`; the WS handshake parses
  `cfg.trace` into the typed `SessionConfig::trace` (`ws.rs:166` → `otel::turn_span`, unchanged).

Proof (infer half, no GPU): `engine::tests::guarded_serve_threads_session_trace_into_stage_spans` —
builds the **exact wire frame the gateway injects** (`{"type":"session.config", …, "trace":"00-X-…-01"}`),
deserializes it into `SessionConfig`, extracts `cfg.trace`, feeds it through
`serve_codec_ar_streams_guarded`, and asserts the `prefill`/`decode`/`codec` stage spans all carry trace
id X. (Runtime-level twin already existed: `serve::tests::serve_loop_emits_stage_spans_under_the_turn_trace`.)

### A.3 — the one-trace-spans-both proof + the join
A full live gateway↔infer handshake in one process is not runnable in the default suite (it needs a booted
gateway binary + a GB10 Infer engine; the existing **live** `ws_live::ws_live_traced_turn_spans_gateway_and_engine`,
GB10-gated, already proves the turn-span level join). So each half is proven independently and the join is
mechanical and exact:

- Gateway emits trace id **X**: `infer_protocol_injects_propagated_traceparent` shows the gateway puts
  `"trace":"00-X-…-01"` on the very `session.config` frame the Infer WS handshake reads.
- Infer, fed traceparent **X**, emits stage spans under **X**: `guarded_serve_threads_session_trace_into_stage_spans`
  deserializes that identical wire form and shows X on every L2 stage span.
- **The join:** both halves carry the SAME canonical `traceparent` string on the SAME `session.config.trace`
  field — the gateway writes it, the engine reads it into `SessionConfig::trace`, runs the whole turn inside
  `otel::turn_span(cfg.trace)` (L1) AND threads `cfg.trace` into the serve loop's stage spans (L2). One
  trace id therefore covers handshake → STT/LLM/TTS → intra-Infer prefill/decode/codec.

Honest gap: the **live WS codec-AR path** runs through the *multiplexed* batcher (`MuxSpine`), which serves
N sessions in one cohort = **N traces**, so a single `ServeSpine.trace` does not apply there by design;
its turn trace is still carried at L1 by `otel::turn_span`. The single-batch *guarded* path (the only
`ServeSpine` builder) is where the L2 single-trace roll-up is realized and proven. Wiring per-stream traces
into the multiplexed loop (and adding stage spans there) is a larger, separate change.

---

## TASK B — magic-constant fixes (accuracy / honest labeling)

| # | constant (was) | site | now |
|---|---|---|---|
| B1 | `compute_secs: 0.010 / 0.005` (claimed "warmup measured", was hardcoded) | `engine.rs` `calibrate_bandwidth_profile` | **derived from a real warmup measurement.** `Engine::warmup` now warms a codec-AR model through the REAL lockstep serve path and records the measured per-tick step time `wall / strides` (= `T_step` exactly); STT records its measured RTF. The co-load stage `compute_secs` = the TTS measured step (clamped to ≤ `T_f`), or `stt_rtf × T_f` for STT. A path warmup couldn't measure falls back to the old value **explicitly labeled a fallback** (never "measured"). New `WarmupCost` struct + `warmup_cost` field. Bonus: the stepped warmup also actually warms the codec-AR graph (the one-shot `synthesize` returned empty for a stepped model, so it was previously never warmed). |
| B2 | `RATED_STREAM_SERVE_SECS = 0.5` (claimed "derived from Ceilings", was hardcoded) | `codec_ar_admission.rs` | **genuinely derived:** `rated_serve_secs = ceilings.tick_secs() × RATED_STRIDES_PER_STREAM` (stored in `Inner`, used by the deadline projection). `RATED_STRIDES_PER_STREAM = 12.5` is the **explicit, documented assumption** (a typical short utterance's stride count). At the rated 40 ms tick this reproduces the historical 0.5 s; with a per-model 80 ms `T_f` it correctly scales to 1.0 s. |
| B3 | `one_frame_ms() = 100` vs rated `T_f = 40 ms` (mismatch) | `codec_ar_admission.rs` | **reconciled + derived:** `one_frame_ms(&ceilings) = round(tick_secs × 1000).max(1)` — the retry-after now equals the SAME `T_f` the duty model rates against (40 ms at the rated tick, 80 ms for a Mimi-class codec), instead of a stray 100 ms. |
| B4 | global `T_f = 0.040` (wrong for 80 ms-frame Mimi codecs) | `engine.rs:1821`, `codec_ar_admission.rs:288` | **per-model.** New `ArStepModel::frame_period_secs() -> Option<f64>` (default `None` = dynamic/unknown). Mimi-class models override it: dia2 + csm return `Some(1920/sample_rate) = 0.08 s` (12.5 Hz). `Engine::codec_ar_frame_period_secs()` queries the loaded model; both the bandwidth calibration (`engine.rs`) and the admission gate (`for_serving`, wired in `lib.rs`) use it, falling back to the **honestly-relabeled** `RATED_FRAME_BUDGET_SECS = 0.040` (now documented as a *rated scheduling-budget POLICY default*, NOT a claim about any model's physical frame period). Single source of truth: `codec_ar_admission::RATED_FRAME_BUDGET_SECS`, re-used by the engine. |

Proofs (no GPU): `codec_ar_admission::tests::rated_serve_and_retry_derive_from_per_model_frame_period`
(B2/B3/B4 derivations + `for_serving` per-model `T_f` + None/bad-value fallback). The pre-existing
`deadline_gate_sheds_a_request_a_deep_queue_cannot_serve_in_time` is unchanged (the derived value at the
rated tick equals the historical 0.5 s).

### What is NOT a hard "measured" derivation, honestly
- B2's `RATED_STRIDES_PER_STREAM` is an **explicit documented assumption** (a stride count), not a
  measurement — but the serve-time is now a true `Ceilings`-derived quantity (it tracks `T_f`), which the
  old `0.5` claimed to be and was not.
- B4's per-model `T_f` is concrete for fixed-rate Mimi codecs (dia2/csm); a dynamic-frame-rate codec or a
  one-shot model legitimately returns `None` (the frame rate is not known a priori — see `dynamic_fr`) and
  uses the rated policy default, which is now **honestly labeled as a policy budget**, not a measured frame
  period.

---

## Files touched
Infer (`waav-infer/`):
- `crates/waav-infer-protocol/src/trace.rs` — traceparent-string serde (A.0)
- `crates/waav-infer-runtime/src/arstep.rs` — `ArStepModel::frame_period_secs()` (B4)
- `crates/waav-infer-backend-torch/src/dia2.rs`, `.../csm.rs` — Mimi 12.5 Hz override (B4)
- `crates/waav-infer-server/src/engine.rs` — trace plumb (A.2), warmup-measured `compute_secs` (B1), per-model `T_f` (B4), tests
- `crates/waav-infer-server/src/codec_ar_admission.rs` — B2 / B3 / B4 + `RATED_FRAME_BUDGET_SECS`, test
- `crates/waav-infer-server/src/lib.rs` — wire the model `T_f` into `for_serving` (B4)

Gateway (`WaaV/gateway/`):
- `src/middleware/request_id.rs` — `mint_traceparent` / `is_w3c_traceparent` (+ tests)
- `src/core/realtime/base.rs` — `RealtimeConfig.trace`
- `src/core/realtime/infer/protocol.rs` — inject on `session.config` + connect header (+ test)
- `src/handlers/realtime/handler.rs` — thread the traceparent realtime_handler → handle_config

## Verification (all run on this box)
Infer (`waav-infer`, torch env):
- `cargo build --workspace --features torch` — green; `cargo clippy -D warnings` (protocol/runtime/server/backend-torch) — clean.
- Tests pass: protocol `serde_round_trips_as_traceparent_string` (+25 protocol lib); runtime `serve::`/`arstep::` (39);
  server `guarded_serve_threads_session_trace_into_stage_spans`, `guarded_engine_serve_routes_codec_ar_and_fires_the_spine`,
  `rated_serve_and_retry_derive_from_per_model_frame_period`, `deadline_gate_sheds_a_request_a_deep_queue_cannot_serve_in_time`
  (+10 admission lib); provider trace/ws_map/s2s (15); gateway-provider-api (5); `ws_live.rs` integration test compiles.

Gateway (`WaaV/gateway`):
- `cargo check --lib` + `cargo clippy --lib -- -D warnings` — clean; `cargo test --lib` — **10 pass, 0 fail** (4 new:
  `infer_protocol_injects_propagated_traceparent`, `mint_traceparent_reuses_inbound_trace_id`,
  `mint_traceparent_mints_fresh_when_absent_or_invalid`, `is_w3c_traceparent_rejects_malformed`; 6 pre-existing
  realtime/config tests — no regression).
- **Env note:** the gateway's `webrtc-sys` (livekit) dep fails to build on this aarch64 box whenever CUDA is present — it
  feeds x86 NVIDIA-codec implib trampolines (`cmpq/je/pushq …`, `src/nvidia/implib/*.tramp.S`) to the aarch64 assembler.
  This is **pre-existing and unrelated** to these pure-Rust changes. webrtc-sys gates the NVIDIA codec on
  `$CUDA_HOME/include/cuda.h` existing, so the gateway builds/tests cleanly with `CUDA_HOME` pointed at a dir without
  `cuda.h` (used for the runs above). The gateway needs no CUDA itself.

No byte-identity regression: every change is control-plane / telemetry; the codec serve numerics are untouched.
