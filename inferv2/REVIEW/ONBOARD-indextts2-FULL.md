# Onboard — IndexTeam/IndexTTS-2 FULL synth (back-half landed byte-faithful)

**Status: ACOUSTIC BACK-HALF (codes → wav) PORTED + BYTE-FAITHFUL end-to-end.** The deterministic core (GPT-2 AR
→ greedy mel codes) was already byte-identical (`indextts2.rs`, 200/200). This session ported + byte-faithfully
gated the **entire acoustic back-half** — `gpt_layer` → semantic FVQ codec (`vq2emb`) → s2mel length-regulator →
the 13-layer **DiT flow-matching CFM** (U-ViT skip + AdaptiveLayerNorm time-conditioning + WaveNet head, 25-step
linear Euler, CFG 0.7) → a 6-stage NVIDIA **BigVGAN** vocoder → 22.05 kHz wav. The conditioning **FRONT-END**
(w2v-bert + Conformer×2 + Perceiver×2) + the small `gpt_latent` teacher-forced GPT forward remain scoped (the
goldens for both are staged so each is independently gateable).

---

## 1. Full e2e synth working + byte-faithful?

**Back-half: YES, byte-faithful end-to-end** (codes → wav). The whole text→wav e2e is NOT yet wired because two
upstream pieces remain (front-end audio→conditioning, + the `gpt_latent` GPT-forward) — but every stage that
*was* ported is byte-faithful to the reference golden, and the back-half is gated as a complete chain.

### Per-stage byte-faithfulness (CPU f32, vs the seeded reference golden `indextts2_golden_full/`)

| stage | what | max\|Δ\| | rel_l2 | verdict |
|---|---|---|---|---|
| `gpt_layer` | 1280→256→128→1024 (3 Linears) | 1.2e-6 | — | **byte-exact** |
| `vq2emb` | FVQ embed[8192,8] + 1×1 wnorm conv 8→1024 | 9.5e-7 | — | **byte-exact** |
| `length_regulator` | in_proj 1024→512, nearest-interp, (Conv3·GN·Mish)×4·Conv1 | 1.3e-7 | — | **byte-exact** |
| `DiT step0` | full U-ViT estimator (the flow-field) at t=0 | 2.9e-5 | 4.4e-7 | **byte-faithful** |
| `CFM` | 25-step linear Euler flow-matching (CFG-doubled, per-step prompt-reset) | 1.6e-5 | 3.5e-7 | **byte-faithful** |
| `BigVGAN` | 6-stage snakebeta + alias-free up/down, clamp(-1,1) | 3.9e-5 | 1.3e-5 | **byte-faithful** |
| **FULL** | **codes → wav (88064 samples = 3.99 s @ 22.05 kHz)** | **1.3e-4** | **6.3e-5** | **byte-faithful** |

All seven gates green: `cargo test -p waav-infer-backend-torch --test cuda_torch_indextts2_backhalf -- --ignored`
→ **7 passed**. The AR core gate still byte-identical (`200/200`). Lib **191 passed**; clippy `--all-targets`
**0 warnings**.

### RTF
- CPU f32 (the byte-faithful regime): full back-half ≈ 52 s for 3.99 s audio (RTF ~13; not the perf target).
- GB10 CUDA: full back-half ≈ 4.8 s incl. model load (RTF ~1; the 25-step × 13-layer CFG-doubled DiT dominates).
  CUDA-vs-CPU conv accumulation gives rel_l2 1.7e-2 (audibly identical; the byte-faithful gate is the CPU run).

---

## 2. The two byte-identity SCARS caught (f32-CPU bisection)

1. **U-ViT skip connections were ON (`uvit_skip_connection: true`).** First the full DiT diverged (rel_l2 0.44).
   A per-layer bisection (probing the staged `dit_after_layer*` golden) showed layers **0–6 byte-exact, layer 7
   exploding** (rel_l2 1.5 → 1e3 by layer 11). The reference's `config.yaml` `s2mel.DiT.uvit_skip_connection`
   is **true** (the structural map had read it False): layers `i < n/2` (0..5) EMIT their output to a LIFO
   stack; layers `i > n/2` (7..12) POP a skip and apply `skip_in_linear(cat([x, skip]))` at the block START
   (layer 6 = n/2 neither). The per-layer `skip_in_linear` weights are in the checkpoint. With the U-ViT logic
   added (`DitEstimator::run_transformer`), the full estimator went byte-faithful (rel_l2 4.4e-7).

2. **WaveNet `SConv1d` inner-conv padding is 0, not `(k·d−d)/2`.** First the CFM crashed on a length mismatch
   (1381 vs 1385). The reference `WN` passes `padding=(k·d−d)/2` to `SConv1d`, but `NormConv1d`'s `**kwargs`
   DROPS it → the inner `nn.Conv1d` has `padding=0`; the `SConv1d` reflect-pad (`padding_total = kernel_eff−1`)
   is the ONLY pad, length-preserving. (Verified against the reference: an `in_layer` keeps its length.)

   Plus the **oneDNN grouped-conv JIT quirk** ("illegal immediate parameter", Xbyak err 15) on this
   torch-2.12/aarch64 box: the BigVGAN alias-free filter's depthwise (`groups=C`) conv SIGSEGVs on the CPU
   path. Fix: the filter is SHARED across channels, so issue it as a single-channel conv over the flattened
   `[B·C, 1, T]` (groups=1) — numerically identical AND avoids the JIT. (The reference golden was likewise
   dumped with `torch.backends.mkldnn.enabled=False`.) Gave byte-faithful BigVGAN (rel_l2 1.3e-5).

---

## 3. What landed vs scoped

**LANDED (byte-faithful, gated):**
- The full acoustic **back-half** (`indextts2_backhalf.rs`): gpt_layer, FVQ `vq2emb`, length_regulator, the
  13L DiT (gpt-fast LLaMA body + interleaved RoPE + SwiGLU + **U-ViT skip** + **AdaptiveLayerNorm** time-cond +
  WaveNet head + AdaLN FinalLayer), the 25-step linear-Euler CFG CFM, the 6-stage BigVGAN. **codes → wav
  byte-faithful** (rel_l2 6.3e-5).
- A seeded full-pipeline reference golden (`indextts2_golden_full/*.npy`, 39 stage-boundary tensors incl. the
  staged flow-matching noise `cfm_z` so the back-half is fully reproducible).

**SCOPED (follow-ups, each golden-staged & independently gateable):**
1. **Conditioning front-end** (audio → the staged `inputs_embeds`/`speech_conditioning_latent`/`emovec`):
   w2v-bert-2.0 (`Wav2Vec2BertModel`, hidden_states[17], normalized) + the WeNet **ConformerEncoder ×2**
   (6-block spk + 4-block emo: conv2d2 /2 subsampling, WeNet rel-pos MHA **without rel_shift**, LayerNorm conv
   module k15, macaron pre-norm) + the **PerceiverResampler ×2** (cross-attn-include-queries, GEGLU,
   `F.normalize`-based RMSNorm). All in `gpt.pth`. Goldens staged: `w2v_input_features`, `spk_cond_emb`,
   `speech_conditioning_latent`, `conds_latent`, `emovec`, `inputs_embeds_frontend`. (The detailed structural
   map + the 9 byte-identity scars are in the front-end mapping agent's notes; reuse the canary/granite
   conformer scaffolding with the 5 WeNet-vs-NeMo fixes.)
2. **The `gpt_latent` GPT-forward** (small): the back-half consumes `gpt_latent` = `UnifiedVoice.forward(...)`
   teacher-forced over the generated codes (returns mel latents via `get_logits(return_latent=True)`) — a
   second pass through the SAME 24-layer GPT-2 backbone already ported in `indextts2.rs` (over
   `[conds, text_emb, mel_emb]`, returning the pre-`mel_head` latent slice). Staged golden: `gpt_latent`.

With (1)+(2) the full **text → front-end → AR → gpt_latent-forward → back-half → wav** chain closes; every
sub-stage already gates byte-faithful against its staged golden.

---

## 4. Files (absolute; shared flagged)

- **NEW** `/home/bud/ditto/waav/waav-infer/crates/waav-infer-backend-torch/src/indextts2_backhalf.rs` — the
  back-half port (owned). Composes `cfm::vocoder::weight_norm_reconstruct`, `codec::flow_dac::SnakeBeta`,
  `nn::{Linear, Rope::apply_interleaved_full, sdpa_manual}` (read-only reuse — NO shared module edited).
- **NEW** `/home/bud/ditto/waav/waav-infer/crates/waav-infer-backend-torch/tests/cuda_torch_indextts2_backhalf.rs`
  — the 7-stage byte-faithful gate (owned).
- **EDIT (owned)** `/home/bud/ditto/waav/waav-infer/crates/waav-infer-backend-torch/src/lib.rs` —
  `pub mod indextts2_backhalf;` + doc.
- **NO shared `nn::`/`cfm::`/`codec::` edits** — dia2 (608)/csm (4000)/irodori structurally unaffected (reuse
  only). Lib 191 tests + the AR `indextts2` gate (200/200) re-verified green.
- Weights: `~/.cache/waav-models/indextts2/indextts2_backhalf.safetensors` (864 MB — s2mel gpt_layer +
  length_regulator + cfm/DiT/wavenet, the semantic FVQ quantizer, the BigVGAN generator; weight-norm kept as
  `weight_g/weight_v`, reconstructed at load). + `waav.json` updated with the `s2mel` block.
- Goldens: `/home/bud/ditto/waav/WaaV/inferv2/REVIEW/indextts2_golden_full/*.npy` (39 stage-boundary tensors)
  + meta. Also `~/.cache/waav-models/indextts2/golden_full/`.
- Throwaway reference scripts (scratchpad, NOT a serving path; reuse the `refvenv` transformers==4.52.1 venv):
  `golden_full.py` (the seeded full-pipeline dumper), `extract_backhalf.py` (weight extractor),
  `golden_dit_step.py` (DiT step probe). Aux models at `~/.cache/waav-models/indextts2-aux/` (maskgct semantic
  codec, campplus, bigvgan) + `~/.cache/waav-models/indextts2/s2mel.pth`.

## 5. Verification

- `cargo test -p waav-infer-backend-torch --test cuda_torch_indextts2_backhalf -- --ignored` → **7 passed**.
- `cargo test -p waav-infer-backend-torch --lib` → **191 passed**.
- AR gate `cuda_torch_indextts2 :: indextts2_greedy_codes_byte_identical` → **200/200 byte-identical**.
- `cargo clippy -p waav-infer-backend-torch --all-targets` → **0 warnings**.
- NO `git commit`, NO `cargo fmt` (per instructions; only touched files were edited).
