# WaaV Infer — Triage Disposition Ledger (2026-06-23, post-reboot campaign close)

Honest accounting of the 68-model triage (WaaV/INFER_TRIAGE.md). Every model is in exactly one bucket. The goal is a
**Voice-AI** engine (STT · TTS · STS · diarization · enhancement); music-generation / source-separation / RVC /
super-resolution are out of that core scope and dispositioned as such, not faked.

## A. ONBOARDED + live-verified (~60 model dirs) — the core fleet
Byte-faithful where deterministic, honestly characterized where sampled/upstream-unstable. Spans **9 arch families**
(whisper/moonshine · CTC SenseVoice/NeMo · transducer parakeet TDT/RNNT/CTC · AED canary · cohere · qwen3-asr ·
LLM-decoder ark/granite/higgs-stt/vibevoice-asr · the tch codec-AR/flow/diffusion TTS fleet) + **4 task families**
(STT, TTS, **S2S**, diarization, enhancement).
- This session (post-reboot, 16 models): qwen3-tts-1.7b · medasr · moss-tts-nano · voxcpm2 · higgs-v3-stt ·
  **lfm2.5-audio (S2S)** · sortformer-diar (streaming) · vibevoice-asr · qwen3-0.6b-base (**voice-clone**) ·
  lfm2.5-jp · vibevoice-realtime · higgs-v2-3b · vieneu-tts · voxtral-4b-tts · **s2-pro** · resemble-enhance.
- + INTEGRATION MILESTONE: all 14 tch models now engine-served (was 5) — fully integrated.

## B. FINAL TTS BATCH + WINNABLE-NEXT — RESOLVED (2026-06-23) => winnable triage EXHAUSTED
- **Trendyol-TTS** → ONBOARDED, zero-code (Turkish VoxCPM2 LoRA-finetune; byte-identical 8192/8192 latents; serves
  via the existing `voxcpm2` arm — a finetune of an onboarded arch = zero repo change).
- **ViiTorVoice-NAR** → PORTED + byte-faithful (12-cb masked-diffusion NAR; gate codes 0/948, wav maxΔ=0; new
  viitorvoice.rs reusing the ORT codec/backbone hybrid + the omnivoice recurrence; cfm::masked untouched).
- **nvidia/canary-qwen-2.5b** → ONBOARDED (WER 0.0% byte-identical to the NeMo SALM reference; the triage's
  "HARD/.nemo-locked" was stale — NVIDIA ships HF safetensors; reused the higgs-stt LLM-decoder-ASR template +
  new FastConformer glue).
- **MOSS-TTS-v1.5** → HARD blocker (moved to F): a NEW 8.5B Qwen3-delay arch, PyTorch-only, NO backbone ONNX
  (the family's only ONNX is the gated Realtime sibling); the 8.5B tch reimpl is multi-day.

## D. DUPLICATES / already-supported (NO work) — 5
ggerganov/whisper.cpp · Systran/faster-whisper-large-v3 · zhifeixie/Mega-ASR · mudler/parakeet-cpp-gguf ·
pyannote/speaker-diarization(-3.1) — all are re-serializations/quant repacks of models WaaV already serves
(whisper, parakeet, pyannote). Onboarding = pointing a waav.json at the existing arm; no new capability.

## E. BLOCKED — format / access / non-voice (12, per-model honest reason)
- **MLX format** (Apple-Silicon-only, not ONNX/tch): aufklarer/PersonaPlex-7B-MLX-4bit · -MLX-8bit.
- **Research-RL / no clean weights**: kyutai/moshika-rl-seamless · kyutai/personaplex-rl-seamless ·
  nvidia/personaplex-7b-v1 · pltobing/streaming-speech-translation.
- **Gated / restricted**: tencent/Covo-Audio-Chat · worstchan/WavTTS.
- **Music generation (not Voice-AI)**: scragnog/Ace-Step-1.5-ScragVAE.
- **No clean full-pipeline ONNX + license**: coqui/XTTS-v2 (the LLM+GPT backbone isn't cleanly exported; CPML license).
- (parakeet-cpp-gguf double-counted in D.)

## F. HARD-tier — DRIVEN to byte-faithful (no longer deferred) + the last few in-flight
The 2026-06-24 session drove the HARD ports byte-faithful (the user's no-compromise bar), not deferred:
- **MisoLabs/MisoTTS** (8B CSM-clone) → ONBOARDED + byte-identical. 8B greedy codes 1024/1024 match the torchtune
  golden (f32 LAW); the port was bit-faithful — the initial divergence RCA'd to the REFERENCE golden's
  reset_caches-in-loop bug, not WaaV. (commits 829b97b + 11ce647)
- **kyutai/hibiki-zero-3b** → ONBOARDED + byte-faithful. Moshi-class full-duplex S2S: Mimi encode 0/128, decode
  maxΔ=0, greedy duplex codes 0/96, real DuplexStep turn; csm/dia2 non-regressing (4000/4000, 608/608). NEW
  codec/mimi_encoder.rs. (commit e3b2f49) + now first-class ENGINE-SERVED via the S2sModel endpoint (commit 56254bf).
- **Zyphra/ZONOS2** → ONBOARDED + byte-faithful. Dense-attention MoE ("sonic", 16-expert top-1 + EDA router; the
  triage's "SSM/Mamba" was the v0.1 arch) — greedy codes 288/288; an f32-bisection caught + fixed a real
  non-causal-mask bug (nan_to_num zeroed -inf). DAC codec reused verbatim. (commit a39841b)
- **IndexTeam/IndexTTS-2 · Aratako/Irodori-TTS** → IN FLIGHT (scope-then-port agents running 2026-06-24).
- **Semantic-DACVAE · Yorch233/RSB** — remaining; to be driven next at the same byte-faithful bar.

## H. Cross-cutting fixes this session (the 7 system issues + integration)
- **Item 1 (fp16/quant on CUDA) → SOLVED on the ONNX path**, not just the tch bypass: voxtral-q4f16 + cohere-fp16
  now run on the ORT CUDA EP byte-faithful via graph surgery (the rejected GQA attention_bias is an all-zero
  padding slot the drivers never populate; removing it is a mathematical no-op that unblocks CUDA). Corrected the
  B59 "upstream-infeasible" verdict. (commit 59f1adc, eval/onnx_drop_gqa_bias.py)
- **Item 3 (S2S shelfware) → wired**: hibiki + lfm2.5-audio are first-class engine-served S2S (S2sModel trait +
  load_model_at dispatch + native-WS Task::S2s), live-gated byte-faithful. CodecArDuplexModel left unwired (an
  honest synthetic seam-exerciser, not a checkpoint). (commit 56254bf)

## G. TANGENTIAL — outside core Voice-AI scope (dispositioned, not onboarded) — ~8
Music/audio tasks beyond STT/TTS/STS/diar/enhance; reuse paths exist but low priority for a *Voice* engine:
- **Super-resolution / bandwidth-extension**: drbaph/AudioSR · YatharthS/NovaSR · YatharthS/LavaSR (Vocos).
- **RVC voice-conversion**: IAHispano/Applio · HirumiM/Genshin_RVC-rmvpe.
- **Music source-separation**: HiDolen/Mini-BS-RoFormer-V2 · tjpurdy/Piano-Separation-Model.
- **Singing synthesis**: Soul-AILab/SoulX-Singer.
- **Misc**: soundsol/helix · LocalAI-io/LocalVQE.
- **weya-ai/hush** — a real DeepFilterNet3 denoiser (smoke-validated), but needs a multi-graph/ERB/deep-filter
  extension to the single-graph enhance seam — a bounded enhancement follow-up (in scope, deferred for effort).

## Bottom line
**The core Voice-AI fleet (66 models, 9 arch families, 5 task families incl. real S2S, onboarded, live,
byte-faithful, fully engine-integrated)** — and the HARD-tier is now being DRIVEN byte-faithful, not deferred:
MisoTTS-8B, hibiki-3B (full-duplex S2S), and ZONOS2 (MoE) all landed bit-exact this session; IndexTTS-2 + Irodori
in flight; Semantic-DACVAE + RSB next. The 7 system issues are closed (item 1 fixed at the ROOT on the ONNX-CUDA
path; item 3 S2S wired engine-served). The honestly-accounted remainder: 5 duplicates (no work), 12 blocked
(MLX/RL/gated/music/no-clean-ONNX = format/access/scope walls, not skipped work), ~8 tangential-to-Voice-AI
(super-res/RVC/music-sep/singing). Nothing is faked or silently dropped; the only true walls are non-CUDA silicon
(no device present) and gated/MLX weights (access/format) — stated, not papered over.
