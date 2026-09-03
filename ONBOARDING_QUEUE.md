# WaaV Infer — Model Onboarding Queue (triage-models workflow, 90 models)

Totals: 90 models — 4 DONE, 12 COVERED (arch supported), 74 TODO. Tiers: T1=12 (config+weights), T2=7 (new arm, existing components), T3=65 (new component stack), T4=6 (not ONNX).

UPDATE 2026-06-14: **nemotron-3.5-asr-streaming-0.6b (onnx-community int4) — DONE/live-verified.** New `nemotron_speech` registry arm + `NemotronRnnt` module + pure-Rust `NemoMel` (NeMo normalize="NA" log-mel) component. First **cache-aware streaming transducer** in the engine (65-frame chunked encoder cache threading + LSTM-decoder RNN-T greedy + lang_id auto-detect). jfk transcript exact vs onnxruntime reference + ground truth; CUDA 1811ms (RTF 0.16) + CPU 2545ms (0.23), int4. Covers BOTH model_list entries (rank-1 base `nvidia/nemotron-3.5-asr-streaming-0.6b` + rank-14 int4-onnx). See PERF_MATRIX note ⁶.

## T1

- **nvidia/parakeet-tdt-0.6b-v3** [COVERED/high] — NeMo transducer TDT (FastConformer enc + TDT joint)
  - onnx: istupakov/parakeet-tdt-0.6b-v3-onnx
  - ref: NeMo EncDecRNNTBPEModel (nemo-toolkit) / onnx-asr
  - needs: none — reuse nemo128 preprocessor-as-graph + transducer decode; add SentencePiece vocab + 25-lang config
  - Same arm as parakeet-tdt-v2/v3 already supported; trivial config+weights add that unlocks 25-language multilingual ASR — high ROI.
- **nvidia/nemotron-speech-streaming-en-0.6b** [COVERED/high] — NeMo cache-aware FastConformer (24L, 8x subsample) + RNNT transducer; 0.6B, streaming (~80ms-1.12s chunks), EN
  - onnx: none shipped on repo — but arch already supported via existing nemo128+transducer export path (sherpa-onnx/NeMo ONNX export)
  - ref: NeMo / sherpa-onnx
  - needs: none — reuse nemo128 preprocessor-as-graph + transducer decode (RNNT) + SentencePiece; config+weights only
  - Exactly the parakeet-rnnt arm WaaV already runs; near-zero effort, adds a strong sub-100ms streaming EN model. Just needs RNNT ONNX export + waav.json.
- **openai/whisper-large-v3** [COVERED/medium] — Whisper attention encoder-decoder
  - onnx: onnx-community/whisper-large-v3-ONNX (encoder + decoder_merged, fp32/fp16/q4); also Systran/openai-whisper exports
  - ref: openai-whisper / HF transformers WhisperForConditionalGeneration (already the engine WaaV verifies whisper against)
  - needs: none — reuse existing WhisperStt module (whisper log-mel + AR enc-dec); pure config+weights
  - Same arch as already-onboarded whisper sizes; literal add is config+weights only. ~2B (1.55B) so heavier but well under 10B. Highest WER quality whisper — worth shipping as a config drop-in.
- **mohammed/fastconformer-quran-ar** [COVERED/medium] — NeMo EncDecHybridRNNTCTCBPE FastConformer-Large (18 enc blocks, 80-mel, SP-BPE 1024); 114.6M, Quranic Arabic; fine-tune of stt_ar_fastconformer_hybrid_large
  - onnx: none shipped — reuse existing NeMo hybrid CTC/RNNT ONNX export path (sherpa-onnx)
  - ref: NeMo / sherpa-onnx
  - needs: none — reuse nemo80 preprocessor + CTC greedy (and/or transducer decode) + SentencePiece; config+weights only
  - Same hybrid FastConformer CTC/RNNT arm already supported; trivial config add. Adds a high-accuracy (0.14% WER) Arabic/Quran capability — value is the domain, not the arch.
- **openai/whisper-large-v3-turbo** [DONE/low] — Whisper attention encoder-decoder (4-layer decoder turbo)
  - onnx: onnx-community/whisper-large-v3-turbo (already used); accuracy-verified per WaaV-Infer onboarding memory
  - ref: HF transformers — already verified 0.00% disagreement vs transformers on LibriSpeech subset
  - needs: none — already onboarded
  - Already DONE in WaaV-Infer (per model-onboarding memory, whisper-large-v3-turbo onboarded with 0.00% disagreement). No work.
- **ggerganov/whisper.cpp** [COVERED/low] — whisper enc-dec
  - onnx: n/a — whisper.cpp is a GGML runtime, not a model; weights map to existing whisper enc-dec (use openai/whisper-* or onnx-community whisper exports)
  - ref: openai-whisper / transformers WhisperForConditionalGeneration
  - needs: none — reuse whisper log-mel + AR enc-dec already supported (all sizes incl large-v3-turbo)
  - Redundant: this is the whisper.cpp inference engine, not a new arch; WaaV already covers every whisper size natively via ORT.
- **nvidia/parakeet-tdt-0.6b-v2** [COVERED/low] — NeMo transducer TDT (FastConformer enc + TDT joint, English)
  - onnx: istupakov/parakeet-tdt-0.6b-v2-onnx
  - ref: NeMo EncDecRNNTBPEModel / onnx-asr
  - needs: none — reuse nemo128 preprocessor-as-graph + transducer decode + SentencePiece
  - Explicitly an already-supported arm (parakeet-tdt-v2 listed); English-only and superseded by v3 multilingual — redundant, ship v3 instead.
- **Systran/faster-whisper-large-v3** [COVERED/low] — Whisper encoder-decoder (large-v3)
  - onnx: none in this repo (CTranslate2 only) — use openai/whisper-large-v3 ONNX (onnx-community/whisper-large-v3) instead
  - ref: openai/whisper-large-v3 via HF transformers; or this repo via faster-whisper/CTranslate2
  - needs: none — reuse whisper enc-dec + whisper log-mel + AR loop (already serves large-v3-turbo)
  - Whisper arm already covered. This repo only ships CTranslate2 (not loadable by our ONNX engine); large-v3 itself is already onboardable from the standard ONNX export — redundant.
- **FunAudioLLM/SenseVoiceSmall** [DONE/low] — SenseVoice frame-synchronous CTC (kaldi-fbank + LFR + CMVN)
  - onnx: FunAudioLLM/SenseVoiceSmall (ships model.onnx / quant onnx) — selected via waav.json {"architecture":"sense_voice_ctc"}
  - ref: FunASR SenseVoiceSmall reference
  - needs: none — `sense_voice_ctc`/`SenseVoiceSmall` already registered (SenseVoiceStt) with kaldi-fbank + CTC greedy
  - Literally this model is already onboarded in the registry (model.rs `sense_voice_ctc` arm). DONE — no work.
- **hexgrad/Kokoro-82M** [DONE/low] — Kokoro one-shot StyleTTS2-style non-AR TTS (single-graph)
  - onnx: onnx-community/Kokoro-82M-v1.0-ONNX (and onnx-community/Kokoro-82M-ONNX)
  - ref: already-live waav-infer KokoroTts path (tts/kokoro.rs) -- regression-check vs kokoro_m1_sample.wav
  - needs: none -- already onboarded (tts/kokoro.rs + g2p + unicode-grapheme frontend)
  - Literally onboarded and live (M1); a new voice/size is pure config+weights. No work.
- **Supertone/supertonic-3** [DONE/low] — Flow-matching (CFM) TTS -- duration/text/vector-field/vocoder four-graph
  - onnx: Supertone/supertonic ONNX (the four graphs already consumed by tts/supertonic.rs)
  - ref: already-live waav-infer SupertonicTts path (mirrors reference py/helper.py TextToSpeech._infer)
  - needs: none -- already onboarded (tts/supertonic.rs + CFM solver + unicode-grapheme frontend)
  - Literally onboarded as the engine's first flow-matching path; selected via waav.json {architecture:supertonic}. No work.
- **LiquidAI/LFM2.5-Audio-1.5B-JP-GGUF** [TODO/low] — Same as LFM2.5-Audio-1.5B (LFM2 audio-LM + Mimi codec), Japanese-tuned weights
  - onnx: none for the JP-GGUF repo itself (GGUF only) — but identical arch to LFM2.5-Audio-1.5B-ONNX; onboard via the ONNX arm with JP weights, or run GGUF path
  - ref: Liquid LFM2-Audio PyTorch/llama.cpp reference on JP audio
  - needs: none beyond LFM2.5-Audio-1.5B base stack — config + JP weights swap once the base arm exists (GGUF→same arch). Same Mimi decoder + LFM decoder + Conformer encoder
  - Pure same-arm reskin of the base model with JP weights; do the base (-ONNX) arm first, then this is config+weights. GGUF-only packaging means either reuse base ONNX arm or a separate ggml path — redundant capability.

## T2

- **nvidia/nemotron-3.5-asr-streaming-0.6b** [COVERED/high] — NeMo FastConformer + RNNT transducer (cache-aware streaming)
  - onnx: onnx-community/nemotron-3.5-asr-streaming-0.6b-onnx-int4 (ships encoder.onnx + decoder.onnx + joint.onnx; int4 — fp16/fp32 may need re-export)
  - ref: NeMo (nemo.collections.asr) on the original .nemo checkpoint, or onnxruntime-genai reference
  - needs: mostly reuse nemo128 preprocessor + transducer decode + SentencePiece; NEW: cache-aware streaming state (encoder self-attn/conv activation cache) plumbing + language-ID prompt conditioning
  - Transducer triple matches existing parakeet-rnnt path; new capability is true cache-aware streaming ASR (80-1120ms chunks) which WaaV lacks — high-value add.
- **CohereLabs/cohere-transcribe-03-2026** [COVERED/high] — FastConformer encoder + lightweight Transformer decoder (attention enc-dec / AED, >90% params in encoder)
  - onnx: onnx-community/cohere-transcribe-03-2026-ONNX (encoder_model.onnx + decoder_model_merged.onnx; fp32/fp16/q4/q4f16/quantized) — official-grade transformers.js export
  - ref: HF transformers CohereLabs/cohere-transcribe-03-2026 (native) for WER on multilingual LibriSpeech / FLEURS subset
  - needs: reuse Canary AED path (preprocessor graph + encoder + AR decoder loop) + tokenizer; NEW only if the mel/preprocessor or decoder I/O schema differs from NeMo Canary (verify mel bins + special tokens)
  - #1 open-ASR-leaderboard WER, 3x faster than whisper, 14 langs — strong capability add. Arch maps cleanly onto existing AED machinery; likely T2 (encoder/decoder schema differs from Canary) rather than pure config drop-in.
- **onnx-community/nemotron-3.5-asr-streaming-0.6b-onnx-int4** [TODO/high] — Cache-aware FastConformer-RNNT (NeMo EncDecRNNTBPEModelWithPrompt, 24L d=1024, 2-layer LSTM joint)
  - onnx: onnx-community/nemotron-3.5-asr-streaming-0.6b-onnx-int4 (this repo — ONNX int4 ready)
  - ref: NeMo EncDecRNNTBPEModelWithPrompt / nvidia/nemotron-3.5-asr-streaming-0.6b
  - needs: cache-aware streaming encoder state (ring/cache tensors) + prompt-kernel language conditioning; reuse nemo128 mel + transducer/RNNT decode (LSTM joint) seams
  - ONNX+int4 already ships and RNNT/nemo128 are reused, but cache-aware chunk streaming + prompt-kernel conditioning are genuinely new — unlocks low-latency streaming ASR, high value.
- **nvidia/diar_streaming_sortformer_4spk-v2** [TODO/high] — Streaming Sortformer: NEST/FastConformer pre-encode (L, 17L) + 18L Transformer (hid 192) + per-frame 4-sigmoid head; Arrival-Order Speaker Cache (AOSC); end-to-end online diarization, max 4 spk
  - onnx: none shipped — NeMo Sortformer ONNX export exists in sherpa-onnx/NeMo tooling (needs export)
  - ref: NeMo / sherpa-onnx
  - needs: reuse nemo128 mel + FastConformer encoder; add Transformer-encoder head graph + multi-label sigmoid diarization decode + AOSC speaker-cache state mgmt (new streaming post-stage, no new neural-codec/LLM)
  - First diarization model in the engine; reuses the FastConformer encoder so it's T2 not T3. New capability (streaming 4-spk diarization) that pairs with STT — high value, moderate effort.
- **ResembleAI/chatterbox** [TODO/high] — Llama-0.5B T3 token model (AR) + S3Gen flow-matching (CausalMaskedDiffWithXvec) + HiFTGenerator vocoder; S3 speech tokenizer
  - onnx: onnx-community/chatterbox-ONNX (full pipeline: embed_tokens.onnx, language_model.onnx, speech_encoder.onnx, conditional_decoder.onnx) + onnx-community/chatterbox-multilingual-ONNX
  - ref: resemble-ai/chatterbox (official PyTorch) — verify MOS/speaker-sim on identical text+ref-audio; A/B mel + waveform vs reference
  - needs: none new kernels — orchestration module (like supertonic.rs): T3 AR loop over language_model.onnx with embed_tokens, S3Gen flow solve + HiFTGen run internal to conditional_decoder.onnx; reuse ByteLevelTokenizer/SentencePiece, GaussianNoise, audio I/O, input-shape seam; add ref-audio voice-encoder (speech_encoder.onnx) path
  - Best candidate: ~0.5B, complete turnkey ONNX export already exists, adds AR-LLM-TTS + zero-shot voice-clone arm with no kernel work — just a new registry arm + orchestration module.
- **weya-ai/hush** [TODO/high] — DeepFilterNet3 speech enhancement + auxiliary separation head (single-speaker isolation / background-speaker suppression), 1.8M params
  - onnx: none in the HF repo as primary — needs conversion; DeepFilterNet ONNX export tooling exists upstream (ERB/DF stages exportable)
  - ref: DeepFilterNet3 / weya-ai hush PyTorch reference (PESQ/STOI/SI-SDR on noisy test set)
  - needs: STFT/ERB audio frontend + DF (deep filtering) complex-mask stage + aux separation head; new lightweight enhancement frontend (no codec, no LLM). Reuse: input-shape backend seam, notation/notch-free audio I/O
  - Tiny (8MB, <1ms/10ms frame), adds a brand-new enhancement capability class to WaaV (real-time denoise + background-speaker suppression) directly useful for the voice gateway front-end; cheap to host.
- **LocalAI-io/LocalVQE** [COVERED/medium] — DeepVQE-derivative joint AEC + noise-suppression + dereverberation, 16kHz, causal streaming, 4.8M params (GGML-native)
  - onnx: none — GGML/GGUF-native (localvqe-v1-f32.gguf) + PyTorch reference; needs ONNX conversion if ONNX path required, else run via ggml
  - ref: LocalVQE PyTorch reference / DeepVQE (Indenbom et al. Interspeech 2023); verify AEC ERLE + denoise SI-SDR
  - needs: STFT frontend + DeepVQE encoder/decoder + echo-reference (far-end) input path. Shares the enhancement frontend with hush — if hush enhancement arm lands, this is largely reuse. Reuse: input-shape seam
  - Same enhancement class as hush so COVERED once an enhancement arm exists; differentiator is AEC (needs far-end reference input). GGML-native fits a future ggml backend; redundant if hush already gives denoise — value is the AEC stage.

## T3

- **pyannote/speaker-diarization-3.1** [TODO/high] — Diarization pipeline: pyannote segmentation (SincNet/PyanNet) + WeSpeaker ResNet embedding + clustering
  - onnx: sherpa-onnx-pyannote-segmentation-3-0 (segmentation ONNX) + a WeSpeaker/3D-Speaker embedding ONNX; full pipeline via k2-fsa/sherpa-onnx (embedding export is the painful part — torchaudio fbank not ONNX-friendly)
  - ref: pyannote.audio 3.1 pipeline (PyTorch) for DER on a held-out diarization set; or sherpa-onnx diarization as a cross-check
  - needs: NEW component stack: sliding-window segmentation graph runner, speaker-embedding extractor (kaldi-fbank already exists), agglomerative/spectral clustering + offline binarization/stitching — none of which exist today (no diarization task in registry)
  - Opens an entirely new task (diarization) WaaV has zero coverage for; very high demand. Largest new surface but tractable via sherpa-onnx ONNX assets.
- **Qwen/Qwen3-ASR-1.7B** [TODO/high] — AuT Conv2D+windowed-attn audio encoder + Qwen3 LLM decoder (Prefix-LM, audio embeds replace <|audio_pad|>)
  - onnx: andrewleech/qwen3-asr-1.7b-onnx (community: encoder.onnx + decoder_init.onnx prefill + decoder_step.onnx with KV cache, fp32+int4) — NO official Qwen ONNX (safetensors only)
  - ref: HF transformers Qwen3-ASR-1.7B (native) on LibriSpeech 5-35s samples — the community ONNX was validated this way
  - needs: NEW: Qwen3 LLM decoder graph + per-step KV-cache management (prefill/step split), 128-bin Fbank→Conv2D windowed audio encoder frontend, audio-token-into-prompt embedding splice, Qwen3 tokenizer (byte-BPE already available)
  - First LLM-decoder ASR in WaaV — unlocks the AR-LLM-decoder component reused later by Voxtral/Canary-Qwen/Granite-speech. Community ONNX makes it tractable; KV-cache step graph is the real work.
- **AutoArk-AI/ARK-ASR-0.6B** [TODO/high] — LLM-ASR (Path C): Whisper-style audio encoder w/ RoPE -> MLP adapter -> Qwen2 decoder LLM; audio embeds replace placeholder tokens; ~1B total, 19 langs
  - onnx: none — needs conversion (trust_remote_code, transformers)
  - ref: transformers (trust_remote_code=True)
  - needs: Whisper-style RoPE audio encoder (reuse log-mel + encoder) + MLP projector + Qwen2 LLM decoder + KV cache + sampler (new Path-C decoder stack)
  - Smallest LLM-ASR here (~1B) and a clean Path-C reference; lands the Qwen LLM-decoder + projector seam cheaply, then canary-qwen reuses it.
- **nvidia/canary-qwen-2.5b** [TODO/high] — SALM / LLM-ASR (Path C): FastConformer encoder (from canary-1b-flash) + linear projection -> Qwen3-1.7B decoder (LoRA); dual ASR + transcript-LLM mode; 2.5B, EN
  - onnx: none — needs conversion (NeMo SALM, not ONNX-native; transformers/NeMo)
  - ref: NeMo / transformers
  - needs: reuse nemo128 mel + FastConformer encoder; add linear projector + Qwen3-1.7B LLM decoder + KV cache + sampler (shares the Path-C decoder built for ARK-ASR)
  - Reuses the existing FastConformer encoder AND the Qwen LLM-decoder stack from ARK-ASR — best ROI Path-C: top OpenASR-LB accuracy + LLM post-processing mode, <3B. Sequence after ARK to amortize the decoder seam.
- **bosonai/higgs-audio-v3-stt-v2** [TODO/high] — Whisper-v3 encoder + Qwen3 LLM decoder (audio-LLM ASR)
  - onnx: none — needs conversion (custom arch, trust_remote_code; safetensors only)
  - ref: HF transformers (trust_remote_code) higgs-audio-v3-stt-v2, Whisper-compatible API
  - needs: Qwen3 LLM decoder + KV cache (new) fused onto reusable Whisper log-mel + Whisper encoder; new fused embedding bridging audio-encoder states into the LLM
  - ~2.68B (<10B); reuses our whisper encoder+log-mel but the Qwen LLM decoder is a genuinely new component stack — first audio-LLM ASR arm, high leverage for the Higgs family (shared w/ TTS-4b).
- **pyannote/speaker-diarization** [TODO/high] — SincNet+LSTM end-to-end segmentation + x-vector(TDNN) embedding + agglomerative clustering pipeline
  - onnx: none — needs conversion (3.1 pipeline is pure PyTorch; community ONNX exports of segmentation-3.0 + wespeaker embedding exist but unofficial)
  - ref: pyannote.audio 3.1 pipeline (DER on AMI/VoxConverse)
  - needs: Whole new diarization pipeline: SincNet+LSTM segmentation graph, x-vector embedding graph, sliding-window stitching + agglomerative clustering — none of the AR/CTC/TTS components reuse; new task type for WaaV
  - Mislabeled ASR on HF but it is diarization. Adds an entirely new task/capability to WaaV. Multi-graph + clustering glue is real effort but small models (~6-26M each); high value as the only diarization arm.
- **bosonai/higgs-audio-v3-tts-4b** [TODO/high] — AR TTS: Qwen3-4B backbone + Higgs multi-codebook audio tokenizer (8 codebooks @25fps) decoder
  - onnx: none — needs conversion (chat-native AR model, SGLang-served; safetensors only)
  - ref: boson-ai/higgs-audio repo (transformers/SGLang-Omni) — WER/CER + MOS vs reference samples
  - needs: Qwen3 LLM decoder + KV cache (new, AR audio-token generation w/ delayed/staggered codebook pattern) + Higgs neural codec decoder to 24kHz (new) + fused multi-codebook embed/head
  - ~4-5B (<10B). First streaming AR-TTS arm (emits audio before full text). Shares the Qwen-LLM-decoder stack with Higgs-STT; pairs with it as a Higgs-family investment. New codec decoder is the main lift.
- **k2-fsa/OmniVoice** [TODO/high] — Diffusion-LM zero-shot voice-clone TTS on Qwen3-0.6B base (600+ langs)
  - onnx: k2-fsa/OmniVoice ships ONNX/GGML for sherpa-onnx (see HF repo discussion #2 and sherpa-onnx integration)
  - ref: k2-fsa OmniVoice (PyTorch) / sherpa-onnx OmniVoice reference
  - needs: Qwen3-0.6B diffusion-LM decoder (iterative denoise loop) + neural codec/vocoder decoder + speaker-prompt encoder for voice clone; reuse SentencePiece/BPE tokenizer
  - Small (0.6B), ONNX exists via sherpa, adds a brand-new diffusion-LM + voice-clone capability and 600+ langs -- most tractable new arch.
- **Qwen/Qwen3-TTS-12Hz-1.7B-CustomVoice** [TODO/high] — AR multi-codebook codec-LM TTS (Talker LM 28L/2048 + 5L Code Predictor groups 1-15 + 12Hz speech-tokenizer vocoder)
  - onnx: xkos/Qwen3-TTS-12Hz-1.7B-ONNX and wavekat/Qwen3-TTS-1.7B-VoiceDesign-ONNX (full FP16/FP32 export: talker_prefill/decode, code_predictor, speech-tokenizer encode/decode)
  - ref: Qwen/Qwen3-TTS-12Hz-1.7B-CustomVoice (Transformers) -- match against official ONNX outputs
  - needs: Qwen3 Talker LM decoder + KV cache + per-step code-predictor head + 12Hz speech-tokenizer (codec) decoder + speaker encoder; reuse byte-level BPE tokenizer
  - ~1.7B (<10B; the 2B label is approximate), full multi-graph ONNX already published -> highest-leverage new AR codec-LM onboard; CustomVoice adds voice-clone.
- **sesame/csm-1b** [TODO/high] — Dual Llama AR decoders (1B backbone + 100M depth decoder) over split-RVQ Mimi codec tokens at 12.5Hz
  - onnx: none — needs conversion (only PyTorch/transformers `Csm` class; Mimi codec has community ONNX but no end-to-end CSM ONNX)
  - ref: HF transformers CsmForConditionalGeneration (sesame/csm-1b)
  - needs: Llama AR LLM decoder + KV cache (two-stage backbone+depth), Mimi neural codec decoder (Kyutai RVQ), Llama tokenizer; reuse AR encdec decode loop + BPE tokenizer seams
  - First conversational AR-codec TTS; biggest new capability but needs full LLM-decoder + Mimi codec stack (~2B params, in scope).
- **pnnbao-ump/VieNeu-TTS-v3-Turbo** [TODO/high] — From-scratch AR LM backbone (0.1B, NeuTTS-Air lineage) predicting discrete codec tokens + MOSS-Audio-Tokenizer-Nano 48kHz neural codec decoder
  - onnx: yes — repo ships torch-free ONNX path (v3 Turbo backbone + ONNX codec); pnnbao-ump/VieNeu-TTS-v3-Turbo
  - ref: vieneu PyPI / VieNeu-TTS reference PyTorch + its own ONNX runtime path
  - needs: AR LM decoder + KV cache (small, ONNX-exported), MOSS-Audio-Tokenizer-Nano codec decoder; reuse BPE tokenizer + AR decode loop
  - Smallest (0.1B) and already ONNX — cheapest new AR-codec TTS arm to stand up; MOSS codec decoder is the only genuinely new piece.
- **Aratako/Irodori-TTS-600M-v3-VoiceDesign** [TODO/high] — Rectified-Flow Diffusion Transformer (RF-DiT): text+reference+caption joint-attention DiT, duration predictor, over Semantic-DACVAE-Japanese-32dim latents, 48kHz
  - onnx: none — needs conversion (repo is PyTorch; no shipped ONNX)
  - ref: Aratako/Irodori-TTS reference PyTorch pipeline
  - needs: RF flow-matching solver (reuse CFM solver, like Supertonic), joint-attention DiT blocks (new), duration predictor, Semantic-DACVAE 32-dim decoder, caption/emoji style encoder; reuse unicode-grapheme frontend
  - Closest to existing Supertonic flow-matching arm — CFM solver + unicode frontend reuse; new caption-driven VoiceDesign capability and modest 0.6B size.
- **FunAudioLLM/Fun-CosyVoice3-0.5B-2512** [TODO/high] — CosyVoice3LM autoregressive LLM (semantic speech tokens) + CausalMaskedDiff DiT flow-matching acoustic model + speaker-embed conditioning
  - onnx: ayousanz/cosy-voice3-onnx (community ONNX export); upstream repo also ships speech_tokenizer_v3.onnx + campplus.onnx components
  - ref: FunAudioLLM/CosyVoice official PyTorch inference (cosyvoice repo) on same text+prompt-audio
  - needs: LLM speech-token decoder + KV cache; speech_tokenizer_v3 codec; DiT flow-matching decoder (reuse CFM solver) + vocoder; speaker (campplus) embed; reuse unicode-grapheme TTS frontend
  - SOTA 0.5B multilingual streaming TTS (~150ms); CFM solver reusable but needs new LLM decoder + codec/DiT stack; ONNX available.
- **Qwen/Qwen3-TTS-12Hz-0.6B-Base** [TODO/high] — Discrete multi-codebook LM TTS: 28-layer Qwen3 talker (Q/K-norm, NEOX RoPE) AR decode of codebook-0 + code_predictor + Qwen3-TTS-Tokenizer-12Hz codec decoder + speaker_encoder
  - onnx: sivasub987/Qwen3-TTS-0.6B-ONNX-INT8 and zukky/Qwen3-TTS-ONNX-DLL (community ONNX: talker_prefill/talker_decode/codec_embed/12hz_decode/speaker_encoder)
  - ref: Qwen official transformers Qwen3-TTS pipeline on same text + reference voice
  - needs: Qwen3 LLM decoder + KV cache (prefill+AR codebook-0); multi-codebook code_predictor; 12Hz codec decoder; speaker_encoder; reuse byte-level BPE tokenizer + unicode TTS frontend
  - 0.6B base talker, fast voice cloning; full ONNX component graph already published; net-new Qwen LLM-decoder arm for TTS.
- **OpenMOSS-Team/MOSS-TTS-Nano-100M** [TODO/high] — Pure autoregressive Audio-Tokenizer + LLM TTS; Cat (Causal Audio Tokenizer with Transformer, CNN-free) codec; 48kHz, 20 langs, zero-shot clone
  - onnx: Jonas0066/MOSS-TTS-Nano-100M-ONNX + OpenMOSS MOSS-Audio-Tokenizer-Nano-ONNX (official standalone CPU ONNX, ~2x faster than PyTorch)
  - ref: OpenMOSS/MOSS-TTS-Nano official PyTorch/HF inference on same text + speaker prompt
  - needs: small AR LLM decoder + KV cache; Cat causal-transformer audio tokenizer + detokenizer (new codec); reuse byte-level BPE + unicode TTS frontend
  - 100M, real-time CPU-only, 20 langs, ONNX shipped end-to-end — cheapest new AR-TTS arm and strong fit for telephony fallback.
- **LiquidAI/LFM2.5-Audio-1.5B-JP** [TODO/high] — End-to-end speech+text LFM2 multimodal backbone + FastConformer audio encoder (~115M) + RQ-Transformer generating Mimi tokens + custom LFM audio detokenizer
  - onnx: LiquidAI/LFM2.5-Audio-1.5B-ONNX (official ONNX for base: encoder + decoder.onnx/fp16/q4); JP variant likely needs its own export from same graph
  - ref: LiquidAI/LFM2.5-Audio-1.5B(-JP) official transformers inference; ASR leg verifiable via WER on JP set
  - needs: LFM2 LLM decoder + KV cache; FastConformer encoder (reuse nemo128 preprocessor-as-graph); RQ-Transformer codebook head; Mimi neural codec decoder; LFM audio detokenizer
  - Smallest true S2S here (1.5B), official ONNX + GGUF; FastConformer front reuses nemo preproc; JP variant may need re-export but graph is identical.
- **LiquidAI/LFM2.5-Audio-1.5B** [TODO/high] — LFM2 audio-LM: FastConformer encoder + LFM2 transformer backbone + RQ/Depthformer token head + Mimi codec (8 codebooks, 24kHz) + LFM-based audio detokenizer
  - onnx: LiquidAI/LFM2.5-Audio-1.5B-ONNX (ships decoder.onnx, audio_encoder.onnx Conformer, audio_embedding.onnx, audio_detokenizer.onnx vocoder, vocoder_depthformer.onnx; FP32/FP16/Q4/Q8)
  - ref: Liquid LFM2-Audio PyTorch reference / liquid docs audio-models pipeline
  - needs: LFM2 transformer LLM decoder + KV cache; RQ/Depthformer codebook head; Mimi neural codec decoder (12.5/24kHz); FastConformer encoder (can lean on existing nemo conformer seam); audio detokenizer vocoder. Reuse: nemo80/128 preproc, input-shape/empty-tensor backend seams
  - Best target of the set: real ONNX export already exists for every sub-graph; adds first full S2S audio-LM + Mimi decoder + LFM decoder stack — unlocks the whole hibiki/Moshi/PersonaPlex family.
- **kyutai/hibiki-zero-3b-pytorch-bf16** [TODO/high] — Moshi-style multistream decoder-only S2S translation: Temporal Transformer (28L, d=2048) + Depth Transformer (6L/codebook, d=1024) over Mimi codec (12.5Hz), source+target+inner-monologue streams
  - onnx: none — pytorch-bf16 only; needs conversion (moshi/candle-moshi reference impls exist; no shipped ONNX)
  - ref: kyutai moshi/hibiki-zero PyTorch (or moshi-core/candle) reference; verify BLEU on Fr/Es/Pt/De→En + streaming latency
  - needs: Mimi neural codec encoder+decoder; multistream Temporal+Depth transformer with per-codebook depformer + KV cache; inner-monologue text stream. Reuse heavily IF LFM2.5-Audio lands first (shares Mimi decoder + depthformer pattern). Reuse: AR enc-dec seam, SentencePiece
  - First true Moshi-family S2S translation model; 3B fits scope. No ONNX so T4-flavored conversion work, but Mimi+depformer components are shared with LFM2.5-Audio — sequence it after the Liquid arm to amortize the Mimi codec decoder.
- **Ceva-IP/DPDFNet** [TODO/high] — DeepFilterNet2 + Dual-Path RNN (DPRNN) causal STFT/ERB-band speech enhancement with deep filtering
  - onnx: Ceva-IP/DPDFNet (ships onnx/*.onnx for baseline/dpdfnet2/4/8 and dpdfnet2_48khz_hr)
  - ref: DPDFNet PyTorch checkpoints (this repo's torch inference) on DNS4/VoiceBank-DEMAND; compare SI-SDR/PESQ/DNSMOS
  - needs: DeepFilterNet-style STFT analysis/synthesis frontend + ERB band compression + deep-filter (per-bin complex filter) apply stage; new DSP frontend/back-end pair. Decoder graph itself is the ONNX model. No new tokenizer/codec.
  - Best first win: tiny (2.3-3.6M), genuinely causal/real-time, ships ONNX, adds a brand-new enhancement capability to WaaV-Infer; main work is the STFT+ERB+deep-filter audio frontend/back-end seam.
- **detail-co/clear** [TODO/high] — DeepFilterNet3 (DFN3-half) — STFT/ERB filterbank frontend + ERB-gain decoder + 5-tap complex deep-filter decoder + GRU recurrent state
  - onnx: detail-co/clear (ships clear-studio.onnx and clear-natural.onnx, fp32 ~8.5MB each; also Core ML)
  - ref: Rikorose/DeepFilterNet reference (libDF/tract.rs) running the same .onnx — A/B PESQ/SI-SDR + sample-level parity on the same noisy input WAV
  - needs: NEW: speech-enhancement task family (none today) + DFN3 DSP wrapper: STFT + ERB filterbank analysis/synthesis, ERB-gain mask apply, 5-tap complex deep-filtering apply, normalization, and GRU/recurrent hidden-state carry across frames; reuse whisper/nemo STFT primitives where possible
  - First enhancement-task model; tractable (single fused ONNX, tiny, real export) and adds a whole new audio-to-audio capability — but needs the ERB/deep-filter DSP wrapper + recurrent state plumbing, hence T3.
- **penta2himajin/tse-conv-tasnet-48k** [TODO/high] — causal Conv-TasNet (1-D conv encoder k96/s48/256-ch, time-domain, no STFT) for target-speaker extraction, 1.45M params, conditioned on 192-dim ECAPA-TDNN enrollment embedding
  - onnx: penta2himajin/tse-conv-tasnet-48k (tse_prod_48k.onnx + .onnx.data external weights; streaming, 89 causal-conv state tensors)
  - ref: Bundled PyTorch sidecar tse_prod_48k.onnx.weights.pt via tests/tse_parity_prod_48k.rs — per-chunk PyTorch↔ORT parity (author reports max dev 5.2e-8)
  - needs: NEW: Conv-TasNet time-domain arm + target-speaker-extraction (TSE) task family + 89-tensor causal-conv streaming state carry; plus an EXTERNAL 192-dim ECAPA-TDNN speaker-embedding provider for the cond input (NOT bundled in this repo, must be sourced/onboarded separately)
  - Tiny (1.45M), clean streaming ONNX with author-provided parity test — low-effort to wire; new Conv-TasNet arm + TSE task is a genuinely new capability. Main dependency: a separate ECAPA-TDNN enrollment-embedding model for conditioning.
- **pyannote/speaker-diarization-community-1** [TODO/medium] — Diarization pipeline (newer community segmentation + embedding + clustering, pyannote lineage)
  - onnx: none official — needs conversion (no published community-1 ONNX yet; segmentation likely exportable like 3.1, embedding export blocked by torchaudio fbank)
  - ref: pyannote.audio community-1 pipeline (PyTorch) for DER
  - needs: same NEW diarization stack as 3.1 (segmentation runner + embedding + clustering); fully subsumed once the 3.1 pipeline lands — only swap the segmentation/embedding weights
  - Redundant with 3.1 effort: build the diarization stack for 3.1 first, then this is a config+weights swap. Defer; lower until 3.1 pipeline exists and a clean ONNX export is produced.
- **ibm-granite/granite-speech-4.1-2b** [TODO/medium] — Conformer-CTC encoder + window Q-Former adapter + Granite-4.0-1b LLM decoder
  - onnx: none — needs conversion
  - ref: transformers GraniteSpeechForConditionalGeneration
  - needs: Granite (granite-4.0-1b) LLM decoder + KV cache, 2-layer window Q-Former projector adapter; reuse nemo80 mel + Conformer-CTC encoder seam
  - 2B AR ASR+translation; encoder/preprocessor reusable but the Q-Former adapter + Granite LLM decoder are new components and no ONNX export ships.
- **FunAudioLLM/Fun-ASR-Nano-2512** [TODO/medium] — Transformer encoder (0.2B) + adaptor + CTC decoder + LLM-based AR decoder (0.6B)
  - onnx: partial — encoder + CTC decoder ship ONNX(int4) on the HF repo; LLM decoder only PyTorch/GGUF (HaujetZhao/Fun-ASR-GGUF runs onnx+gguf mixed)
  - ref: FunASR toolkit (modelscope/FunASR) reference inference
  - needs: LLM AR decoder (0.6B) + KV cache (the LLM head is not ONNX); reuse CTC greedy + encoder ONNX seam for the front half
  - ~0.8B total, in scope and low-latency/streaming; front half is ready ONNX but the LLM decode head needs a new AR-LLM component (or convert to ONNX).
- **mistralai/Voxtral-Mini-4B-Realtime-2602** [TODO/medium] — Streaming audio-LLM (3.4B Mistral text LM + 0.97B custom causal audio encoder, sliding-window attn)
  - onnx: onnx-community/Voxtral-Mini-4B-Realtime-2602-ONNX
  - ref: transformers/vLLM Voxtral (mistral-common) reference
  - needs: Mistral LLM decoder + KV cache, custom causal/streaming audio encoder with sliding-window attention, configurable-delay streaming scheduler
  - 4.4B realtime (<500ms, 13 langs); ONNX community export exists but it is a wholly new causal-audio-encoder + Mistral-LLM streaming stack — biggest new-capability lever but heavy.
- **bosonai/higgs-audio-v3-stt** [TODO/medium] — LLM-ASR (Path C): Higgs audio tokenizer (8 codebooks @25fps, delay pattern, multi-codebook fused embedding) -> LLM backbone (~1.7B); Whisper-compatible API, 60+ langs
  - onnx: none — needs conversion (custom architecture, trust_remote_code; transformers-only)
  - ref: transformers (AutoModel, trust_remote_code) / SGLang-Omni
  - needs: Higgs multi-codebook audio tokenizer/encoder + delay-pattern fused embedding + full LLM decoder w/ paged KV cache + sampler (new Path-C stack)
  - Net-new LLM-ASR capability and a neural-codec-style audio tokenizer; heaviest new stack but <3B and Whisper-compatible — strongest of the genuinely new STT additions.
- **zhifeixie/Mega-ASR** [TODO/medium] — Qwen3-ASR-1.7B (audio-encoder + LLM decoder) + audio-quality router + LoRA robust path
  - onnx: none — needs conversion (Qwen3-ASR base safetensors + router + merged LoRA dirs, no .onnx)
  - ref: Official Mega-ASR repo (transformers) on Voices-in-the-Wild-2M / LibriSpeech degraded subset
  - needs: Qwen LLM decoder + KV cache (new) + Qwen3-ASR audio encoder; audio-quality router (small classifier) + dynamic LoRA-adapter switch at inference
  - ~1.7B base (<10B). Same Qwen-audio-LLM decoder stack as Higgs-STT — build that first; Mega adds router+LoRA-swap complexity. Do Higgs-STT before this; robustness niche.
- **Zyphra/ZONOS2** [TODO/medium] — AR codec-LM TTS (transformer or Mamba-SSM hybrid backbone, eSpeak phonemize -> DAC token prediction)
  - onnx: none -- needs conversion (Zyphra/Zonos-v0.1-transformer ships PyTorch only; no official ONNX)
  - ref: Zyphra official Zonos PyTorch repo (espeak-ng phonemizer + DAC decoder)
  - needs: DAC neural codec decoder (44.1kHz, new) + autoregressive multi-codebook codec-LM decoder w/ KV cache + eSpeak-NG phonemizer frontend; optional Mamba/SSM op for hybrid variant
  - New AR codec-LM archetype + DAC codec decoder; no ONNX so add T4 conversion work; Mamba hybrid variant may need custom SSM kernels.
- **rednote-hilab/dots.tts-soar** [TODO/medium] — LLM + AR flow-matching DiT acoustic head over 48kHz AudioVAE (Qwen2.5-1.5B backbone, no discrete codec tokens)
  - onnx: none -- needs conversion (HF ships PyTorch + an MLX port exists; no ONNX)
  - ref: rednote-hilab/dots.tts-soar (PyTorch reference repo)
  - needs: Qwen2.5-1.5B LLM decoder + KV cache + AR flow-matching DiT head (CFM solve per VAE patch) + AudioVAE/BigVGAN-style decoder + CAM++ speaker x-vector + semantic re-encoder; reuse CFM solver + BPE tokenizer
  - ~2B, novel continuous (tokenizer-free) AR-FM-over-VAE stack; reuses existing CFM solver but needs new AudioVAE decoder + LLM seam, and no ONNX (T4 conversion).
- **openbmb/VoxCPM2** [TODO/medium] — Tokenizer-free diffusion-AR TTS over AudioVAE latent (MiniCPM-4 backbone; LocEnc -> TSLM -> RALM -> LocDiT, 48kHz)
  - onnx: partial -- OpenBMB notes ONNX export for CPU inference, but no verified full 4-stage HF ONNX repo (treat as needs-conversion)
  - ref: openbmb/VoxCPM2 (PyTorch) demo/reference pipeline
  - needs: MiniCPM-4 LM (TSLM/RALM) decoders + KV cache + LocDiT local diffusion decoder + AudioVAE V2 encoder/decoder (48kHz, new); reuse CFM/diffusion solver scaffolding
  - 2B tokenizer-free diffusion-AR; four-stage pipeline + AudioVAE V2 is substantial new component stack; ONNX coverage unconfirmed end-to-end.
- **IndexTeam/IndexTTS-2** [TODO/medium] — Autoregressive GPT (text+emotion -> GPT latents/mel codes) + MaskGCT semantic codec + S2Mel + BigVGAN vocoder; multi-stage emotion/duration-controlled
  - onnx: none — needs conversion (official repo is PyTorch; no shipped ONNX for the GPT or S2Mel/BigVGAN stages)
  - ref: index-tts official PyTorch pipeline (index-tts/index-tts)
  - needs: GPT2-style AR decoder + KV cache, MaskGCT semantic codec, S2Mel flow module, BigVGAN vocoder decoder; new multi-stage frontend (emotion+duration control)
  - Heavy 4-stage pipeline (GPT->semantic codec->S2Mel->BigVGAN); strong quality but large new component surface and no ONNX.
- **Trendyol/Trendyol-TTS** [TODO/medium] — VoxCPM2-based tokenizer-free diffusion-AR (LocEnc->TSLM->FSQ->RALM->LocDiT flow-matching over AudioVAE v2 latents); LoRA merged into base
  - onnx: none for this checkpoint — base VoxCPM has community VoxCPM-ONNX (CPU); Trendyol LoRA-merge weights need re-export
  - ref: OpenBMB VoxCPM2 PyTorch pipeline
  - needs: TSLM/RALM AR transformer + KV cache, FSQ bottleneck, LocDiT flow-matching (reuse CFM solver), AudioVAE v2 decoder; new tokenizer-free latent frontend
  - CFM solver reusable but rest is a new VoxCPM stack; Turkish-only so low marginal capability — onboard base VoxCPM2 once then this is a config swap.
- **mistralai/Voxtral-4B-TTS-2603** [TODO/medium] — Hybrid: 3.4B 26-layer Mistral LLM AR semantic tokens + 390M flow-matching acoustic transformer (8 Euler steps, 37 codes/state) + 300M Voxtral codec conv decoder, 24kHz
  - onnx: none — needs conversion (HF weights are PyTorch/safetensors; community pure-C impl exists but no ONNX)
  - ref: Mistral Voxtral-4B-TTS reference (transformers) / voxtral-tts.c
  - needs: Mistral LLM decoder + KV cache, flow-matching acoustic transformer (reuse CFM solver), Voxtral VQ-FSQ codec conv decoder; new hybrid AR+flow frontend
  - CFM solver reusable; SOTA latency/quality but 4B + 3 new sub-models and CC-BY-NC license limit production use.
- **multimodalart/higgs-audio-v3-tts-4b-transformers** [TODO/medium] — Qwen3-4B AR decoder over interleaved text+audio tokens; Higgs tokenizer 8 codebooks @25fps delayed/staggered, fused multi-codebook embed+head, 24kHz; 100+ languages
  - onnx: none — needs conversion (bosonai/higgs-audio-v3-tts-4b is PyTorch/safetensors; served via SGLang, no ONNX)
  - ref: boson-ai/higgs-audio reference PyTorch / SGLang-Omni
  - needs: Qwen3 LLM decoder + KV cache, multi-codebook (8) delay-pattern embed/head, Higgs neural codec decoder (24kHz); reuse BPE tokenizer + AR decode loop
  - Strong multilingual conversational TTS at ~4B (in scope) but new Qwen-decoder + 8-codebook Higgs codec stack; research/NC license.
- **Aratako/Semantic-DACVAE-Japanese** [TODO/medium] — Descript Audio Codec VAE (DAC) with WavLM semantic distillation — neural audio codec autoencoder
  - onnx: none — needs conversion (safetensors; fine-tune of facebook/dacvae-watermarked, DAC codebase)
  - ref: Descript DAC / facebook dacvae reference; reconstruction SI-SDR + downstream TTS quality
  - needs: new neural codec decoder (DAC-style RVQ/VAE encoder+decoder); reusable as a codec component for future codec-token TTS, but no current WaaV codec exists
  - Not a standalone voice task — it is a codec autoencoder for Japanese TTS (Irodori). Onboarding its DAC decoder unlocks codec-token TTS arms later, so the component has cross-model leverage even though the model itself is not a gateway endpoint.
- **pltobing/streaming-speech-translation** [COVERED/medium] — Cascade STS pipeline: FastConformer RNN-T ASR (NeMo) + TranslateGemma-4B NMT + Qwen3-TTS
  - onnx: ASR ships ONNX (cache-aware FastConformer RNN-T); NMT is GGUF (translategemma-4b q4_k_m); TTS not ONNX
  - ref: the pipeline itself (its bundled NeMo ASR + llama.cpp Gemma + Qwen3-TTS) for parity; BLEU for translation, WER for ASR
  - needs: ASR arm COVERED (reuse nemo + transducer decode). New: Gemma-class LLM decoder + KV cache for NMT, and Qwen3-TTS AR codec-TTS arm; gateway DAG already cascades ASR→translate→TTS
  - Not one model but a cascade — its ASR (NeMo transducer, ONNX) is already a supported family; the value is the streaming EN→RU segmenter/merger logic + Qwen3-TTS. All components <10B (4B NMT); fits WaaV's existing DAG cascade rather than a single-model onboard.
- **microsoft/VibeVoice-ASR** [TODO/low] — LLM-decoder ASR (Qwen2.5-1.5B + acoustic/semantic σ-VAE tokenizers + diffusion head)
  - onnx: none — needs conversion
  - ref: transformers VibeVoice / microsoft VibeVoice reference repo
  - needs: Qwen2.5 LLM decoder + KV cache, dual neural audio tokenizer (σ-VAE acoustic+semantic encoders), 64K long-context handling
  - 7B-class (some cards say 9B) long-form ASR with a heavy novel multi-tokenizer+LLM+diffusion stack; over/near 10B effective, no ONNX — defer.
- **Soul-AILab/SoulX-Transcriber** [TODO/low] — Speaker-attributed ASR (who/when/what) built on Qwen3-Omni-30B-A3B-Instruct MoE omni-LLM
  - onnx: none — needs conversion (30B MoE omni-LLM, transformers)
  - ref: transformers / official Soul-AILab repo
  - needs: Qwen3-Omni MoE LLM serving stack + audio front-end + speaker-attribution decode — out of scope at this size
  - >10B (35B / 30B-A3B MoE) — out of <10B scope; flag low priority, do not onboard.
- **bosonai/higgs-audio-v3-8b-stt-v2** [TODO/low] — LLM-ASR (Path C): Whisper-Large-v3 encoder (frozen) + Qwen3-8B (LoRA merged); 8.91B total
  - onnx: none — needs conversion (transformers>=4.51, AutoModel)
  - ref: transformers
  - needs: Whisper-v3 encoder (reuse) + projector + Qwen3-8B LLM decoder + paged KV cache + sampler
  - 8.91B/~9B total — over practical <10B serving budget for this engine and redundant with the smaller v3-stt; defer behind the 1.7B sibling.
- **MisoLabs/MisoTTS** [TODO/low] — Sesame CSM-style AR TTS: Llama-3.2-style 8B backbone + 300M audio decoder, Mimi RVQ codec (32 codebooks)
  - onnx: none — needs conversion (safetensors; Mimi codec separate)
  - ref: MisoLabsAI/MisoTTS repo (transformers, CSM-style) — MOS/WER vs reference
  - needs: Llama LLM decoder + KV cache (new, dual backbone+depth-decoder) + Mimi neural codec decoder (new) + RVQ 32-codebook embed/head
  - 8B (<10B but near the cap) → large effort for an 8B AR model + Mimi decoder. If building a CSM/Mimi stack, prefer a smaller Sesame-CSM-1B first; defer the 8B. New Mimi decoder is reusable across CSM/Moshi family.
- **OpenMOSS-Team/MOSS-TTS-v1.5** [TODO/low] — AR discrete-codec-token TTS -- 8B transformer (MossTTSDelay) generating 32-layer RVQ via delay pattern + 33 parallel heads, Cat causal-transformer codec
  - onnx: partial -- OpenMOSS-Team/MOSS-Audio-Tokenizer-ONNX (codec decoder only) + MOSS-TTS-GGUF; the 8B LM itself has no ONNX (llama.cpp path)
  - ref: OpenMOSS/MOSS-TTS (PyTorch) v1.5
  - needs: 8B AR transformer decoder w/ KV cache + 32-codebook delay-pattern decode (33 heads) + MOSS Cat codec decoder (ONNX available); large LM is the heavy lift
  - 8B -- right at the in-scope edge and the largest by far; only the audio tokenizer is ONNX (LM is GGUF/llama.cpp), so huge effort for redundant AR-codec capability -> defer.
- **fishaudio/s2-pro** [TODO/low] — Dual-AR (Slow-AR 4B semantic codebook + Fast-AR 400M residual) decoder-only transformer over RVQ codec (10 codebooks, ~21Hz); fish-speech lineage
  - onnx: none — needs conversion (only PyTorch + SGLang engine; community fp8/w4a16 quant builds exist, no ONNX)
  - ref: fishaudio/s2-pro via official SGLang streaming engine
  - needs: NEW: dual-AR LLM decoder + KV cache (two coupled transformers, ~4.4B), RVQ neural codec decoder; no ONNX so full T4-style conversion too. Nothing reusable beyond tokenizer.
  - >10B-class effort in practice (4.4B + SGLang-specific serving, no ONNX, RVQ codec + dual-AR KV scheduler from scratch). Redundant given chatterbox covers the AR-TTS capability cheaper.
- **rednote-hilab/dots.tts-base** [TODO/low] — Qwen2.5-1.5B LLM backbone (BPE, no phonemes) → AR flow-matching DiT acoustic head → 48kHz AudioVAE (BigVGAN-style causal decoder); frozen CAM++ x-vector speaker cond; continuous latents (no discrete codec)
  - onnx: none — needs conversion (PyTorch + an MLX community port only; no ONNX)
  - ref: rednote-hilab/dots.tts (official PyTorch / HF Space)
  - needs: NEW: Qwen2.5-1.5B LLM decoder + KV cache, AR flow-matching DiT denoising head (per-step DiT, not graph-internal like Supertonic), AudioVAE/BigVGAN decoder, CAM++ speaker x-vector encoder. None reusable as Rust components today.
  - ~2B, clean arch but a whole new component stack (LLM decoder + DiT head + AudioVAE) and no ONNX. DiT denoiser is per-step in framework code, not self-contained ONNX, so it can't reuse Supertonic's graph-internal CFM seam.
- **ZzWater/ViiTorVoice-NAR** [TODO/low] — Non-autoregressive decoder-only TTS over codec codebook (prompt-codebook voice clone, ~60ms first-frame); first-block streaming inference
  - onnx: none — needs conversion (HF weights + split gRPC serving stack only, no ONNX)
  - ref: viitor-ai/viitor-voice-nar (official, via gRPC v2 services / HTTP gateway)
  - needs: NEW: NAR decoder transformer + a neural codec decoder (codebook→waveform); audio prompt-codebook path. No ONNX export means conversion work on top.
  - Sparse public arch detail, no ONNX, custom gRPC serving stack, needs new NAR-codec component stack. Low maturity/portability — defer.
- **rednote-hilab/dots.tts-mf** [TODO/low] — Same dots.tts arch as -base (Qwen2.5-1.5B LLM + AR flow-matching DiT + 48kHz AudioVAE); '-mf' is the fast/quantized decoder variant (e.g. int4 ~2.4GB vs ~9GB)
  - onnx: none — needs conversion (PyTorch + MLX port only)
  - ref: rednote-hilab/dots.tts (official); cross-check parity vs dots.tts-base
  - needs: Identical to dots.tts-base: Qwen LLM decoder + KV cache, AR flow-matching DiT head, AudioVAE/BigVGAN decoder, CAM++ x-vector. Once -base lands, -mf is a config/quant variant.
  - Variant of dots.tts-base (faster/quantized decoder) — would become T1-cheap ONCE the base dots.tts stack exists, but until then carries the same full new-stack cost. Onboard -base first; don't do both.
- **microsoft/VibeVoice-1.5B** [TODO/low] — Qwen2.5-1.5B LLM + sigma-VAE acoustic tokenizer (~340M enc/dec, 3200x downsample @24kHz) + ASR-trained semantic tokenizer + lightweight diffusion head (4 layers ~123M); long-form multi-speaker
  - onnx: none official — partial community ONNX exists only for the smaller VibeVoice-Realtime-0.5B (FluffyBunnies/vibevoice-onnx-v2: text-LM + TTS-LM INT8); reported unintelligible-audio issues. The 1.5B has no working ONNX.
  - ref: microsoft/VibeVoice-1.5B (official PyTorch)
  - needs: NEW: Qwen2.5-1.5B LLM decoder + KV cache, sigma-VAE acoustic decoder, diffusion denoising head (per-step), semantic tokenizer. No reusable Rust components; long-context (64k) scheduler also non-trivial.
  - Listed as 3B params (>nominal) and no working ONNX for the 1.5B; community export only works partially on the separate 0.5B. New LLM+VAE+diffusion stack. Defer; if a VibeVoice arm is wanted, target the 0.5B-Realtime export instead.
- **worstchan/WavTTS** [TODO/low] — End-to-end raw-waveform flow-matching DiT (no mel/VAE/codec intermediate); waveform patchification + multi-scale mel supervision; F5-TTS codebase lineage, 16kHz zero-shot; DAC/JiT references
  - onnx: none — needs conversion (PyTorch checkpoint on F5-TTS codebase; no ONNX)
  - ref: cwx-worst-one/WavTTS (official, F5-TTS-based)
  - needs: NEW: flow-matching DiT denoiser over raw-waveform patches run as a per-step Rust loop (F5-TTS style), text/ref-audio conditioning encoder. Can't reuse Supertonic's graph-internal CFM unless re-exported as a self-contained denoiser ONNX (export work).
  - Closest in spirit to Supertonic's flow-matching arm, but the DiT denoiser is in F5-TTS framework code (per-step, external) rather than a self-contained ONNX graph; no export yet. Tractable later if re-exported denoiser-as-graph, but currently new-stack + conversion.
- **bosonai/higgs-audio-v2-generation-3B-base** [TODO/low] — Llama-3.2-3B AR decoder + DualFFN audio adapter (3.6B LLM + 2.2B DualFFN ~5.8B total) over unified Higgs audio tokenizer @25fps, 24kHz
  - onnx: none — needs conversion (PyTorch/safetensors only)
  - ref: boson-ai/higgs-audio v2 reference PyTorch
  - needs: Llama-3.2 LLM decoder + DualFFN adapter + KV cache, Higgs audio tokenizer/codec decoder; same stack family as higgs v3
  - ~5.8B effective params and superseded by higgs-audio-v3 (Qwen3) which shares the codec — redundant; skip in favor of v3.
- **Soul-AILab/SoulX-Singer** [TODO/low] — Non-autoregressive flow-matching singing voice synth: DiT flow-matching mel decoder + Singing Content Encoder (lyrics/score/note-type/F0) + neural vocoder
  - onnx: none — needs conversion (HF ships PyTorch inference only; no ONNX as of 2026-06)
  - ref: Soul-AILab/SoulX-Singer official PyTorch inference + SoulX-Singer-Eval benchmark
  - needs: new score/F0/MIDI musical-conditioning frontend; Singing Content Encoder; DiT flow-matching decoder (reuse CFM solver); neural vocoder; not covered by current grapheme TTS frontend
  - Niche singing-voice (melody/MIDI/F0 control), no ONNX, needs new musical frontend; CFM solver is the only reusable piece — high effort, low gateway value.
- **kyutai/moshika-rl-seamless** [TODO/low] — Moshi full-duplex: 7B Helium temporal Transformer + small depth Transformer (inter-codebook) + Mimi RVQ codec (12Hz/1.1kbps), dual-stream + Inner-Monologue text
  - onnx: none — needs conversion (kyutai ships PyTorch/MLX/Candle bf16/q4/q8 only; streaming dual-stream graph is impractical for static ONNX)
  - ref: kyutai-labs/moshi official Candle/PyTorch inference (moshika voice) for parity
  - needs: 7B Helium AR LLM + KV cache; depth-transformer multi-codebook head; streaming Mimi neural codec encoder+decoder; full-duplex dual-stream scheduler — entirely new stack
  - ~7-8B (near scope edge); no ONNX, requires new full-duplex streaming LLM + Mimi codec + dual-stream runtime — very high effort. Candle reuse (moshi-core) is the realistic path, not ORT.
- **nvidia/personaplex-7b-v1** [TODO/low] — Moshi-arch full-duplex S2S: dual-stream Transformer on 7B Helium backbone + Mimi encoder/decoder @24kHz; hybrid voice+text+system-prompt persona control
  - onnx: none — needs conversion (HF ships PyTorch; community MLX port exists, no ONNX)
  - ref: NVIDIA/personaplex official inference (same Moshi-core path as moshika) for parity
  - needs: shared with Moshi stack: 7B Helium LLM + KV cache; depth-transformer; Mimi codec; full-duplex scheduler + persona/voice-embedding conditioning (NATF/NATM)
  - ~7-8B Moshi-arch; once a moshika/Moshi runtime exists this is largely weights+persona config (T1-relative to that), but the underlying stack is net-new and large. No ONNX.
- **kyutai/personaplex-rl-seamless** [TODO/low] — Moshi-arch full-duplex S2S (kyutai RL-seamless): 7B Helium + depth transformer + Mimi RVQ dual-stream, Inner-Monologue, RL-post-trained for interactivity
  - onnx: none — needs conversion (kyutai PyTorch/Candle only)
  - ref: kyutai-labs/moshi official inference (personaplex-rl-seamless weights) for parity
  - needs: identical to moshika-rl-seamless: 7B Helium LLM + KV cache; depth-transformer codebook head; streaming Mimi codec; full-duplex dual-stream runtime
  - ~7-8B; same Moshi/Helium+Mimi stack as moshika — onboard the Moshi arm once, then this is a weights/config swap. No ONNX; Candle/moshi-core path only.
- **IAHispano/Applio** [TODO/low] — RVC retrieval-based voice conversion: ContentVec/HuBERT content encoder + faiss retrieval index + F0 (RMVPE/crepe) + net_g (VITS-style) generator + NSF-HiFiGAN vocoder
  - onnx: none as a single model (Applio is a toolkit, not one checkpoint) — RVC supports official net_g ONNX export; pre-converted HuBERT/ContentVec ONNX exist; full pipeline must be assembled
  - ref: Applio/RVC-WebUI PyTorch inference reference on the same speaker checkpoint
  - needs: ContentVec/HuBERT SSL encoder; F0 estimator (RMVPE); faiss feature-retrieval index; net_g VITS generator; NSF-HiFiGAN vocoder. New SSL-encoder + retrieval + NSF vocoder stack; no LLM. Reuse: little — distinct from codec-LM and enhancement arms
  - It's a training/inference WebUI framework, not a model — per-voice .pth checkpoints. Multi-component RVC pipeline (SSL+F0+retrieval+vocoder) is high effort for a niche voice-cloning capability; defer.
- **mispeech/dasheng-denoiser** [TODO/low] — Dasheng ViT/transformer audio encoder (mel patches) + 3-layer transformer latent denoiser + DashengTokenizer decoder/vocoder for resynthesis
  - onnx: none — needs conversion (mispeech ships PyTorch; no ONNX on dasheng/dashengtokenizer repos)
  - ref: Dasheng/DashengTokenizer reference PyTorch enhancement pipeline; compare DNSMOS/PESQ on a noisy-speech subset
  - needs: Dasheng mel-patch encoder graph + latent 3-layer transformer + DashengTokenizer neural decoder/vocoder (new codec-style decoder); whisper/nemo mel frontends do not match Dasheng patching. Effectively a new encoder+decoder stack.
  - Latent-space denoiser needing the full Dasheng encoder+tokenizer-decoder; no ONNX and a non-trivial 3-piece conversion — defer behind DPDFNet which covers the same enhancement capability far cheaper.
- **ResembleAI/resemble-enhance** [TODO/low] — Two-stage: UNet complex-spectrogram denoiser (mag mask + phase rotation) + latent conditional flow-matching enhancer (IRMAE autoencoder + CFM) with neural vocoder
  - onnx: none — needs conversion (resemble-ai/resemble-enhance ships PyTorch checkpoints only)
  - ref: resemble-enhance PyPI/GitHub reference pipeline at 44.1kHz; compare DNSMOS/UTMOS + bandwidth-extension spectrograms
  - needs: complex-STFT UNet denoiser frontend + IRMAE encoder/decoder + neural vocoder; CFM solver IS reusable (existing CFM solver component), but the IRMAE latent autoencoder, the UNet, and the 44.1kHz vocoder are all new. Needs ONNX conversion of 2 models.
  - CFM solver is the one reusable piece; everything else (UNet denoiser, IRMAE, 44.1kHz vocoder) is new and PyTorch-only. High effort vs DPDFNet for overlapping denoise capability; super-resolution/bandwidth-extension is the only differentiator.
- **ASLP-lab/LLaSE-G1** [TODO/low] — LLaMA-based decoder-only LM over speech tokens; WavLM-6th-layer continuous input, predicts X-Codec2 tokens (unified SE/TSE/PLC/AEC/SS)
  - onnx: none — needs conversion (ASLP-lab/LLaSE-G1 ships PyTorch; no ONNX)
  - ref: LLaSE-G1 official PyTorch repo (Kevin-naticl/LLaSE-G1) on DNS/AEC/PLC challenge sets; compare per-task DNSMOS/PESQ/SI-SDR
  - needs: WavLM frontend (new SSL encoder) + LLaMA decoder + KV cache (new LLM-decoder stack, partially shares AR enc-dec machinery) + X-Codec2 neural codec decoder (new). Three new heavy components.
  - Powerful unified-SE LM but the heaviest stack here: WavLM + LLaMA-with-KV + X-Codec2 decoder, none of which exist and none ONNX-exported. Compelling capability (TSE/PLC/AEC/SS) but only after the LLM-decoder + neural-codec infrastructure lands.
- **tencent/Covo-Audio-Chat** [TODO/low] — End-to-end speech LLM: Whisper-large-v3 encoder + Qwen2.5-7B backbone + discrete speech tokens + BigVGAN vocoder (tri-modal interleaving; full-duplex FD variant)
  - onnx: none — needs conversion (tencent/Covo-Audio ships PyTorch; ~7-8B LLM not ONNX-practical)
  - ref: Tencent Covo-Audio official inference pipeline / vllm-omni; compare ASR WER + spoken-QA + TTS MOS
  - needs: Qwen2.5-7B LLM decoder + KV cache (new) + Whisper-v3 encoder (COVERED as encoder) + speech-token detokenizer + BigVGAN vocoder (new). Full S2S LLM stack.
  - >7-8B end-to-end speech LLM — flagged low per the >10B/huge-effort rule; needs a full Qwen LLM decoder + KV cache + BigVGAN vocoder. Strategic S2S target but out of the current arch-onboarding scope; route via the gateway, not the fixed-slot infer engine, for now.
- **HirumiM/Genshin_RVC-rmvpe** [TODO/low] — RVC (Retrieval-based Voice Conversion): VITS-style generator/decoder + ContentVec/HuBERT content encoder + RMVPE pitch (F0) + Faiss feature retrieval
  - onnx: none for this specific char checkpoint — RVC ecosystem ships ONNX (hubert_base.onnx, rmvpe.onnx, and per-voice generator ONNX exportable via RVC WebUI)
  - ref: RVC-Project WebUI reference inference (same .pth + .index); compare speaker-similarity (SECS) + pitch contour vs PyTorch output
  - needs: HuBERT/ContentVec content encoder (new SSL frontend) + RMVPE pitch extractor (new) + VITS generator/flow decoder (new) + Faiss index retrieval (new non-NN piece). Net-new VC component stack.
  - Single-character hobby voice, not a general capability; full RVC stack (HuBERT + RMVPE + VITS + Faiss) is net-new. If WaaV ever wants voice-conversion, onboard generic RVC infra (ONNX available) rather than this Genshin checkpoint specifically.
- **soundsol/helix-v0.7** [TODO/low] — UNet1D with FiLM conditioning (2.4M params) — mono/stereo to First-Order-Ambisonics 4-channel spatializer with text-positioning conditioning
  - onnx: none — needs conversion (PyTorch only; no ONNX on the repo)
  - ref: helix-v0.7 reference PyTorch inference; compare FOA channel output / spatial-energy maps (no standard speech metric applies)
  - needs: FiLM-conditioned UNet1D decoder + a text/position conditioning embedding; tiny and self-contained but a brand-new spatial-audio component with no voice-pipeline overlap.
  - Spatial audio (mono->ambisonics), not a speech task — irrelevant to a voice gateway. Tiny (2.4M) so trivially convertible, but no capability WaaV needs; classify as other and skip.
- **HiDolen/Mini-BS-RoFormer-V2-46.8M** [TODO/low] — BS-RoFormer (Band-Split RoPE Transformer) music source separation
  - onnx: none — needs conversion (safetensors only; transformers/lucidrains BS-RoFormer impl)
  - ref: lucidrains/BS-RoFormer (PyTorch) reference; SDR vs MUSDB18HQ
  - needs: new stem-separation stack: STFT/band-split front-end + RoFormer (RoPE attention) blocks + mask/iSTFT synthesis head; no current WaaV component reuses
  - Music stem separation (vocals/drums/bass/other); wholly outside STT/TTS/STS scope, no ONNX, new arch + STFT-mask stack — large effort, no voice-gateway value.
- **YatharthS/NovaSR** [TODO/low] — BigVGAN-style conv1d+snake bandwidth-extension upsampler (16k→48k)
  - onnx: none — needs conversion (50kB custom conv1d weights, github ysharma3501/NovaSR)
  - ref: ysharma3501/NovaSR reference impl; LSD / PESQ on speech super-res set
  - needs: new BWE vocoder stack: conv1d layers + snake activation + resampler; no current WaaV component (no vocoder/upsampler) reuses
  - Tiny (50kB) audio upsampler, not a recognition/synthesis-from-text task; new component stack, no ONNX. Could be a future post-STT/pre-out enhancement node but not core arch.
- **drbaph/AudioSR** [TODO/low] — Latent-diffusion audio super-resolution (AudioLDM-family: mel-VAE + diffusion U-Net + vocoder)
  - onnx: none — needs conversion (fp32 safetensors; audioldm/audiosr codebase)
  - ref: audiosr (Haohe Liu) reference; LSD / log-spectral on 48kHz super-res
  - needs: very large new stack: mel-VAE encoder/decoder + iterative diffusion U-Net sampler + neural vocoder; no WaaV component reuses (CFM solver exists but wrong conditioning/arch)
  - Diffusion super-res to 48kHz; multi-second-per-sample latency, huge multi-network stack, no ONNX — unusable for realtime voice gateway; lowest priority.
- **Yorch233/RSB** [TODO/low] — NCSN++ score-based diffusion generative speech enhancement (ncsnpp_base)
  - onnx: none — needs conversion (27.8M F32 safetensors)
  - ref: SGMSE/ncsnpp reference (PyTorch); PESQ/SI-SDR on noisy speech
  - needs: new diffusion-enhancement stack: NCSN++ score U-Net + reverse-SDE/ODE sampler + STFT/iSTFT; no current WaaV component reuses
  - 27.8M diffusion denoiser; iterative sampler is slow for realtime, no ONNX, new arch. Cheaper enhancement (LavaSR) dominates if enhancement is ever wanted.
- **YatharthS/LavaSR** [TODO/low] — Conv BWE bandwidth-extension + UL-UNAS denoiser (non-diffusion speech enhancement)
  - onnx: none — needs conversion (~50MB custom weights, github ysharma3501/LavaSR)
  - ref: ysharma3501/LavaSR reference impl; PESQ/DNSMOS on enhanced speech
  - needs: new enhancement stack: BWE conv net + UL-UNAS denoiser + resampler (8–48kHz); no current WaaV component reuses
  - Fast (20–80x CPU realtime) universal speech enhancement+denoise; the most realtime-viable of the enhancement set, but still a new arch with no ONNX and outside current scope — best candidate IF an enhancement node is later prioritized.
- **tjpurdy/Piano-Separation-Model-small** [TODO/low] — BS-RoFormer (Band-Split RoPE Transformer) — piano stem separation
  - onnx: none — needs conversion (8.8M safetensors; lucidrains BS-RoFormer)
  - ref: lucidrains/BS-RoFormer reference; SDR on piano-isolation eval
  - needs: same new BS-RoFormer/STFT-mask stack as Mini-BS-RoFormer; no current WaaV component reuses
  - 8.8M music source-separation model (isolate piano); identical arch to Mini-BS-RoFormer, fully outside voice-gateway scope, no ONNX — lowest value, redundant with the other RoFormer.

## T4

- **google/medasr** [TODO/medium] — Conformer-CTC (Google `lasr_ctc` Transformers arch, not NeMo)
  - onnx: none — needs conversion (ships only model.safetensors + config.json, no .onnx/.nemo)
  - ref: HF transformers (trust_remote_code) running google/medasr on its radiology/medical eval set
  - needs: ONNX export of the Conformer encoder + CTC head, then reuse CTC greedy decoder; new log-mel frontend (Google lasr feature dim, NOT nemo80/128) — verify if nemo preprocessor matches, else a new audio frontend graph
  - 105M Conformer-CTC; arch is close to our nemo-conformer-ctc arm but a distinct lasr_ctc config/frontend, so T2-after-export — T4 today because no ONNX; niche medical domain.
- **mudler/parakeet-cpp-gguf** [COVERED/low] — NeMo FastConformer (CTC / RNNT / TDT / hybrid TDT+CTC) — GGUF-packed for parakeet.cpp (ggml)
  - onnx: none — GGUF-only (targets parakeet.cpp / ggml, not ONNX). Use NVIDIA's own ONNX exports of the underlying nvidia/parakeet-* checkpoints instead
  - ref: parakeet.cpp output as a cross-check, but verify accuracy against NeMo on the original nvidia/parakeet-*.nemo checkpoints
  - needs: none new for the arch (nemo128 preprocessor + CTC/transducer decode already exist); but GGUF weights are unusable by the ORT backend — would need GGUF->ONNX conversion or a ggml backend seam
  - Underlying arch fully COVERED (same as parakeet-tdt/rnnt/ctc already supported), but format is GGUF not ONNX — redundant repackaging. Skip; onboard the nvidia/parakeet-* ONNX checkpoints directly instead.
- **coqui/XTTS-v2** [TODO/low] — GPT-2 AR token model over Discrete-VAE (VQ-VAE) audio tokens + Perceiver speaker conditioner + HiFiGAN decoder
  - onnx: none — needs conversion (no official/community full ONNX; coqui-ai/TTS#4014 reports tracing-warning issues; only PyTorch checkpoints)
  - ref: coqui-ai/TTS XTTS-v2 (official PyTorch)
  - needs: GPT-2 AR decoder + KV cache, VQ-VAE/DVAE token decoder, Perceiver speaker encoder, HiFiGAN vocoder — plus the ONNX export work itself (not yet solved upstream)
  - Coqui Public Model License (non-commercial) + no working full ONNX + new AR+VQVAE+HiFiGAN stack. Same capability as chatterbox but no export and worse licensing — skip in favor of chatterbox.
- **aufklarer/PersonaPlex-7B-MLX-4bit** [TODO/low] — NVIDIA PersonaPlex (Moshi arch): Mimi encoder (16 codebooks, 12.5Hz) + Temporal Transformer (32L, d=4096) + Depformer (6L) + Mimi decoder; full-duplex 17-stream S2S
  - onnx: none — MLX 4-bit safetensors (Apple-Silicon only); needs conversion from nvidia/personaplex-7b-v1 base, not from MLX
  - ref: nvidia/personaplex-7b-v1 PyTorch reference (full-duplex S2S turn-taking eval)
  - needs: Mimi codec enc/dec + large Moshi Temporal+Depformer transformer + KV cache (same family as hibiki-zero). MLX format must be dequantized/converted; do not onboard from the MLX quant
  - 7B backbone (largest in set) + MLX-4bit Apple-only packaging = poor fit for GB10 CUDA/ONNX path; would re-quantize from nvidia base. Arch covered by the Moshi family — defer until hibiki-zero proves the Moshi arm, then this is a weights/scale-up.
- **aufklarer/PersonaPlex-7B-MLX-8bit** [TODO/low] — Same as PersonaPlex-7B-MLX-4bit (NVIDIA PersonaPlex / Moshi arch, Mimi codec, full-duplex) — 8-bit MLX quant
  - onnx: none — MLX 8-bit safetensors (Apple-Silicon only); convert from nvidia/personaplex-7b-v1 base instead
  - ref: nvidia/personaplex-7b-v1 PyTorch reference
  - needs: identical to the 4-bit variant — Mimi enc/dec + Moshi Temporal+Depformer transformer + KV cache; differs only by quant precision
  - Duplicate of the 4-bit entry at higher precision; same 7B Moshi backbone, MLX-only, Apple-targeted. Redundant — pick at most one PersonaPlex precision and convert from the nvidia base, only after the Moshi S2S arm exists.
- **scragnog/Ace-Step-1.5-ScragVAE** [TODO/low] — AutoencoderOobleck VAE decoder (retrained decoder half) for the ACE-Step-1.5 DiT music-generation latent space
  - onnx: none — needs conversion (PyTorch/safetensors VAE decoder; also distributed as GGUF/ComfyUI, not ONNX)
  - ref: ACE-Step-1.5 reference VAE/DiT pipeline; compare reconstruction spectrograms (only meaningful inside ACE-Step music gen)
  - needs: Oobleck VAE decoder + the entire ACE-Step DiT text-to-music model to be useful; standalone it is just a 1D audio VAE decoder. No speech components reusable.
  - Out of scope: this is a music-generation VAE decoder (drop-in replacement for ACE-Step's decoder), not STT/TTS/STS/enhancement/diarization. Useless without the ACE-Step DiT; skip for a voice gateway.

## Synthesized queue

This is an analysis/planning task — I have all the data I need in the provided JSON. No file exploration required. Let me produce the prioritized work queue.

# WaaV-Infer Onboarding Queue

Scope notes: 90 entries, but several are DONE (whisper-large-v3-turbo, SenseVoiceSmall, Kokoro, Supertonic) or COVERED-redundant (whisper.cpp, faster-whisper, parakeet GGUF). The real queue is the actionable models below.

---

## TIER 1 — config+weights drop-ins (do these first, near-zero effort)

Same arch as already-onboarded arms; literally `waav.json` + weights + (re)export.

| # | Model | Task | Reuses | Note |
|---|-------|------|--------|------|
| 1 | **nvidia/parakeet-tdt-0.6b-v3** | stt | nemo128 + transducer/TDT + SP | Unlocks 25-lang multilingual ASR. ONNX ships (istupakov). Highest T1 ROI. |
| 2 | **nvidia/nemotron-speech-streaming-en-0.6b** | stt | nemo128 + RNNT | Sub-100ms streaming EN; just needs RNNT ONNX export. |
| 3 | **openai/whisper-large-v3** | stt | WhisperStt | Top-WER whisper; pure config drop. ONNX ready. |
| 4 | **mohammed/fastconformer-quran-ar** | stt | nemo80 + CTC/RNNT | Arabic/Quran domain (0.14% WER); needs NeMo→ONNX export. |
| — | parakeet-tdt-0.6b-v2 | stt | — | Skip: English-only, superseded by v3. |
| — | LFM2.5-Audio-1.5B-JP-GGUF (labeled T1) | sts | — | Defer: same-arm reskin of the T3 LFM base; do base first. |

**(a) Quick wins to do next:** #1 parakeet-tdt-v3 (multilingual, ONNX ready) and #3 whisper-large-v3 (zero new code) are immediate. #2 nemotron-streaming-en and #4 quran-ar follow once their ONNX exports are produced.

---

## TIER 2 — light new orchestration, no new kernels (high value, contained effort)

| # | Model | Task | Effort | Note |
|---|-------|------|--------|------|
| 5 | **ResembleAI/chatterbox** | tts | New registry arm + orchestration module (like supertonic.rs) | **Best single T2 target.** ~0.5B, full turnkey ONNX (embed/LM/speech_encoder/conditional_decoder), zero kernel work. Lands AR-LLM-TTS + zero-shot voice clone. Multilingual ONNX variant too. |
| 6 | **CohereLabs/cohere-transcribe-03-2026** | stt | Reuse Canary AED path; verify mel/special-token schema | #1 open-ASR-leaderboard WER, 3× faster than whisper, 14 langs. Official-grade ONNX. |
| 7 | **nvidia/nemotron-3.5-asr-streaming-0.6b** (int4 ONNX repo) | stt | Reuse nemo128 + RNNT/LSTM-joint; NEW cache-aware streaming state + prompt-kernel lang cond | ONNX+int4 ships. Unlocks true cache-aware chunk streaming (80–1120ms) WaaV lacks. The streaming-state plumbing is the only real work. |
| 8 | **nvidia/diar_streaming_sortformer_4spk-v2** | diarization | Reuse FastConformer encoder; NEW transformer head + 4-sigmoid decode + AOSC speaker cache | First diarization arm; T2 (not T3) because it rides the existing FastConformer encoder. Needs Sortformer ONNX export. |
| 9 | **weya-ai/hush** | enhancement | NEW lightweight enhancement frontend (STFT/ERB + DF mask) | Tiny (8MB, <1ms/frame). DeepFilterNet3 + background-speaker suppression — directly useful at the gateway front-end. Needs DFN ONNX export. |
| — | LocalAI-io/LocalVQE | enhancement | — | COVERED once an enhancement arm exists; differentiator is AEC (far-end ref input). GGML-native. Do after hush. |

**(a) cont'd:** Among T2, **chatterbox (#5)** and **cohere-transcribe (#6)** are the highest-leverage near-term wins (both have ready ONNX, both add headline capability with minimal new code). nemotron-3.5-streaming (#7) is the best streaming-ASR lever.

---

## TIER 3 — NEW component stacks (the campaign's real work)

### (b) NEW component stacks ranked by # of models unlocked

Build these shared components once; each unlocks a batch. **Sequence is the queue.**

**STACK A — Qwen LLM-decoder + projector seam (audio→Qwen AR decode + KV cache).** Unlocks the most STT.
- Seed: **AutoArk-AI/ARK-ASR-0.6B** (smallest ~1B, clean Path-C reference, builds the seam cheapest).
- Then amortizes onto: **nvidia/canary-qwen-2.5b** (reuses FastConformer enc + this decoder — best ROI Path-C), **Qwen/Qwen3-ASR-1.7B** (community ONNX with prefill/step KV split available), **bosonai/higgs-audio-v3-stt-v2** (~2.68B, reuses whisper enc), **zhifeixie/Mega-ASR** (Qwen3-ASR base + router/LoRA), **FunAudioLLM/Fun-ASR-Nano** (front-half ONNX ready, only LLM head new).
- → **~6 STT models off one decoder seam.** Highest-priority T3 stack.

**STACK B — Mimi RVQ neural codec decoder + depth-transformer (Kyutai split-RVQ @12.5Hz).** Unlocks the whole Moshi/S2S family.
- Seed: **LiquidAI/LFM2.5-Audio-1.5B** (only member with full per-subgraph ONNX already exported — encoder/decoder/embedding/detokenizer/depthformer). Build the Mimi decoder + LFM decoder + depthformer here.
- Then amortizes onto: **sesame/csm-1b** (Mimi + dual-Llama), **kyutai/hibiki-zero-3b** (S2S translation, Mimi+depformer), **LFM2.5-Audio-1.5B-JP** (config+weights swap), and *enables* (still large, defer) moshika/personaplex 7B family.
- → **first S2S + ~4 near-term models + the entire 7B Moshi family later.** Do LFM2.5 first to amortize Mimi.

**STACK C — CFM / flow-matching solver (already exists from Supertonic) + per-step DiT/AudioVAE seams.** Reuses the existing CFM solver; adds DiT-head + codec-decoder variants.
- Best reuse (CFM solver + unicode frontend already there): **Aratako/Irodori-TTS-600M** (closest to Supertonic; caption-driven VoiceDesign, 0.6B), **FunAudioLLM/Fun-CosyVoice3-0.5B** (SOTA 0.5B streaming TTS ~150ms; ONNX exists; LLM+speech_tokenizer_v3+DiT).
- Then: dots.tts-base (→ -mf/-soar/Trendyol are config swaps), VoxCPM2 (→ Trendyol Turkish is a swap), IndexTTS-2, Voxtral-4B-TTS.
- Shared codec component: **Aratako/Semantic-DACVAE-Japanese** (DAC decoder) — onboard as a reusable codec; unlocks Irodori + future codec-token TTS.

**STACK D — Qwen3 Talker LM + code-predictor + 12Hz speech-tokenizer codec (Qwen3-TTS family).** Self-contained, multiple ready ONNX exports.
- Seed: **Qwen/Qwen3-TTS-12Hz-1.7B-CustomVoice** (full multi-graph ONNX published — talker_prefill/decode, code_predictor, codec, speaker_encoder). Highest-leverage AR codec-LM TTS onboard.
- Then: **Qwen/Qwen3-TTS-12Hz-0.6B-Base** (smaller, ONNX-INT8 community export) — same arm, config+weights.
- → 2 TTS arms + voice-clone off one talker stack.

**STACK E — small AR-LM + MOSS Cat codec decoder (cheapest new AR-TTS, end-to-end ONNX).** Telephony-fallback friendly.
- **pnnbao-ump/VieNeu-TTS-v3-Turbo** (0.1B, ONNX shipped; only MOSS-Audio-Nano codec is new) and **OpenMOSS-Team/MOSS-TTS-Nano-100M** (100M, real-time CPU, 20 langs, full ONNX). Build the Cat/MOSS-Nano codec decoder once → both land. Excellent low-risk T3 entry points.

**STACK F — Higgs multi-codebook codec (8 cb @25fps, delay pattern) + Qwen3 decoder.** Higgs family.
- **bosonai/higgs-audio-v3-stt** (1.7B STT) pairs with **bosonai/higgs-audio-v3-tts-4b** (shares the Qwen-LLM-decoder + Higgs codec). One family investment, two tasks. No ONNX — conversion required.

**STACK G — diarization pipeline (segmentation runner + speaker embedding + clustering).**
- **pyannote/speaker-diarization-3.1** (sherpa-onnx assets available) seeds it → **community-1** and **pyannote/speaker-diarization** become weights/config swaps. Note Sortformer (T2 #8) is a separate, encoder-reusing path; do that first for the cheap diarization win.

**STACK H — DeepFilterNet / ERB deep-filter DSP wrapper (enhancement, ONNX-ready).**
- **Ceva-IP/DPDFNet** (ships ONNX, 2.3–3.6M, causal) and **detail-co/clear** (ships ONNX) both ride one STFT+ERB+deep-filter+GRU-state wrapper. **penta2himajin/tse-conv-tasnet-48k** (target-speaker extraction, streaming ONNX + author parity test) is a sibling but needs an external ECAPA-TDNN embedding provider. These overlap with the T2 hush enhancement arm — build the DSP wrapper once.

### T3 priority ordering within the tier
1. **Stack A** (ARK-ASR → canary-qwen → Qwen3-ASR → higgs-stt) — most models, mostly tractable, several with community ONNX.
2. **Stack B** (LFM2.5-Audio-1.5B first) — unlocks all S2S; only member with full ONNX, so amortizes Mimi cheaply.
3. **Stack D** (Qwen3-TTS-1.7B → 0.6B) — full ONNX ready, high-leverage AR-TTS.
4. **Stack E** (VieNeu 0.1B + MOSS-Nano 100M) — cheapest new TTS arms, ONNX shipped.
5. **Stack C** (Irodori + CosyVoice3) — reuses existing CFM solver.
6. **Stack H** (DPDFNet + clear) — ONNX ready, new enhancement class.
7. **Stack G** (pyannote-3.1) — after Sortformer.
8. **Stack F** (Higgs family) — no ONNX, conversion-heavy.

---

## TIER 4 — blocked: need an ONNX/HF export first (do export work before queueing)

These cannot enter the runtime until a conversion lands. Listed by whether the export is plausibly worth it.

| Model | Task | Blocker | Verdict |
|-------|------|---------|---------|
| **google/medasr** | stt | No ONNX; lasr_ctc Conformer-CTC + distinct frontend (not nemo80/128) | Export encoder+CTC head → then T2. Niche medical; medium. |
| coqui/XTTS-v2 | tts | No working full ONNX (upstream tracing issues) + non-commercial license | **Skip** — chatterbox covers the capability with a clean export and better license. |
| fishaudio/s2-pro | tts | No ONNX, SGLang-specific, 4.4B dual-AR | Skip — chatterbox covers AR-TTS cheaper. |
| aufklarer/PersonaPlex-7B-MLX-4bit / -8bit | sts | MLX/Apple-only; must convert from nvidia base, not MLX | Skip the MLX quants; if PersonaPlex is wanted, convert from nvidia/personaplex-7b-v1 after the Moshi arm (Stack B) exists. |
| scragnog/Ace-Step-1.5-ScragVAE | other (music) | — | Out of scope (music gen). Skip. |
| HiDolen/Mini-BS-RoFormer, tjpurdy/Piano-Separation | enhancement (music) | — | Out of scope (music stem sep). Skip. |
| soundsol/helix, NovaSR, AudioSR, RSB, LavaSR, dasheng-denoiser, resemble-enhance, LLaSE-G1 | enhancement/spatial | No ONNX, heavy/diffusion or out-of-scope | Defer all behind DPDFNet/clear (Stack H), which cover real-time denoise far cheaper. LavaSR is the best of these *if* an enhancement node is later prioritized. |
| Applio / Genshin_RVC | sts (voice conversion) | Toolkit/per-voice, no single ONNX | Defer; if VC is ever wanted, onboard generic RVC infra, not these checkpoints. |
| Semantic-DACVAE-Japanese | other (codec) | No ONNX | Not a gateway endpoint, but its DAC decoder is a reusable codec component — convert under Stack C (unlocks Irodori + codec-token TTS). |

---

## Out of scope / skip outright (don't queue)
- **DONE:** whisper-large-v3-turbo, SenseVoiceSmall, Kokoro-82M, Supertonic-3.
- **Redundant runtimes/repackaging:** ggerganov/whisper.cpp, Systran/faster-whisper-large-v3, mudler/parakeet-cpp-gguf, parakeet-tdt-v2.
- **Over budget (>10B effective) / no ONNX:** Soul-AILab/SoulX-Transcriber (35B), microsoft/VibeVoice-ASR (~9B), bosonai/higgs-v3-8b-stt-v2 (8.9B), MOSS-TTS-v1.5 (8B), higgs-v2-3B-base (~5.8B, superseded by v3), tencent/Covo-Audio-Chat (~7–8B), moshika/personaplex 7B family (defer to after Stack B), VibeVoice-1.5B (no working ONNX → target 0.5B-Realtime instead).
- **Out of voice scope:** Ace-Step VAE, helix (ambisonics), BS-RoFormer pair, SoulX-Singer (singing).

---

## TL;DR work order
1. **Now (T1):** parakeet-tdt-v3, whisper-large-v3 → then nemotron-streaming-en, quran-ar (after exports).
2. **Next (T2):** chatterbox, cohere-transcribe → nemotron-3.5-streaming → Sortformer diarization → hush enhancement.
3. **T3 by shared stack (build component once, batch the models):** Qwen-decoder seam (ARK→canary-qwen→Qwen3-ASR→higgs-stt) → Mimi/LFM (LFM2.5→csm/hibiki) → Qwen3-TTS (1.7B→0.6B) → cheap AR-TTS (VieNeu+MOSS-Nano) → CFM DiT (Irodori+CosyVoice3) → DFN enhancement (DPDFNet+clear) → pyannote diarization → Higgs family.
4. **T4:** export medasr; otherwise skip the music/diffusion/MLX/no-ONNX-redundant set.
