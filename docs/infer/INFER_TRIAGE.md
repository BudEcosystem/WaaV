# WaaV Infer — Remaining-Models Triage (68 models, 2026-06-14)

Bucketed by winnability × proven WaaV onboarding pattern. Drives the /loop worklist.

**Totals:** {'clean': 11, 'moderate': 35, 'hard': 10, 'blocked': 12}
**Patterns:** {'P1-onnx-direct': 11, 'P6-enhance-onnx': 14, 'P2-nemo-export': 1, 'P3-transformers-shim': 6, 'P4-codec-ar-open': 20, 'P5-per-venv': 4, 'BLOCKED': 11, 'P4b-codec-ar-closed': 1}


## CLEAN (11)

| model | cat | pattern | params | onnx mirror | dup | effort | notes |
|---|---|---|---|---|---|---|---|
| pyannote/speaker-diarization-community-1 | STT | P1-onnx-direct | 0.008B | community:altunenes/speaker-diarization- |  | weights+config swap onto existing pyannote-3.1 ONN | Successor to already-supported pyannote-diarization-3.1; same 3-stage arch (powerset segmentation + WeSpeaker x-vector embedding + Bayesian-HMM/VBx clustering, +PLDA) but NEW commu |
| ggerganov/whisper.cpp | STT | P1-onnx-direct | 1.55B | community:onnx-community/whisper-large-v | whisper-large-v3/turbo | none — duplicate of already-supported whisper (use | DUPLICATE: this is ggerganov's collection of OpenAI Whisper checkpoints (tiny→large-v3, multilingual + .en, plus q5/q8 quants) converted to ggml/GGUF .bin format for the whisper.cp |
| FunAudioLLM/Fun-ASR-Nano-2512 | STT | P1-onnx-direct | 0.8B | community:csukuangfj/sherpa-onnx-funasr- |  | ~half day: wire 3 ONNX graphs + AR decode loop (sh | LLM-decoder ASR (output is TEXT, no audio codec/vocoder, so not codec-AR-closed risk). Original FunAudioLLM repo is Apache-2.0, NOT gated, but ships only model.pt (PyTorch pickle)  |
| mistralai/Voxtral-Mini-4B-Realtime-2602 | STT | P1-onnx-direct | 4.37B | community:onnx-community/Voxtral-Mini-4B |  | weights-only (+ streaming/chunked-encoder serving  | Apache-2.0 STT (ASR), ~4.37B params (≈970M causal audio encoder + ≈3.4B LM decoder), arch VoxtralRealtimeForConditionalGeneration, BF16 safetensors. NO audio codec/vocoder (text-ou |
| google/medasr | STT | P1-onnx-direct | 0.105B | community:csukuangfj/sherpa-onnx-medasr- |  | weights-only (ONNX mirror exists, ~zero code) | Google Health-AI medical-dictation ASR, Conformer-CTC, 105M params, English-only, 4.6% WER on radiology. The OFFICIAL google/medasr repo IS GATED (Health AI Developer Foundations " |
| Systran/faster-whisper-large-v3 | STT | P1-onnx-direct | 1.55B | community:onnx-community/whisper-large-v | whisper-large-v3 | duplicate — already supported (no onboarding work) | Pure CTranslate2 (ct2-transformers-converter, FP16) re-serialization of OpenAI Whisper large-v3 — identical weights/arch to the already-supported whisper-large-v3. Files: config.js |
| OpenMOSS-Team/MOSS-TTS-Nano-100M | TTS | P1-onnx-direct | 0.1B | community:OpenMOSS-Team/MOSS-TTS-Nano-10 |  | weights-only + light AR-loop glue (~half day): wir | Tiny multilingual codec-AR TTS, ~0.1B params (config: GPT-2-style "global_local_transformer" backbone, hidden 768, 12 layers, vocab 16384, RoPE, 32k ctx; arch class MossTTSNanoForC |
| weya-ai/hush | STS | P6-enhance-onnx | 0.0018B | self |  | enhance-onnx ~half day (DeepFilterNet STFT/ERB fra | Audio-to-audio speech enhancement (denoise + background-speaker suppression), NOT speech-to-speech despite the "STS" listing. Arch=DfNetSE = DeepFilterNet3 (FFT=320/hop=160, 32 ERB |
| YatharthS/NovaSR | STS | P6-enhance-onnx | 0B | none |  | torch.onnx.export + enhance.rs SR output path, ~ha | Tiny ~52KB audio super-resolution model: 16kHz->48kHz upsampling (audio-to-audio enhancement, same family as supported GTCRN/DPDFNet/dasheng). Apache-2.0, NOT gated, open weights ( |
| detail-co/clear | STS | P6-enhance-onnx | 0.002B | self |  | weights-only + DFN3 host-side ERB/deep-filter plum | audio-to-audio 48kHz speech enhancement (denoise + dereverb). Repo detail-co/clear resolves to desert-ant-labs/clear. SELF-ships native ONNX fp32: clear-studio.onnx + clear-natural |
| penta2himajin/tse-conv-tasnet-48k | STS | P6-enhance-onnx | 0.00145B | self |  | weights-only + streaming wiring (~1-2h: state-loop | Ungated (API gated:false), CC-BY-4.0, fully open. Repo SHIPS ONNX itself: tse_prod_48k.onnx (624KB graph) + tse_prod_48k.onnx.data (5.3MB external weights); the .weights.pt is only |

## MODERATE (35)

| model | cat | pattern | params | onnx mirror | dup | effort | notes |
|---|---|---|---|---|---|---|---|
| Qwen/Qwen3-TTS-12Hz-1.7B-CustomVoice | TTS | P1-onnx-direct | 1.7B | community:elbruno/Qwen3-TTS-12Hz-1.7B-Cu |  | new arch ~half-to-full day: wire 4-stage ONNX pipe | Codec-AR TTS, codec OPEN: a CustomVoice-specific full-pipeline ONNX mirror exists and is ungated Apache-2.0 (elbruno/Qwen3-TTS-12Hz-1.7B-CustomVoice-ONNX): talker_prefill.onnx (5.6 |
| openbmb/VoxCPM2 | TTS | P1-onnx-direct | 2B | community:ai4all8/VoxCPM2-ONNX |  | new arch ~1-1.5 days | 2B tokenizer-free diffusion-AR TTS on MiniCPM-4 backbone; continuous-latent flow-matching (DiT/CFM), NOT discrete codec-AR. Apache-2.0, NOT gated, commercial-use OK. Official repo  |
| LiquidAI/LFM2.5-Audio-1.5B-JP | STS | P1-onnx-direct | 1.5B | community:LiquidAI/LFM2.5-Audio-1.5B-ONN |  | moderate ~1-1.5 days: run official onnx-export on  | STS audio LLM, UNGATED, fully open weights (LFM Open License 1.0). config.json architectures=["Lfm2AudioForConditionalGeneration"], model_type=lfm2, NO auto_map/trust_remote_code.  |
| LiquidAI/LFM2.5-Audio-1.5B | STS | P1-onnx-direct | 1.5B | community:LiquidAI/LFM2.5-Audio-1.5B-ONN |  | new S2S orchestration ~1-2 days: ORT loads 5 prebu | End-to-end speech-to-speech codec-AR model. Base repo LiquidAI/LFM2.5-Audio-1.5B is UNGATED, LFM Open License v1.0 (components Apache-2.0/CC-BY-4.0/MIT); arch Lfm2AudioForCondition |
| nvidia/diar_streaming_sortformer_4spk-v2 | STT | P2-nemo-export | 0.117B | none |  | new streaming-diar arch + export workaround ~1-2 d | End-to-end streaming speaker diarization (up to 4 spk, 16kHz mono mono-audio, outputs T x 4 speaker-activity probabilities). Repo ships ONLY diar_streaming_sortformer_4spk-v2.nemo  |
| microsoft/VibeVoice-ASR | STT | P3-transformers-shim | 9B | none |  | new arch ~1-2 days via torch sidecar; 9B mem footp | Open, ungated MIT weights (8 bf16 safetensors, ~17.3GB). config.json architectures=VibeVoiceForASRTraining, model_type=vibevoice; backbone is Qwen2-7B decoder (hidden 3584, 28L, vo |
| bosonai/higgs-audio-v3-stt | STT | P3-transformers-shim | 2.68B | none |  | new arch ~half day (per-venv sidecar + boson_multi | Whisper-Large-v3 mel encoder (32L/1280d/128 mel) -> MLP projector -> Qwen3-1.7B decoder; 2.68B params; continuous-embedding encoder->projector->LLM ASR (granite-speech / Qwen3-ASR  |
| bosonai/higgs-audio-v3-8b-stt-v2 | STT | P3-transformers-shim | 8.91B | none |  | new arch ~1 day (P3 sidecar: custom HiggsAudio3Mod | STT (ASR) direction: outputs TEXT, not audio codes -> NOT codec-AR/P4 despite config carrying audio_codebook_size=1024/audio_num_codebooks=8 (inherited from shared HiggsAudio3 conf |
| bosonai/higgs-audio-v3-stt-v2 | STT | P3-transformers-shim | 2.07B | none |  | new arch ~half-to-full day (transformers trust_rem | STT/AST (audio in -> text out), 94 languages, Whisper-compatible API. Arch=HiggsAudio3Model / model_type higgs_audio_3: Qwen3-1.7B-Base text backbone + continuous Whisper-style aud |
| zhifeixie/Mega-ASR | STT | P3-transformers-shim | 1.7B | none | qwen3-asr-1.7b | base already supported (qwen3-asr-1.7b); delta = m | Mega-ASR = a full unmodified Qwen3-ASR-1.7B base checkpoint (already supported in WaaV) + a 92.5MB PEFT LoRA adapter (mega-asr-merged/: adapter_config.json + adapter_model.safetens |
| k2-fsa/OmniVoice | TTS | P4-codec-ar-open | 0.6B | none |  | new arch ~1-2 days | Codec-AR TTS, OPEN UNGATED weights (Apache-2.0, no access banner). AR front-end = custom 'OmniVoice' arch (architectures:["OmniVoice"], model_type:"omnivoice") wrapping Qwen3-0.6B  |
| rednote-hilab/dots.tts-soar | TTS | P4-codec-ar-open | 2B | none |  | new arch ~2-3 days (continuous-latent AR LLM + DiT | Apache-2.0, NOT gated, ~2B params (model.safetensors 4.4GB) — under 10B. arch=DotsTTSForConditionalGeneration / model_type=dots_tts; NOT a stock transformers class (no trust_remote |
| MOSS-TTS-v1.5 | TTS | P4-codec-ar-open | 8B | community:OpenMOSS-Team/MOSS-Audio-Token |  | new arch ~1-2 days (codec ONNX is free; 8B delay-p | Codec-AR TTS, Apache-2.0, UNGATED, ~8B params (under 10B ceiling). Decisive: the neural codec (MOSS-Audio-Tokenizer, 32-layer RVQ, 24kHz/12.5fps) is OPEN and ships OFFICIAL ONNX (e |
| fishaudio/s2-pro | TTS | P4-codec-ar-open | 5B | none |  | new arch ~1-2 days (Dual-AR forward + RVQ/DAC code | Fish Speech S2-Pro. config model_type=fish_qwen3_omni: Dual-AR codec-TTS — slow AR is a Qwen3 4B LLM decoder (36 layers, hidden 2560) emitting primary semantic codebook; fast AR 40 |
| rednote-hilab/dots.tts-base | TTS | P4-codec-ar-open | 2B | none |  | new arch ~1-2 days (reimplement AR flow-matching f | Codec-AR TTS, higgs-audio pattern but distinct arch: semantic encoder + Qwen2.5-1.5B-Base LLM backbone emits hidden states -> AR flow-matching DiT head over a CONTINUOUS-latent 48k |
| ZzWater/ViiTorVoice-NAR | TTS | P4-codec-ar-open | 0.6B | self |  | new arch ~1-2 days: codec drops in via in-repo ONN | Codec is OPEN, not vendor-locked: repo BUNDLES dualcodec_decoder.onnx (codes->wav) + dualcodec_encode_core_30s.onnx (wav->codes) plus dualcodec_25hz_16384_1024.safetensors; DualCod |
| microsoft/VibeVoice-1.5B | TTS | P4-codec-ar-open | 3B | none |  | new arch ~1-2 days: reimplement Qwen2.5 decoder +  | Codec-AR TTS: VibeVoiceForConditionalGeneration / model_type "vibevoice". Qwen2.5-1.5B LLM decoder emits latents -> diffusion head (DDPM 1000 train / 20 infer steps, v-pred, cosine |
| sesame/csm-1b | TTS | P4-codec-ar-open | 2B | none |  | new arch ~1-2 days: Llama-AR backbone + audio deco | Conversational Speech Model: Llama backbone (semantic AR) + smaller audio decoder emitting Mimi RVQ codes @12.5Hz; HF lists ~2B params total (under 10B cap). License Apache-2.0 on  |
| Trendyol/Trendyol-TTS | TTS | P4-codec-ar-open | 2B | community:ai4all8/VoxCPM2-ONNX (base Vox |  | new arch ~2-3 days (export fine-tune + Rust AR/CFM | Turkish LoRA fine-tune merged into openbmb/VoxCPM2 2B base; NOT gated; MIT. Arch is tokenizer-free DIFFUSION-autoregressive (not discrete-codec-AR): MiniCPM-4 backbone + LocDiT dif |
| pnnbao-ump/VieNeu-TTS-v3-Turbo | TTS | P4-codec-ar-open | 0.1B | self |  | new arch ~1 day: wire AR ONNX prefill/decode-step  | STATUS REVERSAL vs prior "VieNeu blocked (closed codec)": v3-Turbo dropped the closed in-package codec for OpenMOSS-Team/MOSS-Audio-Tokenizer-Nano, which is OPEN (Apache-2.0, ungat |
| mistralai/Voxtral-4B-TTS-2603 | TTS | P4-codec-ar-open | 4.1B | none |  | new arch ~few days: Mistral-format AR backbone + f | DECISIVE: codec is OPEN. The neural-codec DECODER (tokens->wav) weights are bundled in the single open consolidated.safetensors (8GB); only the codec ENCODER (audio->tokens, for ar |
| bosonai/higgs-audio-v2-generation-3B-base | TTS | P4-codec-ar-open | 5.8B | none |  | new arch ~1 day: reimplement DualFFN codec-AR forw | Codec-AR TTS, the canonical higgs-audio P4 pattern. Backbone Llama-3.2-3B + DualFFN audio adapter (~5.8B total, listed 6B; <10B ok), emits 8 discrete audio codebooks @25fps (config |
| FunAudioLLM/Fun-CosyVoice3-0.5B-2512 | TTS | P4-codec-ar-open | 0.5B | community:ayousanz/cosy-voice3-onnx (ful |  | new arch ~1-2 days: orchestrate multi-graph AR tok | Codec-AR TTS, ungated, Apache-2.0, ~0.5B. CODEC/VOCODER IS OPEN: HIFT vocoder + flow-matching DiT decoder published as ONNX, and a complete community mirror (ayousanz/cosy-voice3-o |
| Qwen/Qwen3-TTS-12Hz-0.6B-Base | TTS | P4-codec-ar-open | 0.9B | community:sivasub987/Qwen3-TTS-0.6B-ONNX |  | new arch ~half-to-1 day (wire pre-exported communi | Apache-2.0, fully OPEN, NOT gated (free huggingface-cli download). Codec-AR TTS: backbone "talker" LM predicts codebook-0, MTP module predicts residual RVQ codebooks, decoded by Qw |
| LiquidAI/LFM2.5-Audio-1.5B-JP-GGUF | STS | P4-codec-ar-open | 1.5B | community:LiquidAI/LFM2.5-Audio-1.5B-ONN |  | new arch, moderate (~1-2 days) | End-to-end speech-to-speech (audio/text -> audio+text interleaved). NOT gated, open license. THIS repo is GGUF-only (F16/F32/Q4_0/Q8_0 + separate vocoder/tokenizer/mmproj GGUFs); n |
| ASLP-lab/LLaSE-G1 | STS | P4-codec-ar-open | 1.5B | none |  | new arch ~1-2 days: reimplement WavLM-6th-layer fe | Codec-AR speech enhancement (general SE: denoise/dereverb/AEC/TSE/PLC via dual-channel I/O, no task IDs). Pipeline: WavLM-Large extracts 6th-layer continuous feats from degraded au |
| rednote-hilab/dots.tts-mf | TTS | P5-per-venv | 2B | none |  | new arch via per-venv sidecar ~1-1.5 days (custom  | Open + ungated + Apache-2.0 weights (model.safetensors 4.4GB, vocoder.safetensors 724MB OPEN, speaker_encoder/CAMPlus 29MB), total 5.17GB, 2B params -> NOT blocked. Arch DotsTTSFor |
| Soul-AILab/SoulX-Singer | TTS | P5-per-venv | 0.3B | none |  | per-venv + sidecar ~1 day (de-pin SageAttention; b | Zero-shot singing voice synthesis (SVS); NAR flow-matching DiT predicts 24kHz 128-bin mel, neural vocoder -> wav. Apache-2.0, NOT gated. Repo ships pickled .pt only (model.pt SVS 2 |
| LocalAI-io/LocalVQE | STS | P6-enhance-onnx | 0.0048B | none |  | new arch ~1-1.5 days: ONNX-export from open .pt +  | Audio-to-audio voice quality enhancement (joint AEC + noise suppression + dereverberation, 16kHz mono), a streaming CPU-tuned derivative of DeepVQE (Interspeech 2023: residual CNN  |
| HirumiM/Genshin_RVC-rmvpe | STS | P6-enhance-onnx | 0.02B | none |  | new STS/VC pipeline ~1-2 days (export net_g→ONNX + | Fan-made Genshin character RVC-v2 voice-conversion model COLLECTION (Aether/Ayaka/Nahida/Zhongli/Paimon/etc), ~400 epochs, 40kHz. License MIT, page NOT gated. Repo ships ONLY .pth  |
| ResembleAI/resemble-enhance | STS | P6-enhance-onnx | NoneB | community:skeskinen/resemble-denoise-onn |  | denoiser ~weights-only (clean P6); enhancer CFM+vo | Audio-to-audio speech enhancement (44.1kHz), two modules. (1) DENOISER: complex-spectrogram UNet predicting magnitude mask + phase rotation — cleanly winnable, a community ONNX exi |
| soundsol/helix-v0.7 | STS | P6-enhance-onnx | 0.0024B | none |  | ~half day: reconstruct UNet1D forward + export ONN | NOT STS/speech — it is a tiny (2.4M param, 9.6MB) audio-to-audio spatial upmixer: mono 24kHz in -> 4ch FOA Ambisonics (W,X,Y,Z) out, text-conditioned by direction/elevation/distanc |
| HiDolen/Mini-BS-RoFormer-V2-46.8M | STS | P6-enhance-onnx | 0.0468B | none |  | new arch ~half day (trust_remote_code export to ON | Audio-to-audio MUSIC SOURCE SEPARATION (4 stems: bass/drums/other/vocal), NOT speech STS — belongs on the enhance.rs audio-to-audio spectral path (GTCRN/DPDFNet/dasheng family), ca |
| YatharthS/LavaSR | STS | P6-enhance-onnx | 0.013B | none |  | new export+arch ~half day | MISCATEGORIZED as STS — actually audio-to-audio ENHANCEMENT (pipeline_tag=audio-to-audio): bandwidth extension / super-resolution (8-48kHz in -> 48kHz out) + denoising, NOT speech- |
| tjpurdy/Piano-Separation-Model-small | STS | P6-enhance-onnx | 0.0088B | none |  | new arch ~half-to-1 day: ONNX-export BS-RoFormer g | Audio-to-audio source separation: isolates piano stem from a music mix (44.1kHz, stereo, 8.8M params / 17MB safetensors). Arch = BS-RoFormer (lucidrains/BS-RoFormer; arxiv:2309.026 |

## HARD (10)

| model | cat | pattern | params | onnx mirror | dup | effort | notes |
|---|---|---|---|---|---|---|---|
| nvidia/canary-qwen-2.5b | STT | P3-transformers-shim | 2.5B | community:onnx-community/canary-qwen-2.5 |  | hard: reimplement SALM forward from safetensors (F | SALM speech-LLM, 2.5B, CC-BY-4.0, NOT gated. Source repo = NeMo speechlm2 training config.json (references nemo.collections.speechlm2.modules.perception.AudioPerceptionModule + Con |
| MisoLabs/MisoTTS | TTS | P4-codec-ar-open | 8B | community:onnx-community/kyutai-mimi-ONN |  | new arch: full CSM-style AR forward reimplement fr | Sesame/CSM-clone codec-AR TTS: 7.7B Llama-3.2-style backbone (AR over time, predicts codebook-0 + carry hidden state) + reused 300M depth decoder (AR over 31 remaining codebooks) e |
| Zyphra/ZONOS2 | TTS | P4-codec-ar-open | 8B | none |  | hard: reimplement 8B 16-expert "Sonic" MoE AR forw | Codec is OPEN: emits Descript Audio Codec (DAC) tokens; DAC weights are MIT/open and portably runnable (matches P4-codec-ar-open like higgs-audio). Ungated, Apache-2.0. NOT a dupli |
| IndexTeam/IndexTTS-2 | TTS | P4-codec-ar-open | 3.5B | none |  | new arch, multi-stage reimplement ~1-2 weeks | Cascaded codec-AR zero-shot TTS (arXiv 2506.21619): T2S GPT (gpt.pth 3.48GB) -> non-AR flow-matching S2Mel (s2mel.pth 1.2GB) -> BigVGAN-v2 vocoder, conditioned on wav2vec2-BERT fea |
| kyutai/hibiki-zero-3b-pytorch-bf16 | STS | P4-codec-ar-open | 3B | none |  | new arch, multi-day: Moshi multistream RQ-Transfor | Simultaneous speech-to-speech (and speech-to-text) translation FR/ES/PT/DE->EN; Moshi-family codec-AR. Weights OPEN + UNGATED (no access banner). Two safetensors: 6.26GB backbone + |
| Aratako/Irodori-TTS-600M-v3-VoiceDesign | TTS | P5-per-venv | 0.6B | none |  | new arch ~1.5-2 days: per-venv sidecar OR reimplem | Flow-matching / Rectified-Flow Diffusion Transformer (RF-DiT, Echo-TTS-derived) that denoises CONTINUOUS 32-dim DACVAE latents — NOT a codec-AR model (no LLM emitting discrete audi |
| Aratako/Semantic-DACVAE-Japanese | STS | P5-per-venv | 0.1B | none |  | per-venv torch sidecar + custom dacvae pip pkg; bu | NOT a real STS/TTS/STT engine — it's an audio-to-audio CODEC/VAE (encode wav -> continuous 128-dim latent -> decode wav), the building block a downstream codec-AR TTS would use, wi |
| IAHispano/Applio | STS | P6-enhance-onnx | NoneB | self |  | new arch ~multi-day: 4-stage RVC pipeline reimplem | Voice conversion (audio-to-audio / STS), open MIT, UNGATED. NOT a packaged model — repo is the Applio tool/app distribution (~101GB: Gradio app, env.zip, ffmpeg binaries, installer |
| drbaph/AudioSR | STS | P6-enhance-onnx | 1.5B | none |  | hard: reimplement multi-stage latent-diffusion SR  | Audio-to-audio super-resolution (2-16kHz -> 48kHz), MIT, NOT gated, open weights. Repackage of Haohe Liu AudioSR (arXiv 2309.07314) for ComfyUI. Repo holds only two bare ~5.9GB fp3 |
| Yorch233/RSB | STS | P6-enhance-onnx | 0.028B | none |  | new arch + solver loop, ~2-3 days (rewrite upfirdn | Audio-to-audio GENERATIVE speech enhancement (denoise), 16kHz, trained on Voicebank+Demand. ~27.8M params F32, 111MB single model.safetensors, NO config.json/architectures — only a |

## BLOCKED (12)

| model | cat | pattern | params | onnx mirror | dup | effort | notes |
|---|---|---|---|---|---|---|---|
| mudler/parakeet-cpp-gguf | STT | BLOCKED | 0.6B | none | parakeet-tdt-v2 | none — duplicate of already-supported parakeet-tdt | Ungated collection repo containing ONLY .gguf blobs (CTC/RNNT/TDT 0.6b+1.1b, Nemotron-3.5-ASR-streaming, EOU-120m) in f16/q8_0/K-quant, purpose-built for mudler's parakeet.cpp — a  |
| pyannote/speaker-diarization | STT | BLOCKED | 0.01B | community:k2-fsa/sherpa-onnx (sherpa-onn | pyannote-diarization-3.1 | none — gated older duplicate of already-supported  | This is the OLDER pyannote 2.1 diarization pipeline. The HF page is GATED ("You need to agree to share your contact information to access this model"), and the underlying pyannote/ |
| worstchan/WavTTS | TTS | BLOCKED | 2.7B | none |  | blocked: CC-BY-NC license; else P5-per-venv ~1-2 d | Zero-shot TTS = flow-matching (Euler ODE) Diffusion Transformer (DiT) that models RAW WAVEFORM directly via patchification — NO neural codec, NO vocoder, NO autoencoder (this is it |
| kyutai/moshika-rl-seamless | STS | BLOCKED | 8B | community:onnx-community/kyutai-mimi-ONN |  | blocked: gated + CC-BY-NC + novel 8B full-duplex M | GATED: model card shows "You need to agree to share your contact information to access this model" (manual agreement required) -> blocked per triage rules. License CC-BY-NC 4.0 (no |
| nvidia/personaplex-7b-v1 | STS | BLOCKED | 7B | none |  | blocked: gated (HF contact-info agreement); else n | Full-duplex speech-to-speech on the Moshi/Mimi arch (Mimi codec 24kHz encoder+decoder, Moshi temporal+depth Transformer, Helium LM backbone). Mimi codec is OPEN (Kyutai), and WaaV  |
| kyutai/personaplex-rl-seamless | STS | BLOCKED | 8B | none |  | blocked: gated + CC BY-NC non-commercial license | GATED: model card shows "You need to agree to share your contact information to access this model" — not trivially requestable. License is CC BY-NC 4.0 (non-commercial) combined wi |
| aufklarer/PersonaPlex-7B-MLX-4bit | STS | BLOCKED | 7B | none |  | blocked: MLX-only + NC license + gated base; novel | MLX-4bit (Apple-Silicon-only, group_size 64, bits 4) quant of nvidia/personaplex-7b-v1. Full-duplex speech-to-speech on the Kyutai Moshi arch: 32-layer/4096d temporal transformer ( |
| aufklarer/PersonaPlex-7B-MLX-8bit | STS | BLOCKED | 7B | none |  | blocked: MLX-only serialization + gated upstream p | This repo is an MLX 8-bit quantized variant (Apple-Silicon-only "Swift inference" / MLX serving) of nvidia/personaplex-7b-v1 — MLX-only = BLOCKED per criteria. This repo itself is  |
| tencent/Covo-Audio-Chat | STS | BLOCKED | 7.6B | none |  | blocked: non-commercial license + closed output co | End-to-end speech-to-speech audio LLM. BLOCKED on TWO independent grounds. (1) LICENSE is fatal: custom Tencent academic-only license states "use the Covo-Audio only for academic p |
| scragnog/Ace-Step-1.5-ScragVAE | STS | BLOCKED | 0.17B | none |  | blocked: out-of-domain (music-gen VAE), partial mo | NOT a voice/STS model despite the "STS 0.2B" listing — it is a fine-tuned AutoencoderOobleck (Stable Audio Oobleck, stable-audio-tools) VAE *decoder* for the ACE-Step 1.5 TEXT-TO-M |
| pltobing/streaming-speech-translation | STS | BLOCKED | 4B | self | nemotron-speech-streaming | blocked: gated access-form + CC-BY-NC-4.0 (non-com | DECISIVE: GATED (gated="auto", extra_gated_prompt requires a CC-BY-NC-4.0 agreement + access form; file contents return "restricted, must be authenticated") and license is CC-BY-NC |
| coqui/XTTS-v2 | TTS | P4b-codec-ar-closed | 0.46B | none |  | blocked: non-commercial license (CPML 1.0.0) + clo | BLOCKED on two independent grounds. (1) LICENSE: Coqui Public Model License 1.0.0 is explicitly NON-COMMERCIAL ("allows only non-commercial use of a machine learning model and its  |

## BentoML-blog additions (6 new TTS, triaged 2026-06-15)

| model | pattern | params | onnx mirror | win | effort |
|---|---|---|---|---|---|
| microsoft/VibeVoice-Realtime-0.5B | P1-onnx-direct | 0.9B | community:FluffyBunnies/vibevoice-onnx-v2 | moderate | new arch ~1 day: ORT glue for 5-graph pipeline + DDPM s |
| nari-labs/Dia2-2B | P4-codec-ar-open | 2B | community:onnx-community/kyutai-mimi-ONNX  | moderate | new arch ~1-2 days |
| ResembleAI/chatterbox-turbo | P5-per-venv | 0.35B | none | clean | weights-only (reuse existing chatterbox P5 venv; add tt |
| myshell-ai/MeloTTS-English | P1-onnx-direct | 0.05B | community:k2-fsa/sherpa-onnx (vits-melo-tt | clean | weights-only + VITS lexicon/tokens frontend (sherpa-sty |
| neuphonic/neutts-air | P4-codec-ar-open | 0.5B | community:neuphonic/neucodec-onnx-decoder  | moderate | new codec-AR forward + NeuCodec decode glue ~1-2 days |
| 2noise/ChatTTS | P4-codec-ar-open | 0.5B | none | moderate | new arch ~1-2 days: reimplement GPT AR loop + DVAE + Vo |

## CORRECTION (2026-06-15): P5-per-venv is LAST-RESORT, not "clean"
Per directives #2/#4/#5, the preferred onboarding hierarchy (most→least portable) is:
  1. **ONNX via Rust ORT** (P1/P4/P6) — fully portable (CUDA/ROCm/QNN/CPU-AMX/AVX/NEON…).
  2. **Model class on the SHARED runtime** (P3) — system torch+transformers + compat.py shim, NO venv
     (ARK-ASR, granite). Portable: one install loads many models, vLLM-style.
  3. **Reimplement OUR forward from the published safetensors** (P4) — load provider weights, run our
     code (higgs, funasr-nano). The vLLM standard.
  4. **Bud-export ONNX** (last resort for a portable artifact).
  5. **Per-model venv + provider pip pkg (P5)** = TRUE LAST RESORT / validation-reference ONLY. A venv
     pinning old torch is NOT portable (fails directive #2/#4) and wraps the provider runtime (violates
     #5). It does NOT count as a "clean" onboard.

Re-scope of the P5 rows: they should be onboarded by reimplementing the forward portably (→ P4) or via
the shared-env model class (→ P3), NOT by venv. Specifically:
- **ResembleAI/chatterbox-turbo**: re-bucket P5→**P4** (reimplement T3 Llama-AR token LM + S3Gen
  MeanFlow-distilled CFM codec from the open safetensors; codec weights are open). Moderate, not "clean".
- **chatterbox (#24, already onboarded via venv)**: that was the single per-venv fallback / proof. Flag
  for portable reimplementation (same T3+S3Gen path) to bring it in line; the venv build stays only as a
  validation reference.
- rednote-hilab/dots.tts-mf, Soul-AILab/SoulX-Singer, Aratako/Irodori-TTS, Aratako/Semantic-DACVAE-JP:
  target P3 (shared-env class if transformers-loadable) or P4 (reimplement), venv validation-only.

## HARD RULE (2026-06-15, user): ELIMINATE all custom-pip/venv implementations
No counted onboard may depend on a per-model venv or provider pip package. For every such model, download &
read the repo and REIMPLEMENT the architecture portably (modular, performant, accuracy-verified). venv = a
throwaway validation reference only. The `P5-per-venv` bucket is RETIRED (→ P3 shared-env class or P4
reimplement-from-safetensors). **chatterbox (#24)** was venv-onboarded → it does NOT meet the bar and is queued
for portable reimplementation (T3 Llama-AR + S3Gen MeanFlow CFM from open safetensors). chatterbox-turbo,
neutts-air, Dia2, ChatTTS, dots.tts-mf, SoulX-Singer, Irodori, Semantic-DACVAE → all P4/P3, never venv.

## Main-thread survey (2026-06-15): clean wins exhausted, remaining codec-AR TTS all have friction
After the 7 verified onboards, surveyed the next codec-AR TTS candidates — NONE is a clean win:
- neuphonic/neutts-air: Qwen2-0.5B AR + open NeuCodec ONNX DECODER, but (a) espeak-ng G2P frontend (GPL system
  dep — portability friction), (b) codec ENCODER is non-ONNX (pytorch_model.bin) → needed for its cloning, (c)
  cloning-ONLY (no default voice). Needs portable espeak-compatible G2P + codec-encoder ONNX.
- sivasub987/Qwen3-TTS-0.6B-ONNX-INT8: 9-graph full ONNX BUT the shipped sample_inference.py is only a graph-LOAD
  test (no end-to-end orchestration; skips talker-decode), AND the int8 codec uses ConvInteger ops that "may not
  work on all ONNX Runtime builds" (portability risk). Full 9-graph orchestration must be reverse-engineered.
- FunAudioLLM/Fun-CosyVoice3-0.5B: 4 ONNX incl. campplus speaker-encoder → CFM-seam reuse (Matcha/CosyVoice) but
  cloning-based (needs ref). Moderate.
CONCLUSION: remaining models are MODERATE-HARD arcs each needing focused effort (G2P, cloning, incomplete exports,
novel codecs). Least-friction next options: resemble-enhance denoiser (P6, skeskinen/resemble-denoise-onnx community
ONNX — verifiable via corr) OR a focused CosyVoice3 (CFM-seam reuse). DON'T rush these — quality > count (cf. the
OmniVoice "looked-verified-but-not-robust" lesson).

## Batch research wave 2 (2026-06-15) — next-model recipes + reality
- **csm-1b (moderate, DOABLE)**: codec-AR (Llama-1B backbone + Llama-100M depth-decoder + Mimi codec). transformers
  5.12 has NATIVE Csm + Mimi → P3 sidecar, generate(output_audio=True) bundles the nested-AR + Mimi decode. Base
  sesame/csm-1b GATED (403) but UNGATED MIRROR eustlb/csm-1b (the tf port author) works. Cloning-only (fixed seed →
  stable default voice). English-only. → IMPLEMENTING.
- **dots.tts-base (hard)**: NO open ONNX anywhere (verified). Torch-sidecar + VENDOR architecture-only modeling
  (src/dots_tts: config/core/model + backbone/dit + semantic_encoder + vocoder + speaker, ~4-6 files). Continuous-
  latent AR LLM + DiT CFM. Apache-2.0 ungated. Like the CosyVoice3 vendoring pattern but more files.
- **VibeVoice-1.5B (hard)**: Qwen2.5-1.5B LM + acoustic σ-VAE(~680M) + semantic tok + 123M diffusion head. MIT
  ungated, text-frontend (no espeak). NO verified vocoder ONNX → export/vendor the σ-VAE decoder + diffusion head.
  Long-form multi-speaker. Substantial.
- **sortformer-4spk-v2 (hard)**: streaming end-to-end DIARIZATION (.nemo, CC-BY-4.0; v2.1 is NC so use v2). NeMo
  whole-model ONNX export BROKEN (issue #15077, dynamic slicing) → must export pre-encoder + head as TWO ONNX graphs
  (workaround) + reimplement the 128-bin NeMo log-mel (have kaldi_fbank). New diar arch (per-frame speaker-activity).
- **LFM2.5-Audio-1.5B (STS, new modality)**: OFFICIAL ONNX LiquidAI/LFM2.5-Audio-1.5B-ONNX (20 graphs, ungated) +
  transformers Lfm2 base. STS = audio→audio (+text) — a NEW engine task type (WaaV has Stt/Tts/diar/enhance, not STS).
  Needs a new contract. Deferred until the TTS/STT/diar worklist is exhausted.
ORDER: csm (now) → dots.tts / VibeVoice (vendor-modeling, CosyVoice3 pattern) → sortformer (NeMo 2-graph export) →
enhance (clear/tse-tasnet) → STS (new contract). Many remaining are HARD (vendored reimplements) or BLOCKED.
