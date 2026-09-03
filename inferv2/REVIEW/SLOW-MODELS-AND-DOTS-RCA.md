# SLOW-MODELS RE-ATTEST (misotts / s2_pro) + dots Fork-A1 RCA — torch 2.12

Branch `waav-infer-v2-build` (HEAD `946996d`, torch 2.12 + TRT baseline; per-slot `serve_deadline`
now generous 300s/env-tunable → the slow-model deadline blocker is REMOVED). All runs on GB10,
`source /home/bud/torch212_trt_venv/bin/activate && source ./gb10-env.sh`, `WAAV_BATCH_DEV=cuda`
(CUDA-f32, the cleanest Fork-A1 cell). One model on GPU at a time. Edits left on disk; NOT committed.

---

## TASK 1 — re-attest the slow batched rings under torch 2.12, flip default-on if GREEN

### misotts — GREEN → FLIPPED default-on ✅

Oracle: `misotts_force_solo_codes :: misotts_tch_force_solo_codes_identical_ragged`
(MisoLabs/MisoTTS 8B, `model.safetensors` 32.7 GB f32 present).

```
solo[0] 'Hi there.'                       -> 2   frames
solo[1] 'The quick brown fox ...'         -> 47  frames
solo[2] 'Testing one two three.'          -> 125 frames
solo[3] 'Batched serving must be ...'     -> 125 frames
row 0: solo=2   batched=2
row 1: solo=47  batched=47
row 2: solo=125 batched=125
row 3: solo=125 batched=125
✓ Fork-A1 CODES-IDENTICAL-vs-SOLO: 4 rows, 9568 integer codes, max|Δ|=0
test result: ok. 1 passed; finished in 527.57s
```

**Verdict: max|Δ|=0, byte-identical-to-solo.** Flipped the engine.rs gate default
(`crates/waav-infer-server/src/engine.rs` — `WAAV_MISOTTS_BATCHED` default `false → true`) and
rewrote its §8.3 gating comment to mirror csm: deadline blocker removed + ring re-attested under 2.12.
`WAAV_MISOTTS_BATCHED=0` still forces the B=1 one-shot fallback; rides the ring on CUDA by default.

### s2_pro — GREEN → FLIPPED default-on ✅

Oracle: `s2_pro_force_solo_codes :: s2_pro_tch_force_solo_codes_identical_ragged`
(fishaudio/s2-pro, sharded `model-0000{1,2}-of-00002.safetensors` ~9 GB + codec present).

```
solo[0..3] -> 2048 frames each   (f32 generation runs to the 2048-frame cap; cohort is uniform-length)
row 0: solo=2048 batched=2048
row 1: solo=2048 batched=2048
row 2: solo=2048 batched=2048
row 3: solo=2048 batched=2048
✓ Fork-A1 CODES-IDENTICAL-vs-SOLO: 4 rows, 81920 integer codes, max|Δ|=0
test result: ok. 1 passed; finished in 2710.87s  (~45 min)
```

**Verdict: max|Δ|=0, byte-identical-to-solo.** Flipped the engine.rs gate
(`WAAV_S2PRO_BATCHED` default `false → true`) + rewrote its §8.3 comment to mirror csm: deadline
blocker removed + ring re-attested under 2.12. `WAAV_S2PRO_BATCHED=0` still forces the B=1 one-shot.

(Note: every s2_pro f32 row ran to the 2048-frame cap rather than a natural EOS — a separate
generation-length observation, orthogonal to Fork-A1; the byte-identity gate solo==batched holds at the
cap either way.)

---

## TASK 2 — dots Fork-A1 RCA → FIXED to byte-identical ✅

Oracle: `dots_force_solo_codes :: dots_tch_force_solo_codes_identical_ragged`
(`--features cuda`; rednote-hilab dots-tts-base, flow_matching variant). The "solo" reference is the
SAME ring path run as a 1-slot cohort (`TorchDotsBatched::new(m, 1, …)`); "batched" is a 5-slot ragged
cohort. So the oracle compares **1-slot-ring vs 5-slot-ring** for each row's emitted continuous payload
patches (`[1,4,128]`), bar = max|Δ|=0 + identical patch count.

### Reproduction (default, graph ON) — the fork

```
solo[1] 'The quick brown fox ...' -> 18 payload patches
row 0: solo=4  batched=4   ✓
row 1: solo=18 batched=20  ✗  panic: row 1 patch-count mismatch (batched 20 vs solo 18)
finished in 73.87s
```

Reproduces the reported divergence exactly (row 1: solo 18 vs batched 20), deterministically.

### Root cause — the SHARED DiT CUDA-graph, NOT the AR backbone ring

Investigation path:

1. **`forward_ring` is genuinely per-slot B=1 (Fork-A1 clean).** `self_attention.rs:631`
   `Attention::forward_ring` `debug_assert!(b == 1)`; the ring read-back (`RaggedSlotRing::append_*_row`,
   `ragged_ring.rs`) `narrow(0, slot, 1).narrow(2, 0, len).contiguous()` COPIES out exactly one row →
   a `[1, kvh, len, d]` SDPA, independent of `max_slots`. So a slot's backbone hidden is identical for
   a 1-slot vs a 5-slot ring **by construction** (the batch index never enters a reduction). The 3 other
   hybrid rings (vibevoice/indextts2/voxtral_tts) share this exact machinery and ARE byte-identical.
2. **The RNG is correctly content-keyed.** `dots.rs` `step_patch` re-seeds the global libtorch RNG to a
   content-keyed FNV-1a base (`tch::manual_seed(rng_base)` at the prompt-latent draw,
   `rng_base + 1 + patch_idx` before each per-patch FM `randn`), so draws are a pure function of
   `(content, patch_idx)` — slot- and cohort-independent.
3. **The smoking gun: `WAAV_DOTS_GRAPH_EAGER=1` is fully byte-identical.** Forcing the DiT forward eager
   (no capture/replay) makes ALL 5 rows match solo and the D2 concurrent-identical assert pass:
   ```
   row 0..4: solo == batched (4/18/9/22/4 patches)
   ✓ PATCHES-IDENTICAL-vs-SOLO: 5 rows, 29184 f32 latent values, max|Δ|=0
   ✓ D2: identical concurrent requests (slots 0,4) byte-identical
   ```
   This isolates the cause to the **CUDA-graph machinery** and exonerates the backbone ring + RNG.

**Mechanism** (`dots.rs` `Dit::forward_graph`, ~L635; auto-enabled on NVIDIA flow_matching via
`graphable_cuda_graph_enabled(... "dots" ...)` and used even in the f32 oracle):

- The DiT CUDA-graph lives on the **shared** `self.model.dit` (one `RefCell<Option<DitGraph>>`), keyed
  ONLY on the FM sequence length `total` (`captured_total`). Its `pos_ids` / `attn_bias` / `g_cond` are
  captured as **graph-EXTERNAL constants by address** (only `x_in`/`t_in` are written into static buffers
  per step).
- In a **1-slot / solo** run, `total` grows **monotonically** per patch (~+5 positions/patch), so every
  patch trips `need_rebuild` (`captured_total != total`) and re-captures fresh against its own ALIVE
  `pos_ids`/`attn_bias`. Always correct.
- In a **multi-slot cohort**, the shared graph sees `total`s interleaved across slots. Two slots' patches
  can collide on the same `total`, so the second slot finds `captured_total == total`, SKIPS the rebuild,
  and **REPLAYS the first slot's captured graph** — whose external `pos_ids`/`attn_bias` tensors were
  freed when the first slot's `decode_next_audio` returned. The stale read perturbs the velocity at the
  sub-ULP level.
- **Why dots forks where discrete-code rings don't:** dots' stop is a CONTINUOUS EOS softmax-threshold
  (`Projections::eos_fires`: `softmax(eos2(silu(eos0(hidden))))[...,1] > 0.8`). A sub-ULP velocity →
  sub-ULP latent → sub-ULP hidden perturbation flips the threshold crossing by a patch or two
  (row 1: 18 → 20). csm/misotts/s2_pro decide via `argmax` over discrete codes, which absorbs the same
  magnitude of perturbation — exactly why those rings stay byte-identical and this one did not.

This is a CUDA-graph external-tensor lifetime hazard exposed by cohort sharing, **not** a Fork-A1
violation in the AR reduction.

### Fix — byte-identical, graph still ON

`crates/waav-infer-backend-torch/src/dots.rs`, `TorchDotsBatched::step_patch` (audio-span branch, just
before the per-patch FM draw): reset the shared DiT graph before each patch so every patch ALWAYS
re-captures fresh against its own alive tensors — making the cohort behave exactly like solo (which
already rebuilds every patch due to monotonic `total`), while keeping the within-patch (10 Euler steps)
capture+replay speedup:

```rust
// Fork-A1 cross-slot DiT-graph isolation (THE dots fork RCA, max|Δ|=0 fix)
self.model.dit.reset_graph();
tch::manual_seed(rng_base + 1 + patch_idx);
```

Validation (default, graph ON, fix applied):

```
row 0..4: solo == batched (4/18/9/22/4 patches)
✓ PATCHES-IDENTICAL-vs-SOLO: 5 rows, 29184 f32 latent values, max|Δ|=0
✓ D2: identical concurrent requests (slots 0,4) byte-identical
test result: ok. 1 passed; finished in 72.85s
```

**Verdict: FIXED to byte-identical (max|Δ|=0) with the CUDA-graph still active.** Because dots is now
byte-identical AND the deadline blocker is removed, flipped its engine.rs gate
(`WAAV_DOTS_BATCHED` default `false → true`) and rewrote the §8.3 comment from "RCA pending" to the
resolved RCA. `WAAV_DOTS_BATCHED=0` still forces the solo fallback.

---

## Edits on disk (NOT committed — coordinator commits)

| File | Change |
|---|---|
| `crates/waav-infer-backend-torch/src/dots.rs` | `step_patch`: `self.model.dit.reset_graph()` before each patch (cross-slot DiT-graph isolation) + RCA comment |
| `crates/waav-infer-server/src/engine.rs` | `WAAV_MISOTTS_BATCHED` default `false → true` + re-attest comment |
| `crates/waav-infer-server/src/engine.rs` | `WAAV_DOTS_BATCHED` default `false → true` + resolved-RCA comment |
| `crates/waav-infer-server/src/engine.rs` | `WAAV_S2PRO_BATCHED` default `false → true` (oracle GREEN) + re-attest comment |

Compile-check: `cargo build -p waav-infer-server --features torch` → **clean** (Finished, no warnings/errors),
run with the GPU idle (no model loaded). The dots.rs edit was already compile- + test-verified by the
graph-ON validation run (which recompiled `waav-infer-backend-torch`).

## Summary verdicts

| Model | Oracle | max\|Δ\| | Gate flip |
|---|---|---|---|
| misotts | 4 rows / 9568 codes / 527s | **0** | `WAAV_MISOTTS_BATCHED` → default-on ✅ |
| s2_pro  | 4 rows / 81920 codes / 2711s | **0** | `WAAV_S2PRO_BATCHED` → default-on ✅ |
| dots    | 5 rows / 29184 f32 / 73s (after fix) | **0** (was forking 18→20) | `WAAV_DOTS_BATCHED` → default-on ✅ (RCA'd + fixed) |
