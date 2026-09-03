# ORT‑GenAI for WaaV Path‑A Codec‑AR Serving — Deep‑Dive + Verdict

**Date:** 2026‑06‑25. **Question:** Does Microsoft's **onnxruntime‑genai** (ORT‑GenAI) give WaaV‑Infer the
**device‑resident‑KV + continuous‑batching + paged‑attention** serving it lacks on **Path A (ONNX Runtime)** "for
free," or is the right move to **port its device‑resident‑KV technique** (buffer‑sharing GQA) into WaaV's own
codec‑AR batcher? Grounded in `BATCHING-ANALYSIS-SYNTHESIS.md` (the Path‑A ceiling) and `VLLM-PARITY-MATRIX.md`
(the live serving spine).

---

## TL;DR — VERDICT

**Do NOT adopt ORT‑GenAI as WaaV's Path‑A codec‑AR serving runtime. Adopt the ONE technique it proves — the
buffer‑sharing GQA static device‑resident KV cache (`past_present_share_buffer` over a pre‑allocated max‑length
buffer) — by re‑exporting WaaV's codec‑AR LM graph in that form and binding present↔past to one device buffer in
WaaV's own batcher.** That is the precise fix for the documented G1 ceiling (host‑KV re‑stream caps Path‑A at
~1.8×@B16 and regresses to ~1.0×@B64). ORT‑GenAI is a **reference for the technique and a candidate graph‑export
recipe, not a serving substitute.**

Why not adopt the runtime:

1. **It is NOT a continuous‑batching server.** ORT‑GenAI's "continuous decoding" is *single‑stream incremental
   decoding* (modify a generator's input between `GenerateNextToken` calls without recreating it), **not**
   vLLM‑style mid‑flight add/evict of independent ragged requests into a shared batch. Its batch is **static**:
   `batch_size` is fixed at generator creation, rectangular, padded. This is the *exact* thing WaaV's
   `CodecArBatcher` already does and ORT‑GenAI does not — so it would be a **downgrade** of pillar #1
   (Continuous/dynamic batching, already HAVE‑LIVE).
2. **It has NO paged attention.** Paged KV is explicitly framed as a *future design consideration*, not implemented
   or roadmapped (issue #1989, Feb 2026). So it cannot supply the missing pillar #2 either.
3. **It targets text/vision LLMs + Whisper through a fixed text‑decoder generation loop.** A codec‑AR TTS decoder
   needs a **multi‑codebook head, per‑slot acoustic‑delay ring, and CFG (cond/uncond)** — none of which the
   ORT‑GenAI loop models. The `get_logits`/`set_logits` seam is *necessary but not sufficient*; you would
   reimplement WaaV's entire audio glue *inside* a runtime you don't control, while *losing* the live ragged
   batcher.
4. **No Rust binding.** Bindings are C/C++/Python/C#/Java (Obj‑C WIP). WaaV is Rust; adoption means a C‑FFI shim
   plus a second ONNX‑Runtime session lifecycle alongside WaaV's `ort` usage — and the **bit‑identity bar** would
   have to be re‑established against an opaque third‑party generation loop.

What **does** transfer, cleanly and with high value:

- **The device‑resident static‑KV technique** (buffer‑sharing GQA) → port into WaaV's codec‑AR LM export. **This is
  the G1 lever.**
- **Whisper batch insight** for the **ASR path** (multi‑audio rectangular batch) — but WaaV already has the
  whisper cohort batcher, so this is corroboration, not new capability.
- ORT‑GenAI's **multi‑LoRA adapter API** (`OgaCreateAdapters`/`OgaSetActiveAdapter`) is a *design reference* for the
  MISSING pillar #4, not a drop‑in (wrong runtime, wrong models).

---

## 1. What ORT‑GenAI actually provides (sourced)

ORT‑GenAI wraps ONNX Runtime with "the generative AI loop … tokenization and other pre‑processing, inference with
ONNX Runtime, logits processing, search and sampling, and KV cache management." [docs/genai]

**Core API (C / C++ / Python / C# / Java; Obj‑C under development; no Rust).** [README, python.html, c.html]

| Class | Role |
|---|---|
| `Model` | owns the ORT session + config (`genai_config.json`) |
| `Tokenizer` / `TokenizerStream` | text↔token; chat template; `encode_batch`/`decode_batch` |
| `GeneratorParams` | search opts (temp/top‑k/top‑p/`num_beams`/`max_length`/`batch_size`), `try_graph_capture_with_max_batch_size`, `set_guidance` (constrained decode) |
| `Generator` | the decode loop |
| `MultiModalProcessor` | `__call__(prompt, images, audios)` — the audio/vision frontend |
| `Adapters` | LoRA registry |

**The low‑level generation seam (the part that matters for codec‑AR):** [python.html, c.html]
`append_tokens` / `OgaGenerator_AppendTokens`, `generate_next_token`, `get_logits` / `OgaGenerator_GetLogits`,
**`set_logits` / `OgaGenerator_SetLogits`** (supply *custom* logits), `rewind_to` / `OgaGenerator_RewindTo`
(roll the sequence back to a length — KV truncation), `get_output(name)` (named output tensors),
`set_active_adapter` / `OgaSetActiveAdapter`.

**KV‑cache management** [docs/genai, discussion #747, config.html]:
- *Built‑in, not user‑exposed as a manipulable object.* Maintainer (#747): "onnxruntime‑genai currently has a
  built‑in implementation of the kv caching"; **no public API to directly access/manipulate/share KV between
  sequences**, no paged attention.
- Three internal cache classes from `builder.py`: **`DefaultKeyValueCache`, `CombinedKeyValueCache`,
  `WindowedKeyValueCache`** — i.e. contiguous/windowed, **not paged**.
- **`past_present_share_buffer`** (the key technique — see §3): present KV bound to the same memory block as past KV;
  cache sized `batch_size × num_kv_heads × max_length × head_size`, **pre‑allocated to `max_length` up‑front**, BNSH
  layout `(batch, heads, seq, head_size)`. [past-present-share-buffer.html, ContribOperators/GQA]

**The model‑builder (`builder.py`)** converts HF checkpoints → GenAI‑format ONNX with the **GroupQueryAttention**
contrib op (Flash‑Attention‑V2 kernels) and emits `genai_config.json`. GQA "supports past‑present buffer sharing …
By binding the present KV caches to the past KV caches, there is no need to allocate separate on‑device memory for
both," and "past KV caches can be pre‑allocated … so that no new on‑device memory needs to be requested during
inference" — i.e. **static, device‑resident, zero per‑step realloc.** [accelerating‑llama‑2 blog, ORT PR #23061,
GQA contrib‑op docs]

**Features (README, verbatim status):** "Multi‑LoRA, Continuous decoding, Constrained decoding" = **supported**;
**"Speculative decoding" = under development.** Backends: CPU, CUDA, DirectML, NvTensorRtRtx, OpenVINO, QNN, WebGPU
(AMD GPU WIP). **CUDA‑graph** capture via `try_graph_capture_with_max_batch_size` (bucketed batch sizes; "WebGPU
supports continuous decoding with RewindTo and graph capture").

### The two load‑bearing negative findings

- **"Continuous decoding" ≠ continuous batching.** Discussion #858: it lets you "modify the input of the generator
  before the next prediction without having to recreate the generator" — chat/typing single‑stream use. "The use
  cases discussed are single‑stream scenarios … rather than server‑scale concurrent request handling." No mid‑flight
  add/evict of independent ragged requests. **Batch is static**, fixed at `batch_size` (rectangular, padded).
- **No paged attention.** Only contiguous/windowed contiguous caches. Paged‑KV‑for‑decode is a *Pros* bullet in a
  design discussion (#1989, Feb 2026), "not as existing functionality … no … stated roadmap."

---

## 2. Applicability to VOICE codec‑AR

WaaV's codec‑AR TTS decoders are LLM‑style (Llama/Qwen backbones emitting **audio codes**). The honest split:

**Adapts cleanly:**
- *The backbone forward + static KV.* A Llama/Qwen codec backbone is exactly what `builder.py`/GQA target; the
  device‑resident buffer‑sharing KV would work on the acoustic backbone.
- *Custom logits.* `get_logits`/`set_logits` is the precise hook to (a) read backbone logits, (b) run WaaV's
  multi‑codebook sampling/CFG mix externally, (c) write back the chosen path — `rewind_to` gives KV rollback for
  eager‑EoT revision (WaaV's `EagerStage`).
- *Whisper ASR.* Runs through the **same `Generator` API** via `create_multimodal_processor()` +
  `Audios.open()` + beam search; multi‑audio **rectangular batch** (`batch_size = len(audios)`). [whisper page]

**Does NOT adapt (the audio glue):**
- **The audio frontend** — mel/STFT/codec‑encode is WaaV's bit‑faithful kaldi‑fbank / codec path; ORT‑GenAI's
  `MultiModalProcessor` is a *different* (Whisper‑shaped) preprocessor, not WaaV's.
- **The codec decode / vocoder** — ORT‑GenAI's loop ends at *token sequences*; turning audio codes → PCM
  (RVQ/codec decoder + vocoder) is entirely outside it and stays WaaV's.
- **The delay‑pattern multi‑codebook head** — ORT‑GenAI models *one* token stream per sequence; WaaV's per‑slot
  acoustic‑delay ring (delaying codebook k by k frames, `acoustic_delay_ring_delays_by_k_frames`) has **no analog**.
  You'd emulate it via `set_logits` gymnastics per codebook — fighting the runtime.
- **CFG (classifier‑free guidance)** — WaaV runs cond+uncond as a B=2 batch and mixes logits (byte‑identical:
  dia2 608/608, csm 544/544). ORT‑GenAI has no CFG concept; you'd allocate two sequences and mix via `set_logits` —
  doable but, again, reimplementing WaaV glue inside a foreign loop.
- **Ragged concurrent streams** — WaaV's live `step_batch` fans N ragged WS streams into one lockstep loop with
  add/evict mid‑flight (`live_gb10_batcher_concurrent_ragged_is_bit_identical_and_scales`). ORT‑GenAI's static batch
  **cannot** do this. This is a capability WaaV would *lose*, not gain.

**Net:** the *backbone + static‑KV* slice maps; the *audio identity surface* (frontend, codec decode, delay‑pattern,
CFG, ragged multiplexing) is exactly the part WaaV must keep — and is exactly where the bit‑identity bar lives.

---

## 3. vs the WaaV native batcher — the real decision

**WaaV's documented G1 ceiling:** chatterbox codec‑AR (Path A, the *wired* live path) peaks ~1.77×@B16 and
**regresses to ~1.06×@B64** because the `language_model.onnx` "threads KV through HOST every forward"
(`chatterbox.rs:1369`) — the host‑KV re‑stream wall. The device‑resident tch probe scales ~30×@B64. `BATCHING-
ANALYSIS-SYNTHESIS.md` names the only fix: "**device‑resident ring‑KV re‑export** is the only path to near‑linear."

**What ORT‑GenAI's buffer‑sharing GQA gives:** precisely a **device‑resident, statically pre‑allocated, present↔past
shared KV** — no per‑step H2D/D2H re‑stream, no realloc. **That is the G1 lever, as a graph‑export recipe.**

**The decision is therefore about the *export*, not the *runtime*:**

| Option | Gets device‑resident KV? | Keeps live ragged batcher? | Keeps bit‑identity guarantee? | Keeps audio glue (delay/CFG/codec)? | Rust‑native? | Effort |
|---|---|---|---|---|---|---|
| **A. Adopt ORT‑GenAI runtime** | yes (static, B‑fixed) | **NO** (loses continuous batching) | **must re‑establish** vs opaque loop | **NO** (reimplement inside foreign loop) | **NO** (C‑FFI) | very high + ongoing |
| **B. Port the technique into WaaV's batcher** (re‑export codec‑AR LM with GQA buffer‑sharing static KV; bind present↔past on a pre‑alloc max‑len device buffer; keep `CodecArBatcher`/`ArStepModel`) | **yes** | **yes** | **yes** (same `ort` session, same IoBinding seam already in WaaV) | **yes** | **yes** | **focused** — graph re‑export + IoBinding the shared KV in `step_batch` |

**Option B is strictly better.** It targets the exact bottleneck WaaV diagnosed, keeps every pillar WaaV already has
LIVE (continuous batching #1, admission #8, dynamic shapes #11, CUDA graphs #10), and stays inside WaaV's `ort`‑based
bit‑identity discipline. The buffer‑sharing GQA pattern + `try_graph_capture_with_max_batch_size` (bucketed
batch + CUDA‑graph) is the *blueprint*; WaaV implements it natively.

**Concrete what‑it‑would‑take (Option B):**
1. **Re‑export** the codec‑AR backbone (`language_model.onnx`) to the GQA buffer‑sharing form: replace the host‑KV
   in/out edges with a GroupQueryAttention contrib‑op graph using `past_present_share_buffer` over a `max_length`
   pre‑allocated BNSH buffer (`builder.py` is the reference producer; or hand‑author the GQA subgraph).
2. **IoBinding the shared KV device buffer** across strides in `CodecArBatcher::step_batch` (WaaV already uses the
   IoBinding‑on‑StaticGraph seam — INFER_PERF #1 lever) so present binds onto past with zero host transfer; ring‑wrap
   at `max_length`.
3. **Gate bit‑identity:** prove batched‑cohort codes are still token‑for‑token identical to per‑stream solo
   (the existing `live_gb10_batcher_concurrent_ragged_is_bit_identical_and_scales` is the gate to keep green) and
   that the device‑resident export equals the host‑KV export's decode byte‑for‑byte.
4. **Re‑measure the knee:** expect the curve to move from ~1.8×@B16 toward the tch probe's ~30×@B64 envelope on CUDA.

**Risks (all on the audio/identity surface, all inside WaaV's control):**
- **Bit‑identity of the re‑export.** GQA/Flash‑Attention‑V2 kernels reassociate reductions; like the chunked‑prefill
  scar, intermediate state can differ sub‑ULP. Must gate that the *decoded codes* stay identical (greedy argmax
  invariant), not just close. WaaV has the exact playbook for this (the 8‑bug byte‑identity discipline, fused‑op /
  reduction‑order scars).
- **q4f16 interaction.** G5 notes the codec‑AR LM is host‑bound‑worse at `q4f16`; keep serve LM at fp32 first, then
  re‑test quant on the device‑resident export (the empty‑KV‑dtype‑follows‑precision fix already handles dtype).
- **`max_length` pre‑allocation** trades memory for speed — must respect the GB10 unified‑pool arena cap
  (the `gpu_mem_limit`/`kSameAsRequested` 950d491 pattern; G8 already flags arena OOM under load).
- **Static buffer vs WaaV's dynamic frame‑rate / ragged left‑align** — the pre‑alloc must coexist with per‑slot ring
  + LEFT‑aligned ragged KV; bucket batch widths (G7) for the CUDA‑graph shape constraint.

---

## 4. Other voice‑relevant ORT‑GenAI features

- **ASR / Whisper path:** ORT‑GenAI runs Whisper through the same `Generator` loop with multi‑audio rectangular
  batch + beam search. WaaV **already** has the whisper STT cohort batcher (`whisper STT … 1.19× equal‑context
  cohort`, transcripts identical). So this is **corroboration of WaaV's approach, not new capability** — and again it
  is static batch, not the ragged continuous batching WaaV's STT path can do. No reason to switch.
- **Multi‑LoRA (pillar #4, currently MISSING in WaaV):** ORT‑GenAI's `OgaCreateAdapters` / `OgaLoadAdapter` /
  `OgaSetActiveAdapter` is a clean **API reference** for the per‑voice/per‑language adapter‑swap WaaV needs — but it
  is per‑*generator* active‑adapter switching (not confirmed as in‑batch S‑LoRA grouped‑GEMM across a ragged
  cohort), and it's in the wrong runtime/models. Use it to shape WaaV's adapter manifest + per‑slot adapter‑id +
  batched grouped‑GEMM design; do not adopt it.
- **Constrained / guided decode (`set_guidance`):** maps to WaaV's STT bad‑words suppression (pillar #15, HAVE‑LIVE).
  Reference only.
- **Speculative decoding:** "under development" in ORT‑GenAI, and token‑spec is N‑A‑for‑voice frames anyway (WaaV's
  analog is eager‑EoT). Irrelevant.

---

## 5. Recommendation

1. **Reject ORT‑GenAI as a serving runtime for Path‑A codec‑AR.** It would *remove* WaaV's live continuous/ragged
   batching (its biggest Path‑A asset), brings **no** paged attention, models none of the audio glue, has **no Rust
   binding**, and would force re‑proving bit‑identity against an opaque third‑party loop.
2. **Adopt the one technique it proves** — **buffer‑sharing GQA static device‑resident KV** (`past_present_share_
   buffer` over a pre‑allocated max‑length BNSH buffer) — as a **graph‑export recipe + IoBinding pattern** ported into
   WaaV's `CodecArBatcher`/`step_batch`. This is the documented **G1 lever** and is the highest‑value Path‑A perf
   work; it unblocks G2 and recovers the path from the ~1.8× host‑KV ceiling toward the ~30× device‑resident
   envelope, while preserving every HAVE‑LIVE pillar and the bit‑identity bar.
3. **Keep `builder.py` / the GQA contrib‑op as a reference producer** for that re‑export (and as a fallback way to
   generate a device‑resident codec backbone graph), and **mine the LoRA‑adapter API as a design reference** for the
   MISSING multi‑LoRA pillar — neither is an adoption.

**One‑line verdict:** *ORT‑GenAI is the right textbook for the device‑resident‑KV trick and a usable export recipe,
but the wrong serving engine for WaaV's voice codec‑AR — port the `past_present_share_buffer` GQA static‑KV technique
into WaaV's own batcher; do not host the codec‑AR loop inside ORT‑GenAI.*

---

## Sources

- ORT‑GenAI Generate API overview — https://onnxruntime.ai/docs/genai/
- ORT‑GenAI README (feature/arch/backend/binding list) — https://github.com/microsoft/onnxruntime-genai/blob/main/README.md
- Python API (Generator/Model/Tokenizer/GeneratorParams/MultiModalProcessor methods) — https://onnxruntime.ai/docs/genai/api/python.html
- C API (Oga* FFI surface) — https://onnxruntime.ai/docs/genai/api/c.html
- Config reference (`past_present_share_buffer`, `batch_size`, `max_length`, `sliding_window`) — https://onnxruntime.ai/docs/genai/reference/config.html
- Past‑present‑share‑buffer how‑to — https://onnxruntime.ai/docs/genai/howto/past-present-share-buffer.html
- KV‑cache APIs discussion #747 (built‑in, no public manipulation API) — https://github.com/microsoft/onnxruntime-genai/discussions/747
- Continuous‑decoding discussion #858 (single‑stream, NOT continuous batching) — https://github.com/microsoft/onnxruntime-genai/discussions/858
- Qwen3‑VL issue #1989 (paged attention = design consideration, not implemented; Feb 2026) — https://github.com/microsoft/onnxruntime-genai/issues/1989
- builder.py (model builder; DefaultKeyValueCache/CombinedKeyValueCache/WindowedKeyValueCache) — https://github.com/microsoft/onnxruntime-genai/blob/main/src/python/py/models/builder.py
- DeepWiki internal architecture (class/file map) — https://deepwiki.com/microsoft/onnxruntime-genai
- Whisper audio models on ORT‑GenAI (same Generator API, multi‑audio batch, beam search) — https://microsoft-onnxruntime-genai-88.mintlify.app/multimodal/whisper
- GroupQueryAttention contrib op + buffer sharing — https://github.com/microsoft/onnxruntime/blob/main/docs/ContribOperators.md ; https://onnxruntime.ai/blogs/accelerating-llama-2 ; ORT PR #23061 (static KV cache)
- Rust ONNX bindings landscape (`ort`/`onnxruntime` — neither wraps ORT‑GenAI) — https://github.com/pykeio/ort ; https://crates.io/crates/ort
