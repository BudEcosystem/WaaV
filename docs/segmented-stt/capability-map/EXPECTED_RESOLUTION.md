# Expected resolution of the capability map, Release 0 to Release 6

**How this file is made.** It is generated, not written by hand. In the WaaV repository, from the directory `docs/segmented-stt/capability-map/` (the authoritative copy), run:

```
python3 resolve.py --expected > EXPECTED_RESOLUTION.md
```

The generator reads `stt_live_capabilities.json` (map version 2026-10-04.1, revision 3, 412 rows) and calls `resolve()` once per row, per release and per kind of session, with a model string that selects that row (the model itself for an exact row; a filler such as `x` in place of each `*` for a pattern row; `zz-unknown-model` for a provider default row). Nothing in the tables is typed by hand. The capability-map design's planned gateway test `the_shipped_map_resolves_as_the_release_table_says` is to hold the Rust resolver to the same answers; `python3 resolve.py --self-test` runs this resolver's own cases.

**Assumptions behind every line.**

- The session is *covered*: from Release 1 its deployment is on the allow-list (Releases 1 and 2) or the switch default is on (Release 3 onward), and no control record disables it. In Release 0 no session is covered. A session that is not covered gets what Release 0 shows, except that refusals scheduled for a later release (`refuse_from_release`) and the language refusals of Release 3 still apply, and a model whose transport is enabled is refused with `not_covered_yet` rather than `client_not_implemented`.
- The preference is `auto` (no `transcription_mode`). The session names the model (a session that names none gets the row marked "(default)").
- The language and region are not given, so language and region constraints are not checked; where the chosen transport carries one, the line says so in brackets. Today's clients keep their path for a language outside their constraint until Release 3 (addendum A7).
- Transports that need evidence first (`enable_requires`: a live probe with a real key, or a measurement within the deadline) are shown as enabled in their release with the condition in brackets. `python3 resolve.py --expected --evidence map` gives the stricter tables in which only the evidence the map carries today counts.
- Three kinds of session. **Voice agent, automatic turns**: a voice agent whose turn detection is `semantic` or `server_vad`, so the gateway must end each caller turn. **Manual agent, conversation loop or DAG**: a voice agent in manual mode, a conversation loop or a DAG session (addendum A1). **Plain /ws session**: no agent and no conversation loop; the client consumes transcripts and ends its own turns (addendum B4: kept on today's buffering client unless it asks for segmented). "same" means the outcome is the same as in the first outcome column. For a self-hosted or Azure OpenAI deployment that is not covered, the voice-agent column shows the agent-leg code (`stt_not_streaming`) and the other column the named-deployment code (`unsupported_deployment`).
- Warnings are listed for the voice-agent column. `(log)` means the fact is only logged and counted (Release 0, and sessions the switch does not cover in Releases 1 and 2); `(notice)` means an entry in `ready.stt`; no mark means a `config_warning` frame. Codes are the customer contract's (W5 section 3.8).

Outcome words: **native stream**: today's streaming client, unchanged. **today's ... client**: a native client that does not stream, kept with a warning. **per-utterance upload**: the segmented engine with the named transcriber. **gateway-driven commit**: a vendor socket on which the gateway's detector sends the commit. **refused**: refused at setup with the code shown.

## Release 0, groundwork and honest refusal

| Provider | Models | Voice agent, automatic turns | Manual agent, conversation loop or DAG | Plain /ws session | Warnings |
| --- | --- | --- | --- | --- | --- |
| unknown provider (global default) | any other id (`*`) | today's client through the plugin registry (unclassified provider) | same | same |  |
| `alibaba-cloud` | any other id (`*`) | native stream (client unverified) | same | same | `stt_client_unverified` (log), `stt_capability_assumed` (log) |
|  | `*-asr-flash-message*`, `*-asr-flash-streaming*`, `*-realtime*`, `fun-asr-flash-8k-realtime`, `fun-asr-flash-8k-realtime-2026-01-28`, `fun-asr-mtl-realtime-2025-12-10`, `gummy-chat-v1`, `gummy-realtime-v1`, `qwen-audio-3.0-asr-flash-streaming`, `qwen-audio-3.1-asr-flash-message`, `qwen-audio-3.1-asr-flash-streaming` | native stream (client unverified) | same | same | `stt_client_unverified` (log) |
|  | `*-filetrans*`, `fun-asr-202*`, `fun-asr-2025-08-25`, `fun-asr-2025-11-07`, `fun-asr-mtl`, `fun-asr-mtl-2025-08-25`, `paraformer-mtl-v1`, `qwen-audio-3.0-asr-flash-filetrans`, `qwen-audio-3.1-asr-flash-filetrans`, `qwen3-asr-flash-filetrans`, `qwen3-asr-flash-filetrans-2025-11-17`, `sensevoice-v1` | refused `stt_live_unsupported` (`async_only`) | same | same |  |
|  | `fun-asr` | today's client, streaming fun-asr-realtime instead | same | same | `stt_model_substituted` (log) |
|  | `fun-asr-flash-2*`, `fun-asr-flash-2026-06-15`, `qwen-audio-3.0-asr-flash`, `qwen-audio-3.1-asr-flash`, `qwen3-asr-flash`, `qwen3-asr-flash-2*`, `qwen3-asr-flash-2025-09-08`, `qwen3-asr-flash-2026-02-10` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
|  | `fun-asr-mtl-realtime` | native stream (client unverified) | same | same | `stt_client_unverified` (log), `stt_model_deprecated` (log) |
|  | `fun-asr-realtime`, `fun-asr-realtime-2026-02-28`, `paraformer-realtime-8k-v1`, `paraformer-realtime-8k-v2`, `paraformer-realtime-v1`, `paraformer-realtime-v2` | native stream | same | same |  |
|  | `fun-asr-realtime-2025-09-15`, `fun-asr-realtime-2025-11-07` | native stream | same | same | `stt_model_deprecated` (log) |
|  | `paraformer-8k-v1` | today's client, streaming paraformer-realtime-8k-v1 instead | same | same | `stt_model_substituted` (log) |
|  | `paraformer-8k-v2` | today's client, streaming paraformer-realtime-8k-v2 instead | same | same | `stt_model_substituted` (log) |
|  | `paraformer-v1` | today's client, streaming paraformer-realtime-v1 instead | same | same | `stt_model_substituted` (log) |
|  | `paraformer-v2` | today's client, streaming paraformer-realtime-v2 instead | same | same | `stt_model_substituted` (log) |
|  | `qwen3-asr-flash-realtime` (default), `qwen3-asr-flash-realtime*` | native stream (client known broken) | same | same | `stt_client_unverified` (log) |
|  | `qwen3-asr-flash-realtime-2025-10-27`, `qwen3-asr-flash-realtime-2026-02-10` | native stream (client known broken) | same | same | `stt_client_unverified` (log), `stt_model_deprecated` (log) |
| `amivoice` | any other id (`*`) | native stream (client known broken) | same | same | `stt_client_unverified` (log), `stt_model_substituted` (log), `stt_capability_assumed` (log) |
|  | `-a-address-input-private`, `-a-bizfinance`, `-a-bizfinance-input`, `-a-bizinsurance`, `-a-bizinsurance-input`, `-a-name-input-private`, `-a-rule-input-private` | today's client, streaming -a-general instead | same | same | `stt_model_substituted` (log) |
|  | `-a-general` (default), `-a-general-en`, `-a-general-input`, `-a-general-ko`, `-a-general-zh`, `-a-medical`, `-a-medical-input`, `-a2-ja-general`, `-a2-multi-general`, `-a2-zh-general` and 3 more | native stream (client known broken) | same | same | `stt_client_unverified` (log) |
| `assemblyai` | any other id (`*`), `u3-pro`, `u3-sync-pro`, `universal-2`, `universal-3-5-pro`, `universal-3-6-pro` | today's client, streaming universal-streaming-english instead | same | same | `stt_model_substituted` (log) |
|  | `universal-streaming-english` (default), `universal-streaming-multilingual` | native stream | same | same |  |
| `aws-transcribe` | any other id (`*`) | native stream | same | same | `stt_model_substituted` (log), `stt_capability_assumed` (log) |
|  | `call-analytics`, `healthscribe`, `medical` | today's client, streaming standard instead | same | same | `stt_model_substituted` (log) |
|  | `standard` (default) | native stream | same | same |  |
| `azure_openai` | any other id (`*`), `gpt-4o-mini-transcribe`, `gpt-4o-transcribe`, `gpt-4o-transcribe-diarize`, `gpt-live-transcribe`, `gpt-realtime-whisper`, `gpt-transcribe`, `whisper`, `whisper-*` | refused `stt_not_streaming` | refused `unsupported_deployment` | refused `unsupported_deployment` |  |
| `baidu` | any other id (`*`) | native stream (client known broken) | same | same | `stt_client_unverified` (log), `stt_model_substituted` (log), `stt_capability_assumed` (log) |
|  | `1537` (default), `15372`, `15376`, `1637`, `1737`, `1837` | native stream (client known broken) | same | same | `stt_client_unverified` (log) |
|  | `1936` | refused `stt_model_retired` | same | same |  |
|  | `19362`, `80001`, `8001` | today's client, streaming 1537 instead | same | same | `stt_model_substituted` (log) |
| `bhashini` | any other id (`*`), `ai4bharat/conformer-hi-gpu--t4`, `ai4bharat/conformer-multilingual-dravidian-gpu--t4`, `ai4bharat/conformer-multilingual-indo_aryan-gpu--t4`, `ai4bharat/whisper-medium-en--gpu--t4`, `bhashini/ai4bharat/conformer-multilingual-asr`, `bhashini/bodhan/asr-transcribe-core`, `bhashini/bodhan/asr-transcribe-flex`, `bhashini/iisc/asr-bho-t4`, `bhashini/iisc/asr-mai-t4` and 3 more | refused `stt_live_unsupported` (`client_not_implemented`) | today's buffering client | today's buffering client | voice agent: none; manual agent or loop: `stt_buffered_until_commit`; plain: `stt_buffered_until_commit` |
| `cartesia` | any other id (`*`) | native stream | same | same | `stt_capability_assumed` (log) |
|  | `ink-2`, `ink-preview`, `ink-whisper` (default), `ink-whisper-2025-06-04` | native stream | same | same |  |
| `deepgram` | any other id (`*`) | native stream | same | same | `stt_capability_assumed` (log) |
|  | `base`, `base-general` (default), `conversationalai`, `enhanced`, `enhanced-finance`, `enhanced-general`, `enhanced-meeting`, `enhanced-phonecall`, `finance`, `meeting` and 22 more | native stream | same | same |  |
|  | `flux-*`, `flux-general-en`, `flux-general-multi`, `whisper`, `whisper-*`, `whisper-base`, `whisper-large`, `whisper-medium`, `whisper-small`, `whisper-tiny` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
| `elevenlabs` | any other id (`*`), `*realtime*`, `scribe_v2`, `scribe_v2_medical` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
|  | `scribe_v1` | refused `stt_model_retired` | same | same |  |
|  | `scribe_v2_realtime` (default) | native stream | same | same |  |
| `fpt-ai` | any other id (`*`), `general` (default) | refused `stt_live_unsupported` (`client_not_implemented`) | today's buffering client | today's buffering client | voice agent: none; manual agent or loop: `stt_buffered_until_commit`; plain: `stt_buffered_until_commit` |
| `gladia` | any other id (`*`), `solaria-3` | today's client, streaming solaria-1 instead | same | same | `stt_model_substituted` (log) |
|  | `solaria-1` (default) | native stream | same | same |  |
| `gnani` | any other id (`*`) | today's client (behaviour for this model not established) | same | same | `stt_client_unverified` (log), `stt_capability_assumed` (log) |
|  | `default` | native stream (client known broken) | same | same | `stt_client_unverified` (log) |
|  | `gnani-prisma-v2.5` (default), `vachana-audio-intelligence-v2` | today's client (behaviour for this model not established) | same | same | `stt_client_unverified` (log) |
| `google` | any other id (`*`) | native stream | same | same | `stt_capability_assumed` (log) |
|  | `chirp` | native stream (client unverified) | same | same | `stt_client_unverified` (log) |
|  | `chirp_2`, `chirp_3`, `chirp_telephony`, `latest_long` (default), `long`, `medical_conversation`, `medical_dictation`, `short`, `telephony`, `telephony_short` | native stream | same | same |  |
|  | `latest_short` | native stream (client known broken) | same | same | `stt_client_unverified` (log) |
| `groq` | any other id (`*`), `whisper-large-v3`, `whisper-large-v3-turbo` (default) | refused `stt_live_unsupported` (`client_not_implemented`) | today's buffering client | today's buffering client | voice agent: none; manual agent or loop: `stt_buffered_until_commit`; plain: `stt_buffered_until_commit` |
|  | `distil-whisper-large-v3-en` | refused `stt_model_retired` | same | same |  |
| `huawei-cloud` | any other id (`*`) | native stream (client known broken) | same | same | `stt_client_unverified` (log), `stt_model_substituted` (log), `stt_capability_assumed` (log) |
|  | `arabic_16k_general`, `arabic_8k_general`, `cantonese_16k_common`, `chinese_16k_common`, `chinese_16k_court`, `chinese_16k_general` (default), `chinese_16k_it`, `chinese_8k_common`, `chinese_8k_general`, `english_16k_general`, `shanghai_16k_common`, `sichuan_16k_common` | native stream (client known broken) | same | same | `stt_client_unverified` (log) |
|  | `chinese_16k_conversation`, `chinese_16k_travel`, `english_16k_common`, `english_8k_common` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
|  | `chinese_16k_media`, `sichuan_8k_common` | refused `stt_live_unsupported` (`async_only`) | same | same |  |
| `ibm-watson` | any other id (`*`) | native stream | same | same | `stt_capability_assumed` (log) |
|  | `*_BroadbandModel`, `*_NarrowbandModel` | refused `stt_model_retired` | same | same |  |
|  | `ar-MS_Telephony`, `cs-CZ_Telephony`, `de-DE_Multimedia`, `de-DE_Telephony`, `en-AU_Multimedia`, `en-AU_Telephony`, `en-GB_Multimedia`, `en-GB_Telephony`, `en-IN_Telephony`, `en-US_Multimedia` (default) and 23 more | native stream | same | same |  |
|  | `de-DE`, `en-AU`, `en-GB`, `en-IN`, `en-US`, `es-AR`, `es-CL`, `es-CO`, `es-ES`, `es-MX` and 8 more | native stream (client unverified) | same | same | `stt_client_unverified` (log) |
| `iflytek` | any other id (`*`) | native stream (client known broken) | same | same | `stt_client_unverified` (log), `stt_capability_assumed` (log) |
|  | `*ist*`, `*realtime*`, `*stream*`, `iat` (default), `ist`, `ist_huanyu`, `ist_hy`, `ist_open`, `medical`, `sp_ist_vais` | native stream (client known broken) | same | same | `stt_client_unverified` (log) |
|  | `iflyrec_voice_*`, `iflyrec_voice_cn_10m_ed`, `iflyrec_voice_de_de_24h`, `iflyrec_voice_es_es_24h`, `iflyrec_voice_fr_fr_24h`, `iflyrec_voice_ja_jp_24h`, `iflyrec_voice_ko_kr_24h`, `iflyrec_voice_th_th_sp_24h`, `iflyrec_voice_vi_vn_vais_24h`, `iflyrec_voice_yueyu_24h` | today's client, streaming iat instead | same | same | `stt_model_substituted` (log) |
| `microsoft-azure` | any other id (`*`) | native stream | same | same | `stt_model_substituted` (log), `stt_capability_assumed` (log) |
|  | `default` (default) | native stream | same | same |  |
|  | `llm-speech`, `MAI-Transcribe-1`, `MAI-Transcribe-1.5`, `MAI-Transcribe-2`, `MAI-Transcribe-2-Streaming` | today's client, streaming default instead | same | same | `stt_model_substituted` (log) |
| `naver-clova` | any other id (`*`), `clova-speech-long`, `clova-speech-short`, `clova-speech-streaming`, `csr` (default) | refused `stt_live_unsupported` (`client_not_implemented`) | today's buffering client | today's buffering client | voice agent: none; manual agent or loop: `stt_buffered_until_commit`; plain: `stt_buffered_until_commit` |
| `nectec` | any other id (`*`) | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
|  | `partii4` (default) | refused `stt_live_unsupported` (`client_not_implemented`) | today's buffering client | today's buffering client | voice agent: none; manual agent or loop: `stt_buffered_until_commit`; plain: `stt_buffered_until_commit` |
|  | `partii5` | refused `stt_model_retired` | same | same |  |
| `openai` | any other id (`*`), `gpt-transcribe` (default) | refused `stt_live_unsupported` (`client_not_implemented`) | today's buffering client | today's buffering client | voice agent: none; manual agent or loop: `stt_buffered_until_commit`; plain: `stt_buffered_until_commit` |
|  | `gpt-4o-mini-transcribe`, `gpt-4o-mini-transcribe-2025-03-20`, `gpt-4o-mini-transcribe-2025-12-15`, `gpt-4o-transcribe`, `gpt-4o-transcribe-diarize`, `whisper-1` | refused `stt_live_unsupported` (`client_not_implemented`) | today's buffering client | today's buffering client | voice agent: none; manual agent or loop: `stt_buffered_until_commit`, `stt_model_deprecated` (log); plain: `stt_buffered_until_commit`, `stt_model_deprecated` (log) |
|  | `gpt-live-transcribe`, `gpt-live-transcribe*`, `gpt-realtime-whisper`, `gpt-realtime-whisper*` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
| `phonexia` | any other id (`*`), `default`, `large_v2`, `large_v3`, `medium`, `speech-to-text`, `speech-to-text-whisper-enhanced` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
|  | `EN_US_6` | refused `stt_model_retired` | same | same |  |
| `revai` | any other id (`*`) | native stream | same | same | `stt_model_substituted` (log), `stt_capability_assumed` (log) |
|  | `*whisper*` | refused `stt_live_unsupported` (`async_only`) | same | same |  |
|  | `fusion`, `low_cost` | today's client, streaming machine instead | same | same | `stt_model_substituted` (log), `stt_model_deprecated` (log) |
|  | `human` | today's client (behaviour for this model not established) | same | same | `stt_client_unverified` (log) |
|  | `machine` (default), `machine_v2`, `reverb`, `reverb-foreign-language` | native stream | same | same |  |
| `reverie` | any other id (`*`) | native stream (client known broken) | same | same | `stt_client_unverified` (log) |
| `sarvam` | any other id (`*`) | native stream | same | same | `stt_capability_assumed` (log) |
|  | `saaras:v2.5`, `saaras:v3-realtime` | today's client (behaviour for this model not established) | same | same | `stt_client_unverified` (log) |
|  | `saaras:v3`, `saaras:v4` | native stream | same | same |  |
|  | `saarika:v2.5` (default) | native stream | same | same | `stt_model_deprecated` (log) |
| `sberdevices` | any other id (`*`), `callcenter`, `general` (default), `ivr`, `media` | today's timed-upload client (known broken) | same | same | `stt_client_unverified` (log) |
| `self_hosted` | any other id (`*`), `*nemotron*`, `*voxtral*realtime*`, `*whisper*`, `kyutai/stt-1b-en_fr`, `kyutai/stt-2.6b-en` | refused `stt_not_streaming` | refused `unsupported_deployment` | refused `unsupported_deployment` |  |
| `speechmatics` | any other id (`*`) | native stream | same | same | `stt_capability_assumed` (log) |
|  | `enhanced`, `standard` (default) | native stream | same | same |  |
|  | `linden-1`, `melia-1`, `oak-1` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
| `tencent` | any other id (`*`) | native stream (client known broken) | same | same | `stt_client_unverified` (log), `stt_model_substituted` (log), `stt_capability_assumed` (log) |
|  | `16k_ar`, `16k_de`, `16k_en`, `16k_en_edu`, `16k_en_game`, `16k_en_large`, `16k_es`, `16k_fil`, `16k_fr`, `16k_hi` and 23 more | native stream (client known broken) | same | same | `stt_client_unverified` (log) |
|  | `16k_zh-PY`, `16k_zh_dialect`, `16k_zh_en_meeting` | today's client, streaming 16k_zh instead | same | same | `stt_model_substituted` (log) |
| `tinkoff` | any other id (`*`) | native stream (client known broken) | same | same | `stt_client_unverified` (log), `stt_model_substituted` (log), `stt_capability_assumed` (log) |
|  | `default` (default) | native stream (client known broken) | same | same | `stt_client_unverified` (log) |
| `viettel-ai` | any other id (`*`) | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
| `waav-infer` | any other id (`*`), `nemotron*`, `voxtral*realtime*`, `whisper*` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
| `yandex` | any other id (`*`), `deferred-general`, `deferred-general:deprecated`, `deferred-general:rc`, `general` (default), `general:deprecated`, `general:rc` | today's timed-upload client (known broken) | same | same | `stt_client_unverified` (log) |
|  | `deferred*` | today's client (behaviour for this model not established) | same | same | `stt_client_unverified` (log) |

## Release 1, first working calls

Outcome changed from the previous release for 33 row(s), of these providers: `azure_openai`, `elevenlabs`, `groq`, `openai`, `self_hosted`, `waav-infer`.

| Provider | Models | Voice agent, automatic turns | Manual agent, conversation loop or DAG | Plain /ws session | Warnings |
| --- | --- | --- | --- | --- | --- |
| unknown provider (global default) | any other id (`*`) | today's client through the plugin registry (unclassified provider) | same | same |  |
| `alibaba-cloud` | any other id (`*`) | native stream (client unverified) | same | same | `stt_client_unverified` (notice), `stt_capability_assumed` (notice) |
|  | `*-asr-flash-message*`, `*-asr-flash-streaming*`, `*-realtime*`, `fun-asr-flash-8k-realtime`, `fun-asr-flash-8k-realtime-2026-01-28`, `fun-asr-mtl-realtime-2025-12-10`, `gummy-chat-v1`, `gummy-realtime-v1`, `qwen-audio-3.0-asr-flash-streaming`, `qwen-audio-3.1-asr-flash-message`, `qwen-audio-3.1-asr-flash-streaming` | native stream (client unverified) | same | same | `stt_client_unverified` (notice) |
|  | `*-filetrans*`, `fun-asr-202*`, `fun-asr-2025-08-25`, `fun-asr-2025-11-07`, `fun-asr-mtl`, `fun-asr-mtl-2025-08-25`, `paraformer-mtl-v1`, `qwen-audio-3.0-asr-flash-filetrans`, `qwen-audio-3.1-asr-flash-filetrans`, `qwen3-asr-flash-filetrans`, `qwen3-asr-flash-filetrans-2025-11-17`, `sensevoice-v1` | refused `stt_live_unsupported` (`async_only`) | same | same |  |
|  | `fun-asr` | today's client, streaming fun-asr-realtime instead | same | same | `stt_model_substituted` (notice) |
|  | `fun-asr-flash-2*`, `fun-asr-flash-2026-06-15`, `qwen-audio-3.0-asr-flash`, `qwen-audio-3.1-asr-flash`, `qwen3-asr-flash`, `qwen3-asr-flash-2*`, `qwen3-asr-flash-2025-09-08`, `qwen3-asr-flash-2026-02-10` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
|  | `fun-asr-mtl-realtime` | native stream (client unverified) | same | same | `stt_client_unverified` (notice), `stt_model_deprecated` (notice) |
|  | `fun-asr-realtime`, `fun-asr-realtime-2026-02-28`, `paraformer-realtime-8k-v1`, `paraformer-realtime-8k-v2`, `paraformer-realtime-v1`, `paraformer-realtime-v2` | native stream | same | same |  |
|  | `fun-asr-realtime-2025-09-15`, `fun-asr-realtime-2025-11-07` | native stream | same | same | `stt_model_deprecated` (notice) |
|  | `paraformer-8k-v1` | today's client, streaming paraformer-realtime-8k-v1 instead | same | same | `stt_model_substituted` (notice) |
|  | `paraformer-8k-v2` | today's client, streaming paraformer-realtime-8k-v2 instead | same | same | `stt_model_substituted` (notice) |
|  | `paraformer-v1` | today's client, streaming paraformer-realtime-v1 instead | same | same | `stt_model_substituted` (notice) |
|  | `paraformer-v2` | today's client, streaming paraformer-realtime-v2 instead | same | same | `stt_model_substituted` (notice) |
|  | `qwen3-asr-flash-realtime` (default), `qwen3-asr-flash-realtime*` | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
|  | `qwen3-asr-flash-realtime-2025-10-27`, `qwen3-asr-flash-realtime-2026-02-10` | native stream (client known broken) | same | same | `stt_client_unverified` (notice), `stt_model_deprecated` (notice) |
| `amivoice` | any other id (`*`) | native stream (client known broken) | same | same | `stt_client_unverified` (notice), `stt_model_substituted` (notice), `stt_capability_assumed` (notice) |
|  | `-a-address-input-private`, `-a-bizfinance`, `-a-bizfinance-input`, `-a-bizinsurance`, `-a-bizinsurance-input`, `-a-name-input-private`, `-a-rule-input-private` | today's client, streaming -a-general instead | same | same | `stt_model_substituted` (notice) |
|  | `-a-general` (default), `-a-general-en`, `-a-general-input`, `-a-general-ko`, `-a-general-zh`, `-a-medical`, `-a-medical-input`, `-a2-ja-general`, `-a2-multi-general`, `-a2-zh-general` and 3 more | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
| `assemblyai` | any other id (`*`), `u3-pro`, `u3-sync-pro`, `universal-2`, `universal-3-5-pro`, `universal-3-6-pro` | today's client, streaming universal-streaming-english instead | same | same | `stt_model_substituted` (notice) |
|  | `universal-streaming-english` (default), `universal-streaming-multilingual` | native stream | same | same |  |
| `aws-transcribe` | any other id (`*`) | native stream | same | same | `stt_model_substituted` (notice), `stt_capability_assumed` (notice) |
|  | `call-analytics`, `healthscribe`, `medical` | today's client, streaming standard instead | same | same | `stt_model_substituted` (notice) |
|  | `standard` (default) | native stream | same | same |  |
| `azure_openai` | any other id (`*`) | per-utterance upload (`azure_openai_transcriptions`) | same | same | `stt_segmented_mode`, `stt_capability_assumed` |
|  | `gpt-4o-mini-transcribe`, `gpt-4o-transcribe`, `gpt-4o-transcribe-diarize`, `whisper`, `whisper-*` | per-utterance upload (`azure_openai_transcriptions`) | same | same | `stt_segmented_mode`, `stt_model_deprecated` |
|  | `gpt-live-transcribe`, `gpt-realtime-whisper` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
|  | `gpt-transcribe` | per-utterance upload (`azure_openai_transcriptions`) | same | same | `stt_segmented_mode` |
| `baidu` | any other id (`*`) | native stream (client known broken) | same | same | `stt_client_unverified` (notice), `stt_model_substituted` (notice), `stt_capability_assumed` (notice) |
|  | `1537` (default), `15372`, `15376`, `1637`, `1737`, `1837` | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
|  | `1936` | refused `stt_model_retired` | same | same |  |
|  | `19362`, `80001`, `8001` | today's client, streaming 1537 instead | same | same | `stt_model_substituted` (notice) |
| `bhashini` | any other id (`*`), `ai4bharat/conformer-hi-gpu--t4`, `ai4bharat/conformer-multilingual-dravidian-gpu--t4`, `ai4bharat/conformer-multilingual-indo_aryan-gpu--t4`, `ai4bharat/whisper-medium-en--gpu--t4`, `bhashini/ai4bharat/conformer-multilingual-asr`, `bhashini/bodhan/asr-transcribe-core`, `bhashini/bodhan/asr-transcribe-flex`, `bhashini/iisc/asr-bho-t4`, `bhashini/iisc/asr-mai-t4` and 3 more | refused `stt_live_unsupported` (`client_not_implemented`) | today's buffering client | today's buffering client | voice agent: none; manual agent or loop: `stt_buffered_until_commit`; plain: `stt_buffered_until_commit` |
| `cartesia` | any other id (`*`) | native stream | same | same | `stt_capability_assumed` (notice) |
|  | `ink-2`, `ink-preview`, `ink-whisper` (default), `ink-whisper-2025-06-04` | native stream | same | same |  |
| `deepgram` | any other id (`*`) | native stream | same | same | `stt_capability_assumed` (notice) |
|  | `base`, `base-general` (default), `conversationalai`, `enhanced`, `enhanced-finance`, `enhanced-general`, `enhanced-meeting`, `enhanced-phonecall`, `finance`, `meeting` and 22 more | native stream | same | same |  |
|  | `flux-*`, `flux-general-en`, `flux-general-multi`, `whisper`, `whisper-*`, `whisper-base`, `whisper-large`, `whisper-medium`, `whisper-small`, `whisper-tiny` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
| `elevenlabs` | any other id (`*`) | per-utterance upload (`elevenlabs_batch`) | same | same | `stt_segmented_mode`, `stt_capability_assumed` |
|  | `*realtime*` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
|  | `scribe_v1` | refused `stt_model_retired` | same | same |  |
|  | `scribe_v2`, `scribe_v2_medical` | per-utterance upload (`elevenlabs_batch`) | same | same | `stt_segmented_mode` |
|  | `scribe_v2_realtime` (default) | native stream | same | same |  |
| `fpt-ai` | any other id (`*`), `general` (default) | refused `stt_live_unsupported` (`client_not_implemented`) | today's buffering client | today's buffering client | voice agent: none; manual agent or loop: `stt_buffered_until_commit`; plain: `stt_buffered_until_commit` |
| `gladia` | any other id (`*`), `solaria-3` | today's client, streaming solaria-1 instead | same | same | `stt_model_substituted` (notice) |
|  | `solaria-1` (default) | native stream | same | same |  |
| `gnani` | any other id (`*`) | today's client (behaviour for this model not established) | same | same | `stt_client_unverified` (notice), `stt_capability_assumed` (notice) |
|  | `default` | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
|  | `gnani-prisma-v2.5` (default), `vachana-audio-intelligence-v2` | today's client (behaviour for this model not established) | same | same | `stt_client_unverified` (notice) |
| `google` | any other id (`*`) | native stream | same | same | `stt_capability_assumed` (notice) |
|  | `chirp` | native stream (client unverified) | same | same | `stt_client_unverified` (notice) |
|  | `chirp_2`, `chirp_3`, `chirp_telephony`, `latest_long` (default), `long`, `medical_conversation`, `medical_dictation`, `short`, `telephony`, `telephony_short` | native stream | same | same |  |
|  | `latest_short` | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
| `groq` | any other id (`*`) | per-utterance upload (`groq_transcriptions`) | same | today's buffering client | voice agent: `stt_segmented_mode`, `stt_min_billed_duration`, `stt_capability_assumed`; manual agent or loop: `stt_segmented_mode`, `stt_min_billed_duration`, `stt_capability_assumed`; plain: `stt_buffered_until_commit` |
|  | `distil-whisper-large-v3-en` | refused `stt_model_retired` | same | same |  |
|  | `whisper-large-v3`, `whisper-large-v3-turbo` (default) | per-utterance upload (`groq_transcriptions`) | same | today's buffering client | voice agent: `stt_segmented_mode`, `stt_min_billed_duration`; manual agent or loop: `stt_segmented_mode`, `stt_min_billed_duration`; plain: `stt_buffered_until_commit` |
| `huawei-cloud` | any other id (`*`) | native stream (client known broken) | same | same | `stt_client_unverified` (notice), `stt_model_substituted` (notice), `stt_capability_assumed` (notice) |
|  | `arabic_16k_general`, `arabic_8k_general`, `cantonese_16k_common`, `chinese_16k_common`, `chinese_16k_court`, `chinese_16k_general` (default), `chinese_16k_it`, `chinese_8k_common`, `chinese_8k_general`, `english_16k_general`, `shanghai_16k_common`, `sichuan_16k_common` | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
|  | `chinese_16k_conversation`, `chinese_16k_travel`, `english_16k_common`, `english_8k_common` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
|  | `chinese_16k_media`, `sichuan_8k_common` | refused `stt_live_unsupported` (`async_only`) | same | same |  |
| `ibm-watson` | any other id (`*`) | native stream | same | same | `stt_capability_assumed` (notice) |
|  | `*_BroadbandModel`, `*_NarrowbandModel` | refused `stt_model_retired` | same | same |  |
|  | `ar-MS_Telephony`, `cs-CZ_Telephony`, `de-DE_Multimedia`, `de-DE_Telephony`, `en-AU_Multimedia`, `en-AU_Telephony`, `en-GB_Multimedia`, `en-GB_Telephony`, `en-IN_Telephony`, `en-US_Multimedia` (default) and 23 more | native stream | same | same |  |
|  | `de-DE`, `en-AU`, `en-GB`, `en-IN`, `en-US`, `es-AR`, `es-CL`, `es-CO`, `es-ES`, `es-MX` and 8 more | native stream (client unverified) | same | same | `stt_client_unverified` (notice) |
| `iflytek` | any other id (`*`) | native stream (client known broken) | same | same | `stt_client_unverified` (notice), `stt_capability_assumed` (notice) |
|  | `*ist*`, `*realtime*`, `*stream*`, `iat` (default), `ist`, `ist_huanyu`, `ist_hy`, `ist_open`, `medical`, `sp_ist_vais` | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
|  | `iflyrec_voice_*`, `iflyrec_voice_cn_10m_ed`, `iflyrec_voice_de_de_24h`, `iflyrec_voice_es_es_24h`, `iflyrec_voice_fr_fr_24h`, `iflyrec_voice_ja_jp_24h`, `iflyrec_voice_ko_kr_24h`, `iflyrec_voice_th_th_sp_24h`, `iflyrec_voice_vi_vn_vais_24h`, `iflyrec_voice_yueyu_24h` | today's client, streaming iat instead | same | same | `stt_model_substituted` (notice) |
| `microsoft-azure` | any other id (`*`) | native stream | same | same | `stt_model_substituted` (notice), `stt_capability_assumed` (notice) |
|  | `default` (default) | native stream | same | same |  |
|  | `llm-speech`, `MAI-Transcribe-1`, `MAI-Transcribe-1.5`, `MAI-Transcribe-2`, `MAI-Transcribe-2-Streaming` | today's client, streaming default instead | same | same | `stt_model_substituted` (notice) |
| `naver-clova` | any other id (`*`), `clova-speech-long`, `clova-speech-short`, `clova-speech-streaming`, `csr` (default) | refused `stt_live_unsupported` (`client_not_implemented`) | today's buffering client | today's buffering client | voice agent: none; manual agent or loop: `stt_buffered_until_commit`; plain: `stt_buffered_until_commit` |
| `nectec` | any other id (`*`) | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
|  | `partii4` (default) | refused `stt_live_unsupported` (`client_not_implemented`) | today's buffering client | today's buffering client | voice agent: none; manual agent or loop: `stt_buffered_until_commit`; plain: `stt_buffered_until_commit` |
|  | `partii5` | refused `stt_model_retired` | same | same |  |
| `openai` | any other id (`*`) | per-utterance upload (`openai_transcriptions`) | same | today's buffering client | voice agent: `stt_segmented_mode`, `stt_capability_assumed`; manual agent or loop: `stt_segmented_mode`, `stt_capability_assumed`; plain: `stt_buffered_until_commit` |
|  | `gpt-4o-mini-transcribe`, `gpt-4o-mini-transcribe-2025-03-20`, `gpt-4o-mini-transcribe-2025-12-15`, `gpt-4o-transcribe`, `gpt-4o-transcribe-diarize`, `whisper-1` | per-utterance upload (`openai_transcriptions`) | same | today's buffering client | voice agent: `stt_segmented_mode`, `stt_model_deprecated`; manual agent or loop: `stt_segmented_mode`, `stt_model_deprecated`; plain: `stt_buffered_until_commit`, `stt_model_deprecated` (notice) |
|  | `gpt-live-transcribe`, `gpt-live-transcribe*`, `gpt-realtime-whisper`, `gpt-realtime-whisper*` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
|  | `gpt-transcribe` (default) | per-utterance upload (`openai_transcriptions`) | same | today's buffering client | voice agent: `stt_segmented_mode`; manual agent or loop: `stt_segmented_mode`; plain: `stt_buffered_until_commit` |
| `phonexia` | any other id (`*`), `default`, `large_v2`, `large_v3`, `medium`, `speech-to-text`, `speech-to-text-whisper-enhanced` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
|  | `EN_US_6` | refused `stt_model_retired` | same | same |  |
| `revai` | any other id (`*`) | native stream | same | same | `stt_model_substituted` (notice), `stt_capability_assumed` (notice) |
|  | `*whisper*` | refused `stt_live_unsupported` (`async_only`) | same | same |  |
|  | `fusion`, `low_cost` | today's client, streaming machine instead | same | same | `stt_model_substituted` (notice), `stt_model_deprecated` (notice) |
|  | `human` | today's client (behaviour for this model not established) | same | same | `stt_client_unverified` (notice) |
|  | `machine` (default), `machine_v2`, `reverb`, `reverb-foreign-language` | native stream | same | same |  |
| `reverie` | any other id (`*`) | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
| `sarvam` | any other id (`*`) | native stream | same | same | `stt_capability_assumed` (notice) |
|  | `saaras:v2.5`, `saaras:v3-realtime` | today's client (behaviour for this model not established) | same | same | `stt_client_unverified` (notice) |
|  | `saaras:v3`, `saaras:v4` | native stream | same | same |  |
|  | `saarika:v2.5` (default) | native stream | same | same | `stt_model_deprecated` (notice) |
| `sberdevices` | any other id (`*`), `callcenter`, `general` (default), `ivr`, `media` | today's timed-upload client (known broken) | same | same | `stt_client_unverified` (notice) |
| `self_hosted` | any other id (`*`), `*nemotron*`, `*voxtral*realtime*`, `*whisper*` | per-utterance upload (`openai_transcriptions`) | same | same | `stt_segmented_mode` |
|  | `kyutai/stt-1b-en_fr`, `kyutai/stt-2.6b-en` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
| `speechmatics` | any other id (`*`) | native stream | same | same | `stt_capability_assumed` (notice) |
|  | `enhanced`, `standard` (default) | native stream | same | same |  |
|  | `linden-1`, `melia-1`, `oak-1` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
| `tencent` | any other id (`*`) | native stream (client known broken) | same | same | `stt_client_unverified` (notice), `stt_model_substituted` (notice), `stt_capability_assumed` (notice) |
|  | `16k_ar`, `16k_de`, `16k_en`, `16k_en_edu`, `16k_en_game`, `16k_en_large`, `16k_es`, `16k_fil`, `16k_fr`, `16k_hi` and 23 more | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
|  | `16k_zh-PY`, `16k_zh_dialect`, `16k_zh_en_meeting` | today's client, streaming 16k_zh instead | same | same | `stt_model_substituted` (notice) |
| `tinkoff` | any other id (`*`) | native stream (client known broken) | same | same | `stt_client_unverified` (notice), `stt_model_substituted` (notice), `stt_capability_assumed` (notice) |
|  | `default` (default) | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
| `viettel-ai` | any other id (`*`) | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
| `waav-infer` | any other id (`*`) | per-utterance upload (`openai_transcriptions`) | same | same | `stt_segmented_mode`, `stt_capability_assumed` |
|  | `nemotron*`, `voxtral*realtime*`, `whisper*` | per-utterance upload (`openai_transcriptions`) | same | same | `stt_segmented_mode` |
| `yandex` | any other id (`*`), `deferred-general`, `deferred-general:deprecated`, `deferred-general:rc`, `general` (default), `general:deprecated`, `general:rc` | today's timed-upload client (known broken) | same | same | `stt_client_unverified` (notice) |
|  | `deferred*` | today's client (behaviour for this model not established) | same | same | `stt_client_unverified` (notice) |

## Release 2, dark launch complete

Outcome changed from the previous release for 0 row(s).

| Provider | Models | Voice agent, automatic turns | Manual agent, conversation loop or DAG | Plain /ws session | Warnings |
| --- | --- | --- | --- | --- | --- |
| unknown provider (global default) | any other id (`*`) | today's client through the plugin registry (unclassified provider) | same | same |  |
| `alibaba-cloud` | any other id (`*`) | native stream (client unverified) | same | same | `stt_client_unverified` (notice), `stt_capability_assumed` (notice) |
|  | `*-asr-flash-message*`, `*-asr-flash-streaming*`, `*-realtime*`, `fun-asr-flash-8k-realtime`, `fun-asr-flash-8k-realtime-2026-01-28`, `fun-asr-mtl-realtime-2025-12-10`, `gummy-chat-v1`, `gummy-realtime-v1`, `qwen-audio-3.0-asr-flash-streaming`, `qwen-audio-3.1-asr-flash-message`, `qwen-audio-3.1-asr-flash-streaming` | native stream (client unverified) | same | same | `stt_client_unverified` (notice) |
|  | `*-filetrans*`, `fun-asr-202*`, `fun-asr-2025-08-25`, `fun-asr-2025-11-07`, `fun-asr-mtl`, `fun-asr-mtl-2025-08-25`, `paraformer-mtl-v1`, `qwen-audio-3.0-asr-flash-filetrans`, `qwen-audio-3.1-asr-flash-filetrans`, `qwen3-asr-flash-filetrans`, `qwen3-asr-flash-filetrans-2025-11-17`, `sensevoice-v1` | refused `stt_live_unsupported` (`async_only`) | same | same |  |
|  | `fun-asr` | today's client, streaming fun-asr-realtime instead | same | same | `stt_model_substituted` (notice) |
|  | `fun-asr-flash-2*`, `fun-asr-flash-2026-06-15`, `qwen-audio-3.0-asr-flash`, `qwen-audio-3.1-asr-flash`, `qwen3-asr-flash`, `qwen3-asr-flash-2*`, `qwen3-asr-flash-2025-09-08`, `qwen3-asr-flash-2026-02-10` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
|  | `fun-asr-mtl-realtime` | native stream (client unverified) | same | same | `stt_client_unverified` (notice), `stt_model_deprecated` (notice) |
|  | `fun-asr-realtime`, `fun-asr-realtime-2026-02-28`, `paraformer-realtime-8k-v1`, `paraformer-realtime-8k-v2`, `paraformer-realtime-v1`, `paraformer-realtime-v2` | native stream | same | same |  |
|  | `fun-asr-realtime-2025-09-15`, `fun-asr-realtime-2025-11-07` | native stream | same | same | `stt_model_deprecated` (notice) |
|  | `paraformer-8k-v1` | today's client, streaming paraformer-realtime-8k-v1 instead | same | same | `stt_model_substituted` (notice) |
|  | `paraformer-8k-v2` | today's client, streaming paraformer-realtime-8k-v2 instead | same | same | `stt_model_substituted` (notice) |
|  | `paraformer-v1` | today's client, streaming paraformer-realtime-v1 instead | same | same | `stt_model_substituted` (notice) |
|  | `paraformer-v2` | today's client, streaming paraformer-realtime-v2 instead | same | same | `stt_model_substituted` (notice) |
|  | `qwen3-asr-flash-realtime` (default), `qwen3-asr-flash-realtime*` | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
|  | `qwen3-asr-flash-realtime-2025-10-27`, `qwen3-asr-flash-realtime-2026-02-10` | native stream (client known broken) | same | same | `stt_client_unverified` (notice), `stt_model_deprecated` (notice) |
| `amivoice` | any other id (`*`) | native stream (client known broken) | same | same | `stt_client_unverified` (notice), `stt_model_substituted` (notice), `stt_capability_assumed` (notice) |
|  | `-a-address-input-private`, `-a-bizfinance`, `-a-bizfinance-input`, `-a-bizinsurance`, `-a-bizinsurance-input`, `-a-name-input-private`, `-a-rule-input-private` | today's client, streaming -a-general instead | same | same | `stt_model_substituted` (notice) |
|  | `-a-general` (default), `-a-general-en`, `-a-general-input`, `-a-general-ko`, `-a-general-zh`, `-a-medical`, `-a-medical-input`, `-a2-ja-general`, `-a2-multi-general`, `-a2-zh-general` and 3 more | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
| `assemblyai` | any other id (`*`), `u3-pro`, `u3-sync-pro`, `universal-2`, `universal-3-5-pro`, `universal-3-6-pro` | today's client, streaming universal-streaming-english instead | same | same | `stt_model_substituted` (notice) |
|  | `universal-streaming-english` (default), `universal-streaming-multilingual` | native stream | same | same |  |
| `aws-transcribe` | any other id (`*`) | native stream | same | same | `stt_model_substituted` (notice), `stt_capability_assumed` (notice) |
|  | `call-analytics`, `healthscribe`, `medical` | today's client, streaming standard instead | same | same | `stt_model_substituted` (notice) |
|  | `standard` (default) | native stream | same | same |  |
| `azure_openai` | any other id (`*`) | per-utterance upload (`azure_openai_transcriptions`) | same | same | `stt_segmented_mode`, `stt_capability_assumed` |
|  | `gpt-4o-mini-transcribe`, `gpt-4o-transcribe`, `gpt-4o-transcribe-diarize`, `whisper`, `whisper-*` | per-utterance upload (`azure_openai_transcriptions`) | same | same | `stt_segmented_mode`, `stt_model_deprecated` |
|  | `gpt-live-transcribe`, `gpt-realtime-whisper` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
|  | `gpt-transcribe` | per-utterance upload (`azure_openai_transcriptions`) | same | same | `stt_segmented_mode` |
| `baidu` | any other id (`*`) | native stream (client known broken) | same | same | `stt_client_unverified` (notice), `stt_model_substituted` (notice), `stt_capability_assumed` (notice) |
|  | `1537` (default), `15372`, `15376`, `1637`, `1737`, `1837` | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
|  | `1936` | refused `stt_model_retired` | same | same |  |
|  | `19362`, `80001`, `8001` | today's client, streaming 1537 instead | same | same | `stt_model_substituted` (notice) |
| `bhashini` | any other id (`*`), `ai4bharat/conformer-hi-gpu--t4`, `ai4bharat/conformer-multilingual-dravidian-gpu--t4`, `ai4bharat/conformer-multilingual-indo_aryan-gpu--t4`, `ai4bharat/whisper-medium-en--gpu--t4`, `bhashini/ai4bharat/conformer-multilingual-asr`, `bhashini/bodhan/asr-transcribe-core`, `bhashini/bodhan/asr-transcribe-flex`, `bhashini/iisc/asr-bho-t4`, `bhashini/iisc/asr-mai-t4` and 3 more | refused `stt_live_unsupported` (`client_not_implemented`) | today's buffering client | today's buffering client | voice agent: none; manual agent or loop: `stt_buffered_until_commit`; plain: `stt_buffered_until_commit` |
| `cartesia` | any other id (`*`) | native stream | same | same | `stt_capability_assumed` (notice) |
|  | `ink-2`, `ink-preview`, `ink-whisper` (default), `ink-whisper-2025-06-04` | native stream | same | same |  |
| `deepgram` | any other id (`*`) | native stream | same | same | `stt_capability_assumed` (notice) |
|  | `base`, `base-general` (default), `conversationalai`, `enhanced`, `enhanced-finance`, `enhanced-general`, `enhanced-meeting`, `enhanced-phonecall`, `finance`, `meeting` and 22 more | native stream | same | same |  |
|  | `flux-*`, `flux-general-en`, `flux-general-multi`, `whisper`, `whisper-*`, `whisper-base`, `whisper-large`, `whisper-medium`, `whisper-small`, `whisper-tiny` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
| `elevenlabs` | any other id (`*`) | per-utterance upload (`elevenlabs_batch`) | same | same | `stt_segmented_mode`, `stt_capability_assumed` |
|  | `*realtime*` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
|  | `scribe_v1` | refused `stt_model_retired` | same | same |  |
|  | `scribe_v2`, `scribe_v2_medical` | per-utterance upload (`elevenlabs_batch`) | same | same | `stt_segmented_mode` |
|  | `scribe_v2_realtime` (default) | native stream | same | same |  |
| `fpt-ai` | any other id (`*`), `general` (default) | refused `stt_live_unsupported` (`client_not_implemented`) | today's buffering client | today's buffering client | voice agent: none; manual agent or loop: `stt_buffered_until_commit`; plain: `stt_buffered_until_commit` |
| `gladia` | any other id (`*`), `solaria-3` | today's client, streaming solaria-1 instead | same | same | `stt_model_substituted` (notice) |
|  | `solaria-1` (default) | native stream | same | same |  |
| `gnani` | any other id (`*`) | today's client (behaviour for this model not established) | same | same | `stt_client_unverified` (notice), `stt_capability_assumed` (notice) |
|  | `default` | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
|  | `gnani-prisma-v2.5` (default), `vachana-audio-intelligence-v2` | today's client (behaviour for this model not established) | same | same | `stt_client_unverified` (notice) |
| `google` | any other id (`*`) | native stream | same | same | `stt_capability_assumed` (notice) |
|  | `chirp` | native stream (client unverified) | same | same | `stt_client_unverified` (notice) |
|  | `chirp_2`, `chirp_3`, `chirp_telephony`, `latest_long` (default), `long`, `medical_conversation`, `medical_dictation`, `short`, `telephony`, `telephony_short` | native stream | same | same |  |
|  | `latest_short` | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
| `groq` | any other id (`*`) | per-utterance upload (`groq_transcriptions`) | same | today's buffering client | voice agent: `stt_segmented_mode`, `stt_min_billed_duration`, `stt_capability_assumed`; manual agent or loop: `stt_segmented_mode`, `stt_min_billed_duration`, `stt_capability_assumed`; plain: `stt_buffered_until_commit` |
|  | `distil-whisper-large-v3-en` | refused `stt_model_retired` | same | same |  |
|  | `whisper-large-v3`, `whisper-large-v3-turbo` (default) | per-utterance upload (`groq_transcriptions`) | same | today's buffering client | voice agent: `stt_segmented_mode`, `stt_min_billed_duration`; manual agent or loop: `stt_segmented_mode`, `stt_min_billed_duration`; plain: `stt_buffered_until_commit` |
| `huawei-cloud` | any other id (`*`) | native stream (client known broken) | same | same | `stt_client_unverified` (notice), `stt_model_substituted` (notice), `stt_capability_assumed` (notice) |
|  | `arabic_16k_general`, `arabic_8k_general`, `cantonese_16k_common`, `chinese_16k_common`, `chinese_16k_court`, `chinese_16k_general` (default), `chinese_16k_it`, `chinese_8k_common`, `chinese_8k_general`, `english_16k_general`, `shanghai_16k_common`, `sichuan_16k_common` | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
|  | `chinese_16k_conversation`, `chinese_16k_travel`, `english_16k_common`, `english_8k_common` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
|  | `chinese_16k_media`, `sichuan_8k_common` | refused `stt_live_unsupported` (`async_only`) | same | same |  |
| `ibm-watson` | any other id (`*`) | native stream | same | same | `stt_capability_assumed` (notice) |
|  | `*_BroadbandModel`, `*_NarrowbandModel` | refused `stt_model_retired` | same | same |  |
|  | `ar-MS_Telephony`, `cs-CZ_Telephony`, `de-DE_Multimedia`, `de-DE_Telephony`, `en-AU_Multimedia`, `en-AU_Telephony`, `en-GB_Multimedia`, `en-GB_Telephony`, `en-IN_Telephony`, `en-US_Multimedia` (default) and 23 more | native stream | same | same |  |
|  | `de-DE`, `en-AU`, `en-GB`, `en-IN`, `en-US`, `es-AR`, `es-CL`, `es-CO`, `es-ES`, `es-MX` and 8 more | native stream (client unverified) | same | same | `stt_client_unverified` (notice) |
| `iflytek` | any other id (`*`) | native stream (client known broken) | same | same | `stt_client_unverified` (notice), `stt_capability_assumed` (notice) |
|  | `*ist*`, `*realtime*`, `*stream*`, `iat` (default), `ist`, `ist_huanyu`, `ist_hy`, `ist_open`, `medical`, `sp_ist_vais` | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
|  | `iflyrec_voice_*`, `iflyrec_voice_cn_10m_ed`, `iflyrec_voice_de_de_24h`, `iflyrec_voice_es_es_24h`, `iflyrec_voice_fr_fr_24h`, `iflyrec_voice_ja_jp_24h`, `iflyrec_voice_ko_kr_24h`, `iflyrec_voice_th_th_sp_24h`, `iflyrec_voice_vi_vn_vais_24h`, `iflyrec_voice_yueyu_24h` | today's client, streaming iat instead | same | same | `stt_model_substituted` (notice) |
| `microsoft-azure` | any other id (`*`) | native stream | same | same | `stt_model_substituted` (notice), `stt_capability_assumed` (notice) |
|  | `default` (default) | native stream | same | same |  |
|  | `llm-speech`, `MAI-Transcribe-1`, `MAI-Transcribe-1.5`, `MAI-Transcribe-2`, `MAI-Transcribe-2-Streaming` | today's client, streaming default instead | same | same | `stt_model_substituted` (notice) |
| `naver-clova` | any other id (`*`), `clova-speech-long`, `clova-speech-short`, `clova-speech-streaming`, `csr` (default) | refused `stt_live_unsupported` (`client_not_implemented`) | today's buffering client | today's buffering client | voice agent: none; manual agent or loop: `stt_buffered_until_commit`; plain: `stt_buffered_until_commit` |
| `nectec` | any other id (`*`) | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
|  | `partii4` (default) | refused `stt_live_unsupported` (`client_not_implemented`) | today's buffering client | today's buffering client | voice agent: none; manual agent or loop: `stt_buffered_until_commit`; plain: `stt_buffered_until_commit` |
|  | `partii5` | refused `stt_model_retired` | same | same |  |
| `openai` | any other id (`*`) | per-utterance upload (`openai_transcriptions`) | same | today's buffering client | voice agent: `stt_segmented_mode`, `stt_capability_assumed`; manual agent or loop: `stt_segmented_mode`, `stt_capability_assumed`; plain: `stt_buffered_until_commit` |
|  | `gpt-4o-mini-transcribe`, `gpt-4o-mini-transcribe-2025-03-20`, `gpt-4o-mini-transcribe-2025-12-15`, `gpt-4o-transcribe`, `gpt-4o-transcribe-diarize`, `whisper-1` | per-utterance upload (`openai_transcriptions`) | same | today's buffering client | voice agent: `stt_segmented_mode`, `stt_model_deprecated`; manual agent or loop: `stt_segmented_mode`, `stt_model_deprecated`; plain: `stt_buffered_until_commit`, `stt_model_deprecated` (notice) |
|  | `gpt-live-transcribe`, `gpt-live-transcribe*`, `gpt-realtime-whisper`, `gpt-realtime-whisper*` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
|  | `gpt-transcribe` (default) | per-utterance upload (`openai_transcriptions`) | same | today's buffering client | voice agent: `stt_segmented_mode`; manual agent or loop: `stt_segmented_mode`; plain: `stt_buffered_until_commit` |
| `phonexia` | any other id (`*`), `default`, `large_v2`, `large_v3`, `medium`, `speech-to-text`, `speech-to-text-whisper-enhanced` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
|  | `EN_US_6` | refused `stt_model_retired` | same | same |  |
| `revai` | any other id (`*`) | native stream | same | same | `stt_model_substituted` (notice), `stt_capability_assumed` (notice) |
|  | `*whisper*` | refused `stt_live_unsupported` (`async_only`) | same | same |  |
|  | `fusion`, `low_cost` | today's client, streaming machine instead | same | same | `stt_model_substituted` (notice), `stt_model_deprecated` (notice) |
|  | `human` | today's client (behaviour for this model not established) | same | same | `stt_client_unverified` (notice) |
|  | `machine` (default), `machine_v2`, `reverb`, `reverb-foreign-language` | native stream | same | same |  |
| `reverie` | any other id (`*`) | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
| `sarvam` | any other id (`*`) | native stream | same | same | `stt_capability_assumed` (notice) |
|  | `saaras:v2.5`, `saaras:v3-realtime` | today's client (behaviour for this model not established) | same | same | `stt_client_unverified` (notice) |
|  | `saaras:v3`, `saaras:v4` | native stream | same | same |  |
|  | `saarika:v2.5` (default) | native stream | same | same | `stt_model_deprecated` (notice) |
| `sberdevices` | any other id (`*`), `callcenter`, `general` (default), `ivr`, `media` | today's timed-upload client (known broken) | same | same | `stt_client_unverified` (notice) |
| `self_hosted` | any other id (`*`), `*nemotron*`, `*voxtral*realtime*`, `*whisper*` | per-utterance upload (`openai_transcriptions`) | same | same | `stt_segmented_mode` |
|  | `kyutai/stt-1b-en_fr`, `kyutai/stt-2.6b-en` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
| `speechmatics` | any other id (`*`) | native stream | same | same | `stt_capability_assumed` (notice) |
|  | `enhanced`, `standard` (default) | native stream | same | same |  |
|  | `linden-1`, `melia-1`, `oak-1` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
| `tencent` | any other id (`*`) | native stream (client known broken) | same | same | `stt_client_unverified` (notice), `stt_model_substituted` (notice), `stt_capability_assumed` (notice) |
|  | `16k_ar`, `16k_de`, `16k_en`, `16k_en_edu`, `16k_en_game`, `16k_en_large`, `16k_es`, `16k_fil`, `16k_fr`, `16k_hi` and 23 more | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
|  | `16k_zh-PY`, `16k_zh_dialect`, `16k_zh_en_meeting` | today's client, streaming 16k_zh instead | same | same | `stt_model_substituted` (notice) |
| `tinkoff` | any other id (`*`) | native stream (client known broken) | same | same | `stt_client_unverified` (notice), `stt_model_substituted` (notice), `stt_capability_assumed` (notice) |
|  | `default` (default) | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
| `viettel-ai` | any other id (`*`) | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
| `waav-infer` | any other id (`*`) | per-utterance upload (`openai_transcriptions`) | same | same | `stt_segmented_mode`, `stt_capability_assumed` |
|  | `nemotron*`, `voxtral*realtime*`, `whisper*` | per-utterance upload (`openai_transcriptions`) | same | same | `stt_segmented_mode` |
| `yandex` | any other id (`*`), `deferred-general`, `deferred-general:deprecated`, `deferred-general:rc`, `general` (default), `general:deprecated`, `general:rc` | today's timed-upload client (known broken) | same | same | `stt_client_unverified` (notice) |
|  | `deferred*` | today's client (behaviour for this model not established) | same | same | `stt_client_unverified` (notice) |

## Release 3, default on

Outcome changed from the previous release for 38 row(s), of these providers: `alibaba-cloud`, `assemblyai`, `baidu`, `deepgram`, `gladia`, `iflytek`, `microsoft-azure`, `revai`, `sarvam`, `tencent`, `yandex`.

| Provider | Models | Voice agent, automatic turns | Manual agent, conversation loop or DAG | Plain /ws session | Warnings |
| --- | --- | --- | --- | --- | --- |
| unknown provider (global default) | any other id (`*`) | today's client through the plugin registry (unclassified provider) | same | same |  |
| `alibaba-cloud` | any other id (`*`) | native stream (client unverified) | same | same | `stt_client_unverified` (notice), `stt_capability_assumed` (notice) |
|  | `*-asr-flash-message*`, `*-asr-flash-streaming*`, `*-realtime*`, `fun-asr-flash-8k-realtime`, `fun-asr-flash-8k-realtime-2026-01-28`, `fun-asr-mtl-realtime-2025-12-10`, `gummy-chat-v1`, `gummy-realtime-v1`, `qwen-audio-3.0-asr-flash-streaming`, `qwen-audio-3.1-asr-flash-message`, `qwen-audio-3.1-asr-flash-streaming` | native stream (client unverified) | same | same | `stt_client_unverified` (notice) |
|  | `*-filetrans*`, `fun-asr`, `fun-asr-202*`, `fun-asr-2025-08-25`, `fun-asr-2025-11-07`, `fun-asr-mtl`, `fun-asr-mtl-2025-08-25`, `paraformer-8k-v1`, `paraformer-8k-v2`, `paraformer-mtl-v1` and 7 more | refused `stt_live_unsupported` (`async_only`) | same | same |  |
|  | `fun-asr-flash-2*`, `fun-asr-flash-2026-06-15`, `qwen-audio-3.0-asr-flash`, `qwen-audio-3.1-asr-flash`, `qwen3-asr-flash`, `qwen3-asr-flash-2*`, `qwen3-asr-flash-2025-09-08`, `qwen3-asr-flash-2026-02-10` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
|  | `fun-asr-mtl-realtime` | native stream (client unverified) | same | same | `stt_client_unverified` (notice), `stt_model_deprecated` (notice) |
|  | `fun-asr-realtime`, `fun-asr-realtime-2026-02-28`, `paraformer-realtime-8k-v1`, `paraformer-realtime-8k-v2`, `paraformer-realtime-v1`, `paraformer-realtime-v2` | native stream | same | same |  |
|  | `fun-asr-realtime-2025-09-15`, `fun-asr-realtime-2025-11-07` | native stream | same | same | `stt_model_deprecated` (notice) |
|  | `qwen3-asr-flash-realtime` (default), `qwen3-asr-flash-realtime*` | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
|  | `qwen3-asr-flash-realtime-2025-10-27`, `qwen3-asr-flash-realtime-2026-02-10` | native stream (client known broken) | same | same | `stt_client_unverified` (notice), `stt_model_deprecated` (notice) |
| `amivoice` | any other id (`*`) | native stream (client known broken) | same | same | `stt_client_unverified` (notice), `stt_model_substituted` (notice), `stt_capability_assumed` (notice) |
|  | `-a-address-input-private`, `-a-bizfinance`, `-a-bizfinance-input`, `-a-bizinsurance`, `-a-bizinsurance-input`, `-a-name-input-private`, `-a-rule-input-private` | today's client, streaming -a-general instead | same | same | `stt_model_substituted` (notice) |
|  | `-a-general` (default), `-a-general-en`, `-a-general-input`, `-a-general-ko`, `-a-general-zh`, `-a-medical`, `-a-medical-input`, `-a2-ja-general`, `-a2-multi-general`, `-a2-zh-general` and 3 more | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
| `assemblyai` | any other id (`*`), `universal-3-6-pro` | today's client, streaming universal-streaming-english instead | same | same | `stt_model_substituted` (notice) |
|  | `u3-pro`, `u3-sync-pro`, `universal-3-5-pro` | per-utterance upload (`assemblyai_sync`) | same | same | `stt_segmented_mode` |
|  | `universal-2` | refused `stt_live_unsupported` (`async_only`) | same | same |  |
|  | `universal-streaming-english` (default), `universal-streaming-multilingual` | native stream | same | same |  |
| `aws-transcribe` | any other id (`*`) | native stream | same | same | `stt_model_substituted` (notice), `stt_capability_assumed` (notice) |
|  | `call-analytics`, `healthscribe`, `medical` | today's client, streaming standard instead | same | same | `stt_model_substituted` (notice) |
|  | `standard` (default) | native stream | same | same |  |
| `azure_openai` | any other id (`*`) | per-utterance upload (`azure_openai_transcriptions`) | same | same | `stt_segmented_mode`, `stt_capability_assumed` |
|  | `gpt-4o-mini-transcribe`, `gpt-4o-transcribe`, `gpt-4o-transcribe-diarize`, `whisper`, `whisper-*` | per-utterance upload (`azure_openai_transcriptions`) | same | same | `stt_segmented_mode`, `stt_model_deprecated` |
|  | `gpt-live-transcribe`, `gpt-realtime-whisper` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
|  | `gpt-transcribe` | per-utterance upload (`azure_openai_transcriptions`) | same | same | `stt_segmented_mode` |
| `baidu` | any other id (`*`) | native stream (client known broken) | same | same | `stt_client_unverified` (notice), `stt_model_substituted` (notice), `stt_capability_assumed` (notice) |
|  | `1537` (default), `15372`, `15376`, `1637`, `1737`, `1837` | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
|  | `1936`, `19362` | refused `stt_model_retired` | same | same |  |
|  | `80001`, `8001` | today's client, streaming 1537 instead | same | same | `stt_model_substituted` (notice) |
| `bhashini` | any other id (`*`), `ai4bharat/conformer-hi-gpu--t4`, `ai4bharat/conformer-multilingual-dravidian-gpu--t4`, `ai4bharat/conformer-multilingual-indo_aryan-gpu--t4`, `ai4bharat/whisper-medium-en--gpu--t4`, `bhashini/ai4bharat/conformer-multilingual-asr`, `bhashini/bodhan/asr-transcribe-core`, `bhashini/bodhan/asr-transcribe-flex`, `bhashini/iisc/asr-bho-t4`, `bhashini/iisc/asr-mai-t4` and 3 more | refused `stt_live_unsupported` (`client_not_implemented`) | today's buffering client | today's buffering client | voice agent: none; manual agent or loop: `stt_buffered_until_commit`; plain: `stt_buffered_until_commit` |
| `cartesia` | any other id (`*`) | native stream | same | same | `stt_capability_assumed` (notice) |
|  | `ink-2`, `ink-preview`, `ink-whisper` (default), `ink-whisper-2025-06-04` | native stream | same | same |  |
| `deepgram` | any other id (`*`) | native stream | same | same | `stt_capability_assumed` (notice) |
|  | `base`, `base-general` (default), `conversationalai`, `enhanced`, `enhanced-finance`, `enhanced-general`, `enhanced-meeting`, `enhanced-phonecall`, `finance`, `meeting` and 22 more | native stream | same | same |  |
|  | `flux-*`, `flux-general-en`, `flux-general-multi` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
|  | `whisper`, `whisper-*`, `whisper-base`, `whisper-large`, `whisper-medium`, `whisper-small`, `whisper-tiny` | per-utterance upload (`deepgram_prerecorded`) [regions except eu, au, in] | same | same | `stt_segmented_mode` |
| `elevenlabs` | any other id (`*`) | per-utterance upload (`elevenlabs_batch`) | same | same | `stt_segmented_mode`, `stt_capability_assumed` |
|  | `*realtime*` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
|  | `scribe_v1` | refused `stt_model_retired` | same | same |  |
|  | `scribe_v2`, `scribe_v2_medical` | per-utterance upload (`elevenlabs_batch`) | same | same | `stt_segmented_mode` |
|  | `scribe_v2_realtime` (default) | native stream | same | same |  |
| `fpt-ai` | any other id (`*`), `general` (default) | refused `stt_live_unsupported` (`client_not_implemented`) | today's buffering client | today's buffering client | voice agent: none; manual agent or loop: `stt_buffered_until_commit`; plain: `stt_buffered_until_commit` |
| `gladia` | any other id (`*`) | today's client, streaming solaria-1 instead | same | same | `stt_model_substituted` (notice) |
|  | `solaria-1` (default) | native stream | same | same |  |
|  | `solaria-3` | refused `stt_live_unsupported` (`async_only`) | same | same |  |
| `gnani` | any other id (`*`) | today's client (behaviour for this model not established) | same | same | `stt_client_unverified` (notice), `stt_capability_assumed` (notice) |
|  | `default` | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
|  | `gnani-prisma-v2.5` (default), `vachana-audio-intelligence-v2` | today's client (behaviour for this model not established) | same | same | `stt_client_unverified` (notice) |
| `google` | any other id (`*`) | native stream | same | same | `stt_capability_assumed` (notice) |
|  | `chirp` | native stream (client unverified) | same | same | `stt_client_unverified` (notice) |
|  | `chirp_2`, `chirp_3`, `chirp_telephony`, `latest_long` (default), `long`, `medical_conversation`, `medical_dictation`, `short`, `telephony`, `telephony_short` | native stream | same | same |  |
|  | `latest_short` | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
| `groq` | any other id (`*`) | per-utterance upload (`groq_transcriptions`) | same | today's buffering client | voice agent: `stt_segmented_mode`, `stt_min_billed_duration`, `stt_capability_assumed`; manual agent or loop: `stt_segmented_mode`, `stt_min_billed_duration`, `stt_capability_assumed`; plain: `stt_buffered_until_commit` |
|  | `distil-whisper-large-v3-en` | refused `stt_model_retired` | same | same |  |
|  | `whisper-large-v3`, `whisper-large-v3-turbo` (default) | per-utterance upload (`groq_transcriptions`) | same | today's buffering client | voice agent: `stt_segmented_mode`, `stt_min_billed_duration`; manual agent or loop: `stt_segmented_mode`, `stt_min_billed_duration`; plain: `stt_buffered_until_commit` |
| `huawei-cloud` | any other id (`*`) | native stream (client known broken) | same | same | `stt_client_unverified` (notice), `stt_model_substituted` (notice), `stt_capability_assumed` (notice) |
|  | `arabic_16k_general`, `arabic_8k_general`, `cantonese_16k_common`, `chinese_16k_common`, `chinese_16k_court`, `chinese_16k_general` (default), `chinese_16k_it`, `chinese_8k_common`, `chinese_8k_general`, `english_16k_general`, `shanghai_16k_common`, `sichuan_16k_common` | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
|  | `chinese_16k_conversation`, `chinese_16k_travel`, `english_16k_common`, `english_8k_common` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
|  | `chinese_16k_media`, `sichuan_8k_common` | refused `stt_live_unsupported` (`async_only`) | same | same |  |
| `ibm-watson` | any other id (`*`) | native stream | same | same | `stt_capability_assumed` (notice) |
|  | `*_BroadbandModel`, `*_NarrowbandModel` | refused `stt_model_retired` | same | same |  |
|  | `ar-MS_Telephony`, `cs-CZ_Telephony`, `de-DE_Multimedia`, `de-DE_Telephony`, `en-AU_Multimedia`, `en-AU_Telephony`, `en-GB_Multimedia`, `en-GB_Telephony`, `en-IN_Telephony`, `en-US_Multimedia` (default) and 23 more | native stream | same | same |  |
|  | `de-DE`, `en-AU`, `en-GB`, `en-IN`, `en-US`, `es-AR`, `es-CL`, `es-CO`, `es-ES`, `es-MX` and 8 more | native stream (client unverified) | same | same | `stt_client_unverified` (notice) |
| `iflytek` | any other id (`*`) | native stream (client known broken) | same | same | `stt_client_unverified` (notice), `stt_capability_assumed` (notice) |
|  | `*ist*`, `*realtime*`, `*stream*`, `iat` (default), `ist`, `ist_huanyu`, `ist_hy`, `ist_open`, `medical`, `sp_ist_vais` | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
|  | `iflyrec_voice_*`, `iflyrec_voice_cn_10m_ed`, `iflyrec_voice_de_de_24h`, `iflyrec_voice_es_es_24h`, `iflyrec_voice_fr_fr_24h`, `iflyrec_voice_ja_jp_24h`, `iflyrec_voice_ko_kr_24h`, `iflyrec_voice_th_th_sp_24h`, `iflyrec_voice_vi_vn_vais_24h`, `iflyrec_voice_yueyu_24h` | refused `stt_live_unsupported` (`async_only`) | same | same |  |
| `microsoft-azure` | any other id (`*`) | native stream | same | same | `stt_model_substituted` (notice), `stt_capability_assumed` (notice) |
|  | `default` (default) | native stream | same | same |  |
|  | `llm-speech`, `MAI-Transcribe-1.5`, `MAI-Transcribe-2`, `MAI-Transcribe-2-Streaming` | today's client, streaming default instead | same | same | `stt_model_substituted` (notice) |
|  | `MAI-Transcribe-1` | refused `stt_model_retired` | same | same |  |
| `naver-clova` | any other id (`*`), `clova-speech-long`, `clova-speech-short`, `clova-speech-streaming`, `csr` (default) | refused `stt_live_unsupported` (`client_not_implemented`) | today's buffering client | today's buffering client | voice agent: none; manual agent or loop: `stt_buffered_until_commit`; plain: `stt_buffered_until_commit` |
| `nectec` | any other id (`*`) | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
|  | `partii4` (default) | refused `stt_live_unsupported` (`client_not_implemented`) | today's buffering client | today's buffering client | voice agent: none; manual agent or loop: `stt_buffered_until_commit`; plain: `stt_buffered_until_commit` |
|  | `partii5` | refused `stt_model_retired` | same | same |  |
| `openai` | any other id (`*`) | per-utterance upload (`openai_transcriptions`) | same | today's buffering client | voice agent: `stt_segmented_mode`, `stt_capability_assumed`; manual agent or loop: `stt_segmented_mode`, `stt_capability_assumed`; plain: `stt_buffered_until_commit` |
|  | `gpt-4o-mini-transcribe`, `gpt-4o-mini-transcribe-2025-03-20`, `gpt-4o-mini-transcribe-2025-12-15`, `gpt-4o-transcribe`, `gpt-4o-transcribe-diarize`, `whisper-1` | per-utterance upload (`openai_transcriptions`) | same | today's buffering client | voice agent: `stt_segmented_mode`, `stt_model_deprecated`; manual agent or loop: `stt_segmented_mode`, `stt_model_deprecated`; plain: `stt_buffered_until_commit`, `stt_model_deprecated` (notice) |
|  | `gpt-live-transcribe`, `gpt-live-transcribe*`, `gpt-realtime-whisper`, `gpt-realtime-whisper*` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
|  | `gpt-transcribe` (default) | per-utterance upload (`openai_transcriptions`) | same | today's buffering client | voice agent: `stt_segmented_mode`; manual agent or loop: `stt_segmented_mode`; plain: `stt_buffered_until_commit` |
| `phonexia` | any other id (`*`), `default`, `large_v2`, `large_v3`, `medium`, `speech-to-text`, `speech-to-text-whisper-enhanced` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
|  | `EN_US_6` | refused `stt_model_retired` | same | same |  |
| `revai` | any other id (`*`) | native stream | same | same | `stt_model_substituted` (notice), `stt_capability_assumed` (notice) |
|  | `*whisper*`, `fusion`, `low_cost` | refused `stt_live_unsupported` (`async_only`) | same | same |  |
|  | `human` | refused `stt_live_unsupported` (`disabled`) | same | same |  |
|  | `machine` (default), `machine_v2`, `reverb`, `reverb-foreign-language` | native stream | same | same |  |
| `reverie` | any other id (`*`) | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
| `sarvam` | any other id (`*`) | native stream | same | same | `stt_capability_assumed` (notice) |
|  | `saaras:v2.5` | refused `stt_live_unsupported` (`disabled`) | same | same |  |
|  | `saaras:v3`, `saaras:v4` | native stream | same | same |  |
|  | `saaras:v3-realtime` | today's client (behaviour for this model not established) | same | same | `stt_client_unverified` (notice) |
|  | `saarika:v2.5` (default) | native stream | same | same | `stt_model_deprecated` (notice) |
| `sberdevices` | any other id (`*`), `callcenter`, `general` (default), `ivr`, `media` | today's timed-upload client (known broken) | same | same | `stt_client_unverified` (notice) |
| `self_hosted` | any other id (`*`), `*nemotron*`, `*voxtral*realtime*`, `*whisper*` | per-utterance upload (`openai_transcriptions`) | same | same | `stt_segmented_mode` |
|  | `kyutai/stt-1b-en_fr`, `kyutai/stt-2.6b-en` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
| `speechmatics` | any other id (`*`) | native stream | same | same | `stt_capability_assumed` (notice) |
|  | `enhanced`, `standard` (default) | native stream | same | same |  |
|  | `linden-1`, `melia-1`, `oak-1` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
| `tencent` | any other id (`*`) | native stream (client known broken) | same | same | `stt_client_unverified` (notice), `stt_model_substituted` (notice), `stt_capability_assumed` (notice) |
|  | `16k_ar`, `16k_de`, `16k_en`, `16k_en_edu`, `16k_en_game`, `16k_en_large`, `16k_es`, `16k_fil`, `16k_fr`, `16k_hi` and 23 more | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
|  | `16k_zh-PY`, `16k_zh_dialect` | today's client, streaming 16k_zh instead | same | same | `stt_model_substituted` (notice) |
|  | `16k_zh_en_meeting` | refused `stt_live_unsupported` (`async_only`) | same | same |  |
| `tinkoff` | any other id (`*`) | native stream (client known broken) | same | same | `stt_client_unverified` (notice), `stt_model_substituted` (notice), `stt_capability_assumed` (notice) |
|  | `default` (default) | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
| `viettel-ai` | any other id (`*`) | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
| `waav-infer` | any other id (`*`) | per-utterance upload (`openai_transcriptions`) | same | same | `stt_segmented_mode`, `stt_capability_assumed` |
|  | `nemotron*`, `voxtral*realtime*`, `whisper*` | per-utterance upload (`openai_transcriptions`) | same | same | `stt_segmented_mode` |
| `yandex` | any other id (`*`), `general` (default), `general:deprecated`, `general:rc` | today's timed-upload client (known broken) | same | same | `stt_client_unverified` (notice) |
|  | `deferred*`, `deferred-general`, `deferred-general:deprecated`, `deferred-general:rc` | refused `stt_live_unsupported` (`async_only`) | same | same |  |

## Release 4, live-only models and low latency

Outcome changed from the previous release for 11 row(s), of these providers: `azure_openai`, `cartesia`, `openai`.

| Provider | Models | Voice agent, automatic turns | Manual agent, conversation loop or DAG | Plain /ws session | Warnings |
| --- | --- | --- | --- | --- | --- |
| unknown provider (global default) | any other id (`*`) | today's client through the plugin registry (unclassified provider) | same | same |  |
| `alibaba-cloud` | any other id (`*`) | native stream (client unverified) | same | same | `stt_client_unverified` (notice), `stt_capability_assumed` (notice) |
|  | `*-asr-flash-message*`, `*-asr-flash-streaming*`, `*-realtime*`, `fun-asr-flash-8k-realtime`, `fun-asr-flash-8k-realtime-2026-01-28`, `fun-asr-mtl-realtime-2025-12-10`, `gummy-chat-v1`, `gummy-realtime-v1`, `qwen-audio-3.0-asr-flash-streaming`, `qwen-audio-3.1-asr-flash-message`, `qwen-audio-3.1-asr-flash-streaming` | native stream (client unverified) | same | same | `stt_client_unverified` (notice) |
|  | `*-filetrans*`, `fun-asr`, `fun-asr-202*`, `fun-asr-2025-08-25`, `fun-asr-2025-11-07`, `fun-asr-mtl`, `fun-asr-mtl-2025-08-25`, `paraformer-8k-v1`, `paraformer-8k-v2`, `paraformer-mtl-v1` and 7 more | refused `stt_live_unsupported` (`async_only`) | same | same |  |
|  | `fun-asr-flash-2*`, `fun-asr-flash-2026-06-15`, `qwen-audio-3.0-asr-flash`, `qwen-audio-3.1-asr-flash`, `qwen3-asr-flash`, `qwen3-asr-flash-2*`, `qwen3-asr-flash-2025-09-08`, `qwen3-asr-flash-2026-02-10` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
|  | `fun-asr-mtl-realtime` | native stream (client unverified) | same | same | `stt_client_unverified` (notice), `stt_model_deprecated` (notice) |
|  | `fun-asr-realtime`, `fun-asr-realtime-2026-02-28`, `paraformer-realtime-8k-v1`, `paraformer-realtime-8k-v2`, `paraformer-realtime-v1`, `paraformer-realtime-v2` | native stream | same | same |  |
|  | `fun-asr-realtime-2025-09-15`, `fun-asr-realtime-2025-11-07` | native stream | same | same | `stt_model_deprecated` (notice) |
|  | `qwen3-asr-flash-realtime` (default), `qwen3-asr-flash-realtime*` | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
|  | `qwen3-asr-flash-realtime-2025-10-27`, `qwen3-asr-flash-realtime-2026-02-10` | native stream (client known broken) | same | same | `stt_client_unverified` (notice), `stt_model_deprecated` (notice) |
| `amivoice` | any other id (`*`) | native stream (client known broken) | same | same | `stt_client_unverified` (notice), `stt_model_substituted` (notice), `stt_capability_assumed` (notice) |
|  | `-a-address-input-private`, `-a-bizfinance`, `-a-bizfinance-input`, `-a-bizinsurance`, `-a-bizinsurance-input`, `-a-name-input-private`, `-a-rule-input-private` | today's client, streaming -a-general instead | same | same | `stt_model_substituted` (notice) |
|  | `-a-general` (default), `-a-general-en`, `-a-general-input`, `-a-general-ko`, `-a-general-zh`, `-a-medical`, `-a-medical-input`, `-a2-ja-general`, `-a2-multi-general`, `-a2-zh-general` and 3 more | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
| `assemblyai` | any other id (`*`), `universal-3-6-pro` | today's client, streaming universal-streaming-english instead | same | same | `stt_model_substituted` (notice) |
|  | `u3-pro`, `u3-sync-pro`, `universal-3-5-pro` | per-utterance upload (`assemblyai_sync`) | same | same | `stt_segmented_mode` |
|  | `universal-2` | refused `stt_live_unsupported` (`async_only`) | same | same |  |
|  | `universal-streaming-english` (default), `universal-streaming-multilingual` | native stream | same | same |  |
| `aws-transcribe` | any other id (`*`) | native stream | same | same | `stt_model_substituted` (notice), `stt_capability_assumed` (notice) |
|  | `call-analytics`, `healthscribe`, `medical` | today's client, streaming standard instead | same | same | `stt_model_substituted` (notice) |
|  | `standard` (default) | native stream | same | same |  |
| `azure_openai` | any other id (`*`) | per-utterance upload (`azure_openai_transcriptions`) | same | same | `stt_segmented_mode`, `stt_capability_assumed` |
|  | `gpt-4o-mini-transcribe`, `gpt-4o-transcribe`, `gpt-4o-transcribe-diarize`, `whisper`, `whisper-*` | per-utterance upload (`azure_openai_transcriptions`) | same | same | `stt_segmented_mode`, `stt_model_deprecated` |
|  | `gpt-live-transcribe`, `gpt-realtime-whisper` | gateway-driven commit (`openai_realtime_transcription`) [after a live probe] | same | same |  |
|  | `gpt-transcribe` | per-utterance upload (`azure_openai_transcriptions`) | same | same | `stt_segmented_mode` |
| `baidu` | any other id (`*`) | native stream (client known broken) | same | same | `stt_client_unverified` (notice), `stt_model_substituted` (notice), `stt_capability_assumed` (notice) |
|  | `1537` (default), `15372`, `15376`, `1637`, `1737`, `1837` | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
|  | `1936`, `19362` | refused `stt_model_retired` | same | same |  |
|  | `80001`, `8001` | today's client, streaming 1537 instead | same | same | `stt_model_substituted` (notice) |
| `bhashini` | any other id (`*`), `ai4bharat/conformer-hi-gpu--t4`, `ai4bharat/conformer-multilingual-dravidian-gpu--t4`, `ai4bharat/conformer-multilingual-indo_aryan-gpu--t4`, `ai4bharat/whisper-medium-en--gpu--t4`, `bhashini/ai4bharat/conformer-multilingual-asr`, `bhashini/bodhan/asr-transcribe-core`, `bhashini/bodhan/asr-transcribe-flex`, `bhashini/iisc/asr-bho-t4`, `bhashini/iisc/asr-mai-t4` and 3 more | refused `stt_live_unsupported` (`client_not_implemented`) | today's buffering client | today's buffering client | voice agent: none; manual agent or loop: `stt_buffered_until_commit`; plain: `stt_buffered_until_commit` |
| `cartesia` | any other id (`*`) | gateway-driven commit (`cartesia_manual_finalize`) [after a live probe] | same | same | `stt_capability_assumed` |
|  | `ink-2`, `ink-preview`, `ink-whisper` (default), `ink-whisper-2025-06-04` | gateway-driven commit (`cartesia_manual_finalize`) [after a live probe] | same | same |  |
| `deepgram` | any other id (`*`) | native stream | same | same | `stt_capability_assumed` (notice) |
|  | `base`, `base-general` (default), `conversationalai`, `enhanced`, `enhanced-finance`, `enhanced-general`, `enhanced-meeting`, `enhanced-phonecall`, `finance`, `meeting` and 22 more | native stream | same | same |  |
|  | `flux-*`, `flux-general-en`, `flux-general-multi` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
|  | `whisper`, `whisper-*`, `whisper-base`, `whisper-large`, `whisper-medium`, `whisper-small`, `whisper-tiny` | per-utterance upload (`deepgram_prerecorded`) [regions except eu, au, in] | same | same | `stt_segmented_mode` |
| `elevenlabs` | any other id (`*`) | per-utterance upload (`elevenlabs_batch`) | same | same | `stt_segmented_mode`, `stt_capability_assumed` |
|  | `*realtime*` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
|  | `scribe_v1` | refused `stt_model_retired` | same | same |  |
|  | `scribe_v2`, `scribe_v2_medical` | per-utterance upload (`elevenlabs_batch`) | same | same | `stt_segmented_mode` |
|  | `scribe_v2_realtime` (default) | native stream | same | same |  |
| `fpt-ai` | any other id (`*`), `general` (default) | refused `stt_live_unsupported` (`client_not_implemented`) | today's buffering client | today's buffering client | voice agent: none; manual agent or loop: `stt_buffered_until_commit`; plain: `stt_buffered_until_commit` |
| `gladia` | any other id (`*`) | today's client, streaming solaria-1 instead | same | same | `stt_model_substituted` (notice) |
|  | `solaria-1` (default) | native stream | same | same |  |
|  | `solaria-3` | refused `stt_live_unsupported` (`async_only`) | same | same |  |
| `gnani` | any other id (`*`) | today's client (behaviour for this model not established) | same | same | `stt_client_unverified` (notice), `stt_capability_assumed` (notice) |
|  | `default` | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
|  | `gnani-prisma-v2.5` (default), `vachana-audio-intelligence-v2` | today's client (behaviour for this model not established) | same | same | `stt_client_unverified` (notice) |
| `google` | any other id (`*`) | native stream | same | same | `stt_capability_assumed` (notice) |
|  | `chirp` | native stream (client unverified) | same | same | `stt_client_unverified` (notice) |
|  | `chirp_2`, `chirp_3`, `chirp_telephony`, `latest_long` (default), `long`, `medical_conversation`, `medical_dictation`, `short`, `telephony`, `telephony_short` | native stream | same | same |  |
|  | `latest_short` | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
| `groq` | any other id (`*`) | per-utterance upload (`groq_transcriptions`) | same | today's buffering client | voice agent: `stt_segmented_mode`, `stt_min_billed_duration`, `stt_capability_assumed`; manual agent or loop: `stt_segmented_mode`, `stt_min_billed_duration`, `stt_capability_assumed`; plain: `stt_buffered_until_commit` |
|  | `distil-whisper-large-v3-en` | refused `stt_model_retired` | same | same |  |
|  | `whisper-large-v3`, `whisper-large-v3-turbo` (default) | per-utterance upload (`groq_transcriptions`) | same | today's buffering client | voice agent: `stt_segmented_mode`, `stt_min_billed_duration`; manual agent or loop: `stt_segmented_mode`, `stt_min_billed_duration`; plain: `stt_buffered_until_commit` |
| `huawei-cloud` | any other id (`*`) | native stream (client known broken) | same | same | `stt_client_unverified` (notice), `stt_model_substituted` (notice), `stt_capability_assumed` (notice) |
|  | `arabic_16k_general`, `arabic_8k_general`, `cantonese_16k_common`, `chinese_16k_common`, `chinese_16k_court`, `chinese_16k_general` (default), `chinese_16k_it`, `chinese_8k_common`, `chinese_8k_general`, `english_16k_general`, `shanghai_16k_common`, `sichuan_16k_common` | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
|  | `chinese_16k_conversation`, `chinese_16k_travel`, `english_16k_common`, `english_8k_common` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
|  | `chinese_16k_media`, `sichuan_8k_common` | refused `stt_live_unsupported` (`async_only`) | same | same |  |
| `ibm-watson` | any other id (`*`) | native stream | same | same | `stt_capability_assumed` (notice) |
|  | `*_BroadbandModel`, `*_NarrowbandModel` | refused `stt_model_retired` | same | same |  |
|  | `ar-MS_Telephony`, `cs-CZ_Telephony`, `de-DE_Multimedia`, `de-DE_Telephony`, `en-AU_Multimedia`, `en-AU_Telephony`, `en-GB_Multimedia`, `en-GB_Telephony`, `en-IN_Telephony`, `en-US_Multimedia` (default) and 23 more | native stream | same | same |  |
|  | `de-DE`, `en-AU`, `en-GB`, `en-IN`, `en-US`, `es-AR`, `es-CL`, `es-CO`, `es-ES`, `es-MX` and 8 more | native stream (client unverified) | same | same | `stt_client_unverified` (notice) |
| `iflytek` | any other id (`*`) | native stream (client known broken) | same | same | `stt_client_unverified` (notice), `stt_capability_assumed` (notice) |
|  | `*ist*`, `*realtime*`, `*stream*`, `iat` (default), `ist`, `ist_huanyu`, `ist_hy`, `ist_open`, `medical`, `sp_ist_vais` | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
|  | `iflyrec_voice_*`, `iflyrec_voice_cn_10m_ed`, `iflyrec_voice_de_de_24h`, `iflyrec_voice_es_es_24h`, `iflyrec_voice_fr_fr_24h`, `iflyrec_voice_ja_jp_24h`, `iflyrec_voice_ko_kr_24h`, `iflyrec_voice_th_th_sp_24h`, `iflyrec_voice_vi_vn_vais_24h`, `iflyrec_voice_yueyu_24h` | refused `stt_live_unsupported` (`async_only`) | same | same |  |
| `microsoft-azure` | any other id (`*`) | native stream | same | same | `stt_model_substituted` (notice), `stt_capability_assumed` (notice) |
|  | `default` (default) | native stream | same | same |  |
|  | `llm-speech`, `MAI-Transcribe-1.5`, `MAI-Transcribe-2`, `MAI-Transcribe-2-Streaming` | today's client, streaming default instead | same | same | `stt_model_substituted` (notice) |
|  | `MAI-Transcribe-1` | refused `stt_model_retired` | same | same |  |
| `naver-clova` | any other id (`*`), `clova-speech-long`, `clova-speech-short`, `clova-speech-streaming`, `csr` (default) | refused `stt_live_unsupported` (`client_not_implemented`) | today's buffering client | today's buffering client | voice agent: none; manual agent or loop: `stt_buffered_until_commit`; plain: `stt_buffered_until_commit` |
| `nectec` | any other id (`*`) | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
|  | `partii4` (default) | refused `stt_live_unsupported` (`client_not_implemented`) | today's buffering client | today's buffering client | voice agent: none; manual agent or loop: `stt_buffered_until_commit`; plain: `stt_buffered_until_commit` |
|  | `partii5` | refused `stt_model_retired` | same | same |  |
| `openai` | any other id (`*`) | per-utterance upload (`openai_transcriptions`) | same | today's buffering client | voice agent: `stt_segmented_mode`, `stt_capability_assumed`; manual agent or loop: `stt_segmented_mode`, `stt_capability_assumed`; plain: `stt_buffered_until_commit` |
|  | `gpt-4o-mini-transcribe`, `gpt-4o-mini-transcribe-2025-03-20`, `gpt-4o-mini-transcribe-2025-12-15`, `gpt-4o-transcribe`, `gpt-4o-transcribe-diarize`, `whisper-1` | per-utterance upload (`openai_transcriptions`) | same | today's buffering client | voice agent: `stt_segmented_mode`, `stt_model_deprecated`; manual agent or loop: `stt_segmented_mode`, `stt_model_deprecated`; plain: `stt_buffered_until_commit`, `stt_model_deprecated` (notice) |
|  | `gpt-live-transcribe`, `gpt-live-transcribe*`, `gpt-realtime-whisper`, `gpt-realtime-whisper*` | gateway-driven commit (`openai_realtime_transcription`) | same | same |  |
|  | `gpt-transcribe` (default) | per-utterance upload (`openai_transcriptions`) | same | today's buffering client | voice agent: `stt_segmented_mode`; manual agent or loop: `stt_segmented_mode`; plain: `stt_buffered_until_commit` |
| `phonexia` | any other id (`*`), `default`, `large_v2`, `large_v3`, `medium`, `speech-to-text`, `speech-to-text-whisper-enhanced` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
|  | `EN_US_6` | refused `stt_model_retired` | same | same |  |
| `revai` | any other id (`*`) | native stream | same | same | `stt_model_substituted` (notice), `stt_capability_assumed` (notice) |
|  | `*whisper*`, `fusion`, `low_cost` | refused `stt_live_unsupported` (`async_only`) | same | same |  |
|  | `human` | refused `stt_live_unsupported` (`disabled`) | same | same |  |
|  | `machine` (default), `machine_v2`, `reverb`, `reverb-foreign-language` | native stream | same | same |  |
| `reverie` | any other id (`*`) | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
| `sarvam` | any other id (`*`) | native stream | same | same | `stt_capability_assumed` (notice) |
|  | `saaras:v2.5` | refused `stt_live_unsupported` (`disabled`) | same | same |  |
|  | `saaras:v3`, `saaras:v4` | native stream | same | same |  |
|  | `saaras:v3-realtime` | today's client (behaviour for this model not established) | same | same | `stt_client_unverified` (notice) |
|  | `saarika:v2.5` (default) | native stream | same | same | `stt_model_deprecated` (notice) |
| `sberdevices` | any other id (`*`), `callcenter`, `general` (default), `ivr`, `media` | today's timed-upload client (known broken) | same | same | `stt_client_unverified` (notice) |
| `self_hosted` | any other id (`*`), `*nemotron*`, `*voxtral*realtime*`, `*whisper*` | per-utterance upload (`openai_transcriptions`) | same | same | `stt_segmented_mode` |
|  | `kyutai/stt-1b-en_fr`, `kyutai/stt-2.6b-en` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
| `speechmatics` | any other id (`*`) | native stream | same | same | `stt_capability_assumed` (notice) |
|  | `enhanced`, `standard` (default) | native stream | same | same |  |
|  | `linden-1`, `melia-1`, `oak-1` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
| `tencent` | any other id (`*`) | native stream (client known broken) | same | same | `stt_client_unverified` (notice), `stt_model_substituted` (notice), `stt_capability_assumed` (notice) |
|  | `16k_ar`, `16k_de`, `16k_en`, `16k_en_edu`, `16k_en_game`, `16k_en_large`, `16k_es`, `16k_fil`, `16k_fr`, `16k_hi` and 23 more | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
|  | `16k_zh-PY`, `16k_zh_dialect` | today's client, streaming 16k_zh instead | same | same | `stt_model_substituted` (notice) |
|  | `16k_zh_en_meeting` | refused `stt_live_unsupported` (`async_only`) | same | same |  |
| `tinkoff` | any other id (`*`) | native stream (client known broken) | same | same | `stt_client_unverified` (notice), `stt_model_substituted` (notice), `stt_capability_assumed` (notice) |
|  | `default` (default) | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
| `viettel-ai` | any other id (`*`) | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
| `waav-infer` | any other id (`*`) | per-utterance upload (`openai_transcriptions`) | same | same | `stt_segmented_mode`, `stt_capability_assumed` |
|  | `nemotron*`, `voxtral*realtime*`, `whisper*` | per-utterance upload (`openai_transcriptions`) | same | same | `stt_segmented_mode` |
| `yandex` | any other id (`*`), `general` (default), `general:deprecated`, `general:rc` | today's timed-upload client (known broken) | same | same | `stt_client_unverified` (notice) |
|  | `deferred*`, `deferred-general`, `deferred-general:deprecated`, `deferred-general:rc` | refused `stt_live_unsupported` (`async_only`) | same | same |  |

## Release 5, wider vendor coverage and hardening

Outcome changed from the previous release for 96 row(s), of these providers: `alibaba-cloud`, `baidu`, `bhashini`, `fpt-ai`, `gnani`, `google`, `huawei-cloud`, `microsoft-azure`, `naver-clova`, `nectec`, `phonexia`, `sberdevices`, `speechmatics`, `tencent`, `viettel-ai`, `yandex`.

| Provider | Models | Voice agent, automatic turns | Manual agent, conversation loop or DAG | Plain /ws session | Warnings |
| --- | --- | --- | --- | --- | --- |
| unknown provider (global default) | any other id (`*`) | today's client through the plugin registry (unclassified provider) | same | same |  |
| `alibaba-cloud` | any other id (`*`) | native stream (client unverified) | same | same | `stt_client_unverified` (notice), `stt_capability_assumed` (notice) |
|  | `*-asr-flash-message*`, `*-asr-flash-streaming*`, `*-realtime*`, `fun-asr-flash-8k-realtime`, `fun-asr-flash-8k-realtime-2026-01-28`, `fun-asr-mtl-realtime-2025-12-10`, `gummy-chat-v1`, `gummy-realtime-v1`, `qwen-audio-3.0-asr-flash-streaming`, `qwen-audio-3.1-asr-flash-message`, `qwen-audio-3.1-asr-flash-streaming` | native stream (client unverified) | same | same | `stt_client_unverified` (notice) |
|  | `*-filetrans*`, `fun-asr`, `fun-asr-202*`, `fun-asr-2025-08-25`, `fun-asr-2025-11-07`, `fun-asr-mtl`, `fun-asr-mtl-2025-08-25`, `paraformer-8k-v1`, `paraformer-8k-v2`, `paraformer-mtl-v1` and 7 more | refused `stt_live_unsupported` (`async_only`) | same | same |  |
|  | `fun-asr-flash-2*` | per-utterance upload (`regional_rest`) [after a live probe] | same | same | `stt_segmented_mode` |
|  | `fun-asr-flash-2026-06-15`, `qwen-audio-3.0-asr-flash`, `qwen-audio-3.1-asr-flash` | per-utterance upload (`regional_rest`) [after a live probe; regions only beijing, singapore] | same | same | `stt_segmented_mode` |
|  | `fun-asr-mtl-realtime` | native stream (client unverified) | same | same | `stt_client_unverified` (notice), `stt_model_deprecated` (notice) |
|  | `fun-asr-realtime`, `fun-asr-realtime-2026-02-28`, `paraformer-realtime-8k-v1`, `paraformer-realtime-8k-v2`, `paraformer-realtime-v1`, `paraformer-realtime-v2` | native stream | same | same |  |
|  | `fun-asr-realtime-2025-09-15`, `fun-asr-realtime-2025-11-07` | native stream | same | same | `stt_model_deprecated` (notice) |
|  | `qwen3-asr-flash` | per-utterance upload (`regional_rest`) [after a live probe; regions only beijing, singapore, us-east-1] | same | same | `stt_segmented_mode`, `stt_min_billed_duration` |
|  | `qwen3-asr-flash-2*` | per-utterance upload (`regional_rest`) [after a live probe] | same | same | `stt_segmented_mode`, `stt_min_billed_duration` |
|  | `qwen3-asr-flash-2025-09-08`, `qwen3-asr-flash-2026-02-10` | per-utterance upload (`regional_rest`) [after a live probe] | same | same | `stt_segmented_mode`, `stt_min_billed_duration`, `stt_model_deprecated` |
|  | `qwen3-asr-flash-realtime` (default), `qwen3-asr-flash-realtime*` | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
|  | `qwen3-asr-flash-realtime-2025-10-27`, `qwen3-asr-flash-realtime-2026-02-10` | native stream (client known broken) | same | same | `stt_client_unverified` (notice), `stt_model_deprecated` (notice) |
| `amivoice` | any other id (`*`) | native stream (client known broken) | same | same | `stt_client_unverified` (notice), `stt_model_substituted` (notice), `stt_capability_assumed` (notice) |
|  | `-a-address-input-private`, `-a-bizfinance`, `-a-bizfinance-input`, `-a-bizinsurance`, `-a-bizinsurance-input`, `-a-name-input-private`, `-a-rule-input-private` | today's client, streaming -a-general instead | same | same | `stt_model_substituted` (notice) |
|  | `-a-general` (default), `-a-general-en`, `-a-general-input`, `-a-general-ko`, `-a-general-zh`, `-a-medical`, `-a-medical-input`, `-a2-ja-general`, `-a2-multi-general`, `-a2-zh-general` and 3 more | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
| `assemblyai` | any other id (`*`), `universal-3-6-pro` | today's client, streaming universal-streaming-english instead | same | same | `stt_model_substituted` (notice) |
|  | `u3-pro`, `u3-sync-pro`, `universal-3-5-pro` | per-utterance upload (`assemblyai_sync`) | same | same | `stt_segmented_mode` |
|  | `universal-2` | refused `stt_live_unsupported` (`async_only`) | same | same |  |
|  | `universal-streaming-english` (default), `universal-streaming-multilingual` | native stream | same | same |  |
| `aws-transcribe` | any other id (`*`) | native stream | same | same | `stt_model_substituted` (notice), `stt_capability_assumed` (notice) |
|  | `call-analytics`, `healthscribe`, `medical` | today's client, streaming standard instead | same | same | `stt_model_substituted` (notice) |
|  | `standard` (default) | native stream | same | same |  |
| `azure_openai` | any other id (`*`) | per-utterance upload (`azure_openai_transcriptions`) | same | same | `stt_segmented_mode`, `stt_capability_assumed` |
|  | `gpt-4o-mini-transcribe`, `gpt-4o-transcribe`, `gpt-4o-transcribe-diarize`, `whisper`, `whisper-*` | per-utterance upload (`azure_openai_transcriptions`) | same | same | `stt_segmented_mode`, `stt_model_deprecated` |
|  | `gpt-live-transcribe`, `gpt-realtime-whisper` | gateway-driven commit (`openai_realtime_transcription`) [after a live probe] | same | same |  |
|  | `gpt-transcribe` | per-utterance upload (`azure_openai_transcriptions`) | same | same | `stt_segmented_mode` |
| `baidu` | any other id (`*`) | per-utterance upload (`regional_rest`) [after a live probe] | same | same | `stt_segmented_mode`, `stt_capability_assumed` |
|  | `1537` (default), `1637`, `1737`, `1837`, `80001`, `8001` | per-utterance upload (`regional_rest`) [after a live probe] | same | same | `stt_segmented_mode` |
|  | `15372`, `15376` | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
|  | `1936`, `19362` | refused `stt_model_retired` | same | same |  |
| `bhashini` | any other id (`*`) | per-utterance upload (`regional_rest`) [after a live probe] | same | today's buffering client | voice agent: `stt_segmented_mode`, `stt_capability_assumed`; manual agent or loop: `stt_segmented_mode`, `stt_capability_assumed`; plain: `stt_buffered_until_commit` |
|  | `ai4bharat/conformer-hi-gpu--t4` | per-utterance upload (`regional_rest`) [after a live probe; languages only hi] | same | today's buffering client | voice agent: `stt_segmented_mode`; manual agent or loop: `stt_segmented_mode`; plain: `stt_buffered_until_commit` |
|  | `ai4bharat/conformer-multilingual-dravidian-gpu--t4`, `bhashini/iitm/asr-dravidian--gpu--t4` | per-utterance upload (`regional_rest`) [after a live probe; languages only 4 listed] | same | today's buffering client | voice agent: `stt_segmented_mode`; manual agent or loop: `stt_segmented_mode`; plain: `stt_buffered_until_commit` |
|  | `ai4bharat/conformer-multilingual-indo_aryan-gpu--t4` | per-utterance upload (`regional_rest`) [after a live probe; languages only 8 listed] | same | today's buffering client | voice agent: `stt_segmented_mode`; manual agent or loop: `stt_segmented_mode`; plain: `stt_buffered_until_commit` |
|  | `ai4bharat/whisper-medium-en--gpu--t4` | per-utterance upload (`regional_rest`) [after a live probe; languages only en] | same | today's buffering client | voice agent: `stt_segmented_mode`; manual agent or loop: `stt_segmented_mode`; plain: `stt_buffered_until_commit` |
|  | `bhashini/ai4bharat/conformer-multilingual-asr` | per-utterance upload (`regional_rest`) [after a live probe; languages only 23 listed] | same | today's buffering client | voice agent: `stt_segmented_mode`; manual agent or loop: `stt_segmented_mode`; plain: `stt_buffered_until_commit` |
|  | `bhashini/bodhan/asr-transcribe-core`, `bhashini/bodhan/asr-transcribe-flex` | per-utterance upload (`regional_rest`) [after a live probe] | same | today's buffering client | voice agent: `stt_segmented_mode`; manual agent or loop: `stt_segmented_mode`; plain: `stt_buffered_until_commit` |
|  | `bhashini/iisc/asr-bho-t4` | per-utterance upload (`regional_rest`) [after a live probe; languages only bho] | same | today's buffering client | voice agent: `stt_segmented_mode`; manual agent or loop: `stt_segmented_mode`; plain: `stt_buffered_until_commit` |
|  | `bhashini/iisc/asr-mai-t4` | per-utterance upload (`regional_rest`) [after a live probe; languages only mai] | same | today's buffering client | voice agent: `stt_segmented_mode`; manual agent or loop: `stt_segmented_mode`; plain: `stt_buffered_until_commit` |
|  | `bhashini/iitm/asr-indoaryan--gpu--t4` | per-utterance upload (`regional_rest`) [after a live probe; languages only 6 listed] | same | today's buffering client | voice agent: `stt_segmented_mode`; manual agent or loop: `stt_segmented_mode`; plain: `stt_buffered_until_commit` |
|  | `bhashini/iitm/asr-misc--gpu--t4` | per-utterance upload (`regional_rest`) [after a live probe; languages only bho, ur] | same | today's buffering client | voice agent: `stt_segmented_mode`; manual agent or loop: `stt_segmented_mode`; plain: `stt_buffered_until_commit` |
| `cartesia` | any other id (`*`) | gateway-driven commit (`cartesia_manual_finalize`) [after a live probe] | same | same | `stt_capability_assumed` |
|  | `ink-2`, `ink-preview`, `ink-whisper` (default), `ink-whisper-2025-06-04` | gateway-driven commit (`cartesia_manual_finalize`) [after a live probe] | same | same |  |
| `deepgram` | any other id (`*`) | native stream | same | same | `stt_capability_assumed` (notice) |
|  | `base`, `base-general` (default), `conversationalai`, `enhanced`, `enhanced-finance`, `enhanced-general`, `enhanced-meeting`, `enhanced-phonecall`, `finance`, `meeting` and 22 more | native stream | same | same |  |
|  | `flux-*`, `flux-general-en`, `flux-general-multi` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
|  | `whisper`, `whisper-*`, `whisper-base`, `whisper-large`, `whisper-medium`, `whisper-small`, `whisper-tiny` | per-utterance upload (`deepgram_prerecorded`) [regions except eu, au, in] | same | same | `stt_segmented_mode` |
| `elevenlabs` | any other id (`*`) | per-utterance upload (`elevenlabs_batch`) | same | same | `stt_segmented_mode`, `stt_capability_assumed` |
|  | `*realtime*` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
|  | `scribe_v1` | refused `stt_model_retired` | same | same |  |
|  | `scribe_v2`, `scribe_v2_medical` | per-utterance upload (`elevenlabs_batch`) | same | same | `stt_segmented_mode` |
|  | `scribe_v2_realtime` (default) | native stream | same | same |  |
| `fpt-ai` | any other id (`*`) | per-utterance upload (`regional_rest`) [after a live probe] | same | today's buffering client | voice agent: `stt_segmented_mode`, `stt_capability_assumed`; manual agent or loop: `stt_segmented_mode`, `stt_capability_assumed`; plain: `stt_buffered_until_commit` |
|  | `general` (default) | per-utterance upload (`regional_rest`) [after a live probe] | same | today's buffering client | voice agent: `stt_segmented_mode`; manual agent or loop: `stt_segmented_mode`; plain: `stt_buffered_until_commit` |
| `gladia` | any other id (`*`) | today's client, streaming solaria-1 instead | same | same | `stt_model_substituted` (notice) |
|  | `solaria-1` (default) | native stream | same | same |  |
|  | `solaria-3` | refused `stt_live_unsupported` (`async_only`) | same | same |  |
| `gnani` | any other id (`*`) | per-utterance upload (`regional_rest`) [after a live probe; languages only 10 listed] | same | same | `stt_segmented_mode`, `stt_capability_assumed` |
|  | `default` | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
|  | `gnani-prisma-v2.5` (default), `vachana-audio-intelligence-v2` | per-utterance upload (`regional_rest`) [after a live probe; languages only 10 listed] | same | same | `stt_segmented_mode` |
| `google` | any other id (`*`) | native stream | same | same | `stt_capability_assumed` (notice) |
|  | `chirp`, `latest_short` | per-utterance upload (`google_recognize`) | same | same | `stt_segmented_mode` |
|  | `chirp_2`, `chirp_3`, `chirp_telephony`, `latest_long` (default), `long`, `medical_conversation`, `medical_dictation`, `short`, `telephony`, `telephony_short` | native stream | same | same |  |
| `groq` | any other id (`*`) | per-utterance upload (`groq_transcriptions`) | same | today's buffering client | voice agent: `stt_segmented_mode`, `stt_min_billed_duration`, `stt_capability_assumed`; manual agent or loop: `stt_segmented_mode`, `stt_min_billed_duration`, `stt_capability_assumed`; plain: `stt_buffered_until_commit` |
|  | `distil-whisper-large-v3-en` | refused `stt_model_retired` | same | same |  |
|  | `whisper-large-v3`, `whisper-large-v3-turbo` (default) | per-utterance upload (`groq_transcriptions`) | same | today's buffering client | voice agent: `stt_segmented_mode`, `stt_min_billed_duration`; manual agent or loop: `stt_segmented_mode`, `stt_min_billed_duration`; plain: `stt_buffered_until_commit` |
| `huawei-cloud` | any other id (`*`) | per-utterance upload (`regional_rest`) [after a live probe] | same | same | `stt_segmented_mode`, `stt_capability_assumed` |
|  | `arabic_16k_general` | per-utterance upload (`regional_rest`) [after a live probe; regions except cn-north-4, cn-east-3] | same | same | `stt_segmented_mode` |
|  | `arabic_8k_general`, `chinese_16k_court`, `chinese_16k_it`, `chinese_8k_general`, `english_16k_general` | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
|  | `cantonese_16k_common`, `shanghai_16k_common`, `sichuan_16k_common` | per-utterance upload (`regional_rest`) [after a live probe; regions only cn-north-4] | same | same | `stt_segmented_mode` |
|  | `chinese_16k_common`, `chinese_16k_travel`, `chinese_8k_common` | per-utterance upload (`regional_rest`) [after a live probe; regions only cn-north-4, cn-east-3] | same | same | `stt_segmented_mode` |
|  | `chinese_16k_conversation` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
|  | `chinese_16k_general` (default), `english_8k_common` | per-utterance upload (`regional_rest`) [after a live probe] | same | same | `stt_segmented_mode` |
|  | `chinese_16k_media`, `sichuan_8k_common` | refused `stt_live_unsupported` (`async_only`) | same | same |  |
|  | `english_16k_common` | per-utterance upload (`regional_rest`) [after a live probe; regions except cn-north-4] | same | same | `stt_segmented_mode` |
| `ibm-watson` | any other id (`*`) | native stream | same | same | `stt_capability_assumed` (notice) |
|  | `*_BroadbandModel`, `*_NarrowbandModel` | refused `stt_model_retired` | same | same |  |
|  | `ar-MS_Telephony`, `cs-CZ_Telephony`, `de-DE_Multimedia`, `de-DE_Telephony`, `en-AU_Multimedia`, `en-AU_Telephony`, `en-GB_Multimedia`, `en-GB_Telephony`, `en-IN_Telephony`, `en-US_Multimedia` (default) and 23 more | native stream | same | same |  |
|  | `de-DE`, `en-AU`, `en-GB`, `en-IN`, `en-US`, `es-AR`, `es-CL`, `es-CO`, `es-ES`, `es-MX` and 8 more | native stream (client unverified) | same | same | `stt_client_unverified` (notice) |
| `iflytek` | any other id (`*`) | native stream (client known broken) | same | same | `stt_client_unverified` (notice), `stt_capability_assumed` (notice) |
|  | `*ist*`, `*realtime*`, `*stream*`, `iat` (default), `ist`, `ist_huanyu`, `ist_hy`, `ist_open`, `medical`, `sp_ist_vais` | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
|  | `iflyrec_voice_*`, `iflyrec_voice_cn_10m_ed`, `iflyrec_voice_de_de_24h`, `iflyrec_voice_es_es_24h`, `iflyrec_voice_fr_fr_24h`, `iflyrec_voice_ja_jp_24h`, `iflyrec_voice_ko_kr_24h`, `iflyrec_voice_th_th_sp_24h`, `iflyrec_voice_vi_vn_vais_24h`, `iflyrec_voice_yueyu_24h` | refused `stt_live_unsupported` (`async_only`) | same | same |  |
| `microsoft-azure` | any other id (`*`) | native stream | same | same | `stt_model_substituted` (notice), `stt_capability_assumed` (notice) |
|  | `default` (default) | native stream | same | same |  |
|  | `llm-speech`, `MAI-Transcribe-1.5`, `MAI-Transcribe-2` | per-utterance upload (`azure_fast_transcription`) [regions only 6 listed] | same | same | `stt_segmented_mode` |
|  | `MAI-Transcribe-1` | refused `stt_model_retired` | same | same |  |
|  | `MAI-Transcribe-2-Streaming` | today's client, streaming default instead | same | same | `stt_model_substituted` (notice) |
| `naver-clova` | any other id (`*`) | per-utterance upload (`regional_rest`) [after a live probe; languages only 4 listed] | same | today's buffering client | voice agent: `stt_segmented_mode`, `stt_min_billed_duration`, `stt_capability_assumed`; manual agent or loop: `stt_segmented_mode`, `stt_min_billed_duration`, `stt_capability_assumed`; plain: `stt_buffered_until_commit` |
|  | `clova-speech-long`, `clova-speech-short`, `clova-speech-streaming` | refused `stt_live_unsupported` (`client_not_implemented`) | today's buffering client | today's buffering client | voice agent: none; manual agent or loop: `stt_buffered_until_commit`; plain: `stt_buffered_until_commit` |
|  | `csr` (default) | per-utterance upload (`regional_rest`) [after a live probe; languages only 4 listed] | same | today's buffering client | voice agent: `stt_segmented_mode`, `stt_min_billed_duration`; manual agent or loop: `stt_segmented_mode`, `stt_min_billed_duration`; plain: `stt_buffered_until_commit` |
| `nectec` | any other id (`*`) | per-utterance upload (`regional_rest`) [after a live probe] | same | same | `stt_segmented_mode`, `stt_capability_assumed` |
|  | `partii4` (default) | per-utterance upload (`regional_rest`) [after a live probe] | same | today's buffering client | voice agent: `stt_segmented_mode`; manual agent or loop: `stt_segmented_mode`; plain: `stt_buffered_until_commit` |
|  | `partii5` | refused `stt_model_retired` | same | same |  |
| `openai` | any other id (`*`) | per-utterance upload (`openai_transcriptions`) | same | today's buffering client | voice agent: `stt_segmented_mode`, `stt_capability_assumed`; manual agent or loop: `stt_segmented_mode`, `stt_capability_assumed`; plain: `stt_buffered_until_commit` |
|  | `gpt-4o-mini-transcribe`, `gpt-4o-mini-transcribe-2025-03-20`, `gpt-4o-mini-transcribe-2025-12-15`, `gpt-4o-transcribe`, `gpt-4o-transcribe-diarize`, `whisper-1` | per-utterance upload (`openai_transcriptions`) | same | today's buffering client | voice agent: `stt_segmented_mode`, `stt_model_deprecated`; manual agent or loop: `stt_segmented_mode`, `stt_model_deprecated`; plain: `stt_buffered_until_commit`, `stt_model_deprecated` (notice) |
|  | `gpt-live-transcribe`, `gpt-live-transcribe*`, `gpt-realtime-whisper`, `gpt-realtime-whisper*` | gateway-driven commit (`openai_realtime_transcription`) | same | same |  |
|  | `gpt-transcribe` (default) | per-utterance upload (`openai_transcriptions`) | same | today's buffering client | voice agent: `stt_segmented_mode`; manual agent or loop: `stt_segmented_mode`; plain: `stt_buffered_until_commit` |
| `phonexia` | any other id (`*`) | per-utterance upload (`regional_rest`) [after a live probe] | same | same | `stt_segmented_mode`, `stt_capability_assumed` |
|  | `default`, `large_v2`, `large_v3`, `medium`, `speech-to-text`, `speech-to-text-whisper-enhanced` | per-utterance upload (`regional_rest`) [after a live probe] | same | same | `stt_segmented_mode` |
|  | `EN_US_6` | refused `stt_model_retired` | same | same |  |
| `revai` | any other id (`*`) | native stream | same | same | `stt_model_substituted` (notice), `stt_capability_assumed` (notice) |
|  | `*whisper*`, `fusion`, `low_cost` | refused `stt_live_unsupported` (`async_only`) | same | same |  |
|  | `human` | refused `stt_live_unsupported` (`disabled`) | same | same |  |
|  | `machine` (default), `machine_v2`, `reverb`, `reverb-foreign-language` | native stream | same | same |  |
| `reverie` | any other id (`*`) | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
| `sarvam` | any other id (`*`) | native stream | same | same | `stt_capability_assumed` (notice) |
|  | `saaras:v2.5` | refused `stt_live_unsupported` (`disabled`) | same | same |  |
|  | `saaras:v3`, `saaras:v4` | native stream | same | same |  |
|  | `saaras:v3-realtime` | today's client (behaviour for this model not established) | same | same | `stt_client_unverified` (notice) |
|  | `saarika:v2.5` (default) | native stream | same | same | `stt_model_deprecated` (notice) |
| `sberdevices` | any other id (`*`) | per-utterance upload (`regional_rest`) [after a live probe; languages only 5 listed] | same | same | `stt_segmented_mode`, `stt_capability_assumed` |
|  | `callcenter`, `general` (default), `ivr`, `media` | per-utterance upload (`regional_rest`) [after a live probe; languages only 5 listed] | same | same | `stt_segmented_mode` |
| `self_hosted` | any other id (`*`), `*nemotron*`, `*voxtral*realtime*`, `*whisper*` | per-utterance upload (`openai_transcriptions`) | same | same | `stt_segmented_mode` |
|  | `kyutai/stt-1b-en_fr`, `kyutai/stt-2.6b-en` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
| `speechmatics` | any other id (`*`) | native stream | same | same | `stt_capability_assumed` (notice) |
|  | `enhanced`, `standard` (default) | native stream | same | same |  |
|  | `linden-1` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
|  | `melia-1`, `oak-1` | per-utterance upload (`speechmatics_batch`) [after a live probe and a deadline measurement; regions only eu1, us1] | same | same | `stt_segmented_mode` |
| `tencent` | any other id (`*`) | per-utterance upload (`regional_rest`) [after a live probe; regions only china] | same | same | `stt_segmented_mode`, `stt_min_billed_duration`, `stt_capability_assumed` |
|  | `16k_ar`, `16k_de`, `16k_en`, `16k_es`, `16k_fil`, `16k_fr`, `16k_hi`, `16k_id`, `16k_ja`, `16k_ko` and 13 more | per-utterance upload (`regional_rest`) [after a live probe; regions only china] | same | same | `stt_segmented_mode`, `stt_min_billed_duration` |
|  | `16k_en_edu`, `16k_en_game`, `16k_en_large`, `16k_zh-TW`, `16k_zh_court`, `16k_zh_edu`, `16k_zh_en_2.0`, `16k_zh_en_speaker_2.0`, `16k_zh_large`, `16k_zh_medical`, `Hy-ASR-3.0-preview` | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
|  | `16k_zh_dialect` | today's client, streaming 16k_zh instead | same | same | `stt_model_substituted` (notice) |
|  | `16k_zh_en_meeting` | refused `stt_live_unsupported` (`async_only`) | same | same |  |
| `tinkoff` | any other id (`*`) | native stream (client known broken) | same | same | `stt_client_unverified` (notice), `stt_model_substituted` (notice), `stt_capability_assumed` (notice) |
|  | `default` (default) | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
| `viettel-ai` | any other id (`*`) | per-utterance upload (`regional_rest`) [after a live probe and a deadline measurement] | same | same | `stt_segmented_mode` |
| `waav-infer` | any other id (`*`) | per-utterance upload (`openai_transcriptions`) | same | same | `stt_segmented_mode`, `stt_capability_assumed` |
|  | `nemotron*`, `voxtral*realtime*`, `whisper*` | per-utterance upload (`openai_transcriptions`) | same | same | `stt_segmented_mode` |
| `yandex` | any other id (`*`) | per-utterance upload (`regional_rest`) [after a live probe] | same | same | `stt_segmented_mode`, `stt_min_billed_duration`, `stt_capability_assumed` |
|  | `deferred*`, `deferred-general`, `deferred-general:deprecated`, `deferred-general:rc` | refused `stt_live_unsupported` (`async_only`) | same | same |  |
|  | `general` (default), `general:deprecated`, `general:rc` | per-utterance upload (`regional_rest`) [after a live probe] | same | same | `stt_segmented_mode`, `stt_min_billed_duration` |

## Release 6, interruption recovery and interim text

Outcome changed from the previous release for 0 row(s).

| Provider | Models | Voice agent, automatic turns | Manual agent, conversation loop or DAG | Plain /ws session | Warnings |
| --- | --- | --- | --- | --- | --- |
| unknown provider (global default) | any other id (`*`) | today's client through the plugin registry (unclassified provider) | same | same |  |
| `alibaba-cloud` | any other id (`*`) | native stream (client unverified) | same | same | `stt_client_unverified` (notice), `stt_capability_assumed` (notice) |
|  | `*-asr-flash-message*`, `*-asr-flash-streaming*`, `*-realtime*`, `fun-asr-flash-8k-realtime`, `fun-asr-flash-8k-realtime-2026-01-28`, `fun-asr-mtl-realtime-2025-12-10`, `gummy-chat-v1`, `gummy-realtime-v1`, `qwen-audio-3.0-asr-flash-streaming`, `qwen-audio-3.1-asr-flash-message`, `qwen-audio-3.1-asr-flash-streaming` | native stream (client unverified) | same | same | `stt_client_unverified` (notice) |
|  | `*-filetrans*`, `fun-asr`, `fun-asr-202*`, `fun-asr-2025-08-25`, `fun-asr-2025-11-07`, `fun-asr-mtl`, `fun-asr-mtl-2025-08-25`, `paraformer-8k-v1`, `paraformer-8k-v2`, `paraformer-mtl-v1` and 7 more | refused `stt_live_unsupported` (`async_only`) | same | same |  |
|  | `fun-asr-flash-2*` | per-utterance upload (`regional_rest`) [after a live probe] | same | same | `stt_segmented_mode` |
|  | `fun-asr-flash-2026-06-15`, `qwen-audio-3.0-asr-flash`, `qwen-audio-3.1-asr-flash` | per-utterance upload (`regional_rest`) [after a live probe; regions only beijing, singapore] | same | same | `stt_segmented_mode` |
|  | `fun-asr-mtl-realtime` | native stream (client unverified) | same | same | `stt_client_unverified` (notice), `stt_model_deprecated` (notice) |
|  | `fun-asr-realtime`, `fun-asr-realtime-2026-02-28`, `paraformer-realtime-8k-v1`, `paraformer-realtime-8k-v2`, `paraformer-realtime-v1`, `paraformer-realtime-v2` | native stream | same | same |  |
|  | `fun-asr-realtime-2025-09-15`, `fun-asr-realtime-2025-11-07` | native stream | same | same | `stt_model_deprecated` (notice) |
|  | `qwen3-asr-flash` | per-utterance upload (`regional_rest`) [after a live probe; regions only beijing, singapore, us-east-1] | same | same | `stt_segmented_mode`, `stt_min_billed_duration` |
|  | `qwen3-asr-flash-2*` | per-utterance upload (`regional_rest`) [after a live probe] | same | same | `stt_segmented_mode`, `stt_min_billed_duration` |
|  | `qwen3-asr-flash-2025-09-08`, `qwen3-asr-flash-2026-02-10` | per-utterance upload (`regional_rest`) [after a live probe] | same | same | `stt_segmented_mode`, `stt_min_billed_duration`, `stt_model_deprecated` |
|  | `qwen3-asr-flash-realtime` (default), `qwen3-asr-flash-realtime*` | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
|  | `qwen3-asr-flash-realtime-2025-10-27`, `qwen3-asr-flash-realtime-2026-02-10` | native stream (client known broken) | same | same | `stt_client_unverified` (notice), `stt_model_deprecated` (notice) |
| `amivoice` | any other id (`*`) | native stream (client known broken) | same | same | `stt_client_unverified` (notice), `stt_model_substituted` (notice), `stt_capability_assumed` (notice) |
|  | `-a-address-input-private`, `-a-bizfinance`, `-a-bizfinance-input`, `-a-bizinsurance`, `-a-bizinsurance-input`, `-a-name-input-private`, `-a-rule-input-private` | today's client, streaming -a-general instead | same | same | `stt_model_substituted` (notice) |
|  | `-a-general` (default), `-a-general-en`, `-a-general-input`, `-a-general-ko`, `-a-general-zh`, `-a-medical`, `-a-medical-input`, `-a2-ja-general`, `-a2-multi-general`, `-a2-zh-general` and 3 more | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
| `assemblyai` | any other id (`*`), `universal-3-6-pro` | today's client, streaming universal-streaming-english instead | same | same | `stt_model_substituted` (notice) |
|  | `u3-pro`, `u3-sync-pro`, `universal-3-5-pro` | per-utterance upload (`assemblyai_sync`) | same | same | `stt_segmented_mode` |
|  | `universal-2` | refused `stt_live_unsupported` (`async_only`) | same | same |  |
|  | `universal-streaming-english` (default), `universal-streaming-multilingual` | native stream | same | same |  |
| `aws-transcribe` | any other id (`*`) | native stream | same | same | `stt_model_substituted` (notice), `stt_capability_assumed` (notice) |
|  | `call-analytics`, `healthscribe`, `medical` | today's client, streaming standard instead | same | same | `stt_model_substituted` (notice) |
|  | `standard` (default) | native stream | same | same |  |
| `azure_openai` | any other id (`*`) | per-utterance upload (`azure_openai_transcriptions`) | same | same | `stt_segmented_mode`, `stt_capability_assumed` |
|  | `gpt-4o-mini-transcribe`, `gpt-4o-transcribe`, `gpt-4o-transcribe-diarize`, `whisper`, `whisper-*` | per-utterance upload (`azure_openai_transcriptions`) | same | same | `stt_segmented_mode`, `stt_model_deprecated` |
|  | `gpt-live-transcribe`, `gpt-realtime-whisper` | gateway-driven commit (`openai_realtime_transcription`) [after a live probe] | same | same |  |
|  | `gpt-transcribe` | per-utterance upload (`azure_openai_transcriptions`) | same | same | `stt_segmented_mode` |
| `baidu` | any other id (`*`) | per-utterance upload (`regional_rest`) [after a live probe] | same | same | `stt_segmented_mode`, `stt_capability_assumed` |
|  | `1537` (default), `1637`, `1737`, `1837`, `80001`, `8001` | per-utterance upload (`regional_rest`) [after a live probe] | same | same | `stt_segmented_mode` |
|  | `15372`, `15376` | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
|  | `1936`, `19362` | refused `stt_model_retired` | same | same |  |
| `bhashini` | any other id (`*`) | per-utterance upload (`regional_rest`) [after a live probe] | same | today's buffering client | voice agent: `stt_segmented_mode`, `stt_capability_assumed`; manual agent or loop: `stt_segmented_mode`, `stt_capability_assumed`; plain: `stt_buffered_until_commit` |
|  | `ai4bharat/conformer-hi-gpu--t4` | per-utterance upload (`regional_rest`) [after a live probe; languages only hi] | same | today's buffering client | voice agent: `stt_segmented_mode`; manual agent or loop: `stt_segmented_mode`; plain: `stt_buffered_until_commit` |
|  | `ai4bharat/conformer-multilingual-dravidian-gpu--t4`, `bhashini/iitm/asr-dravidian--gpu--t4` | per-utterance upload (`regional_rest`) [after a live probe; languages only 4 listed] | same | today's buffering client | voice agent: `stt_segmented_mode`; manual agent or loop: `stt_segmented_mode`; plain: `stt_buffered_until_commit` |
|  | `ai4bharat/conformer-multilingual-indo_aryan-gpu--t4` | per-utterance upload (`regional_rest`) [after a live probe; languages only 8 listed] | same | today's buffering client | voice agent: `stt_segmented_mode`; manual agent or loop: `stt_segmented_mode`; plain: `stt_buffered_until_commit` |
|  | `ai4bharat/whisper-medium-en--gpu--t4` | per-utterance upload (`regional_rest`) [after a live probe; languages only en] | same | today's buffering client | voice agent: `stt_segmented_mode`; manual agent or loop: `stt_segmented_mode`; plain: `stt_buffered_until_commit` |
|  | `bhashini/ai4bharat/conformer-multilingual-asr` | per-utterance upload (`regional_rest`) [after a live probe; languages only 23 listed] | same | today's buffering client | voice agent: `stt_segmented_mode`; manual agent or loop: `stt_segmented_mode`; plain: `stt_buffered_until_commit` |
|  | `bhashini/bodhan/asr-transcribe-core`, `bhashini/bodhan/asr-transcribe-flex` | per-utterance upload (`regional_rest`) [after a live probe] | same | today's buffering client | voice agent: `stt_segmented_mode`; manual agent or loop: `stt_segmented_mode`; plain: `stt_buffered_until_commit` |
|  | `bhashini/iisc/asr-bho-t4` | per-utterance upload (`regional_rest`) [after a live probe; languages only bho] | same | today's buffering client | voice agent: `stt_segmented_mode`; manual agent or loop: `stt_segmented_mode`; plain: `stt_buffered_until_commit` |
|  | `bhashini/iisc/asr-mai-t4` | per-utterance upload (`regional_rest`) [after a live probe; languages only mai] | same | today's buffering client | voice agent: `stt_segmented_mode`; manual agent or loop: `stt_segmented_mode`; plain: `stt_buffered_until_commit` |
|  | `bhashini/iitm/asr-indoaryan--gpu--t4` | per-utterance upload (`regional_rest`) [after a live probe; languages only 6 listed] | same | today's buffering client | voice agent: `stt_segmented_mode`; manual agent or loop: `stt_segmented_mode`; plain: `stt_buffered_until_commit` |
|  | `bhashini/iitm/asr-misc--gpu--t4` | per-utterance upload (`regional_rest`) [after a live probe; languages only bho, ur] | same | today's buffering client | voice agent: `stt_segmented_mode`; manual agent or loop: `stt_segmented_mode`; plain: `stt_buffered_until_commit` |
| `cartesia` | any other id (`*`) | gateway-driven commit (`cartesia_manual_finalize`) [after a live probe] | same | same | `stt_capability_assumed` |
|  | `ink-2`, `ink-preview`, `ink-whisper` (default), `ink-whisper-2025-06-04` | gateway-driven commit (`cartesia_manual_finalize`) [after a live probe] | same | same |  |
| `deepgram` | any other id (`*`) | native stream | same | same | `stt_capability_assumed` (notice) |
|  | `base`, `base-general` (default), `conversationalai`, `enhanced`, `enhanced-finance`, `enhanced-general`, `enhanced-meeting`, `enhanced-phonecall`, `finance`, `meeting` and 22 more | native stream | same | same |  |
|  | `flux-*`, `flux-general-en`, `flux-general-multi` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
|  | `whisper`, `whisper-*`, `whisper-base`, `whisper-large`, `whisper-medium`, `whisper-small`, `whisper-tiny` | per-utterance upload (`deepgram_prerecorded`) [regions except eu, au, in] | same | same | `stt_segmented_mode` |
| `elevenlabs` | any other id (`*`) | per-utterance upload (`elevenlabs_batch`) | same | same | `stt_segmented_mode`, `stt_capability_assumed` |
|  | `*realtime*` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
|  | `scribe_v1` | refused `stt_model_retired` | same | same |  |
|  | `scribe_v2`, `scribe_v2_medical` | per-utterance upload (`elevenlabs_batch`) | same | same | `stt_segmented_mode` |
|  | `scribe_v2_realtime` (default) | native stream | same | same |  |
| `fpt-ai` | any other id (`*`) | per-utterance upload (`regional_rest`) [after a live probe] | same | today's buffering client | voice agent: `stt_segmented_mode`, `stt_capability_assumed`; manual agent or loop: `stt_segmented_mode`, `stt_capability_assumed`; plain: `stt_buffered_until_commit` |
|  | `general` (default) | per-utterance upload (`regional_rest`) [after a live probe] | same | today's buffering client | voice agent: `stt_segmented_mode`; manual agent or loop: `stt_segmented_mode`; plain: `stt_buffered_until_commit` |
| `gladia` | any other id (`*`) | today's client, streaming solaria-1 instead | same | same | `stt_model_substituted` (notice) |
|  | `solaria-1` (default) | native stream | same | same |  |
|  | `solaria-3` | refused `stt_live_unsupported` (`async_only`) | same | same |  |
| `gnani` | any other id (`*`) | per-utterance upload (`regional_rest`) [after a live probe; languages only 10 listed] | same | same | `stt_segmented_mode`, `stt_capability_assumed` |
|  | `default` | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
|  | `gnani-prisma-v2.5` (default), `vachana-audio-intelligence-v2` | per-utterance upload (`regional_rest`) [after a live probe; languages only 10 listed] | same | same | `stt_segmented_mode` |
| `google` | any other id (`*`) | native stream | same | same | `stt_capability_assumed` (notice) |
|  | `chirp`, `latest_short` | per-utterance upload (`google_recognize`) | same | same | `stt_segmented_mode` |
|  | `chirp_2`, `chirp_3`, `chirp_telephony`, `latest_long` (default), `long`, `medical_conversation`, `medical_dictation`, `short`, `telephony`, `telephony_short` | native stream | same | same |  |
| `groq` | any other id (`*`) | per-utterance upload (`groq_transcriptions`) | same | today's buffering client | voice agent: `stt_segmented_mode`, `stt_min_billed_duration`, `stt_capability_assumed`; manual agent or loop: `stt_segmented_mode`, `stt_min_billed_duration`, `stt_capability_assumed`; plain: `stt_buffered_until_commit` |
|  | `distil-whisper-large-v3-en` | refused `stt_model_retired` | same | same |  |
|  | `whisper-large-v3`, `whisper-large-v3-turbo` (default) | per-utterance upload (`groq_transcriptions`) | same | today's buffering client | voice agent: `stt_segmented_mode`, `stt_min_billed_duration`; manual agent or loop: `stt_segmented_mode`, `stt_min_billed_duration`; plain: `stt_buffered_until_commit` |
| `huawei-cloud` | any other id (`*`) | per-utterance upload (`regional_rest`) [after a live probe] | same | same | `stt_segmented_mode`, `stt_capability_assumed` |
|  | `arabic_16k_general` | per-utterance upload (`regional_rest`) [after a live probe; regions except cn-north-4, cn-east-3] | same | same | `stt_segmented_mode` |
|  | `arabic_8k_general`, `chinese_16k_court`, `chinese_16k_it`, `chinese_8k_general`, `english_16k_general` | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
|  | `cantonese_16k_common`, `shanghai_16k_common`, `sichuan_16k_common` | per-utterance upload (`regional_rest`) [after a live probe; regions only cn-north-4] | same | same | `stt_segmented_mode` |
|  | `chinese_16k_common`, `chinese_16k_travel`, `chinese_8k_common` | per-utterance upload (`regional_rest`) [after a live probe; regions only cn-north-4, cn-east-3] | same | same | `stt_segmented_mode` |
|  | `chinese_16k_conversation` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
|  | `chinese_16k_general` (default), `english_8k_common` | per-utterance upload (`regional_rest`) [after a live probe] | same | same | `stt_segmented_mode` |
|  | `chinese_16k_media`, `sichuan_8k_common` | refused `stt_live_unsupported` (`async_only`) | same | same |  |
|  | `english_16k_common` | per-utterance upload (`regional_rest`) [after a live probe; regions except cn-north-4] | same | same | `stt_segmented_mode` |
| `ibm-watson` | any other id (`*`) | native stream | same | same | `stt_capability_assumed` (notice) |
|  | `*_BroadbandModel`, `*_NarrowbandModel` | refused `stt_model_retired` | same | same |  |
|  | `ar-MS_Telephony`, `cs-CZ_Telephony`, `de-DE_Multimedia`, `de-DE_Telephony`, `en-AU_Multimedia`, `en-AU_Telephony`, `en-GB_Multimedia`, `en-GB_Telephony`, `en-IN_Telephony`, `en-US_Multimedia` (default) and 23 more | native stream | same | same |  |
|  | `de-DE`, `en-AU`, `en-GB`, `en-IN`, `en-US`, `es-AR`, `es-CL`, `es-CO`, `es-ES`, `es-MX` and 8 more | native stream (client unverified) | same | same | `stt_client_unverified` (notice) |
| `iflytek` | any other id (`*`) | native stream (client known broken) | same | same | `stt_client_unverified` (notice), `stt_capability_assumed` (notice) |
|  | `*ist*`, `*realtime*`, `*stream*`, `iat` (default), `ist`, `ist_huanyu`, `ist_hy`, `ist_open`, `medical`, `sp_ist_vais` | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
|  | `iflyrec_voice_*`, `iflyrec_voice_cn_10m_ed`, `iflyrec_voice_de_de_24h`, `iflyrec_voice_es_es_24h`, `iflyrec_voice_fr_fr_24h`, `iflyrec_voice_ja_jp_24h`, `iflyrec_voice_ko_kr_24h`, `iflyrec_voice_th_th_sp_24h`, `iflyrec_voice_vi_vn_vais_24h`, `iflyrec_voice_yueyu_24h` | refused `stt_live_unsupported` (`async_only`) | same | same |  |
| `microsoft-azure` | any other id (`*`) | native stream | same | same | `stt_model_substituted` (notice), `stt_capability_assumed` (notice) |
|  | `default` (default) | native stream | same | same |  |
|  | `llm-speech`, `MAI-Transcribe-1.5`, `MAI-Transcribe-2` | per-utterance upload (`azure_fast_transcription`) [regions only 6 listed] | same | same | `stt_segmented_mode` |
|  | `MAI-Transcribe-1` | refused `stt_model_retired` | same | same |  |
|  | `MAI-Transcribe-2-Streaming` | today's client, streaming default instead | same | same | `stt_model_substituted` (notice) |
| `naver-clova` | any other id (`*`) | per-utterance upload (`regional_rest`) [after a live probe; languages only 4 listed] | same | today's buffering client | voice agent: `stt_segmented_mode`, `stt_min_billed_duration`, `stt_capability_assumed`; manual agent or loop: `stt_segmented_mode`, `stt_min_billed_duration`, `stt_capability_assumed`; plain: `stt_buffered_until_commit` |
|  | `clova-speech-long`, `clova-speech-short`, `clova-speech-streaming` | refused `stt_live_unsupported` (`client_not_implemented`) | today's buffering client | today's buffering client | voice agent: none; manual agent or loop: `stt_buffered_until_commit`; plain: `stt_buffered_until_commit` |
|  | `csr` (default) | per-utterance upload (`regional_rest`) [after a live probe; languages only 4 listed] | same | today's buffering client | voice agent: `stt_segmented_mode`, `stt_min_billed_duration`; manual agent or loop: `stt_segmented_mode`, `stt_min_billed_duration`; plain: `stt_buffered_until_commit` |
| `nectec` | any other id (`*`) | per-utterance upload (`regional_rest`) [after a live probe] | same | same | `stt_segmented_mode`, `stt_capability_assumed` |
|  | `partii4` (default) | per-utterance upload (`regional_rest`) [after a live probe] | same | today's buffering client | voice agent: `stt_segmented_mode`; manual agent or loop: `stt_segmented_mode`; plain: `stt_buffered_until_commit` |
|  | `partii5` | refused `stt_model_retired` | same | same |  |
| `openai` | any other id (`*`) | per-utterance upload (`openai_transcriptions`) | same | today's buffering client | voice agent: `stt_segmented_mode`, `stt_capability_assumed`; manual agent or loop: `stt_segmented_mode`, `stt_capability_assumed`; plain: `stt_buffered_until_commit` |
|  | `gpt-4o-mini-transcribe`, `gpt-4o-mini-transcribe-2025-03-20`, `gpt-4o-mini-transcribe-2025-12-15`, `gpt-4o-transcribe`, `gpt-4o-transcribe-diarize`, `whisper-1` | per-utterance upload (`openai_transcriptions`) | same | today's buffering client | voice agent: `stt_segmented_mode`, `stt_model_deprecated`; manual agent or loop: `stt_segmented_mode`, `stt_model_deprecated`; plain: `stt_buffered_until_commit`, `stt_model_deprecated` (notice) |
|  | `gpt-live-transcribe`, `gpt-live-transcribe*`, `gpt-realtime-whisper`, `gpt-realtime-whisper*` | gateway-driven commit (`openai_realtime_transcription`) | same | same |  |
|  | `gpt-transcribe` (default) | per-utterance upload (`openai_transcriptions`) | same | today's buffering client | voice agent: `stt_segmented_mode`; manual agent or loop: `stt_segmented_mode`; plain: `stt_buffered_until_commit` |
| `phonexia` | any other id (`*`) | per-utterance upload (`regional_rest`) [after a live probe] | same | same | `stt_segmented_mode`, `stt_capability_assumed` |
|  | `default`, `large_v2`, `large_v3`, `medium`, `speech-to-text`, `speech-to-text-whisper-enhanced` | per-utterance upload (`regional_rest`) [after a live probe] | same | same | `stt_segmented_mode` |
|  | `EN_US_6` | refused `stt_model_retired` | same | same |  |
| `revai` | any other id (`*`) | native stream | same | same | `stt_model_substituted` (notice), `stt_capability_assumed` (notice) |
|  | `*whisper*`, `fusion`, `low_cost` | refused `stt_live_unsupported` (`async_only`) | same | same |  |
|  | `human` | refused `stt_live_unsupported` (`disabled`) | same | same |  |
|  | `machine` (default), `machine_v2`, `reverb`, `reverb-foreign-language` | native stream | same | same |  |
| `reverie` | any other id (`*`) | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
| `sarvam` | any other id (`*`) | native stream | same | same | `stt_capability_assumed` (notice) |
|  | `saaras:v2.5` | refused `stt_live_unsupported` (`disabled`) | same | same |  |
|  | `saaras:v3`, `saaras:v4` | native stream | same | same |  |
|  | `saaras:v3-realtime` | today's client (behaviour for this model not established) | same | same | `stt_client_unverified` (notice) |
|  | `saarika:v2.5` (default) | native stream | same | same | `stt_model_deprecated` (notice) |
| `sberdevices` | any other id (`*`) | per-utterance upload (`regional_rest`) [after a live probe; languages only 5 listed] | same | same | `stt_segmented_mode`, `stt_capability_assumed` |
|  | `callcenter`, `general` (default), `ivr`, `media` | per-utterance upload (`regional_rest`) [after a live probe; languages only 5 listed] | same | same | `stt_segmented_mode` |
| `self_hosted` | any other id (`*`), `*nemotron*`, `*voxtral*realtime*`, `*whisper*` | per-utterance upload (`openai_transcriptions`) | same | same | `stt_segmented_mode` |
|  | `kyutai/stt-1b-en_fr`, `kyutai/stt-2.6b-en` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
| `speechmatics` | any other id (`*`) | native stream | same | same | `stt_capability_assumed` (notice) |
|  | `enhanced`, `standard` (default) | native stream | same | same |  |
|  | `linden-1` | refused `stt_live_unsupported` (`client_not_implemented`) | same | same |  |
|  | `melia-1`, `oak-1` | per-utterance upload (`speechmatics_batch`) [after a live probe and a deadline measurement; regions only eu1, us1] | same | same | `stt_segmented_mode` |
| `tencent` | any other id (`*`) | per-utterance upload (`regional_rest`) [after a live probe; regions only china] | same | same | `stt_segmented_mode`, `stt_min_billed_duration`, `stt_capability_assumed` |
|  | `16k_ar`, `16k_de`, `16k_en`, `16k_es`, `16k_fil`, `16k_fr`, `16k_hi`, `16k_id`, `16k_ja`, `16k_ko` and 13 more | per-utterance upload (`regional_rest`) [after a live probe; regions only china] | same | same | `stt_segmented_mode`, `stt_min_billed_duration` |
|  | `16k_en_edu`, `16k_en_game`, `16k_en_large`, `16k_zh-TW`, `16k_zh_court`, `16k_zh_edu`, `16k_zh_en_2.0`, `16k_zh_en_speaker_2.0`, `16k_zh_large`, `16k_zh_medical`, `Hy-ASR-3.0-preview` | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
|  | `16k_zh_dialect` | today's client, streaming 16k_zh instead | same | same | `stt_model_substituted` (notice) |
|  | `16k_zh_en_meeting` | refused `stt_live_unsupported` (`async_only`) | same | same |  |
| `tinkoff` | any other id (`*`) | native stream (client known broken) | same | same | `stt_client_unverified` (notice), `stt_model_substituted` (notice), `stt_capability_assumed` (notice) |
|  | `default` (default) | native stream (client known broken) | same | same | `stt_client_unverified` (notice) |
| `viettel-ai` | any other id (`*`) | per-utterance upload (`regional_rest`) [after a live probe and a deadline measurement] | same | same | `stt_segmented_mode` |
| `waav-infer` | any other id (`*`) | per-utterance upload (`openai_transcriptions`) | same | same | `stt_segmented_mode`, `stt_capability_assumed` |
|  | `nemotron*`, `voxtral*realtime*`, `whisper*` | per-utterance upload (`openai_transcriptions`) | same | same | `stt_segmented_mode` |
| `yandex` | any other id (`*`) | per-utterance upload (`regional_rest`) [after a live probe] | same | same | `stt_segmented_mode`, `stt_min_billed_duration`, `stt_capability_assumed` |
|  | `deferred*`, `deferred-general`, `deferred-general:deprecated`, `deferred-general:rc` | refused `stt_live_unsupported` (`async_only`) | same | same |  |
|  | `general` (default), `general:deprecated`, `general:rc` | per-utterance upload (`regional_rest`) [after a live probe] | same | same | `stt_segmented_mode`, `stt_min_billed_duration` |

## Counts per release

Each row of the map counted once (a model or a pattern), for each kind of session, preference `auto`, covered session. "Native" counts every outcome that keeps today's client: a streaming client, a substituting or known-broken client, and a buffering client kept with a warning.

| Release | Session | Native | Per-utterance upload | Gateway-driven commit | Refused | Refusal codes |
| --- | --- | --- | --- | --- | --- | --- |
| 0 | voice agent, automatic turns | 297 | 0 | 0 | 115 | `stt_live_unsupported` 93, `stt_model_retired` 7, `stt_not_streaming` 15 |
| 0 | manual agent, conversation loop or DAG | 329 | 0 | 0 | 83 | `stt_live_unsupported` 61, `stt_model_retired` 7, `unsupported_deployment` 15 |
| 0 | plain /ws session | 329 | 0 | 0 | 83 | `stt_live_unsupported` 61, `stt_model_retired` 7, `unsupported_deployment` 15 |
| 1 | voice agent, automatic turns | 297 | 29 | 0 | 86 | `stt_live_unsupported` 79, `stt_model_retired` 7 |
| 1 | manual agent, conversation loop or DAG | 318 | 29 | 0 | 65 | `stt_live_unsupported` 58, `stt_model_retired` 7 |
| 1 | plain /ws session | 329 | 18 | 0 | 65 | `stt_live_unsupported` 58, `stt_model_retired` 7 |
| 2 | voice agent, automatic turns | 297 | 29 | 0 | 86 | `stt_live_unsupported` 79, `stt_model_retired` 7 |
| 2 | manual agent, conversation loop or DAG | 318 | 29 | 0 | 65 | `stt_live_unsupported` 58, `stt_model_retired` 7 |
| 2 | plain /ws session | 329 | 18 | 0 | 65 | `stt_live_unsupported` 58, `stt_model_retired` 7 |
| 3 | voice agent, automatic turns | 266 | 39 | 0 | 107 | `stt_live_unsupported` 98, `stt_model_retired` 9 |
| 3 | manual agent, conversation loop or DAG | 287 | 39 | 0 | 86 | `stt_live_unsupported` 77, `stt_model_retired` 9 |
| 3 | plain /ws session | 298 | 28 | 0 | 86 | `stt_live_unsupported` 77, `stt_model_retired` 9 |
| 4 | voice agent, automatic turns | 261 | 39 | 11 | 101 | `stt_live_unsupported` 92, `stt_model_retired` 9 |
| 4 | manual agent, conversation loop or DAG | 282 | 39 | 11 | 80 | `stt_live_unsupported` 71, `stt_model_retired` 9 |
| 4 | plain /ws session | 293 | 28 | 11 | 80 | `stt_live_unsupported` 71, `stt_model_retired` 9 |
| 5 | voice agent, automatic turns | 205 | 135 | 11 | 61 | `stt_live_unsupported` 52, `stt_model_retired` 9 |
| 5 | manual agent, conversation loop or DAG | 208 | 135 | 11 | 58 | `stt_live_unsupported` 49, `stt_model_retired` 9 |
| 5 | plain /ws session | 237 | 106 | 11 | 58 | `stt_live_unsupported` 49, `stt_model_retired` 9 |
| 6 | voice agent, automatic turns | 205 | 135 | 11 | 61 | `stt_live_unsupported` 52, `stt_model_retired` 9 |
| 6 | manual agent, conversation loop or DAG | 208 | 135 | 11 | 58 | `stt_live_unsupported` 49, `stt_model_retired` 9 |
| 6 | plain /ws session | 237 | 106 | 11 | 58 | `stt_live_unsupported` 49, `stt_model_retired` 9 |

