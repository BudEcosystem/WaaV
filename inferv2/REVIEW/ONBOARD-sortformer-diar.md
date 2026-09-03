# ONBOARD — nvidia/diar_streaming_sortformer_4spk-v2 (streaming speaker diarization)

**Status: ONBOARDED (community ONNX mirror) — LIVE diarize + near-bit-faithful accuracy + RTF measured on GB10.**

| | |
|---|---|
| Model | NVIDIA Sortformer v2 streaming speaker diarization (≤4 speakers, 16 kHz mono) |
| Task | `diarize` (streaming) — 2nd diarization arch beside offline pyannote |
| Acquisition | **community ONNX mirror** (`cgus/diar_streaming_sortformer_4spk-v2-onnx`), NOT nemo-export |
| Weights | `~/.cache/waav-models/sortformer-diar-4spk/diar_streaming_sortformer_4spk-v2.onnx` (492 MB, fp32, opset 17) |
| Accuracy | **near-bit-faithful** vs onnxruntime reference: short clip max |Δ| = **5.96e-07**, long clip (through cache-compression) max |Δ| = **7.7e-04**; **0 binary (>0.5) frame×spk disagreements** on both (1180 + 2292 cells) |
| RTF (GB10) | CUDA EP release: **0.0125** (45.8 s clip); CUDA debug 0.048–0.056; CPU debug 0.059–0.062 |
| Tests | 4 new sortformer unit tests green; full `waav-infer-core` lib suite 75/0; workspace builds; clippy clean on touched crates |

## 1. HfApi verification — the nemo-export wall, and the way around it

`HfApi.model_info('nvidia/diar_streaming_sortformer_4spk-v2', files_metadata=True)` confirms the
triage's **P2-nemo-export** flag exactly: the official repo ships **ONLY** `.nemo`
(`diar_streaming_sortformer_4spk-v2.nemo`, 471 MB) + figures — no ONNX, no safetensors, no config.

A NeMo→ONNX export in-session would require a working `nemo_toolkit[asr]` install
(`SortformerEncLabelModel.restore_from`), which is heavy and violates the no-per-venv-serving rule for
the *served* path. **But a community ONNX mirror exists and is clean**, so the export wall was avoided:

- `cgus/diar_streaming_sortformer_4spk-v2-onnx` — a **single 492 MB ONNX** (exact v2 base model),
  converted by **altunenes** for the [parakeet-rs](https://github.com/altunenes/parakeet-rs) Rust NeMo
  port. License CC-BY-4.0 (base v2). This is the artifact onboarded.
- (Also surveyed: `cgus/...-v2.1-onnx`, `ooobo/...`, `tonythethompson/...`, `christopherthompson81/sortformer_parakeet_onnx`
  bundling encoder/decoder + int8 — all the same altunenes export. mlx-community fp16/gguf are non-ONNX.)

The mirror's README points at parakeet-rs, which ships a complete reference Rust implementation
(`src/sortformer.rs`) + the export script (`scripts/export_diar_sortformer.py`) — these gave the exact
I/O schema, streaming state machine, and feature frontend, with no guessing.

## 2. ONNX I/O schema (HfApi/onnx-verified)

The graph wraps the conformer **pre-encode + streaming frontend + sigmoid T×4 head**. The conformer
×8 subsampling happens **inside** the graph; the Rust side feeds raw log-mel frames and runs the
NeMo FIFO/speaker-cache state machine around it.

```
INPUTS                                        OUTPUTS
chunk            [1, time_chunk, 128]  f32     spkcache_fifo_chunk_preds  [1, time_out, 4]  f32 (sigmoid)
chunk_lengths    [1]                   i64     chunk_pre_encode_embs      [1, time_pe, 512] f32
spkcache         [1, time_cache, 512]  f32     chunk_pre_encode_lengths   [1]               i64
spkcache_lengths [1]                   i64
fifo             [1, time_fifo, 512]   f32
fifo_lengths     [1]                   i64
```

Geometry (export defaults; the cgus v2 ONNX carries NO override metadata, so defaults are used):
`chunk_len=124, fifo_len=124, spkcache_len=188, right_context=1, subsampling=8, emb_dim=512, frame=80 ms`.
Feature frontend = NeMo `AudioToMelSpectrogramPreprocessor(normalize="NA")`: n_fft=512, hop=160,
win=400 (Hann centered), 128 Slaney mels (0–8000 Hz), pre-emphasis 0.97, **log guard 2⁻²⁴ (5.96e-8)**.

## 3. Integration — reused the diarize output seam + shared NeMo mel; new streaming state machine

Sortformer is **single-graph end-to-end** with an arrival-order speaker cache (the model itself tracks
consistent speaker ids across the stream) — so there is **no** separate embedding+clustering step like
pyannote. It emits the **SAME `DiarSegment` output seam** (`[start,end)` seconds + global speaker id)
as the offline `crate::diarize::Diarizer`.

Reused:
- **`waav_infer_components::nemo_mel::NemoMel`** (the shared NeMo normalize="NA" log-mel) — instantiated
  with Sortformer's 2⁻²⁴ guard. This is the same component the Nemotron streaming STT uses; bit-faithful
  to `librosa.stft`/`librosa.filters.mel` (Slaney filterbank + reflect pad already shared).
- **`crate::diarize::DiarSegment`** — the existing diarization output type (no new output format).
- **`StaticGraph`** backend seam (ORT, EP-agnostic) — the 6-in/3-out graph driven by name; outputs read
  in graph order.

New (ported faithfully from NeMo `SortformerModules.streaming_update` via parakeet-rs):
- The Rust **FIFO / speaker-cache / silence-profile state machine**: chunked streaming inference,
  FIFO→cache pop, smart cache compression (log-pred scores → disable-low → top-k boost → gather), and
  the CallHome/DIHARD3 post-processing (median filter, hysteresis onset/offset, pad, min-dur, merge).

### Files (★ = SHARED, additive touch — flagged for coordinator in `.SORTFORMER_TOUCH_NOTE`)

| File | Change |
|---|---|
| `crates/waav-infer-core/src/sortformer.rs` | **NEW** — `Sortformer`, `SortformerConfig`, state machine, post-proc, 4 unit tests (~720 lines) |
| ★ `crates/waav-infer-core/src/lib.rs` | `pub mod sortformer;` + re-export `Sortformer`/`SortformerConfig` |
| ★ `crates/waav-infer-server/src/engine.rs` | `load_sortformer()` + `load_sortformer_graph_only()` near `load_diarizer` |
| ★ `crates/waav-infer-server/src/bin/waav_infer.rs` | `DiarizeStream` CLI subcommand + `diarize_stream_once()` (with `--dump-raw` for accuracy) |

No registry/`model.rs` touch needed — diarization loads outside the single-model registry (same as
`load_diarizer`/`load_enhancer`), so there was **no contention** with the concurrent model-onboarding
agents on `model.rs`.

## 4. SMOKE — real multi-speaker diarization via the engine

CLI: `waav-infer diarize-stream <wav> --model <onnx> [--ep cuda|cpu] [--config callhome|dihard3] [--dump-raw <f32>]`

**Fixture A** (`/tmp/waav_sortformer/two_spk.wav`, 23.5 s): JFK (male, 0–11 s) + 0.5 s silence +
kokoro TTS (female, 11.5–23.5 s). WaaV output (CUDA, GB10):

```
0.00-2.48s spk0  3.13-4.56s spk0  5.21-10.88s spk0   (JFK)
11.61-12.88s spk1  13.21-23.36s spk1                 (kokoro)
=> 2 speakers   — correct count + boundaries
```

**Fixture B** (`/tmp/waav_sortformer/multi_long.wav`, 45.8 s): JFK + kokoro + dia-dialogue + **JFK again**
— exercises (a) the FIFO overflow + **speaker-cache compression** path (>4 chunks) and (b) speaker re-ID:

```
0.00-10.88s spk0  (JFK)        23.69-34.00s spk2  (dia)
11.53-23.12s spk1 (kokoro)     34.97-45.81s spk0  (JFK RETURNS → relabeled spk0, not a new id)
=> 3 speakers   — correct count, AND arrival-order cache re-identifies the returning speaker
```

## 5. ACCURACY — near-bit-faithful vs the onnxruntime reference

Reference engine = a faithful Python port of parakeet-rs `sortformer.rs` (the NeMo streaming state
machine) on **plain onnxruntime (CPU)** with a numpy/librosa log-mel frontend
(`/tmp/waav_sortformer/ref_sortformer.py`). WaaV's raw T×4 sigmoid probs (`--dump-raw`) vs the
reference `ref_raw.npy`, element-wise:

| clip | frames | max abs |Δ| | mean abs |Δ| | frames |Δ|>1e-3 | binary (>0.5) disagreement | per-spk active counts |
|---|---|---|---|---|---|---|
| two_spk (23.5 s) | 295 | **5.96e-07** | 2.9e-08 | 0/295 | **0 / 1180** | identical `[109,132,0,0]` |
| multi_long (45.8 s, cache-compress) | 573 | **7.7e-04** | 3.4e-06 | 0/573 | **0 / 2292** | identical `[218,133,124,0]` |

The 5.96e-07 short-clip delta is a **single float epsilon** — the only difference between WaaV's pure-Rust
realfft mel frontend and Python's numpy frontend, propagated through the **identical** ONNX graph. The
larger 7.7e-4 on the long clip is the speaker-cache top-k frame selection occasionally choosing a
marginally different frame when scores tie near the float-epsilon boundary; it never crosses 1e-3 and
**never flips a binarization decision** (0 disagreements / 2292 cells). Segment boundaries match to 2
decimals on both clips. This is as close to byte-identical as a cross-language frontend reaches.

## 6. PERF — RTF on GB10

45.8 s clip (CUDA EP, NVIDIA GB10, sm_121, unified 121 GB):

| build | EP | wall | RTF |
|---|---|---|---|
| release | cuda | 574 ms | **0.0125** (~80× realtime) |
| debug | cuda | 2203 ms | 0.0481 |
| debug | cpu | 2825 ms | 0.0617 |

Diarization is offline-batch-style streaming (chunked ~10 s windows), so RTF ≪ 1 with ample headroom;
the streaming `diarize_chunk`/`feed` API (per the reference) can be added for true online use if needed.

## 7. Notes / follow-ups (non-blocking)

- **v2.1**: `cgus/...-v2.1-onnx` is the same export pattern; onboarding it is weights-only (swap the path
  + it carries the NVIDIA Open Model License instead of CC-BY-4.0). v2 was chosen to match the triage
  target exactly.
- **Streaming online API**: only the full-buffer `diarize`/`diarize_raw` paths are wired (resets state
  per call). The reference's stateful `feed`/`flush`/`diarize_chunk` (carry FIFO+cache across calls) are
  a straightforward addition on the same state machine if an online endpoint is wanted.
- **No DER-vs-GT**: accuracy is measured against the NeMo/onnxruntime *reference engine* output (the
  onboarding bar), not a labeled DER corpus — the reference itself is the ground truth for faithfulness.
  Segment-level correctness independently confirmed on the synthetic 2/3-speaker fixtures.
