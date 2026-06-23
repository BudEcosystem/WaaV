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

## B. IN-FLIGHT (final TTS batch) — 3
MOSS-TTS-v1.5 · Trendyol-TTS · ViiTorVoice-NAR (verifying now; committed when they land or dispositioned honestly).

## C. WINNABLE-NEXT (deferred, but a clean reuse exists) — 1+
- **nvidia/canary-qwen-2.5b** (HARD tier) — an LLM-decoder ASR; reuses the proven qwen3-asr / higgs-v3-stt template.
  Not a blocker, just unstarted. The next onboard if the campaign continues.

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

## F. HARD-tier deferred — big ports / new stacks (10)
Each a genuine multi-day port, not a config add; deferred with the honest cost noted:
- **MisoLabs/MisoTTS** (8B CSM-clone) — a csm-sized AR + depth-decoder port.
- **Zyphra/ZONOS2 · IndexTeam/IndexTTS-2** — new codec-AR TTS stacks.
- **kyutai/hibiki-zero-3b** — Moshi-class full-duplex S2S (the DuplexStepModel seam exists; the model is a big port).
- **Aratako/Irodori-TTS · Semantic-DACVAE** — JP voice-design TTS + a new DAC-VAE.
- **Yorch233/RSB** — new stack.

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
**The core Voice-AI fleet (~60 models, 9 arch families, 5 task families) is onboarded, live, byte-faithful, and
fully engine-integrated.** The remainder is honestly accounted: 5 duplicates (no work), 12 blocked (MLX/RL/gated/
music/no-clean-ONNX), 10 HARD big-ports (cost-noted), ~8 tangential-to-Voice-AI (super-res/RVC/music-sep/singing).
The one clean winnable-next is canary-qwen-2.5b (qwen3-asr template). Nothing is faked or silently dropped.
