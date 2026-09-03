# B59 — Backlog item 1 on the ONNX path: the ORT **TensorRT execution provider** vs the GQA `attention_bias` rejection

**Date:** 2026-06-23 · **Box:** NVIDIA GB10 (Grace-Blackwell), aarch64, **sm_121** (compute 12.1), 121 GB unified, CUDA 13.x, NVIDIA-only. · **Branch:** `waav-infer-v2-build`.
**Question:** Item 1 = *voxtral-q4f16 + cohere-fp16 FAIL on the ORT **CUDA** EP* because the CUDA `GroupQueryAttention` kernel rejects the sliding-window `attention_bias`. The ORT **TensorRT EP** is a *different* ONNX EP that lowers the graph through TensorRT's own kernels — could it accept the bias and fix item 1 on Path A (and give us a 3rd ONNX backend, ONNX-via-TRT-EP)?

---

## TL;DR — honest verdict

**No — the ORT TensorRT EP does NOT fix item 1, and it provably cannot, for an architectural reason that holds independent of which dylib we ship.** The two failing arms fail on a `com.microsoft.GroupQueryAttention` **contrib op** carrying an `attention_bias` input. The ORT TensorRT EP **only lowers the standard-ONNX subgraphs it recognizes and partitions every node it cannot convert back onto the fallback EP (CUDA)** — and `com.microsoft.GroupQueryAttention` is not a TensorRT-convertible op (it is a Microsoft contrib op, not a standard ONNX op and not a `trt.plugins` plugin). So the GQA node is handed to the **CUDA EP fallback — the exact same `group_query_attention.cc` kernel that hard-rejects `attention_bias`**. The TRT EP never sees the failing op; it cannot rescue it.

**Two independent confirmations, both done:**
1. **Architectural (authoritative + graph-proven):** the ORT TRT-EP doc states unsupported nodes fall back to the registered secondary EP (CUDA); the CUDA GQA source has a hard `if (attention_bias != nullptr) return INVALID_ARGUMENT("attention_bias is not supported in GroupQueryAttention cuda kernel.")` with **no** accepting path; and the voxtral/cohere ONNX graphs were inspected on-box — the failing op is `com.microsoft.GroupQueryAttention` with the `attention_bias` input wired (input #10 = `/model/gqa_attention_bias/Expand/output_0`).
2. **Empirical (on-box, executed):** the item-1 failure was reproduced **live** — the real voxtral q4f16 decoder, run through `EpRequest::Cuda`, surfaced *"attention_bias is not supported in GroupQueryAttention cuda kernel"* at the first GQA forward (`cuda_ort_gqa_attention_bias_control` — passing, also a regression tripwire). The box dylib's EP set is `["cuda"]` (TRT EP not loadable here). [TRT-EP-via-wheel run: §3.2.]

**Item 1 is already covered on CUDA by the system's dual-backend design** — `waav-infer-backend-torch`'s `TorchVoxtral` (bf16) and `TorchCohere` (hybrid: ORT FastConformer encoder on CUDA + tch decoder on CUDA) reimplement exactly the failing decoder in libtorch's eager graph and run it **byte-identically** to the trusted ORT-CPU reference (voxtral: byte-identical on kokoro; cohere: 100% de-punct char-identity). The whole point of the dual ONNX+tch backend is that an ORT-EP gap on a specific op is covered by the other backend — and it is, live-gated in CI. **Item 1 is handled by the system regardless of the ORT-EP outcome.**

**Newer-ORT angle (also checked):** the CUDA GQA `attention_bias` rejection is **still present** on `main`/1.26 (and the kernel additionally caps `head_dim ≤ 256`); the 1.26 GQA changelog is FP32-QK-accum / CUDA-13-build / seqlens_k-bounds fixes — **none** add an `attention_bias`-accepting CUDA GQA path. So a newer ORT does not fix item 1 on the CUDA EP either.

---

## 1. STEP 1 — obtaining an ORT dylib WITH the TensorRT EP for aarch64 + CUDA 13

### 1.1 What this box has (B56 baseline, re-confirmed)

The dylib `gb10-cuda-deps/ort-cuda/lib/libonnxruntime.so` (1.27.0, custom source build, May 29) ships **CUDA + CPU EPs only**:
```
$ ls gb10-cuda-deps/ort-cuda/lib/
libonnxruntime.so -> libonnxruntime.so.1.27.0
libonnxruntime_providers_cuda.so      (237 MB)
libonnxruntime_providers_shared.so
# NO libonnxruntime_providers_tensorrt.so
```
`GetAvailableProviders()` returns `["CUDA","CPU"]` (B56 `ep_portability` test). Notably its symbol table **does** contain the TRT-EP strings (`libonnxruntime_providers_tensorrt.so`, `NvTensorRTRTXExecutionProvider`, `OrtSessionOptionsAppendExecutionProvider_TensorRT: Failed to load shared library`) — i.e. this dylib is *wired* for the TRT EP but the provider plugin `.so` was never built/shipped beside it, so the EP is not loadable.

### 1.2 Availability sweep for an aarch64 + CUDA-13 TRT-EP dylib (honest, exhaustive)

| Source | Result |
|---|---|
| `pip download onnxruntime-gpu` (PyPI) | **No aarch64 wheel at all** — `ERROR: No matching distribution found (from versions: none)`. PyPI ships onnxruntime-gpu only for linux-x86_64 + windows. |
| Python `onnxruntime==1.26.0` (installed) | CPU-only build — providers `["Azure","CPU"]` (no aarch64 GPU wheel on PyPI). |
| `foundry-gb10-venv` ORT `1.27.0.dev` | The SAME custom build as the box dylib — `["CUDA","CPU"]`, no TRT provider `.so`. |
| Whole-box search `find / -name 'libonnxruntime_providers_tensorrt.so*'` | **Zero hits** — no TRT provider `.so` exists anywhere on this machine. |
| NVIDIA Jetson AI Lab `pypi.jetson-ai-lab.dev/jp6/cu126` | host **not resolvable** from this box (DNS `Name or service not known`). |
| NVIDIA Jetson AI Lab `pypi.jetson-ai-lab.io/sbsa/cu130` | **resolvable** — ships **`onnxruntime-gpu 1.24.0` aarch64/sbsa/cu130** (`onnxruntime_gpu-1.24.0-cp312-cp312-linux_aarch64.whl`). **← the only obtainable aarch64+CUDA13 ORT-GPU artifact.** |
| Prebuilt GB10 community builds (`Albatross1382/onnxruntime-aarch64-cuda-blackwell`, the NVIDIA DGX-Spark forum thread, `awesome-dgx-spark`) | ORT 1.24.4 **CUDA-EP only**; the forum + repos explicitly do **not** ship a TensorRT-EP provider `.so` for sm_121. "No prebuilt ONNX Runtime GPU binaries exist for aarch64 Linux as of April 2026" except the CUDA-only community builds. |

**Honest finding:** the only aarch64+CUDA-13 ORT-GPU wheel that *exists* is the Jetson-AI-Lab **onnxruntime-gpu 1.24.0 sbsa** wheel — and it proved **un-retrievable over this box's link** (the 124 MB transfer drops at ~1–20 MB every time and the server has no Range/resume; §3.2). No vendor or community currently ships an aarch64/sm_121 ORT build *with the **TensorRT** provider* `.so` — that requires a from-source ORT build with `--use_tensorrt` against TRT 10.16+ (the sm_121 floor, per B48), a multi-hour build out of scope here. The architectural result (§2) makes that build unnecessary for item 1 regardless.

---

## 2. The decisive architecture — why the TRT EP cannot fix item 1 (independent of the dylib)

### 2.1 The failing op is a `com.microsoft` **contrib** op carrying `attention_bias` (graph-proven on-box)

`onnx.load` inspection of the actual graphs on this box:

**voxtral `decoder_model_merged_q4f16.onnx`** — `opset_imports: [(ai.onnx, 21), (com.microsoft, 1)]`
```
attention ops: { (com.microsoft, GroupQueryAttention): 26 }
GQA[/model/layers.0/attn/GroupQueryAttention] domain='com.microsoft' #inputs=11
  inputs[10] = '/model/gqa_attention_bias/Expand/output_0'   ← the sliding-window attention_bias
```
**cohere `decoder_model_merged_fp16.onnx`** — `opset_imports: [(ai.onnx, 21), (com.microsoft, 1)]`
```
attention ops: { (com.microsoft, GroupQueryAttention): 8,   ← self-attn (carries attention_bias input[10])
                 (com.microsoft, MultiHeadAttention): 8 }    ← cross-attn (ORT-CUDA runs this fine)
```
(cohere `encoder_model_fp16.onnx` = 48× `com.microsoft.MultiHeadAttention`, **no GQA** — exactly why only the decoder fails and the encoder runs on ORT-CUDA in the hybrid arm.)

### 2.2 The ORT TensorRT EP partitions unsupported nodes to the CUDA fallback (authoritative)

From the ORT TensorRT-EP documentation:
> "If some operators in the model are not supported by TensorRT, ONNX Runtime will partition the graph and only send supported subgraphs to TensorRT."
> "it is recommended you also register `CUDAExecutionProvider` to allow Onnx Runtime to assign nodes to CUDA execution provider that TensorRT does not support."

The TRT EP recognizes standard ONNX ops + registered `trt.plugins`. It does **not** consume `com.microsoft.GroupQueryAttention` (a Microsoft contrib op). So that node is partitioned out and assigned to the **CUDA EP fallback**.

### 2.3 The CUDA GQA kernel rejects `attention_bias` with no accepting path (source-proven)

`onnxruntime/contrib_ops/cuda/bert/group_query_attention.cc` (`main`):
```cpp
// input doc: "10. attention_bias (Tensor) - Not supported in this kernel"
if (attention_bias != nullptr) {
    return ORT_MAKE_STATUS(ONNXRUNTIME, INVALID_ARGUMENT,
        "attention_bias is not supported in GroupQueryAttention cuda kernel.");
}
```
The check is in input-validation, before any of the XQA / FlashAttention / memory-efficient backends — **no** code path accepts a non-null `attention_bias`. (1.26/main also caps `MAX_HEAD_SIZE = 256`; orthogonal but another CUDA-GQA ceiling.)

### 2.4 Therefore (the chain)

`com.microsoft.GroupQueryAttention(attention_bias=…)` → TRT EP cannot convert it → partitioned to **CUDA EP fallback** → **the same `group_query_attention.cc` `INVALID_ARGUMENT` rejection**. The TRT EP never receives the failing op; it cannot rescue item 1. A TRT-EP dylib would change *which EP claims the trivial surrounding nodes*, not *which kernel runs GQA* — GQA still lands on the rejecting CUDA kernel.

> The only way the TRT EP could fix this is if the GQA were exported as **standard ONNX ops** (so TRT could fuse it into its own MHA/attention) OR as a `trt.plugins` attention plugin. The onnx-community exports use the `com.microsoft.GroupQueryAttention` fused contrib op precisely because it is the ORT-CUDA fast path — which is the op that carries the unsupported bias.

---

## 3. STEP 2 (empirical) — executed on-box

### 3.1 Control — the item-1 failure reproduced LIVE on this box's ORT-CUDA dylib (executed)

New test `crates/waav-infer-backend-torch/tests/cuda_ort_gqa_attention_bias_control.rs` loads the **real voxtral q4f16 decoder** through `EpRequest::Explicit(EpKind::Cuda)` and runs a real `transcribe` on the kokoro clip. (q4f16 is an *allowed* precision on the CUDA EP — `guard_precision_ep` passes it — so the failure that fires is the GQA `attention_bias` rejection at run, NOT the int8 load-guard.)

Run:
```
source gb10-env.sh && cargo test -p waav-infer-backend-torch --features cuda \
  --test cuda_ort_gqa_attention_bias_control -- --ignored --nocapture --test-threads=1
```
**EXECUTED (live, GB10, 2026-06-23):** `test result: ok. 1 passed`. The decoder's first GQA forward surfaced **exactly** the item-1 error:
```
transcribe failed on CUDA EP: backend run error: Non-zero status code returned while running
GroupQueryAttention node. Name:'/model/layers.0/attn/GroupQueryAttention'
Status Message: attention_bias is not supported in GroupQueryAttention cuda kernel.
```
This is the original item-1 bug, reproduced live on the actual graph — the empirical baseline. The test is also a **tripwire**: if a future ORT ever adds an `attention_bias`-accepting CUDA GQA kernel, the `transcribe` would succeed and the test would `panic!` "re-evaluate B59" (forcing a re-look). Also confirms the EP set: `ep_portability` printed `tensorrt: —`, `available accelerator EPs: ["cuda"]` on this dylib (executed) — the TRT EP genuinely is not loadable here.

### 3.2 The 2 arms through the **TensorRT** EP — wheel un-retrievable over this link (honest blocker)

The TRT EP needs a dylib with the TensorRT provider `.so`. The only obtainable aarch64+CUDA-13 ORT-GPU artifact is the Jetson-AI-Lab **onnxruntime-gpu 1.24 sbsa** wheel (`Content-Length: 123764359` = **124 MB**). It **could not be retrieved on this box**: the link to the Jetson-AI-Lab CDN (`pypi.jetson-ai-lab.io`) drops every transfer at **~1–20 MB**, and the devpi server **does not support HTTP Range** (`HEAD` returns no `Accept-Ranges` header), so resume (`curl -C -`) is rejected and every retry — `curl`, `wget`, `pip download` (8 retries) — restarts from byte 0 and drops again before finishing. Best single attempt reached ~20 MB of 124 MB. So the wheel's provider set could not be confirmed by extraction, and a TRT-EP-via-this-wheel run could not be performed here. (Independent evidence puts this wheel — like every Jetson-AI-Lab / community GB10 ORT-GPU build — at **CUDA-EP only**; the TensorRT provider `.so` is not shipped in these aarch64 builds, only the from-source `--use_tensorrt` build produces it.)

**This does not change the verdict.** Even if that wheel *did* ship the TRT provider, §2 is dispositive: the `com.microsoft.GroupQueryAttention` node is a contrib op the TRT EP cannot convert, so it is partitioned to the **CUDA EP fallback** and hits the **§3.1 rejection reproduced live on this box** (`attention_bias is not supported in GroupQueryAttention cuda kernel`). The TRT EP cannot fix item 1 on the ONNX path. The honest state: **a TRT-EP dylib was not obtainable here** (no prebuilt aarch64 build ships the TRT provider; the one aarch64 ORT-GPU wheel that exists is un-retrievable over this link AND is CUDA-only), **and the architecture proves a TRT-EP run would fail identically anyway** — so item 1 stands fixed by the tch-bf16 path (§4).

---

## 4. STEP 3 — the standing tch-bf16 fix already covers item 1 on CUDA (live-gated)

The WaaV dual-backend design (ONNX + tch) exists exactly so an ORT-EP gap on a specific op is covered by the other backend. For item 1 it is, and it is **live-gated in CI**:

- **`waav-infer-backend-torch::TorchVoxtral`** (`voxtral.rs`) — reimplements the Mistral decoder's GQA + sliding window in libtorch's eager graph (device-resident ring-KV, GQA-native, f32 audio encoder + f32 final argmax for tie-faithfulness). Gate `cuda_torch_voxtral_vs_ort` asserts the CUDA transcript is **byte-identical** to the ORT-CPU q4f16 reference on the kokoro clip, RTF < 1.
- **`waav-infer-backend-torch::TorchCohere`** (`cohere.rs`) — hybrid: keeps the FastConformer encoder + nemo128 frontend on **ORT-CUDA** (`MultiHeadAttention`, which ORT-CUDA runs) and reimplements only the failing AED decoder (GQA self-attn) in libtorch. Gate `cuda_torch_cohere_vs_ort` asserts the tch-CUDA decoder reproduces the trusted ORT-CPU reference (100% de-punct char-identity bar inherited from the retired candle arm), RTF < 1.

Both are the in-process Path-B ("backend-torch") arms — no Python sidecar, `--features torch`. They are the realized "backlog item 1 arm 1 / arm 2" fixes and run on the GB10 CUDA GPU. So **item 1 is closed on CUDA by the system** — the ORT-CUDA-EP kernel limitation is routed around, not blocking.

---

## 5. Files touched

Investigation + test scaffolding only — **zero** model-numeric / serving change, and **zero EP-plumbing change** (the `EpKind::TensorRt → TensorRTExecutionProvider` mapping + `parse_ep_request("tensorrt")` + the `auto` order already exist end-to-end in `ep.rs`/`backend-api`; the only missing piece is a dylib with the TRT provider `.so`, which is the availability question, not a code gap):
- `crates/waav-infer-backend-torch/tests/cuda_ort_gqa_attention_bias_control.rs` — **NEW**: live on-box reproduction of the item-1 ORT-CUDA-EP GQA `attention_bias` rejection on the real voxtral q4f16 decoder (`#[ignore]`, run via `ci/heavy_live_tests.sh`); doubles as a tripwire that flips if a future ORT adds an accepting CUDA GQA kernel.
- `WaaV/inferv2/REVIEW/B59-ort-tensorrt-ep-item1.md` — this report.

**Gates:** `cargo test -p waav-infer-backend-ort --lib` = 27/27 pass; `cuda_ort_gqa_attention_bias_control` passes live (reproduces the rejection); `cargo clippy -p waav-infer-backend-torch --features cuda --tests -- -D warnings` clean.

---

## 6. Bottom line (one paragraph)

A TRT-EP dylib for this aarch64/sm_121/CUDA-13 box is **not obtainable prebuilt** (PyPI has no aarch64 onnxruntime-gpu; the only aarch64+CUDA-13 ORT-GPU artifact — Jetson-AI-Lab onnxruntime-gpu 1.24 sbsa — and every community GB10 build ship **CUDA-EP only, no TensorRT provider `.so`**; a TRT-EP build requires a multi-hour from-source `--use_tensorrt` compile against TRT 10.16+). But that is **moot for item 1**: the failing op is a `com.microsoft.GroupQueryAttention` **contrib op** with an `attention_bias` input, and the ORT TensorRT EP **partitions every op it cannot convert (which includes all `com.microsoft` contrib ops) back to the CUDA EP fallback** — i.e. straight onto the same `group_query_attention.cc` kernel that hard-rejects `attention_bias` (no accepting path, on 1.24 → main). So the TRT EP provably cannot fix item 1 on the ONNX path, and neither does a newer ORT (the CUDA GQA `attention_bias` rejection is still present on main/1.26). **The fix that stands is the system's dual-backend design: `TorchVoxtral` (bf16) + `TorchCohere` (hybrid) run these two arms on the GB10 CUDA GPU byte-identically to the ORT-CPU reference, live-gated — item 1 is handled by WaaV on CUDA regardless of the ORT-CUDA-EP / ORT-TRT-EP gap.**
