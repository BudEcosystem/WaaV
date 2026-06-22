# B53 — Per-graph precision manifest (closes G-5)

**Gap (B51 §3-G5):** a multi-graph / *partially-variant* model — one whose sub-graphs ship **different**
on-disk precision sets — cannot be precision-switched, because the precision selector was **global**: a
single `WAAV_PRECISION` env / a single `waav.json` `precision` field applied the same `_{precision}` suffix
to **every** graph. Chatterbox-onnx is the canonical case: it ships precision variants **only** for its
`language_model` graph (fp16 / q4 / q4f16), while `speech_encoder` / `embed_tokens` /
`conditional_decoder` are **fp32-only**. So `WAAV_PRECISION=q4f16` on chatterbox tries to load
`speech_encoder_q4f16.onnx`, **which does not exist**, and the load dies (observed live, §"G-5 negative"
below). There was no way to say "LM at q4f16, the rest at fp32".

**Fix:** a **per-graph precision override** in `waav.json` — each ONNX sub-graph can pick its own
precision/weights variant, with the existing single global `precision` staying the default for graphs that
don't override. Fully backward-compatible: a manifest with only a global `precision` (or the legacy
bare-string `weights` form) resolves **byte-identically** to before.

**Files changed (exact):**
- `crates/waav-infer-core/src/model.rs` — the manifest schema + the resolver. **(+329 / −20, ONE tracked
  file.)** No ORT-loader change was needed (see "Why the ORT loader is untouched"). No model numerics, no
  other serving code.
- `~/.cache/waav-models/chatterbox-onnx/waav.json` — a **config-only** per-graph scaffold for the live test
  (a model-dir config file, NOT repo source).

`source gb10-env.sh`; `free -g` 56 GB free before the live runs; GB10 sm_121, ORT-1.27 CUDA EP. Date
2026-06-22.

---

## 1. The per-graph manifest schema

A `waav.json` `weights[<graph>]` value may now be **EITHER** a bare string (the legacy explicit-file form,
unchanged) **OR** an object `{ "precision": ..., "file": ... }` keyed by the **logical graph name** the
registry passes to `weight_path` (`encoder`, `language_model`, `speech_encoder`, …):

```jsonc
{
  "architecture": "chatterbox",
  "precision": "fp16",                 // model-wide default (still honored for un-overridden graphs)
  "weights": {
    "language_model":      { "precision": "q4f16" },        // THIS graph → q4f16 variant
    "conditional_decoder": { "precision": "fp32" },         // THIS graph → fp32 (drops the suffix)
    "speech_encoder":      { "file": "onnx/se_special.onnx" }, // explicit file (highest precedence)
    "embed_tokens":        "onnx/embed_tokens.onnx"         // LEGACY bare-string == { "file": ... }
  }
}
```

Both shapes fold into one internal type so the resolver reasons about a single shape:

```rust
pub struct GraphWeight {
    pub file:      Option<String>,   // explicit relative path (bare-string OR { "file": ... })
    pub precision: Option<String>,   // this graph's own precision token ({ "precision": ... })
}
```

`Manifest.weights` changed from `HashMap<String, String>` → `HashMap<String, GraphWeight>`. The field is
read **only** through the public `weight_path` method (no external crate touches `.weights` — verified by
grep across the workspace), so the type change is fully contained; `server` + `components` build clean.

### Resolution precedence (per graph)

For each graph, `weight_path(dir, logical, stem)` resolves:

1. **per-graph explicit `file`** (`weights[logical].file`, incl. the legacy bare string) → that exact path;
2. else `onnx/{stem}{_precision}.onnx`, where the precision is `graph_precision_token(logical)`:
   - **per-graph `weights[logical].precision`** (highest), else
   - the **model-wide** token: `$WAAV_PRECISION` → global `precision`, else
   - `None` ⇒ **fp32 / unsuffixed**.

Both the per-graph and the model-wide tokens pass through the **same single** `Manifest::canonicalize` →
`waav_infer_components::canonical_precision` (the existing notation mapper: `half`→`fp16`, `8bit`/`q8`→
`int8`, `4bit`→`q4`; unknown tokens — e.g. a model's literal `quantized` suffix — pass through verbatim).
So notation is uniform regardless of where the token came from.

**One deliberate precedence rule:** `$WAAV_PRECISION` does **NOT** outrank a per-graph `precision`. A graph
pinned `fp32` because no quant variant exists on disk (e.g. chatterbox `speech_encoder`) must keep that pin
even when an operator flips the global env — otherwise the load breaks again (the exact G-5 failure). The
env still drives every graph that has **no** per-graph pin.

---

## 2. Backward-compatibility proof — the global-only path stays byte-identical

This is the load-bearing guarantee: **an existing single-precision model resolves exactly as before.** Two
levels of proof.

### 2a. The unit proof (`legacy_global_only_resolves_byte_identical_to_old_path`)

The test transcribes the **OLD** `weight_path` verbatim (explicit-string override → else
`onnx/{stem}{_global_precision}.onnx`, no per-graph precision) into a local `old_weight_path`, then asserts
the NEW resolver (which now routes through `graph_precision_token`) produces the **identical** `PathBuf`
for **every** graph, across a matrix of global precisions × the full chatterbox graph set:

```
global ∈ { None, fp32, fp16, q4, q4f16, int8, quantized }
graph  ∈ { speech_encoder, embed_tokens, language_model, conditional_decoder }
assert new == old   // for every (global, graph) cell
```

Plus: a legacy manifest whose `weights` are bare **strings** still resolves to those exact files, and an
un-overridden graph still follows the global suffix. **Why this is byte-identical by construction:** when no
graph carries a per-graph `precision` object, `graph_precision_token(logical)` falls straight through to
`precision_token()` for *every* graph — the same `$WAAV_PRECISION → global → fp32` token, the same
`canonical_precision`, the same suffix match arm (`None|fp32|"" ⇒ no suffix`). The only new branch
(`weights[logical].precision`) is `None` for a legacy manifest, so it is never taken.

### 2b. The LIVE proof (voxtral-realtime, an existing model, unchanged)

`voxtral-realtime/waav.json` is a real shipped manifest using BOTH a global `precision: q4f16` AND
legacy **bare-string** `weights` overrides. Transcribed live on the box (CPU EP — B51 §1.4: voxtral q4f16
on CUDA hits the G-1 GQA-bias gap; CPU is the working ONNX path):

```
transcript: "Hello world! This is WAV, infer a portable voice inference engine, running live on the
             GB10 Grace Blackwell."     (correct)
strace openat → graph files opened:  audio_encoder_q4f16.onnx, embed_tokens_q4f16.onnx,
                                     decoder_model_merged_q4f16.onnx
```

The bare-string explicit overrides resolved to the **same** `_q4f16.onnx` files as before — the legacy
explicit-string form is byte-identical through the new `GraphWeight` shape.

---

## 3. The G-5 multi-graph LIVE result (chatterbox: two sub-graphs at different precisions)

Per-graph `waav.json` (LM q4f16 + the other three fp32):

```jsonc
{ "architecture": "chatterbox",
  "weights": { "speech_encoder":{"precision":"fp32"}, "embed_tokens":{"precision":"fp32"},
               "language_model":{"precision":"q4f16"}, "conditional_decoder":{"precision":"fp32"} } }
```

**G-5 POSITIVE — synthesized valid audio:**
```
./target/release/waav-infer run --tts-dir chatterbox-onnx --ep cuda "Hello world, this is a test …"
→ spoke 3.64s of audio in 6951ms (RTF 1.910) → /tmp/cb_pergraph_q4f16.wav
WAV: 24 kHz mono, 87360 frames (3.64 s), peak 32767, 84408/87360 non-zero  → real speech, not silence
```
(RTF 1.91 matches B51 §1.2's chatterbox-q4f16-CUDA 1.95; the `cudnn_frontend "No execution plans"` lines
are benign ORT plan-fallback warnings, the known GB10 q4f16 behavior.)

**Syscall-level proof the two sub-graphs loaded at DIFFERENT precisions** (`strace -e openat` on the synth):
```
/onnx/language_model_q4f16.onnx        ← q4f16  (the quant variant)
/onnx/speech_encoder.onnx              ← fp32   (unsuffixed)
/onnx/embed_tokens.onnx                ← fp32
/onnx/conditional_decoder.onnx         ← fp32
```
Exactly one model, one graph at q4f16 + three at fp32, opening the right file for each — the resolution the
global suffix knob **cannot** express.

**G-5 NEGATIVE — the global env still can't do it (the original failure, reproduced):** with the per-graph
manifest moved aside, `WAAV_PRECISION=q4f16` (global suffix → all graphs):
```
Error: load graph …/onnx/speech_encoder_q4f16.onnx … does not exist
```
i.e. without per-graph precision, the partially-variant model is unloadable at q4f16 — the gap this closes.

---

## 4. Tests added (all green; `cargo test -p waav-infer-core --lib` = 69 passed / 0 failed)

| test | covers |
|---|---|
| `per_graph_precision_resolves_each_graph_independently` | **(a)** a per-graph override resolves EACH graph to its specified precision: chatterbox LM→q4f16 while the fp32-only graphs stay unsuffixed; and a 2-different-precision model (LM q4 + decoder fp16). |
| `legacy_global_only_resolves_byte_identical_to_old_path` | **(b)** the back-compat proof: NEW resolver == an in-test transcription of the OLD `weight_path`, for global ∈ {None,fp32,fp16,q4,q4f16,int8,quantized} × the full chatterbox graph set; + legacy bare-string overrides byte-identical. |
| `per_graph_precedence_and_notation` | **(c)** precedence (per-graph file > per-graph precision > global > fp32-default) + notation mapping (global `half`→fp16, per-graph `4bit`→q4, per-graph `fp32` cancels the global suffix, `$WAAV_PRECISION` does NOT outrank a per-graph pin — the G-5 guarantee; + `canonicalize` alias/pass-through). |
| `malformed_per_graph_entry_degrades_to_default` | resilience: a number/array `weights` entry degrades to the global default, never fails the load. |
| `manifest_drives_weights_and_precision` (existing, unchanged) | still passes — the legacy explicit-string + global-suffix path is intact. |

`cargo clippy -p waav-infer-core -p waav-infer-components --all-targets -- -D warnings` → clean.
`cargo test -p waav-infer-components --lib standardize` → 5 passed (the notation mapper I depend on).

---

## 5. Why the ORT loader is untouched (and why that's correct)

The ORT backend derives a graph's precision **from the resolved weight filename**, not from any precision
string passed in:

- `PrecisionClass::of_path(path)` (`backend-ort/src/lib.rs`) classifies int8-vs-other by the
  `{stem}_{precision}.onnx` suffix; `guard_precision_ep` refuses int8-on-CUDA off that.
- The model modules read each graph's dtype from the **loaded graph** itself (`StaticGraph::input_types()`,
  e.g. `stt/encdec.rs`, `stt/cohere.rs`) — so the KV-cache empty-tensor dtype, etc. follow the actual ONNX
  file's element types, not a global precision token.

So precision is **purely a `weight_path` resolution concern**: once `weight_path` returns the correct
per-graph file, the entire downstream dtype pipeline (EP guard, KV-cache dtype, graph numerics) is
automatically correct. Per-graph precision is therefore a single localized change in the resolver — no
ORT-loader edit, no model-numerics change. This is the same single choke point every one of the ~16
registry arms already routes through (`manifest.weight_path(dir, logical, stem)`), so all archs gain
per-graph precision with zero per-arm change.

---

## 6. Headline

- **G-5 closed.** A partially-variant model (chatterbox: variants only for `language_model`) now
  precision-switches **per sub-graph** via `waav.json` — proven live on GB10: one graph q4f16 + three fp32,
  synthesizing valid 3.64 s audio (RTF 1.91), with `strace` confirming the four distinct precision files.
- **Byte-identical back-compat**, proven two ways: a unit test asserting the new resolver == a verbatim
  copy of the old one across a precision×graph matrix, and a live voxtral transcript whose (global + legacy
  bare-string) manifest resolved the same `_q4f16` files as before.
- **One file changed** (`crates/waav-infer-core/src/model.rs`); no ORT loader, no model numerics. Tests +
  clippy green.
