# B9 — Backlog item #6: test-coverage gaps closed (REVIEW/07-test-scenario-coverage.md)

The brutal review (`07-test-scenario-coverage.md`) found ZERO/THIN coverage on the live concurrent/overload
codec-AR paths: **handler-level concurrency** (F1 verify), **chaos/fault-injection**, **fairness/starvation**,
and **oversized/malformed-input** gates. This closes them with REAL fail-before/pass-after tests — no trivial
asserts. Every test is **deterministic + CPU-only** (fake `ArStepModel` doubles, no GPU, no model files), so
they run in the default `cargo test --workspace` pass and leak nothing.

## Test files added (all NEW, under `crates/waav-infer-server/tests/`)

| File | Tests | Item 6 gap covered |
|---|---|---|
| `chaos_concurrency.rs` | 3 | 6.1 handler-level concurrency (F1) + 6.4 chaos/fault-injection |
| `overload_fairness.rs` | 2 | 6.2 overload-shed/no-leak + fairness/no-starvation |
| `oversized_input.rs` | 4 | 6.3 oversized / zero-length / garbage → typed reject, no panic |

**9 tests total. Status: all 9 PASS** (`cargo test -p waav-infer-server --test chaos_concurrency --test
overload_fairness --test oversized_input`), each < 0.1 s, stable across repeated runs (no flakiness in the
threaded ones). `cargo clippy -p waav-infer-server --tests` is **clean** (0 warnings).

All tests drive the REAL `pub` serving surface the WS `speak` / REST `/v1/audio/speech` codec-AR path uses:
- `CodecArBatcher::submit` (the exact entry both `ws.rs:317` and `lib.rs:780` call — confirmed),
- `CodecArAdmission::try_admit` (the GATE #9 control plane every submit passes through),
- `serve_codec_ar_multiplexed` (the shared lockstep loop),
- and the real axum router (`build_router` → `speech` handler) via `tower::ServiceExt::oneshot`.

## What each test proves

### Item 6.1 — handler-level concurrency, the real gap (F1 verify)
`item6_handler_concurrency_more_than_four_codec_ar_streams_admitted_concurrently`
Fires **N=12** concurrent `CodecArBatcher::submit`s through an admission gate sized 24 (the GATE #9 codec-AR
budget, NOT the flat `max_concurrency`=4). A fake `step_batch` records each tick's cohort size; the test asserts
the shared loop advanced **>4 distinct streams in ONE batched tick** (observed peak cohort = **12**). This is the
F1 permit-release proof: the old flat `max_concurrency`=4 (or a per-request-mutex serialization) would cap the
peak cohort at ≤4 → the `>4` assert FAILS-BEFORE, PASSES-AFTER. Every admitted stream closes on `Final`.

### Item 6.2 — overload / fairness
`item6_overload_spike_admits_exactly_cap_sheds_rest_typed_and_no_leak`
Spike of **M=512** admissions fired across **16 threads** at `CodecArAdmission::try_admit` (the CAS bound
genuinely raced). Asserts: **exactly MAX_ADMIT=8 admit** while tickets are held, the rest (504) are **typed-shed
`AdmissionRejected`** (retriable + retry-after), in-flight **never exceeds 8** under the race, and after every
ticket drops the gate's counters (`inflight`, `vram_reserved`) **return to 0** (no leak) and the gate is reusable.
Unbounded admit → 512 admitted / inflight > cap → FAILS-BEFORE.

`item6_fairness_one_long_plus_n_short_no_starvation`
**1 long (budget 40) + 6 short (budget 3)** streams share ONE lockstep loop (long submitted first, so short-stream
churn happens around it). A per-slot progress witness asserts the long stream advanced its full 41× (served to
completion, NOT starved by the churn) AND every short stream made its full advances (not starved by the long
one) — all close on `Final`, loop `Ok`, bounded wall-time. A non-fair head-of-line scheduler → a stream frozen /
never-terminating → FAILS-BEFORE.

### Item 6.3 — oversized / malformed input (typed, never a panic)
- `item6_oversized_speak_text_is_typed_413_not_panic` — a 32 KiB `input` (> `max_text_bytes` 16 KiB, < body cap
  64 KiB) → HTTP **413 `payload_too_large`** typed envelope, via the REAL `speech` handler. No panic / 500 / OOM.
- `item6_zero_length_input_is_typed_400_not_panic` — `""`, `"   "`, `"\n\t  "` → HTTP **400 `bad_config`**.
- `item6_garbage_voice_is_typed_400_not_panic` — a control-char garbage voice → HTTP **400 `bad_config`**.
- `item6_garbage_utterance_in_loop_is_typed_error_terminal_not_panic` — a garbage utterance (embedded NUL) in the
  codec-AR shared loop → that stream's **typed `Error` terminal**, the valid neighbours still `Final`, loop `Ok`.

### Item 6.4 — chaos / fault-injection (CPU-fake, deterministic)
`item6_chaos_one_slot_backend_fault_does_not_poison_the_others`
N=5 concurrent streams; ONE slot's backend (`decode_audio`) faults mid/post-stream with a typed error. Asserts
**ONLY that slot** closes on a typed `Error` terminal, every **other** slot keeps producing + closes on `Final`,
the loop returns `Ok` (**no shared-loop poison**) and does **not hang** (bounded wall-time ~0.2 ms). This is the
per-slot containment the seam provides (empirically confirmed: a whole-loop error would leave survivors with NO
terminal → FAILS-BEFORE).

`item6_chaos_wedged_consumer_is_dropped_and_others_unaffected_f2`
One stream's egress sink always returns `false` (a wedged/never-draining consumer). Asserts that stream is
**dropped as SlowConsumer** (no terminal forced through the dead sink), every **other** stream is unaffected
(`Final` + full audio), and the shared loop **never blocks** (bounded wall-time ~0.3 ms). This is the **F2**
non-blocking-egress proof: the pre-F2 blocking `thread::sleep`-up-to-10s egress would stall the whole loop on the
wedged stream → the wall-time bound FAILS-BEFORE.

## Source change (minimal seam — the only src edit)

`crates/waav-infer-server/src/engine.rs`: `Engine::from_tts_for_test` promoted from `#[cfg(test)] pub(crate)` →
`pub` (non-`cfg(test)`). An out-of-crate integration test (`tests/`, a separate compilation unit) cannot see a
`#[cfg(test)]` constructor, so this is the seam that lets `oversized_input.rs` stand up a CPU-only `Engine` around
a fake `TtsModel` and exercise the REAL HTTP handlers (the genuine `PayloadTooLarge`/`BadConfig` envelopes) with
no model files / no GPU. No other src files touched; all 53 existing server lib unit tests still pass.

## Coverage status vs item-6 gaps

| Gap (REVIEW/07) | Status | Notes |
|---|---|---|
| Handler-level concurrency (F1) | **COVERED, default pass** | peak cohort 12 > 4 through admission+batcher |
| Overload spike / shed / no-leak | **COVERED, default pass** | exactly-cap admit, typed shed, counters → 0 |
| Fairness / starvation | **COVERED, default pass** | 1 long + 6 short, all progress + terminate |
| Oversized / zero-length / garbage | **COVERED, default pass** | typed 413/400 via real handler + loop terminal |
| Chaos: mid-stream backend fault | **COVERED, default pass** | per-slot decode fault contained, no poison |
| Chaos: wedged/slow consumer (F2) | **COVERED, default pass** | dropped as SlowConsumer, loop never blocks |

### Finding (architectural, documented not gated)
A `step_batch`-level error (the batched forward fails for the **whole tick**) is **batch-scoped, not per-slot**:
`driver.tick(...)?` propagates it out of `serve_codec_ar_multiplexed`, so the loop returns `Err` and the in-flight
survivors get **no terminal** (their egress channels just close). This is by-construction (the lockstep tick is one
batched forward over all active slots). The seam's **genuinely per-slot** fault points are `prefill` and
`decode_audio`, both of which ARE contained (typed terminal on that slot, cohort untouched) — which is what the
item-6.4 chaos gate exercises. If true per-slot containment of an in-tick step fault is later desired, the loop
would need to fall back to per-slot `step` on a `step_batch` error and isolate the offending row — not currently a
requirement, but worth noting for the failure catalog.

### Live-GPU gates (NOT replaced by these — complementary)
These deterministic gates are **in addition to** the existing `#[ignore]`d live-GPU gates in
`codec_ar_batcher.rs` (`live_concurrent_…`, `live_gb10_batcher_…`, run process-isolated via
`ci/heavy_live_tests.sh`) which prove **bit-identity + throughput scaling** on the REAL chatterbox CUDA model.
Bit-identity-under-real-weights still needs the live-GPU gate (the CPU fakes prove structure/containment/
concurrency/typed-rejects, not numerical fidelity of a real codec). No item-6 gap is left needing a live-GPU gate
for its *structural* claim; only the numerical-fidelity claim remains live-GPU-only (already covered).
