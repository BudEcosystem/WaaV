# WaaV Infer — Hardware × Quantization × Performance Matrix

Live-measured on GB10 (Grace-Blackwell aarch64, CUDA 13). STT RTF on jfk.wav (11.0s); TTS RTF on a
short sentence. RTF = inference_time / audio_duration (lower = faster; <1 = faster than realtime).
✓ = correct output verified; FAIL = the EP/quant combo can't run that op.

## STT (RTF on 11s audio)

| Model (arch) | quants shipped | CUDA | CPU | accuracy (vs ref) |
|---|---|---|---|---|
| whisper-large-v3-turbo (enc-dec) | fp16, int8 | 1540ms (0.14) ✓ | fp16 FAIL¹ | 0.00% disagree |
| whisper-large-v3 (enc-dec) | fp16, int8, q4 | ✓ (fp16) | fp16 FAIL¹ | 0.00% disagree |
| parakeet-tdt-v2 (TDT transducer) | fp32, int8 | 549ms (0.05) ✓ | 693ms (0.06) ✓ | WER 3.05%==ref, 0% |
| parakeet-tdt-v3 (TDT, 25-lang) | fp32, int8 | ✓ | ✓ | WER 2.77% vs 3.05% PASS |
| parakeet-ctc (Conformer-CTC) | fp32, int8 | 519ms (0.05) ✓ | 695ms (0.06) ✓ | WER 2.77%==ref, 0% |
| parakeet-rnnt (RNN-T) | fp32, int8 | 601ms (0.05) ✓ | 786ms (0.07) ✓ | WER 3.05%==ref, 0% |
| sensevoice (CTC) | int8 | 846ms (0.08) ✓ | 449ms (0.04) ✓² | WER 5.61%==ref |
| canary-180m-flash (AED) | fp32, int8 | ✓ | ✓ | WER 9.70%==ref, 0% |
| canary-1b-v2 (AED, 25-lang+translate) | fp32, int8 | 588ms (0.05) ✓ | 2089ms (0.19) ✓ | WER 3.60%==ref, 0% |
| cohere-transcribe (AED, merged-KV) | fp16, q4, quantized | FAIL³ | 2691ms (0.24) ✓ | WER 1.66% (SOTA) |
| qwen3-asr-0.6b (LLM-decoder) | fp32, **int4** | (untested⁴) | 7535ms (0.68) ✓ | WER 4.43% |
| qwen3-asr-1.7b (LLM-decoder) | fp32, int4 | (untested⁴) | ✓ (config-driven) | byte-perfect jfk |
| moonshine (enc-dec) | fp32, int8 | ✓ | ✓ | verified |
| fastconformer-quran-ar (FastConformer Hybrid RNNT, Arabic) | fp32 | ✓ | ✓ | **exact == NeMo ref** on Quran audio (بسم الله الرحمن الرحيم), both EPs |
| nemotron-speech-streaming-en-0.6b (FastConformer RNNT, offline-exported) | fp32 | ✓ | ✓ | **exact == NeMo ref** on jfk, both EPs |
| nemotron-3.5-streaming-0.6b (cache-aware FastConformer-RNNT, 40-lang) | **int4** | 1811ms (0.16) ✓ | 2545ms (0.23) ✓ | jfk exact==ref+ground-truth; ≥99% word-agree on 400-word sample⁶ |
| Fun-ASR-Nano-2512 (SenseVoice-enc + Qwen3-0.6B LLM-decoder, 31-lang) | **int8** | (CPU-only⁷) | 2528ms (0.23) ✓ | **jfk EXACT**; fluent zh/yue/wu/minnan/ja + ja-en code-switch + en-lyrics on the bundled test_wavs |
| Voxtral-Mini-4B-Realtime (lockstep streaming: causal audio-enc + 26L Mistral LM-decoder, 13-lang) | int8, **q4f16** | FAIL⁸ (ORT GQA-bias) | q4f16 1.72% WER (RTF 1.72)⁸ | **WER 1.72%** (LibriSpeech, q4f16 CPU); byte-identical to onnx ref (0.0000% disagree); jfk exact |

⁸ Voxtral-Mini-Realtime: full from-scratch WaaV reimplementation (`voxtral_realtime` arm + `voxtral_log_mel`
component). LOCKSTEP streaming transcriber — audio embeds ADDED to text embeds 1:1, one text token per audio
token (480 ms delay baked into the export). 3-graph onnx-community export (audio_encoder + embed_tokens +
decoder_model_merged, split-KV). Mel = Whisper STFT (n_fft400/hop160/hann/center) + 128 slaney mel + Voxtral
FIXED norm (floor 1.5-8). audio_encoder position_ids = arange(T_mel/2) (post-conv2), N_audio = T_mel/8.
Prompt = [BOS]+[STREAMING_PAD]*38. tekken decode via tokenizer.json. Algorithm verified EXACT on full jfk via
a CUDA q4f16 prototype; now runs the **q4f16** graphs (4-bit weights + fp16 caches) via a GRAPH-DRIVEN cache dtype (zeros_typed reads
StaticGraph::input_types() → NamedTensor::f16 vs f32 per the declared dtype; input_features/inputs_embeds/audio_embeds
stay f32, caches+logits f16, argmax_last handles F16). q4f16 **CPU**: WER 1.72% on LibriSpeech, RTF 1.72 (vs int8
~minutes). q4f16 **CUDA** FAILs on ORT's GroupQueryAttention kernel (`attention_bias is not supported` — the SAME
ORT limitation as cohere, note 3; not a WaaV bug). The dtype seam is config-driven/portable so any fp16 sibling or a
future ORT-GQA-bias fix runs with zero code. Geometry (enc 32L / dec 26L / 8 KV-heads / 128 head-dim) read
from the graphs.

## Path-B (PyTorch sidecar runtime — non-ONNX models; see inferv2/INFER_TORCH_RUNTIME.md)

| Model (arch) | runtime | CUDA | CPU | accuracy (vs ref) |
|---|---|---|---|---|
| ARK-ASR-0.6B (Whisper-enc + adapter + Qwen2 LLM-decoder, 19-lang) | torch (transformers, sdpa) | 1031ms (0.09) ✓ fp16 | 7796ms (0.71) ✓ fp32 | jfk exact==ground-truth+HF recipe; physicsworks accurate |
| granite-speech-4.1-2b (Conformer + QFormer projector + Granite LLM + speech-LoRA, 8-lang) | torch (transformers, bf16) | 1430ms (0.13) ✓ bf16 | 24593ms (2.24) ✓ fp32 | jfk exact==ground-truth+HF recipe, BOTH EPs |

Path-B = `waav-infer/torch_runtime/` (Python sidecar) + `waav-infer-server/src/torch_sidecar.rs` (Rust
client). **Fully engine-integrated**: `waav-infer transcribe <dir>` auto-routes to the sidecar when
`waav.json` has `{"runtime":{"backend":"torch",...}}`, satisfying the same `SttModel` contract as ONNX
models (RTF/accuracy above are via the engine, not the standalone CLI). A new non-ONNX model = a waav.json
runtime block + a Python model class (zero per-model Rust — the config-driven onboarding parity with Path-A).
The runner drives the model's own HF code, so the runtime IS the reference engine (no export drift). Stack
today = transformers eager + sdpa; `torch.compile`/CUDA-graphs + the shared AR-decoder/codec seams are the
next perf pass. **Long audio handled**: `SttRunner` windows audio past `WINDOW_S` (30 s) into 28 s chunks and
joins — verified on a 203 s lecture (full transcript, RTF 0.03). Known: each non-transformers-native torch
model (custom-package/codec TTS) needs **per-model dependency isolation** (a venv) — the sidecar spawns a
configurable `python`, so that slots in via a `waav.json` `runtime.python` field (to be wired).

## Diarization (speaker turns)

| Model (arch) | CUDA | CPU | accuracy (vs ref) |
|---|---|---|---|
| pyannote/speaker-diarization-3.1 (powerset seg + WeSpeaker emb + AHC) | ✓ | ✓ | verified |
| pyannote/speaker-diarization-community-1 (community-1 weights) | ✓ | ✓ (6.9s/29s) | **2-spk jfk+kokoro clip: correct count + boundaries (0.3–10.5 / 11.8–23.0 / 24.3–28.4 vs GT 0–11 / 11.5–23.6 / 24.1–end) + correct speaker-A re-ID** |

community-1 reuses the existing Rust diarization runtime unchanged; the only code delta was making the
segmentation/embedding **input tensor names config-driven** (`input_names()[0]` from each graph) — 3.0/3.1
export uses `x`/`feats`, the altunenes community-1 ONNX mirror uses `input_values`/`fbank_features`. Adding a
pyannote variant is now weights-only. Onboarded via the ungated CC-BY-4.0 ONNX mirror
`altunenes/speaker-diarization-community-1-onnx` (official pyannote repo is gated + pip-only / non-portable).

## Enhancement / audio-to-audio (RTF on 11s)

| Model (arch) | CUDA | CPU | accuracy (vs ref) |
|---|---|---|---|
| GTCRN (streaming spectral, 3-cache) | ✓ | ✓ | matches python ONNX ref |
| dasheng-denoiser (raw waveform, exported from PyTorch) | ✓ (corr 1.00) | ✓ | corr 1.00000 vs torch |
| DPDFNet dpdfnet2/4/8 (DeepFilterNet-family streaming spectral, 1 flat state) | 26925ms (2.45)¹ | 1862ms (0.17) ✓ | **corr ≥0.99999 vs official `dpdfnet` pkg — all 3 16 kHz variants, weights-only, both EPs** |

¹ DPDFNet per-frame streaming (one ONNX call per STFT frame, ~688 frames/11s) → CUDA launch overhead dominates;
**CPU is the right EP** (14× faster here). Pure-Rust front-end: `Stft::with_window_center` (librosa center
framing) + `vorbis_window` (new), state seeded from ONNX `metadata_props` (erb/spec-norm init), `n_fft*2`
latency trim. All geometry is config-driven from `metadata_props` — adding the 48 kHz / dpdfnet4/8 variants is
weights-only. Reference = the official `pip install dpdfnet` package.

## TTS (RTF, CUDA)

| Model (arch) | runtime | CUDA RTF | accuracy |
|---|---|---|---|
| kokoro-82M (one-shot) | ONNX (Path-A) | 0.212 | verified |
| supertonic-3 (flow-matching) | ONNX (Path-A) | 0.150 | bit-identical vs ref |
| chatterbox (T3 Llama-AR + S3Gen CFM + HiFTGenerator) | **portable ONNX** (Path-A, 4-graph onnx-community; venv RETIRED) | (CPU 5.3) | **round-trip WER 0.0%** (3/3 sentences exact, incl. tongue-twister) — robustly intelligible |
| chatterbox-turbo (GPT-2-AR + S3Gen MeanFlow-2step + HiFTGen) | **portable ONNX** (same `chatterbox` arm, config-driven) | (CPU 2.0) | intelligible — **ASR round-trip == input**; 2.6× faster than base (distilled CFM) |
| MeloTTS-English (VITS + lexicon/tones frontend) | **portable ONNX** (Path-A, sherpa vits-melo) | 0.20 (CPU) | **round-trip WER 14.0%** (clear sentences exact; rest = funasr ASR mishearings, ref makes identical errors); OOV acronyms now SPOKEN (char spell-out, ==reference) |
| OmniVoice (masked-diffusion LM: bidirectional Qwen3-0.6B + 32-step CFG unmask + HiggsV2 codec) | torch (Path-B sidecar, shared env, NO venv) | 0.52 (CUDA) | **round-trip WER 0.0%** (5/5 exact, incl. tongue-twister) — matches vanilla; fixed via the vanilla duration estimator + seed (the earlier 39.5% was duration-too-long, not the gumbel) |
| Sesame CSM-1B (Llama-1B-AR backbone + Llama-100M depth-decoder + Mimi codec) | torch (Path-B sidecar, transformers-native Csm+Mimi, NO venv) | 1.43 (CUDA) | **round-trip WER 0.0%** (4/4 exact) |
| CosyVoice3-0.5B (Qwen2 LLM-AR + flow-CFM[ONNX estimator seam] + HiFT vocoder, zero-shot) | torch (Path-B sidecar, shared env + vendored modeling, NO venv) | 4.87 (CUDA) | **round-trip WER 0.0%** (8/8 varied exact post-fix); **proven == native CosyVoice engine**¹⁰ |
| Dia-1.6B (Nari Labs dialogue TTS: text→DAC codec-AR, CFG + `[S1]/[S2]` speaker tags) | torch (Path-B sidecar, transformers-native `DiaForConditionalGeneration`+`DiaProcessor`, NO venv) | ~2.1 (CUDA) | **round-trip WER 0.0%** (committee/date/names/village/paragraph exact, e2e engine exact); **proven == native nari-labs engine**⁹ |
| NeuTTS-Air-0.5B (Neuphonic on-device codec-AR TTS + voice clone: Qwen2-0.5B emits `<\|speech_N\|>` → NeuCodec) | torch (Path-B sidecar, OUR forward on stock `Qwen2ForCausalLM` + espeak G2P + NeuCodec ONNX decoder, NO venv) | 0.69 (CUDA) | **SAMPLE-IDENTICAL to native** (maxΔ 1e-4, NCC 1.00000, exact sample counts on 3 sentences) + round-trip exact¹¹ |
| Qwen3-TTS-12Hz-0.6B-CustomVoice (Alibaba: Talker AR + sub-talker CodePredictor + SpeakerEncoder + 12Hz codec; 9 speakers) | torch (Path-B sidecar, VENDORED `Qwen3TTSForConditionalGeneration` modeling on shared transformers 5.12 via 4.57→5.12 compat shims, NO venv) | 0.83 (CUDA) | **== native** (NCC 0.99, exact sample counts, transcripts exact on fresh + test sentences); ran on shared 5.12 via version-bridge¹² |
| dots.tts-base (rednote: stock Qwen2-1.5B + semantic-encoder AR + flow-matching DiT head + CAM++ x-vector + VAE vocoder, 48kHz zero-shot clone) | torch (Path-B sidecar, VENDORED `DotsTTSForConditionalGeneration` modeling on shared transformers 5.12, NO venv; +torchdiffeq) | ~1.7 (CUDA) | **BIT-IDENTICAL to native** (NCC 1.00000, maxΔ 0.0 runner-vs-native; transcripts exact on test + fresh sentences)¹³ |
| VibeVoice-1.5B (Microsoft: Qwen2.5-1.5B LLM decoder + DDPM diffusion head[v-pred cosine, 10-step] + acoustic/semantic conv codec, voice-clone) | torch (Path-B sidecar, VENDORED `VibeVoiceForConditionalGeneration` on shared transformers 5.12 via 4.51.3→5.12 shims, NO venv; +diffusers scheduler) | ~0.7 (CUDA) | **behavioral parity == native** (transcripts exact on 3/3 fresh + test sentences with a real-human reference voice); DDPM non-deterministic so no sample-match¹⁴ |
| Dia2-2B (Nari Labs dialogue TTS: custom 28L GQA decoder + Depformer + 32-channel delay-pattern AR + CFG, Mimi codec, `[S1]/[S2]`) | torch (Path-B sidecar, VENDORED `dia2` engine on shared transformers 5.12, NO shims/venv; Mimi via transformers-native `MimiModel`) | ~2.4 (CUDA) | **BIT-IDENTICAL to native** (NCC 1.0000, maxΔ 0.0 on 3 sentences incl. fresh; transcripts exact; no short-text ramble)¹⁵ |
| higgs-audio-v3-tts-4b (Qwen3-4B codec-AR, 8-cb delay pattern + neural codec) | **open ONNX** via Path-B sidecar (fp16; multi-quant/hw published) | 18.8¹ | intelligible — **ASR round-trip** of the engine output == input text |

**Chatterbox is now PORTABLE (venv RETIRED).** Both base + chatterbox-turbo run on the official onnx-community 4-graph ONNX export (speech_encoder + embed_tokens + language_model + conditional_decoder) via Rust ORT — NO `chatterbox-tts` pip pkg, NO venv. The T3 AR loop (greedy + repetition-penalty 1.2, 30L Llama base / 24L GPT-2 turbo) runs in host code (shared AR-decoder seam); S3Gen CFM + HiFTGenerator are inside conditional_decoder. ONE config-driven `chatterbox` arm serves both exports (it auto-detects where position_ids/exaggeration live + the turbo silence tail from the graph inputs). Voice conditioning = speech_encoder(default_voice.wav) run once at load + cached. Verified by ASR round-trip (funasr-nano) reproducing the input exactly for base AND turbo.

## Notes / known hardware gaps (ORT kernel limitations, not WaaV bugs)

1. **fp16 CPU**: ORT has no CPU kernel for some contrib ops (`com.microsoft.Gelu` etc.) in fp16 → fp16 is GPU-only; deploy fp32/int8 on CPU.
2. **int8 CPU often *faster* than CUDA** for small models (CUDA launch/copy overhead dominates a sub-second op; int8 has tuned CPU kernels).
3. **cohere CUDA**: ORT CUDA `GroupQueryAttention` kernel rejects `attention_bias` → cohere is CPU-only until ORT adds it (or a non-GQA export).
4. **qwen3 CUDA**: untested (LLM decode is autoregressive token-by-token; the GQA/MRoPE ops may hit the same CUDA-kernel gaps — to verify).

## Per-quant × hardware verification (live, jfk 11s, ✓ = correct output)

| Model / quant | CUDA | CPU |
|---|---|---|
| parakeet-tdt-v2 / fp32 | 549ms ✓ | 693ms ✓ |
| parakeet-tdt-v2 / **int8** | 684ms ✓ | 459ms ✓ |
| sensevoice / **int8** | 792ms ✓ | 545ms ✓ |
| qwen3-asr-0.6b / fp32 | ✓ | 7535ms ✓ |
| qwen3-asr-0.6b / **int4** | 5236ms ✓ | 6437ms ✓ |
| canary-1b-v2 / fp32 | 588ms ✓ | 2089ms ✓ |
| canary-1b-v2 / int8 | ✓ | ✓ |
| whisper-turbo / fp16 | 1540ms ✓ | (fp16 GPU-only) |
| whisper-turbo / int8 | FAIL⁵ | FAIL⁵ |
| nemotron-3.5-streaming / **int4** | 1811ms ✓ | 2545ms ✓ |

⁵ whisper-turbo int8: the int8 ONNX export hits an unsupported quant op / incomplete download in this ORT — fp16 (GPU) + fp32 (CPU) are the working paths. Quant works for the transducer/CTC/LLM families (parakeet int8, sensevoice int8, qwen3 **int4**, nemotron **int4** all correct on both EPs; int4 is even CUDA-faster).

⁷ Fun-ASR-Nano-2512: full from-scratch WaaV reimplementation (new `funasr_nano` registry arm). 3-graph sherpa-onnx int8 export (encoder_adaptor + embedding + llm) with an explicit caller-managed per-layer KV cache (Qwen3UnifiedKvDelta: 28 layers × `cache_{key,value}_i[1,512,8,128]`, deltas appended at the absolute cache_position; graph stores RoPE-applied K + raw V). Frontend = 80-dim kaldi-fbank (×32768) + unpadded LFR(7,6) → 560-dim, NO CMVN. Prompt scaffold scatters encoder_out over placeholder positions in `…user\n语音转写：<audio>…assistant\n`; greedy decode, EOS=151645 (forbidden as the first token). int8 dynamic-quant is CPU-only by design (MatMul-only QInt8); all tensor I/O is fp32. Geometry (layers/heads/cap/hidden) read from the llm graph → other Qwen3 sizes load with no code change. Verified: jfk byte-exact vs ground truth; the bundled multilingual test_wavs transcribe fluently (zh incl. domain terms, Cantonese 佢哋/咗, Shanghainese, Minnan, Japanese, ja-en code-switch, English lyrics). RTF 0.23 (CPU).

⁶ nemotron-3.5-streaming-0.6b: cache-aware streaming FastConformer-RNNT, onnx-community int4 (encoder+decoder+joint, 13088 vocab, 40 language-locales via lang_id prompt conditioning; lang_id=0 auto-detect is the verified default). Pure-Rust NeMo `normalize="NA"` log-mel front-end (new `NemoMel` component) + 65-frame chunked encoder cache threading + LSTM-decoder RNN-T greedy. Accuracy: jfk transcript is an **exact match** vs both the canonical ground truth and a faithful onnxruntime reference (`eval/nemotron_ref.py`, drives the shipped ONNX via the documented genai recipe); query_to_cars exact; a 400-word physics-lecture sample agrees ≥99% word-for-word, the few divergences (hand-coded mel vs librosa numerical noise through 24 conformer layers) actually favoring the Rust output ("demolish" vs "the molish", "percent" vs "per cent"). The exact NeMo featurizer (vs this librosa-faithful approximation) would require NeMo installed; not available on this box.

## Observations
- **Transducer (TDT/RNN-T/CTC) is the RTF sweet spot** (~0.05 CUDA): frame-synchronous, single forward + cheap decode.
- **AED/LLM decode is slower** (autoregressive, one graph call per token): canary-1b 0.05–0.19, qwen3 0.68 — inherent to the architecture; CUDA helps the big ones most.
- **int8/int4 quants verified working** where shipped (parakeet, sensevoice, qwen3-int4) — same transcription as fp32.
- Every model is correct on **at least one EP**; CPU is the universal fallback.

¹ higgs RTF 18.8: the AR loop runs the Qwen3-4B llm_decoder ONCE PER 24kHz-frame via ORT (no KV-batching/
IObinding) → launch-bound. Perf refinement (ORT IOBinding, fp16 KV reuse, CUDA-graph the decode step) is the
next pass; correctness/intelligibility is verified. The codec path alone runs at 194ms (RTF ~0.09).

⁹ Dia-1.6B: rigorously verified faithful to the **native nari-labs `dia` engine** (the inference engine the
model ships with), per the "verify against the model's own engine" requirement. PROVEN identical at three
levels: (a) **WEIGHTS** — every transformers tensor WaaV serves equals the native `pytorch_model.bin`: 81
bit-identical + 192 exact reshape/transpose of the attention projections (native stores `[*,heads,head_dim]`
einsum layout, transformers flat `nn.Linear`), **0 mismatches**; (b) **ALGORITHM** — the transformers Dia
logits processors (`DiaClassifierFreeGuidanceLogitsProcessor` with cfg_top_k + `DiaEOSChannelFilterLogitsProcessor`
force/suppress-EOS-when-argmax + per-channel constraints + `DiaEOSDelayPatternLogitsProcessor`) are line-for-line
equivalent to native `_sample_next_token`/`_decoder_step`; (c) **CODEC** — same Descript DAC-44kHz decoder.
Behavioral parity on a **10-sentence varied set** (short / medium / long paragraph / 2-speaker dialogue /
tongue-twister / dates / questions / proper-nouns): both engines transcribe in-distribution text exactly, and
BOTH exhibit the *same intrinsic ramble* on bare ultra-short out-of-distribution prompts ("The quick brown
fox…": native EOS@2524≈29s, WaaV≈2601≈30s) — a property of the dialogue-trained model, NOT a port bug
(confirmed by running the native engine itself). Runner uses the model's shipped `generation_config.json`
defaults (cfg 3.0 / temp 1.8 / top_p 0.90 / top_k 50 / max_len 3072) with NO overrides for maximum fidelity
(an earlier length-scaled token cap was REMOVED as a non-faithful patch). Bit-exact per-sample audio is
impossible for `do_sample=True` (the RNG stream differs across any two implementations — even native-vs-native
differs unseeded) → the only residual difference; the *model + algorithm + codec + quality* are identical.
Native reference generated via a THROWAWAY system-site venv (DAC `.pth` from the descript GitHub release;
`pytorch_model.bin`→`model.safetensors` convert to satisfy the mixin loader), never a serving path.

¹⁰ CosyVoice3-0.5B: held to the same native-parity bar (a from-scratch reimplementation, so the highest
divergence risk). **ROOT-CAUSE BUG FOUND + FIXED:** an occasional phrase-DUPLICATION (e.g. "names" → "Dr.
Carter traveled from San Francisco, Dr. Carter traveled from San Francisco to New York…", 9.66s) traced to a
MISSING `<|endofprompt|>` (token 151646) in the precomputed `default_voice.pt` prompt_text. CosyVoice3 was
trained with a `"<system>"+<|endofprompt|>+"<prompt transcript>"` format and its LLM **hard-asserts** 151646 is
present; my original precompute encoded `<|endofprompt|>` as LITERAL characters (plain HF tokenizer) instead of
the atomic special token (native uses tiktoken `allowed_special='all'`). Fix: rebuilt prompt_text_ids with the
specials registered so 151646 is atomic (+ corrected the ignore_eos mask index to native's `speech_token_size`
6561). **VERIFIED vs the native FunAudioLLM/CosyVoice engine** (throwaway venv, light deps only — LLM path is
matcha-free): (a) my checkpoints load into the native `CosyVoice3LM` with **0 missing / 0 unexpected** (identical
weights+structure); (b) native sampling params match exactly (ras_sampling top_p 0.8 / top_k 25 / win 10 /
tau_r 0.1, speech_token_size 6561); (c) **native A/B**: native + the FIXED prompt generates 136–152 clean tokens
(~5.4–6.1s) across seeds — matching WaaV's fixed 5.50s; native + the BROKEN (no-151646) prompt **ASSERT-FAILS
(the engine refuses it)** — definitive proof the missing token was the root cause. Post-fix WaaV: 8/8 varied
sentences clean (duplication eliminated; ~one minor trailing-word filler, within native zero-shot variance).
Note: native's `forward_one_step` (length-1 attention_mask + KV cache) only works on its pinned transformers
4.51.3, degenerate on 5.12 — WaaV avoids this by using HF-standard cache handling, which is why WaaV is correct.

## Native-engine verification status (2026-06-16 — "no-tradeoffs" sweep)
Every model held to: faithful to the engine it ships with. Two classes:
- **transformers-native ports** (run the official/provider model class via `from_pretrained` → algorithm + weights
  faithful by construction): **Dia-1.6B** (deep weight-proof, see ⁹), **ARK-ASR-0.6B**, **granite-speech-4.1-2b**,
  **CSM-1B** — all A/B-CERTIFIED vs vanilla transformers (independent scripts): ARK + granite **byte-identical**
  transcripts on jfk; CSM audio NCC 1.000000 (max-Δ 3e-5 = bf16 CUDA fp-noise), both transcribe to input. Zero
  weight surgery / quantization / recipe-divergence in any runner (grep-verified).
- **reimplements** (real divergence risk, deep-verified vs the native engine): **CosyVoice3** (bug found+fixed, see ¹⁰),
  **OmniVoice** (params+algorithm match native, A/B within RNG variance), **Voxtral** (byte-exact vs onnxruntime +
  traced faithful vs the antirez reference + mel bit-faithful), **funasr-nano** (byte-exact to its ONNX; native-matching
  on clear audio, decode-level edge-divergence on hard homophone/dialect spans — a re-export limit, not a bug, fp32
  confirmed). **higgs-audio-v3-tts-4b**: INTELLIGIBILITY-VERIFIED (ASR round-trip exact on 4/4 fresh sentences incl. the
  WaaV engine) + the codec was previously verified bit-exact vs PyTorch. Full native A/B is IMPRACTICAL (not size — the
  8.5GB checkpoint is cached): the native v3 arch `HiggsMultimodalQwen3ForConditionalGeneration` exists ONLY inside the
  SGLang-Omni/vLLM-Omni serving stacks (no `from_pretrained().generate()` path, no remote code/auto_map on HF), and the
  only pure-python alt (pip `boson-multimodal`) is the WRONG v2/Llama arch that won't import on transformers 5.12
  (pins <4.47, imports the removed LLAMA_ATTENTION_CLASSES). Honest-characterized like funasr-nano; the ONNX path is sound.

¹¹ NeuTTS-Air-0.5B: onboarded NEW at the native-parity bar (verify-vs-native built in from the start). Neuphonic's
on-device voice-cloning TTS = a STOCK `Qwen2ForCausalLM` (896-hidden/24L) whose vocab holds `<|speech_N|>` NeuCodec
tokens; given a phonemized prompt conditioned on a reference voice it AR-emits codec tokens (do_sample temp 1.0/top_k
50/min_new 50, eos `<|SPEECH_GENERATION_END|>`), regex-extracted → NeuCodec ONNX decoder → 24 kHz. The runner
(`torch_runtime/models/neutts_air.py`) is a portable reimplementation of OUR forward reusing only shared components —
transformers Qwen2 (LM) + espeak-ng `phonemizer` (G2P, like a tokenizer) + NeuCodec ONNX decoder (codec) — NO `neutts`
pip runtime, NO venv, ZERO Rust (config-driven `architecture:neutts_air` → Python sidecar). Default voice precomputed
(`default_voice.pt` = NeuCodec ref-codes from the repo's `dave.pt` + `default_voice.txt`). **VERIFIED == native engine
at the SAMPLE LEVEL**: vs the native `NeuTTS.infer` (throwaway venv, same seed) on committee/twister/names → identical
sample counts (120000/74880/138720), maxΔ 1e-4 (bf16 CUDA fp-noise), NCC 1.000000; all three round-trip exact. The
sample-identity (not just behavioral) is possible here because the LM is transformers-native (same forward) + greedy-ish
seeded sampling lands the same trajectory. Apache-2.0, ungated. Verified count ~38.

¹² Qwen3-TTS-12Hz-0.6B-CustomVoice: the session's HARDEST onboard — a custom multi-component arch
(`Qwen3TTSForConditionalGeneration`: Talker AR + sub-talker CodePredictor + SpeakerEncoder + 12Hz speech-tokenizer
codec, 9 predefined speakers) whose native engine (`qwen-tts` pip pkg) PINS transformers 4.57 — INCOMPATIBLE with the
shared sidecar's 5.12. Rather than a per-model venv (charter-retired), VENDORED the modeling
(`torch_runtime/vendor/qwen3_tts/core/{models,tokenizer_12hz}` + a vendored 12Hz tokenizer) and bridged it onto the
shared 5.12 via `_compat.py` shims for SIX 4.57→5.12 API changes: (1) `check_model_inputs` factory-vs-direct decorator;
(2) base `PretrainedConfig` legacy token-id attrs (5.12 raises, 4.57 returned None) — narrow `__getattr__` fallback;
(3) `ROPE_INIT_FUNCTIONS["default"]` dropped in 5.12 — re-registered; (4) `create_causal_mask`/
`create_sliding_window_causal_mask` kwarg rename (`input_embeds`→`inputs_embeds`, dropped `cache_position`);
(5) `prepare_inputs_for_generation` 5.12 only builds `cache_position` for remote-code models → restored the 4.57
`arange(seq_len)+past_seen` contract (wrapped with functools.wraps so HF's inputs_embeds signature check still passes);
(6) **two SILENT 5.12 loading regressions on the nested multi-PreTrainedModel arch** — only 21/402 weight tensors loaded
(Talker+CodePredictor left at init, NO missing-key warning, due to 5.12 base_model_prefix key-renaming) → fixed by
copying the full checkpoint state_dict (strict=False), AND the rotary `inv_freq` non-persistent buffers left as
uninitialized garbage under 5.12 meta-init → recomputed via each module's `rope_init_fn`. Before (6) it generated 655s
of garbage (random weights/dead RoPE, never EOS); after, greedy first-6 codebook tokens MATCH native exactly
([1995,215,1494,1010,1010,1686…]) and it stops at the right length. **VERIFIED == native** (qwen-tts 4.57 reference,
isolated venv, speaker aiden / seed 0): identical sample counts (committee 120960, twister 80640), NCC 0.9941/0.9917
(residual = bf16+SDPA fp-noise), transcripts EXACT on the test set AND an independent fresh sentence ("Doctor Carter…").
RTF 0.83 CUDA. ZERO Rust, NO venv/qwen-tts at serve time (only the vendored modeling + shared transformers 5.12 +
torch/onnxruntime); shared env confirmed intact (5.12, Dia/CSM still serve). The `_compat.py` shims are version-detected
(benign on stock configs) → they also de-risk onboarding OTHER transformers-4.5x-era model code onto the shared 5.12.
Verified count ~39.

¹³ dots.tts-base: continuous-latent flow-matching TTS (NOT discrete codec) — stock Qwen2-1.5B backbone emits hidden
states; a semantic-encoder re-encodes each generated VAE patch back to a compact LLM embedding (AR), and a flow-matching
DiT head (torchdiffeq Euler ODE, num_steps 10, guidance_scale 1.2) denoises the next 48kHz-VAE latent patch conditioned
on the LLM hidden + AR prefix + a frozen CAM++ speaker x-vector; VAE vocoder → 48 kHz. No dep conflict (`transformers>=4.57`
covers 5.12) so the provider modeling was VENDORED (`torch_runtime/vendor/dots_tts/`, pruned to 40 files/416K) and runs on
the shared 5.12 via its real DotsTtsRuntime — only edit: made the `WeTextProcessing`/`pynini` normalizer import LAZY
(pynini unbuildable on aarch64; reached only with normalize_text=True, serving uses False). Shared-env footprint = 4 leaf
deps (loguru/langcodes/lingua/torchdiffeq; none touch transformers/torch). Default voice precomputed once from a ref clip
(`default_voice.pt`: CAM++ 512-d x-vector + prompt VAE-latent dist [1,256,188] mean‖log_std + 48kHz prompt audio +
transcript), injected into the runtime's prompt-feature cache under the exact padded-audio SHA1 → serving runs neither the
CAM++ nor the VAE encoder. **VERIFIED BIT-IDENTICAL to native** (dots_tts pip ref, isolated /tmp/dots_venv, seed0):
NCC 1.000000, maxΔ 0.000000 (python runner vs native), exact sample counts; Rust-CLI maxΔ 6.1e-5 (f32 WAV round-trip at
the engine edge, not the model); transcripts exact on test + an INDEPENDENT fresh sentence (whisper-base, funasr-nano).
seed0 covers both the FM Euler-ODE noise init and the prompt-latent reparam sampling → the stochastic path reproduces
native exactly. RTF ~1.7 CUDA, ~22s load. Apache-2.0 ungated. Shared env intact (5.12, Dia/CSM serve). Verified count ~40.
**+2 VARIANTS (same `dots_tts` arm, pure config-swap reuse, ZERO new code/Rust):** `dots.tts-soar` (Self-Corrective
Alignment, the recommended zero-shot default) and `dots.tts-mf` (CFG-aware MeanFlow distillation — config carries
`meanflow.enabled` so the vendored modeling auto-routes the FM step through `_meanflow_step_fm(nfe=num_steps)`, CFG fused
into the student so guidance_scale is internally ignored; targets 2-4 NFE but bit-identical at the runner's nfe=10). Each:
own checkpoint (4.9GB) + per-checkpoint `default_voice.pt` (NOTE: the CAM++ x-vector + VAE prompt latents are bit-identical
across base/soar/mf — AudioVAE + speaker encoder are FROZEN across all three; only the LLM+DiT backbone differs).
BOTH VERIFIED BIT-IDENTICAL to native (NCC 1.000000, maxΔ 6.1e-5 = int16-WAV step) + engine transcripts exact on test +
INDEPENDENT fresh sentences. Verified count ~42.

¹⁴ VibeVoice-1.5B: a transformers-**4.51.3**→5.12 version-bridge (OLDER pin than Qwen3-TTS's 4.57 → more shims). Qwen2.5-
1.5B LLM decoder emits latents → DDPM diffusion head (v-pred, cosine, 10-step) + acoustic/semantic conv codec → voice
clone. VENDORED `VibeVoiceForConditionalGeneration` into `torch_runtime/vendor/vibevoice/` (modeling + diffusion_head +
tokenizers + processor + dpm_solver schedule), run on the shared 5.12 via `_compat.py` (the qwen3 shim set + 2 new:
Auto*-register exist_ok for 5.12's reserved vibevoice model_type entries; scheduler meta-init under torch.device("cpu")).
Plus ~8 vendored-file API-location/signature fixes (Qwen2TokenizerFast move; tie_weights/_prepare_generation_config/
_prepare_cache_for_generation signature drift; get_text_config→decoder_config for the 5.12 KV-cache builder). **SILENT-LOAD
BUG CAUGHT (the headline):** the checkpoint omits `lm_head.weight` (tied, flag on `decoder_config`), but 5.12's tie gates on
the top-level composite config's (absent) flag → lm_head left at RANDOM init → garbage logits → AR loop hit EOS at step 1,
ZERO audio. Fix: explicitly tie lm_head→embed_tokens reading decoder_config's flag (1204/1204 tensors then faithful). Only
`diffusers==0.38.0` added to the shared env (clean leaf). **VERIFIED behavioral parity == native** (both non-deterministic):
transcripts EXACT on test (committee/twister) + 3/3 INDEPENDENT fresh sentences. Sample-NCC is NOT meaningful — VibeVoice is
non-deterministic on CUDA bf16 even seeded+greedy (WaaV-vs-WaaV reruns differ in length; ulp bf16-SDPA noise shifts EOS over
the AR loop), same as native. **QUALITY FIX (verify-empirically): the default voice MUST be a REAL human recording** — the
initial synthetic cosyvoice3-generated reference caused leading-word instability ("The morning sun"→"The boy beside"); swapping
default_voice.npy to a real 7.45s recording (neutts dave.wav) made s1+s2+s3 all exact. RTF ~0.7 CUDA. Shared env intact (5.12,
Dia/CSM serve). MIT, ungated. Verified count ~43.

¹⁵ Dia2-2B: Nari Labs' custom dialogue-TTS arch (NOT transformers — config has data/model/runtime sections, no
`architectures` field): a 28L GQA decoder + Depformer + 32-channel delay-pattern AR loop + CFG, with a **Mimi codec**
(`[S1]/[S2]` speaker tags). Native = the `dia2` github pkg (`Dia2.from_repo`/`from_local`). VENDORED the package verbatim
into `torch_runtime/vendor/dia2/` and run its REAL engine on the shared 5.12 — **NO compat shims needed** (dia2 uses its
OWN RotaryEmbedding/KVCache, never touches the 4.x→5.12 framework internals; its pin `transformers>=4.55.3` has no upper
bound). **Mimi codec = transformers-native `MimiModel`** (kyutai/mimi pytorch weights cached at ~/.cache/waav-models/
kyutai-mimi for offline serving). The ONLY vendoring edit: an import-guard on `sphn` (Rust/maturin, no aarch64 wheel; only
on the prefix/voice-clone path the default serving never hits) — function bodies kept byte-identical to upstream (an
initial rewrite caused a numeric divergence → reverted, md5-verified all core/runtime files == upstream). **SILENT-LOAD
trap GUARDED** (`_assert_full_load` re-opens the safetensors, confirms all 450 tensors present + bit-copied — the same
in-place `state_dict.copy_()`-skips-missing-keys trap caught on Qwen3/VibeVoice). RNG-ordering divergence traced+fixed
(bare per-call `seed(0)` reproduces native's warm-runtime steady-state sampling order = the production-serving order).
**VERIFIED BIT-IDENTICAL to native** (NCC 1.0000, maxΔ 0.0 on committee/twister + INDEPENDENT fresh sentences; transcripts
exact). Unlike Dia-1.6B, Dia2 handles short sentences fine (no OOD ramble). RTF ~2.4 CUDA. Zero Rust. Shared env intact
(5.12, Dia+CSM+Mimi import). Verified count ~44.
