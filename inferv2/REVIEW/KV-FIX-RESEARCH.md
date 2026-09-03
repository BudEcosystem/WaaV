# Fixing the ONNX-Runtime (Path-A) host-KV re-stream — research + ranked options

**2026-06-25. GB10 (Grace-Blackwell sm_121) + aarch64.** Scope: the codec-AR lockstep batcher caps at
**~1.8×@B16 (regressing to ~0.95×@B64)** on the chatterbox `language_model.onnx` because the KV cache
round-trips through the host every decode stride. Target: recover the **~30×@B64** device-resident curve the
Path-B tch probe already hits (`BATCHING-ANALYSIS-SYNTHESIS.md` §Performance). This report pins the exact
data movement in the WaaV source, researches the ONNX-Runtime device-resident-KV options against the
authoritative docs, and recommends a concrete fix path. Every external claim is cited inline.

---

## 0. TL;DR / Recommendation

**Recommended path: a buffer-sharing GQA re-export (`past_present_share_buffer=1`, pre-allocated max-length
KV) consumed via a device-resident IoBinding ping-pong in the `ort` backend.** This is the only option that
both eliminates the host↔device KV transfer *and* the per-stride device re-alloc/copy, which together are
the `O(B·layers·heads·seq·dim)` wall capping Path-A. The two halves are separable but only pay off together:

- **Runtime half (IoBinding ping-pong)** keeps the KV `Value` on CUDA and re-binds the step-N output buffer
  as the step-N+1 input — removes the H2D/D2H copy. *Available today in the `ort` rc.12 Rust API.*
- **Export half (`past_present_share_buffer`)** makes `present.*` write *in place* into the pre-allocated
  `past.*` buffer — removes the per-stride device re-alloc + present→past copy. *Requires a one-time
  re-export of `language_model.onnx` with a static max-length GQA buffer.*

**Do NOT adopt onnxruntime-genai** as the decode engine: its model-builder supports only a fixed allowlist
of *text* decoder-only LLMs and cannot export a codec-AR TTS decoder (evidence below). Its KV-cache
machinery is exactly the `past_present_share_buffer`+GQA+IoBinding pattern we would build directly — so we
borrow the *technique*, not the *library*.

Ranked: **(1) buffer-sharing re-export + device IoBinding ping-pong** ≫ (2) IoBinding-only (no re-export) ≫
(3) adopt ORT-GenAI. Detailed below in §C.

---

## A. WaaV source analysis — the EXACT data movement

### A.1 The graph contract (host KV in, host KV out)

`crates/waav-infer-core/src/tts/chatterbox.rs` module header pins it (lines 60–63):

> "the graph takes the split-KV as HOST `past_key_values.*` inputs and emits host `present.*` outputs that
> the AR loop re-streams EVERY stride … That host re-stream is `O(B · max_past · n_layers · 2)`
> loop-invariant work."

The LM is a 30-layer Llama, **`n_layers=30`, `kv_heads=16`, `head_dim=64`** (read from the graph at load,
`chatterbox.rs:454-455`), exported with `com.microsoft.GroupQueryAttention` (confirmed by the GQA
left-align/`seqlens_k` identity the batcher relies on, `chatterbox.rs:659-667`, and the repo-wide GQA
references in `cuda_ort_*` tests).

### A.2 The per-stride host transfer (the O(B·layers·heads·seq·dim) wall)

The single-stream primitive `LmDecoder::lm_forward` (`chatterbox.rs:604-648`):

```rust
feeds.append(&mut past);                       // 60 host past_key_values.* tensors fed IN
let mut out = self.language_model.run(feeds)?;  // StaticGraph::run  (host→device for ALL feeds)
…
let grown = feedback_present_kv(&mut out, self.n_layers)?;  // 60 host present.* tensors read OUT
```

`feedback_present_kv` (`chatterbox.rs:1829-1839`) simply **renames** the 60 host `present.{i}.{key,value}`
output tensors to `past_key_values.{i}.{key,value}` for the next stride. So **every stride feeds 60 host KV
tensors in and reads 60 host KV tensors out**, each `[B, 16, past, 64]` / `[B, 16, past+1, 64]`. KV is a
*named graph input AND output*, round-tripped host↔device each step — the canonical anti-pattern.

The batched seam `lm_forward_batched` (`chatterbox.rs:683-839`) is worse: it **rebuilds the entire padded KV
buffer on the host every stride**. Lines 735-773 allocate `buf = vec![0f32; b * h * max_past * d]` *per
layer per key/value* (60 host allocations/stride), copy each slot's KV LEFT-aligned into it, and re-feed via
`feed_float`. Lines 805-835 then read all 60 `present.*` back, `to_f32_vec()` each, and gather per-slot.
That is `O(B · n_layers · 2 · kv_heads · max_past · head_dim)` host copies **plus** the H2D upload of the
whole padded buffer **plus** the D2H download of the whole grown buffer, **every stride**. On GB10 this is
the wall: peak 1.81×@B16, regressing to 0.95×@B64 (`chatterbox.rs:68-70` measured curve).

### A.3 The StaticGraph/EpRequest seam — every `run` is a full host materialization

`crates/waav-infer-backend-ort/src/lib.rs`:

- `OrtModel::run` (`lib.rs:438-460`): for **every** input it calls `to_ort_value(&self.session, &t)` and
  accounts `self.h2d_input_bytes += tensor_byte_len(&t)` — "every input is materialized + queued to the
  device on each `run`" (lib.rs:442-444). Outputs go through `extract_named_output` which calls
  `try_extract_tensor::<T>()` → `.to_vec()` (lib.rs:406-435), a **device→host copy** for every output.
- `to_ort_value` (`lib.rs:352-397`) builds an ort `Value` from a **host** `Vec` (`Tensor::from_array` or,
  for empty axes, `Tensor::<T>::new(alloc, shape)` on the *default* allocator). There is no device-handle
  input path — `NamedTensor` only ever holds host `Vec` data (`TensorData::{F32,F16,I64,…}`).

### A.4 IoBinding IS wired — but cannot keep KV resident across strides

The `run_bound` IoBinding path exists (`lib.rs:489-566`) and is the "#1 engine perf change" for
*loop-invariant constants* (Supertonic CFM). Its limitation for AR-KV is structural and is **measured**:

- Outputs are bound to **host-accessible** memory: `CUDA_PINNED` + `MemoryType::CPUOutput` on CUDA
  (`lib.rs:515-525`). So even `keep_on_device("present.*")` lands the present buffer in **pinned host**
  memory and `extract_named_output` still copies it to a host `Vec` (lib.rs:561-563, doc lib.rs:402-405:
  "the value is always CPU-extractable here"). It never stays device-resident for re-binding.
- The varying inputs (`IoBinding::inputs()`) are still host `NamedTensor`s pushed through `to_ort_value`
  every step (`lib.rs:531-536`) — so the past KV is re-uploaded H2D every stride regardless.
- `chatterbox.rs:589-603` documents the **measured** outcome: on the real 30-layer LM at KV-depth 200,
  `run_bound` = **0.77× (≈23–29% slower)** than plain `run`, "for KV that is read back to host anyway." The
  micro-bench is `perf_lever_run_bound_vs_run_on_real_chatterbox_lm_cuda` (`lib.rs:1456-1574`): note it
  rebuilds a **fresh host `feeds()`** every iteration (`lib.rs:1540-1551`), proving the binding never
  carries step-N's device output into step-N+1's input.

**Root cause, precisely:** WaaV's `IoBinding` abstraction (`backend-api/src/lib.rs:241-336`) only carries
host `NamedTensor` inputs and *residency-hint* output names (`device_outputs: Vec<String>`). It has no way
to (a) hold a device-resident `Value` as input, nor (b) hand a device-resident output `Value` back to be
rebound. The graph also re-allocates `present.*` (shape `[B,H,past+1,D]`, a *new growing* buffer) every
stride rather than writing into a shared pre-allocated `past.*` buffer — so even a perfect ping-pong would
still re-allocate device memory each step. **Both** the host round-trip (runtime) **and** the per-stride
re-alloc/copy (export) must be fixed.

---

## B. ONNX-Runtime device-resident KV — the options (researched)

### B.0 Versions (load-bearing)

- WaaV pins `ort = "=2.0.0-rc.12"` with `default-features=false, features=[std, ndarray, tracing, api-24,
  load-dynamic, half]` (`Cargo.toml:43`; `Cargo.lock` ort/ort-sys both `2.0.0-rc.12`).
- `ort 2.0.0-rc.12` is multiversioned over **ONNX Runtime v1.17–v1.24**; the `api-24` flag sets the
  **minimum ORT to 1.24** and unlocks the newest features. [pykeio/ort releases] [ort version-mapping]
  → So WaaV runs against **ORT ≥1.24**, well past the 1.18+ where GQA `past_present_share_buffer` is mature.
  *(Correction to a common mis-mapping: `api-24` ⇒ ORT 1.24, NOT 1.20.)*

### B.1 IoBinding / device-resident `OrtValue` (the runtime fix — canonical)

ORT's I/O Binding lets you "arrange for inputs to be copied to the device and for outputs to be
pre-allocated on the device prior to calling `Run()`." [ORT I/O Binding doc] The **device-tensor** doc spells
out the ping-pong: bind the **same pre-allocated device buffer as both output and the subsequent input** so
"intermediate tensors do not have to be copied back to CPU" — and names "key-value caching in large language
models (LLMs)" as the motivating case. [ORT device-tensor doc]

The C++ surface: allocate via a CUDA allocator (`Ort::Allocator gpu_allocator(session, cuda_mem_info)` →
`Ort::Value::CreateTensor(gpu_allocator, …)`), then `binding.bind_ortvalue_input` /
`binding.bind_ortvalue_output` + `run_with_iobinding`. [ORT device-tensor doc] [ORT I/O Binding doc]

**The `ort` rc.12 Rust API exposes all of this** [docs.rs/ort 2.0.0-rc IoBinding / Value / Session]:

- `Allocator::new(&session, MemoryInfo::new(AllocationDevice::CUDA, 0, AllocatorType::Device,
  MemoryType::Default))` → a CUDA device allocator.
- `Tensor::<T>::new(&cuda_allocator, shape)` → a **device-resident** `Value` (uninitialized device memory).
- `IoBinding::bind_input<T,S>(&mut self, name, ort_value: &Value<T>)` — binds a (device) `Value` as input.
- `IoBinding::bind_output<T,S>(&mut self, name, ort_value: Value<T>)` — binds a pre-allocated (device)
  `Value` as the output target (vs `bind_output_to_device(name, &MemoryInfo)` which lets ORT allocate).
- `Session::run_binding(&mut self, &IoBinding) -> Result<SessionOutputs>` — outputs come back as `Value`s;
  when bound to a device target they stay on device and can be re-bound as the next input (`into_dyn()` to a
  `DynValue`). [docs.rs/ort Value / Session run_binding]

So a **device-resident KV ping-pong is implementable in the existing `ort` crate version** with no FFI.

### B.2 `past_present_share_buffer` + GroupQueryAttention (the export fix — in-place ring KV)

This is the second, complementary half. With buffer sharing, "the past and present KV cache buffers point to
the same memory block," vs the default where "the present KV cache buffers are re-allocated before every
forward pass … and copied to the past KV cache buffers." [ORT genai past-present-share-buffer doc] The win:
"By binding the present KV caches to the past KV caches, there is no need to allocate separate on-device
memory … the past KV caches can be pre-allocated with enough on-device memory so that no new on-device
memory needs to be requested during inference. This reduces memory usage … and lowers latency by eliminating
on-device memory allocation requests." [ORT genai past-present-share-buffer doc] [ORT accelerating-llama-2
blog]

**Format requirements (the export contract)** [ORT ContribOperators GroupQueryAttention; ORT
past-present-share-buffer doc]:

- KV tensors are **BNSH**: `(batch, num_kv_heads, cache_sequence_length, head_size)`, where
  `cache_sequence_length` becomes the **max sequence length** when sharing is on (a *static* pre-allocated
  buffer, not the growing `past+1` chatterbox emits today).
- GQA derives `seqlens_k = ReduceSum(attention_mask) - 1` and `total_sequence_length` drives where the new
  K/V is written — which is exactly the left-align identity `lm_forward_batched` already relies on
  (`chatterbox.rs:659-667`), so the math is unchanged.
- "the past-present buffer sharing can be enabled or disabled **without needing to change the ONNX model**"
  is the *runtime toggle* claim [ORT past-present-share-buffer doc] — but that toggle lives in
  **onnxruntime-genai's** generation loop (it's a key in the `search` block of `genai_config.json`, "not set
  by ORT GenAI at runtime — a configuration parameter users specify" [ORT genai config reference]). For a
  **raw ORT session driven by our own AR loop**, buffer sharing is realized by *exporting* the model so
  `present.*` and `past.*` share the static max-length buffer (and binding them to one device `Value`). The
  chatterbox export today emits `present.*` at `[B,H,past+1,D]` — a fresh growing buffer — so **it needs a
  re-export** to the static-buffer form to get true in-place sharing. (Whether it can be patched
  graph-surgically vs re-exported is the effort question in §C.)

ORT's own KV-cache guidance for raw sessions is sparse: maintainers redirect to ORT GenAI for the built-in
KV cache [ORT discussion #21589; ORT-GenAI discussion #747], reinforcing that the device-resident ring-KV is
something we wire ourselves from the IoBinding + buffer-sharing primitives.

### B.3 onnxruntime-genai (ORT GenAI) — borrow the technique, not the library

ORT GenAI "provides the generative AI loop … inference with ONNX Runtime, logits processing, search and
sampling, **and KV cache management**" with a built-in device-resident cache using `past_present_share_buffer`
+ GQA. [ORT GenAI overview; ORT-GenAI discussion #747] **But it is not adoptable as WaaV's decode engine:**

- Its **model-builder (`builder.py`) supports only a fixed allowlist of text decoder-only LLMs**: "ChatGLM,
  Gemma, Granite, LLaMA, Mistral, Nemotron, Phi, Qwen, AMD OLMo" — "no evidence the tool can handle … TTS
  codec-AR decoders … outputting non-text logits (audio codecs)." [onnxruntime-genai model-builder DeepWiki]
  Chatterbox's LM outputs an 8194-way **codec** vocab, not text, and is not a buildable architecture.
- Its generation loop owns sampling/search; WaaV's AR loop owns repetition-penalty + per-slot ragged
  lockstep batching (`argmax_with_penalty`, `lm_forward_batched`) — handing the loop to GenAI would discard
  the lockstep cohort batcher entirely and is a poor fit for the multi-tenant ragged-batch serve path.
- It does NOT expose its KV-cache primitive independently of the full loop [ORT-GenAI discussion #747].

**Verdict:** GenAI validates the exact pattern we should build (`past_present_share_buffer`+GQA+IoBinding)
but is the wrong integration boundary. We replicate the *mechanism* in the `ort` backend, keeping WaaV's AR
loop and batcher.

### B.4 Export-pattern vs runtime-pattern — which applies to chatterbox

| Concern | Lever | Applies to chatterbox? |
|---|---|---|
| KV leaves GPU host↔device every stride | **Runtime: IoBinding device ping-pong** (B.1) | YES — `ort` rc.12 supports it; no re-export strictly required for *this half* |
| `present.*` re-allocated + copied to `past.*` every stride (device-side) | **Export: `past_present_share_buffer` static max-len GQA buffer** (B.2) | YES — current export emits growing `[B,H,past+1,D]`; needs static-buffer re-export to share in place |

Both apply. The runtime half alone removes the H2D/D2H wall; the export half additionally removes the device
re-alloc/copy and unlocks the flat ring-KV the 30× curve needs.

---

## C. Ranked fix options + the recommended plan

### Option 1 (RECOMMENDED) — Buffer-sharing GQA re-export + device-resident IoBinding ping-pong

**Mechanism.** (a) Re-export `language_model.onnx` with `past_present_share_buffer=1`-style static
max-length GQA buffers (BNSH `[B,16,MAX_SEQ,64]`, `present.*` aliasing `past.*`). (b) In the `ort` backend,
add a *stateful* device-KV binding: pre-allocate the 60 KV `Value`s once on CUDA via `Tensor::new(&cuda_alloc,
[B,16,MAX_SEQ,64])`, bind each as **both** `past_key_values.{i}.{k,v}` input **and** `present.{i}.{k,v}`
output (same `Value`), feed only `inputs_embeds` + `attention_mask` (+`position_ids` for turbo) +
`total_sequence_length` as varying host inputs, and `run_binding` each stride. KV never leaves the device;
the new K/V is written in place at `seqlens_k`. Only the `[B,vocab]` logits cross to host (the one bounded
per-stride D2H the ledger already allows, `chatterbox.rs:636-643`).

**Expected gain.** Removes both the `O(B·layers·heads·seq·dim)` host transfer AND the device re-alloc/copy —
the two terms that make the curve regress past B16. This is the configuration the Path-B tch device-resident
probe uses to hit **~30×@B64** (`BATCHING-ANALYSIS-SYNTHESIS.md`). Realistic target: recover most of the
~30× curve (flat per-stride wall through B16, near-linear to B64), bounded by ORT-CUDA GQA kernel efficiency
vs tch.

**WaaV effort + exact files.**
- *Export (one-time):* re-export script for `language_model.onnx` → static-buffer GQA. Acquire/build under
  `torch_runtime/models/chatterbox.py` (the existing chatterbox export source) + verify with the
  `item1_ort_cuda_registry`/`cuda_ort_gqa_*` GQA-on-CUDA tests. Ship as a new `waav.json` weights variant so
  selection is config-only (the zero-code model-registry pattern).
- *Backend seam:* extend `crates/waav-infer-backend-api/src/lib.rs` `IoBinding` to carry **device-resident
  input `Value`s** + **shared in/out buffer names** (today it only has host `NamedTensor` inputs +
  `device_outputs: Vec<String>` residency hints). Add a stateful `run_bound`-style path in
  `crates/waav-infer-backend-ort/src/lib.rs` that pre-allocates the KV `Value`s via a CUDA `Allocator`
  (`lib.rs` already constructs `MemoryInfo`/pinned memory at 515-527 — extend to `AllocationDevice::CUDA`
  device alloc) and binds shared in/out buffers, NOT `bind_output_to_device` to pinned host.
- *Decoder:* a device-KV variant of `lm_forward` / `lm_forward_batched` in
  `crates/waav-infer-core/src/tts/chatterbox.rs` that holds the 60 device `Value` handles in `SlotState`
  (replacing the host `past: Vec<NamedTensor>` at `chatterbox.rs:139`) and feeds `total_sequence_length`
  instead of rebuilding host KV. Gate on `KvResidencyRegime` (the regime arbiter at `chatterbox.rs:519` is
  already designed for exactly this — flip it to `DeviceKv` for the new export so `arm_prefix_cache_if_win`
  takes the win path too).

**Accuracy (bit-identity).** The GQA math is unchanged — `seqlens_k`/`total_sequence_length` selects the
same write index the current LEFT-align identity uses (`chatterbox.rs:659-667`), and the buffer just changes
*where* it lives, not the arithmetic. The TF32-off batch-invariance discipline (`ep.rs:81-96`) is orthogonal
and stays. Gate with the existing `batched_forward_codes_identical_to_per_slot` +
`live_ragged_batched_forward_bit_identical_and_scales` + the AR-compounding identity test — the new device
path must produce byte-identical codes to the host `run` reference. Risk to bit-identity is LOW (no
reduction-order change; same kernel).

**Risks.** (1) Re-export correctness — the GQA static-buffer reorder (the `x = 16/sizeof(T)` key reorder
[ORT ContribOperators]) must match what the ORT-CUDA GQA kernel expects; mitigated by bit-identity gating
vs the current export. (2) Static `MAX_SEQ` pre-allocation sizes device memory up front — must be bounded by
the codec-AR max audio length and fit the GB10 arena cap (`ep.rs:97-117` already caps the arena). (3)
`ort` rc.12 device-Value lifetime across `run_binding` calls is an rc API — pin behavior with a focused
device-ping-pong unit test before wiring the full decoder. (4) The ragged-cohort case still right-pads to
`max_past` within the static buffer (the ~27% pad waste, G7) — acceptable, and the static buffer makes it
free of re-alloc.

### Option 2 — IoBinding-only device ping-pong, NO re-export

**Mechanism.** Keep the existing `present.*=[B,H,past+1,D]` growing export, but in the backend bind step-N's
`present.*` device `Value` outputs as step-N+1's `past_key_values.*` device inputs (device→device), avoiding
the host round-trip. Requires the WaaV `IoBinding` extension from Option 1's backend seam, but no export work.

**Expected gain.** Removes the H2D/D2H host transfer (the dominant term) but **NOT** the per-stride device
re-allocation of the growing `present.*` buffer (ORT must allocate a new `[B,H,past+1,D]` each step because
the shape grows). On GB10's unified pool this re-alloc is itself costly and fragmenting (the documented arena
issue, `ep.rs:97-109`). Partial win — likely lands between today's 1.8× and the 30× curve, but bounded by
the growing-buffer re-alloc that buffer-sharing would remove. Honestly: a meaningful step, not the full fix.

**Effort.** Lower than Option 1 (no export pipeline): only the `backend-api` `IoBinding` device-Value
extension + a stateful ort binding path + the chatterbox device-KV decoder variant. Same files as Option 1
minus `torch_runtime/models/chatterbox.py` and the `waav.json` variant.

**Accuracy.** Same kernel/math, byte-identical (the output Value is the same data, just not copied to host) —
LOW risk. **Risk:** the growing-shape output may force ORT to *re-bind a fresh output Value each step*
(can't reuse one device buffer for a changing shape), so the device-side allocation churn — and on GB10 the
arena-fragmentation crash class (`ep.rs:98-109`, the twice-observed box kill) — remains. This is why Option 1
(static buffer) is strictly safer on GB10.

### Option 3 — Adopt onnxruntime-genai

**Mechanism.** Replace WaaV's AR decode + batcher with ORT GenAI's generation loop + built-in device-KV.

**Expected gain.** GenAI's KV cache is device-resident (`past_present_share_buffer`+GQA) so *if* the model
were buildable it would hit a similar curve — but see blockers.

**Effort / blockers — effectively a non-starter:** (1) the model-builder cannot export a codec-AR TTS
decoder (fixed text-LLM allowlist) [model-builder DeepWiki], so chatterbox's LM is unbuildable; (2) GenAI
owns sampling/search and does not expose its KV primitive standalone [ORT-GenAI #747], so WaaV's
repetition-penalty + ragged multi-tenant lockstep batcher would be discarded; (3) it adds a large external
dependency + its own `genai_config.json` lifecycle. **Recommendation: do not adopt; reuse the technique
(Option 1) instead.**

### Ranking

1. **Option 1 (buffer-sharing re-export + device IoBinding ping-pong)** — only path to the full ~30× curve;
   removes both the host transfer and the device re-alloc; safest on GB10's unified pool. **RECOMMENDED.**
2. **Option 2 (IoBinding-only)** — removes the host transfer with no export work, but leaves the
   growing-buffer device re-alloc (and the GB10 fragmentation risk); a good *incremental* landing that shares
   ~all of Option 1's backend seam, so it's a natural Phase-1 toward Option 1.
3. **Option 3 (ORT GenAI)** — wrong integration boundary; model not buildable; do not adopt.

### Recommended step-by-step plan

1. **Backend seam first (shared by Opt 1 & 2).** Extend `backend-api` `IoBinding` to carry device-resident
   input `Value`s + shared in/out buffer names; add a CUDA `Allocator`-backed stateful binding path in
   `backend-ort/src/lib.rs` (`Tensor::new(&cuda_alloc, …)`, `bind_input(&Value)`, shared-buffer
   `bind_output(Value)`). Land a focused **device-ping-pong unit test** that binds one output `Value` as the
   next input and asserts the result is byte-identical to the host `run` loop — pins the rc.12 device-Value
   lifetime semantics.
2. **Phase 1 = Option 2 (no re-export):** wire the chatterbox device-KV `lm_forward`/`lm_forward_batched`
   variant against the *current* growing export. Gate bit-identity (`batched_forward_codes_identical_to_per_slot`,
   AR-compounding identity). Re-measure the B1..B64 curve; expect a partial gain. This proves the seam end to
   end and is shippable on its own.
3. **Phase 2 = Option 1 (re-export):** re-export `language_model.onnx` to the static max-length
   `past_present_share_buffer` GQA form (via `torch_runtime/models/chatterbox.py`), ship as a `waav.json`
   weights variant, flip `KvResidencyRegime` → `DeviceKv`. Bind the 60 KV `Value`s once per cohort, feed
   `total_sequence_length`. Re-gate bit-identity vs the host reference and re-measure toward ~30×.
4. **Promote the doc constants** (`CHATTERBOX_HEADLINE_PEAK_BATCH_SPEEDUP`/`_BATCH`, `chatterbox.rs:27-30`)
   ONLY after the live `live_headline_batched_scaling_matches_doc_curve` gate re-measures the new curve — per
   the existing rule (`chatterbox.rs:24-26`).
5. **GB10 guardrails:** size `MAX_SEQ` from the codec-AR audio bound; keep the arena cap (`ep.rs`); add the
   G6 cohort-width histogram so the new curve is observable live.

---

## Cited sources

WaaV source (absolute paths):
- `/home/bud/ditto/waav/waav-infer/crates/waav-infer-core/src/tts/chatterbox.rs` — host-KV doc header
  (lines 55–79), measured curve (68–70, 27–30), `lm_forward` (604–648), `lm_forward_batched` (683–839),
  `feedback_present_kv` (1829–1839), `empty_split_kv` (1803–1824), `KvResidencyRegime` (519–531), KV geometry
  (454–455), GQA left-align identity (659–667), `run_bound` re-scope rationale (589–603).
- `/home/bud/ditto/waav/waav-infer/crates/waav-infer-backend-ort/src/lib.rs` — `OrtModel::run` (438–460),
  `run_bound` (489–566), `to_ort_value` (352–397), `extract_named_output` (406–435), pinned-host output
  binding (515–527), `perf_lever_run_bound_vs_run_on_real_chatterbox_lm_cuda` (1456–1574).
- `/home/bud/ditto/waav/waav-infer/crates/waav-infer-backend-ort/src/ep.rs` — CUDA EP / arena cap / TF32-off
  discipline (37–131).
- `/home/bud/ditto/waav/waav-infer/crates/waav-infer-backend-api/src/lib.rs` — `IoBinding` type (241–336).
- `/home/bud/ditto/waav/waav-infer/Cargo.toml` (line 43) + `Cargo.lock` — `ort = "=2.0.0-rc.12"`,
  `features=[…, api-24, load-dynamic, half]`.
- `/home/bud/ditto/waav/WaaV/inferv2/REVIEW/BATCHING-ANALYSIS-SYNTHESIS.md` — the 1.8× vs 30× framing (G1).

External (URLs):
- ONNX Runtime — Past Present Share Buffer (GenAI how-to): https://onnxruntime.ai/docs/genai/howto/past-present-share-buffer.html
- ONNX Runtime — Device tensors / IoBinding ping-pong: https://onnxruntime.ai/docs/performance/device-tensor.html
- ONNX Runtime — I/O Binding (tune-performance): https://onnxruntime.ai/docs/performance/tune-performance/iobinding.html
- ONNX Runtime — Accelerating LLaMA-2 (GQA + buffer sharing): https://onnxruntime.ai/blogs/accelerating-llama-2
- ONNX Runtime — GenAI config reference (`past_present_share_buffer` key, decoder I/O names): https://onnxruntime.ai/docs/genai/reference/config.html
- ONNX Runtime — Generate API (GenAI) overview: https://onnxruntime.ai/docs/genai/
- ONNX Runtime — ContribOperators (GroupQueryAttention BNSH / share-buffer format): https://github.com/microsoft/onnxruntime/blob/main/docs/ContribOperators.md
- ONNX Runtime — KV cache APIs discussion #21589: https://github.com/microsoft/onnxruntime/discussions/21589
- onnxruntime-genai — KV cache APIs discussion #747: https://github.com/microsoft/onnxruntime-genai/discussions/747
- onnxruntime-genai — Model Builder & Quantization (architecture allowlist): https://deepwiki.com/microsoft/onnxruntime-genai/5-model-builder-and-quantization
- onnxruntime-genai — repo: https://github.com/microsoft/onnxruntime-genai
- `ort` crate — releases / multiversioning (v1.17–v1.24, api-24 ⇒ ORT 1.24): https://github.com/pykeio/ort/releases
- `ort` crate — version mapping: https://ort.pyke.io/migrating/version-mapping
- `ort` crate — IoBinding (rc API: bind_input/bind_output/bind_output_to_device): https://docs.rs/ort/2.0.0-rc.10/ort/io_binding/struct.IoBinding.html
- `ort` crate — Value (CUDA Allocator + Tensor::new device alloc + into_dyn): https://docs.rs/ort/2.0.0-rc.10/ort/value/struct.Value.html
- `ort` crate — Session::run_binding → SessionOutputs: https://docs.rs/ort/2.0.0-rc.10/ort/session/struct.Session.html
- `ort` crate — crates.io: https://crates.io/crates/ort
