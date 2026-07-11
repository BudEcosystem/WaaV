# G6 — Live observability for the codec-AR lockstep batcher's cohort width

**2026-06-24, GB10.** Implements **G6** from `BATCHING-ANALYSIS-SYNTHESIS.md` (the do-now observability item):
add a `/metrics` series so operators can SEE how many concurrent streams the codec-AR lockstep batcher
actually advances per batched tick. Before this, the batcher's effectiveness was **invisible in prod** —
no metric distinguished "16 streams batching through one `step_batch`" from "16 serialized solo ticks".

## What landed

A Prometheus **histogram** recorded once per batched tick at the **real live call site** — the
`serve_codec_ar_multiplexed`/`_bounded` loop the `CodecArBatcher` runs on live traffic (NOT a test double),
the exact spot where the cohort width is known.

### Metric

| | |
|---|---|
| **Histogram** | `waav_infer_codec_ar_cohort_width` |
| **Buckets** | `[1, 2, 4, 8, 16, 32, 64]` (powers of two up to the 64 roofline the batching analysis measured) |
| **Observed value** | `active_rows.len()` — the cohort the lockstep batcher advances through ONE `step_batch` this tick (the exact slice `driver.tick` hands `step_batch`) |
| **Companion gauge** | `waav_infer_codec_ar_inflight_streams` — live count of occupied slots this tick (instantaneous concurrency) |

Reading it: cohort `1` ⇒ no batching benefit (the host-KV regression knee or a single live stream); a fat
right tail (`le="16"`/`le="32"`) ⇒ the 55×@64 batching lever is engaged. The histogram is the per-tick
distribution over time; the gauge is the instantaneous occupancy.

### Emission call site

`crates/waav-infer-runtime/src/serve.rs`, inside `serve_codec_ar_multiplexed_inner` (the shared loop body),
guarded by `if !active_rows.is_empty()` immediately before `driver.tick(model, &set)`:

```rust
metrics::histogram!(COHORT_WIDTH_METRIC).record(active_rows.len() as f64);
metrics::gauge!(INFLIGHT_STREAMS_METRIC).set(live.iter().filter(|c| c.is_some()).count() as f64);
```

This is the ONE live tick both `serve_codec_ar_multiplexed` and `serve_codec_ar_multiplexed_bounded` route
through (`…_inner`), so the metric fires for every codec-AR stream the batcher serves. The `metrics::*`
macros are a **no-op when no recorder is installed** (the unit/bit-identity gates are untouched) and route to
the process-wide Prometheus recorder otherwise — the same pattern item-5's admission metrics use.

### Rendering as a TRUE bucketed histogram

A `metrics`-facade histogram with no configured buckets renders as a Prometheus **summary** (quantiles), not
`…_bucket{le="N"}` lines. So the server's exporter configures the cohort buckets for this exact metric name
(`crates/waav-infer-server/src/bin/waav_infer.rs`):

```rust
PrometheusBuilder::new()
    .set_buckets_for_metric(
        Matcher::Full(waav_infer_runtime::COHORT_WIDTH_METRIC.to_string()),
        waav_infer_runtime::COHORT_WIDTH_BUCKETS,
    )?
    .install_recorder()?
```

`COHORT_WIDTH_METRIC`, `COHORT_WIDTH_BUCKETS`, `INFLIGHT_STREAMS_METRIC` are `pub` constants re-exported from
`waav-infer-runtime` so the runtime emitter, the server exporter wiring, and the G6 gate share ONE definition.

## The gate (rendered non-zero, cohort ≥2)

`g6_cohort_width_histogram_emits_nonzero_into_rendered_prometheus`
(`crates/waav-infer-runtime/src/serve.rs`, mirrors `item5_admission_emits_nonzero_metrics_into_rendered_prometheus`):

- scopes a LOCAL `PrometheusRecorder` with the `[1,2,4,8,16,32,64]` buckets configured (exactly as the
  server installs them, so the histogram renders as a real bucketed histogram);
- drives **8 concurrent** ragged admits through the LIVE `serve_codec_ar_multiplexed` loop;
- renders `/metrics` and asserts: the histogram `_count ≥ 1` AND a **cohort width ≥2 was observed**
  (`total observations − le="1" bucket ≥ 1`).

**Rendered exposition (from the gate, `--nocapture`):**

```
# TYPE waav_infer_codec_ar_inflight_streams gauge
waav_infer_codec_ar_inflight_streams 3

# TYPE waav_infer_codec_ar_cohort_width histogram
waav_infer_codec_ar_cohort_width_bucket{le="1"} 0
waav_infer_codec_ar_cohort_width_bucket{le="2"} 0
waav_infer_codec_ar_cohort_width_bucket{le="4"} 1
waav_infer_codec_ar_cohort_width_bucket{le="8"} 6
waav_infer_codec_ar_cohort_width_bucket{le="16"} 6
waav_infer_codec_ar_cohort_width_bucket{le="32"} 6
waav_infer_codec_ar_cohort_width_bucket{le="64"} 6
waav_infer_codec_ar_cohort_width_bucket{le="+Inf"} 6
waav_infer_codec_ar_cohort_width_sum 40
waav_infer_codec_ar_cohort_width_count 6
```

`le="1"` = 0 and `le="4"` = 1, `le="8"` = 6 ⇒ every one of the 6 batched ticks advanced a cohort of width
3–4 (a serialized per-request loop could never batch >1). Gate output:
`G6 cohort-width histogram RENDERED non-zero @cohort≥2: count=6 le1=0 total=6 cohort≥2_observations=6 ✓`

## Verification

| Check | Result |
|---|---|
| `g6_cohort_width_histogram_emits_nonzero_into_rendered_prometheus` | **PASS** (rendered non-zero, cohort ≥2) |
| `cargo test -p waav-infer-runtime` | **237 passed**, 0 failed (incl. the existing `multiplexed_ragged_concurrent…` bit-identity gate of the modified loop) |
| `cargo test -p waav-infer-server --lib` | **68 passed**, 0 failed (incl. item-5 admission metrics) |
| `live_concurrent_codec_ar_streams_share_one_loop_and_are_bit_identical` (the `CodecArBatcher`→loop path) | **PASS** — cohort still token-for-token bit-identical with the metric in place |
| `cargo clippy --workspace --all-targets -D warnings` | **clean** |
| `cargo build -p waav-infer-server --bin waav-infer` | **builds** (bucket wiring) |

### Note on the live-CUDA `live_gb10_batcher` gate

`live_gb10_batcher_concurrent_ragged_is_bit_identical_and_scales` currently **fails to OOM** on this box — a
CUDA BFC-arena allocation failure in the vocoder `/conv_pre/Conv` node (`Available memory of 18.3 GB is
smaller than requested 21.7 GB`) at the **reference-build stage** (`codec_ar_batcher.rs:894`, the
`serve_codec_ar` per-slot reference), *before* any batched serving or bit-identity comparison runs.

**This is PRE-EXISTING and unrelated to G6.** Proven by stashing all G6 changes and rebuilding: the baseline
fails identically (byte-for-byte same arena message, same boundary, same line). G6 adds only two control-plane
`metrics::*` calls and allocates nothing on the GPU. The failure is the documented GB10 unbounded-ORT-CUDA-
arena pressure when this gate loads 3 chatterbox instances (~28 GB each) in one process — a separate infra
item (arena-cap fix not applied to the chatterbox vocoder session in this gate), outside G6's scope. The
batching-accuracy invariant the LAW protects is independently proven green by the two double-driven gates that
exercise the exact modified tick.

## Files

- `crates/waav-infer-runtime/src/serve.rs` — `COHORT_WIDTH_METRIC` / `COHORT_WIDTH_BUCKETS` /
  `INFLIGHT_STREAMS_METRIC` constants; the per-tick `histogram!`/`gauge!` emission in
  `serve_codec_ar_multiplexed_inner`; the `g6_*` gate + two render-parse helpers.
- `crates/waav-infer-runtime/src/lib.rs` — re-export of the three constants.
- `crates/waav-infer-runtime/Cargo.toml` — `metrics` dep (facade only, P-8: no exporter in the runtime) +
  `metrics-exporter-prometheus` dev-dep (the gate's local recorder).
- `crates/waav-infer-server/src/bin/waav_infer.rs` — `set_buckets_for_metric` on the production
  `PrometheusBuilder` so the cohort histogram renders as a true bucketed histogram on `/metrics`.
</content>
</invoke>
