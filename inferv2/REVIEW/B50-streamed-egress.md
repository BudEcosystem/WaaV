# B50 — Streamed codec-AR egress: batched-path incremental TTFA (item 7 / F6 / VERDICT #3)

**Goal.** The codec-AR serve path emitted first audio only after FULL synthesis under concurrency
(*batched-path TTFA == full-synthesis*). Make it STREAM: decode + emit audio incrementally as codec frames
are generated, so TTFA ≪ full-synthesis, **bit-faithful** (Σ emitted == one-shot whole-body decode exactly),
barge-in preserved.

**Status: DONE.** The multiplexed loop now decodes + emits each slot's already-final body prefix MID-AR-LOOP.
Proven bit-faithful + TTFA ≪ full on a causal-chunkable model (runtime unit gates); the production reality
(Mimi/DAC/chatterbox are non-causal ⇒ honest whole-body) live-verified on csm. `cargo test -p
waav-infer-runtime -p waav-infer-server --lib` green (235/0, 66/0); clippy `--all-targets -D warnings` clean.

---

## 1. Where TTFA was being lost (investigation)

The streaming machinery already existed and was correct — except in the ONE place the live handlers use.

- **Single-stream** `serve_codec_ar_stream` (serve.rs) already runs the AR loop then decodes via the
  genuinely-incremental `ArStepModel::decode_audio_stream` seam (segments emitted as the decoder finalizes
  them), closing on an explicit G2 terminal through the delta-faithful `StreamEgress` (egress.rs — already
  `Σ deltas == full`, the I1 gate).
- **The multiplexed / batched loop** `serve_codec_ar_multiplexed_inner` → `drain_finished_stream`
  (serve.rs:776, pre-fix) decoded + emitted **only when a slot reached EOS** (slot completion). Under
  concurrency every stream therefore got its first audio at END → **TTFA == full-synthesis latency**.
- This is the exact path the live WS handler drives: `ws.rs:328 batcher.submit` →
  `CodecArBatcher` (codec_ar_batcher.rs:429 `serve_codec_ar_multiplexed_bounded`) → `_inner`. So the #7 fix
  that "reached single-stream" never reached the batched path the handlers actually use (VERDICT #3 / 00-SYNTHESIS).

The `UnboundedSender` the brutal review flagged at egress:57 is already fixed (GATE #9): the egress channel is
bounded (`EGRESS_CHANNEL_DEPTH=8192`, non-blocking `bounded_send`) — that is orthogonal to TTFA and was left intact.

**Net: the gap was producing audio once at slot completion in the mux loop, vs producing it per-chunk during the AR loop.**

---

## 2. The codec-decoder reality (why "chunk the decoder" cannot be the answer)

I probed the REAL codec decoders on GB10 to determine whether `decode(body[..p])` is a bit-exact prefix of
`decode(body)` (the prerequisite for committing audio early). It is **not**, for every codec family:

| codec | structure | bit-faithfully chunkable? | evidence |
|---|---|---|---|
| **Mimi** (csm, dia2) | causal sliding-window transformer (win 250) + causal SEANet convs + RVQ | **NO** | `decode(body[..t/4]) != decode(body)[..]` — diverges at **sample 0–1**, bf16 maxΔ=**1.37e-2**, f32 maxΔ=**9.07e-4** (live csm, both regimes). Root cause: the length-dependent `_get_extra_padding_for_conv1d` right-pad + float non-associativity. |
| **DAC** (dia, higgs, omnivoice) | **symmetric**-padded convs (`nn.Conv1d(padding=p)` both sides) | **NO** | symmetric pad bleeds the future into every position — genuinely non-causal by construction (codec/dac.rs module docs). |
| **chatterbox** S3Gen | bidirectional UpsampleConformer + bidirectional-attention CFM U-Net + overlap-add ISTFT | **NO** | already documented: `encode(prefix) != encode(whole)[:prefix]`, maxabs ≈ 1.49 (chatterbox.rs:1549). |

So **no production codec decoder is bit-faithfully prefix-/chunk-decodable.** The bit-identity law forbids
faking sub-segments. A "stateful streaming Mimi decoder" could be bit-identical to the *reference's streaming
decoder* — but NOT to the whole-body `decode()` (the sacred reference here), because the right-pad differs
between whole-body and any chunking. (This is the same conclusion already reached for chatterbox, now confirmed
for Mimi and DAC.) **Incrementality is a model property; it must never be faked.**

A causal-chunkable codec (e.g. a dots-style causal-conv vocoder that carries a `conv_tail`) *would* qualify —
the design below is exactly the seam such a decoder plugs into.

---

## 3. The incremental decode+emit design (how the causal state is carried)

Two pieces — a new model seam + the mux-loop wiring — keep the win **bit-faithful for every model**:

### 3.1 New seam: `ArStepModel::decode_committed_prefix` (arstep.rs)

```rust
fn decode_committed_prefix(&mut self, slot, body, committed_frames)
    -> Result<(Vec<f32> /*pcm_delta*/, usize /*frames_now_committed*/), InferError>;
```

Called PERIODICALLY *while the AR loop is still running*. The model returns the PCM of the body frames whose
audio **cannot change** as more frames arrive (its causal receptive field is fully inside the committed region),
beyond what was already committed, plus the new committed-frame count.

- **The law (hard accept):** `Σ(pcm_delta) ++ (post-loop flush of the tail) == decode_audio(full_body)`,
  byte-for-byte. A sample committed here can never change — so a model emits a frame's audio early **only**
  where its decoder is genuinely causal up to that boundary. The model itself decides what is final (it owns
  its conv ring-state / left-context / receptive field) — the runtime never assumes chunkability.
- **Default = commit nothing** (`(empty, committed_frames)`). So **every non-causal decoder (Mimi/DAC/chatterbox)
  is a bit-faithful no-op**: it streams the one whole-body segment at completion exactly as before
  (TTFA == whole-body is the honest metric for it). The default is the safe, correct, no-behaviour-change path.

This is the during-loop sibling of the existing `decode_audio_stream` (which decodes a *complete* body in
segments after the loop). The carried causal state lives inside the model's `decode_committed_prefix` impl —
for a causal codec it is the conv ring-buffers + the transformer KV/left-context; the seam only exchanges
`(committed_frames, pcm_delta)` so the runtime stays codec-agnostic.

### 3.2 Mux-loop wiring (serve.rs `serve_codec_ar_multiplexed_inner`)

- `LiveStream` now owns a **persistent `StreamEgress`** (across ticks, one FSM, one terminal),
  `committed_frames`, and `emitted_samples`.
- **Step 4.5 (new)** — after the batched tick advance, for each still-active, not-hung-up slot, on the cadence
  (`F6_DECODE_CADENCE_FRAMES = 8`), call `stream_committed_prefix`: ask `decode_committed_prefix`, push the new
  PCM as delta-not-cumulative chunks through the slot's egress NOW, bump `emitted_samples`, heartbeat J16. A
  mid-loop decode fault is deferred (surfaced on drain); a consumer hangup stops *that* slot's emit (it keeps
  draining the cohort so the lockstep cadence is unperturbed).
- **`drain_finished_stream` (rewritten)** — at slot completion it re-decodes the WHOLE body via
  `decode_audio_stream` (the sacred whole-body reference) but **skips the leading `emitted_samples`** already
  shipped mid-loop, emitting only the tail. So `Σ(mid-loop deltas) ++ (flushed tail) == decode_audio(full_body)`
  byte-for-byte. For a non-causal model `emitted_samples == 0` ⇒ it emits the whole body exactly as before.
- **Barge-in** — `drain_finished_stream`'s `Cancelled` branch closes on the DISTINCT `Cancelled` terminal
  (the consumer drops the uncommitted tail). Any audio committed mid-loop was *already final* bit-faithful
  samples and is never un-sent — the cancel just tells the consumer to drop what it had not yet played.

Public API unchanged (`serve_codec_ar_multiplexed`/`_bounded` signatures stable — the batcher is a drop-in);
the cadence is an internal constant.

---

## 4. Bit-faithful proof (concatenated == whole-body)

A genuinely-causal test double (`CausalMidLoopModel`, serve.rs tests): per-frame causal decode
(`pcm[i] = id[i]/256 × upsample`, a pure function of frame `i`), so `decode_audio(body[..k])` *is* a bit-exact
prefix of `decode_audio(body)`. It overrides `decode_committed_prefix` to commit all-but-a-trailing-margin
(a realistic 1–2 frame right receptive field), forcing BOTH the mid-loop commits and the drain flush to run.

- **`f6_mux_incremental_decode_emit_concat_is_bit_identical_to_whole_body`** (6 concurrent ragged streams):
  `Σ(mid-loop deltas ++ flushed tail) == decode_audio(whole body)` **byte-for-byte** for every stream, each
  closing on Final, per-stream isolated. **maxΔ = 0.** PASS.
- **Live csm reality** (`cuda_torch_csm_streaming.rs`, GB10): the Mimi decode of an early body prefix DIFFERS
  from the whole-body decode in BOTH bf16 (maxΔ=1.37e-2) and f32 (maxΔ=9.07e-4) ⇒ csm/dia2 correctly inherit
  the default (no early commit). The whole-body `decode_codes` is unchanged. PASS.
- **Existing one-shot byte-identity gate still passes** (`cuda_torch_csm` L3): greedy CUDA-bf16 codes
  BYTE-IDENTICAL to the sidecar golden (125 frames × 32 codebooks). The F6 work touched only the runtime serve
  path — zero codec/model numerics changed. The 18 backend-torch `codec::` bit-faithful gates also pass.

**Honest note on warmup/overlap:** for the production codecs no warmup makes streaming bit-faithful (the
right-pad is length-dependent — proven), so they honestly emit the whole body at completion. The documented
delta is exactly the table in §2 (bf16 1.37e-2 / f32 9.07e-4 at the first quarter prefix); committing early
there would violate the law, so we don't. The mid-loop path is reserved for genuinely-causal codecs, where it
is maxΔ=0 (the unit gate).

---

## 5. Measured TTFA vs full-synthesis

`f6_mux_ttfa_is_far_below_full_synthesis` (4 concurrent streams, 2 ms/AR-step so the AR loop has measurable
wall time, causal codec):

| metric | value |
|---|---|
| full-synthesis (whole concurrent batch) | **~223 ms** |
| **TTFA (first audio chunk)** | **~59.5 ms** (~27 % of full) |
| TTFA ≪ full assertion (`ttfa·2 < full`) | **PASS** for every stream |
| streamed audio == whole-body decode | **byte-identical** (the win costs no sample) |

Pre-fix the same model would have delivered first audio only at ~223 ms (slot completion). First audio now
lands one cadence (8 frames) after enough body exists to commit — the genuine first-token TTFA, **under
concurrency**, not just single-stream.

---

## 6. Barge-in / cancellation still works

- **`f6_mux_barge_in_after_committed_audio_closes_cancelled`**: a cancel fired MID-loop (after audio was
  committed) closes the stream on the DISTINCT `Cancelled` terminal (not Final); the audio the consumer already
  received is a bit-faithful PREFIX of its whole-body decode (`ref.starts_with(&received)`); the neighbour
  stream is untouched and closes on Final. PASS.
- The existing `multiplexed_barge_in_cancels_only_its_own_slot` and the full multiplexed suite (6 tests) still
  pass — per-slot isolation and the cancel-within-one-frame contract are intact.

---

## 7. Files changed

- **`crates/waav-infer-runtime/src/arstep.rs`** (+43): new `ArStepModel::decode_committed_prefix` seam with a
  default that commits nothing (the bit-faithful no-op for non-causal codecs).
- **`crates/waav-infer-runtime/src/serve.rs`** (+~417 net): `LiveStream` gains a persistent `StreamEgress` +
  `committed_frames` + `emitted_samples`; new step 4.5 in `serve_codec_ar_multiplexed_inner`
  (`F6_DECODE_CADENCE_FRAMES`); new `stream_committed_prefix` helper; `drain_finished_stream` rewritten to flush
  only the uncommitted tail through the persistent egress; 3 new F6 gates + the `CausalMidLoopModel`/`admit_timed`
  test doubles.
- **`crates/waav-infer-backend-torch/tests/cuda_torch_csm_streaming.rs`** (new, live GB10): proves the csm Mimi
  codec is non-causal in bf16 + f32 (so the F6 default is correct for it) and that the whole-body decode is intact.

No changes to model numerics, the codec module, or the public mux-loop API.

## 8. Verification

```
cargo test -p waav-infer-runtime -p waav-infer-server --lib   → 235/0, 66/0  ✅
cargo test -p waav-infer-backend-torch --lib codec::          → 18/0         ✅
cargo clippy -p waav-infer-runtime -p waav-infer-server --all-targets -D warnings  → clean ✅
cargo clippy -p waav-infer-backend-torch --tests -D warnings  → clean        ✅
# live GB10 (source gb10-env.sh, --ignored):
cuda_torch_csm_streaming::cuda_csm_mimi_is_non_causal_so_f6_default_is_correct  → PASS
cuda_torch_csm::cuda_csm_codes_byte_identical_to_sidecar (existing gate)        → PASS (L3 LAW)
```
