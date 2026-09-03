# Gated Models (WaaV Infer onboarding campaign)

Models from the onboarding list whose **Hugging Face source repo is gated** (`gated=auto` — requires
agreeing to share contact information / an access form before download). Status verified live via the HF
API on 2026-06-16. "Worked around" = an **ungated** community mirror / ONNX export exists, so WaaV could
onboard the model without touching the gated repo.

## Gated but ONBOARDED (routed around via an ungated mirror)

| Model (gated source) | Modality | Gate | Workaround used | WaaV status |
|---|---|---|---|---|
| `sesame/csm-1b` | TTS (codec-AR) | contact-info | ungated mirror **`eustlb/csm-1b`** (the transformers-port author) | ✅ onboarded as **CSM-1B** (#35), verified == native |
| `pyannote/speaker-diarization-community-1` | Diarization | contact-info | ungated ONNX mirror **`altunenes/speaker-diarization-community-1-onnx`** | ✅ onboarded as **pyannote-community-1** (#28), verified |
| `pyannote/speaker-diarization-3.1` | Diarization | contact-info | community ONNX (k2-fsa/sherpa-onnx seg + WeSpeaker emb) | ✅ supported (existing diarization arm) |
| `pyannote/speaker-diarization` (2.1) | Diarization | contact-info | — (older duplicate of 3.1/community-1) | superseded; not separately onboarded |

## Gated AND BLOCKED (no ungated path + additional blockers)

| Model (gated source) | Modality | Gate + other blockers | Why blocked |
|---|---|---|---|
| `google/medasr` | STT (Conformer-CTC) | contact-info (Health AI Dev Foundations) | no working ungated ONNX mirror (`csukuangfj/sherpa-onnx-medasr-ctc` = 404) |
| `kyutai/moshika-rl-seamless` | STS (full-duplex) | gated **+ CC-BY-NC** + 8B Moshi MoE | non-commercial license + gated + novel 8B arch |
| `nvidia/personaplex-7b-v1` | STS (full-duplex) | gated + 7B | gated + over practical budget; Moshi/Mimi arch |
| `kyutai/personaplex-rl-seamless` | STS (full-duplex) | gated **+ CC-BY-NC** + 8B | non-commercial + gated |
| `pltobing/streaming-speech-translation` | STS (streaming translate) | gated (access form) **+ CC-BY-NC-4.0** | non-commercial + gated; file contents auth-restricted |
| `aufklarer/PersonaPlex-7B-MLX-4bit` | STS | gated BASE (`personaplex-7b-v1`) + MLX-only + NC | the MLX repo itself is ungated, but it's an Apple-Silicon-only quant of a gated/NC base |
| `aufklarer/PersonaPlex-7B-MLX-8bit` | STS | gated BASE + MLX-only | same — MLX-only serialization, gated upstream |

## Notes
- **Gating ≠ unusable.** For 4 of these (csm-1b + the 3 pyannote diarization repos) an ungated mirror or ONNX
  export exists, so WaaV onboarded the model via the open artifact and never needed access to the gated repo.
- The genuinely-blocked gated models are all **also** blocked by other rules (CC-BY-NC non-commercial license,
  >10B / 7-8B size, or MLX-Apple-Silicon-only serialization) — gating is rarely the sole blocker.
- `google/medasr` is the one case where **gating is effectively the decisive blocker** (small 105M Conformer-CTC,
  permissive arch, but no ungated weights/ONNX anywhere) — onboardable only with granted access to the gated repo.
- Models that LOOK like they might be gated but are **NOT** (verified ungated, and onboarded directly):
  `nari-labs/Dia-1.6B-0626`, `nari-labs/Dia2-2B`, `microsoft/VibeVoice-1.5B`, `rednote-hilab/dots.tts-*`,
  `Qwen/Qwen3-TTS-12Hz-0.6B-CustomVoice`, `neuphonic/neutts-air`, `FunAudioLLM/Fun-CosyVoice3-*`.
</content>
