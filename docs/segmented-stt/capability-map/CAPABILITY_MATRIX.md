# Speech-to-text capability matrix

**How this file is made.** Generated from `stt_live_capabilities.json` (map version 2026-10-04.1, 412 rows, 34 providers) by running, in `docs/segmented-stt/capability-map/` in the WaaV repository, `python3 resolve.py --matrix > CAPABILITY_MATRIX.md`. Every line is computed from the map and the resolver: the outcomes by calling `resolve()` for each row in each release, the limit, billing, defect and unverified lines from the rows' fields. Outcomes assume what `EXPECTED_RESOLUTION.md` assumes: a covered session, preference `auto`, no language or region given, and transports that need a live probe or a measurement shown as enabled in their release.

Words used. **Vendor interface**: what the vendor offers for the model, as the row's transports record it: *stream* (a socket that transcribes audio as it arrives), *socket with client commit* (the vendor transcribes when the caller's side marks the end of an utterance), *file, one request*, *file, asynchronous job* (submit, then poll; too slow for a call). **Today**: what Release 0, groundwork and honest refusal, gives: today's code plus the honest refusals. **Plan**: each release in which the outcome changes. Outcomes: *native stream* (today's streaming client, unchanged), *today's ... client* (a native client that does not stream, kept with a warning), *per-utterance upload*, *gateway-driven commit*, *refused* with its code. Where the voice-agent and the other sessions differ, both are given.

## Overview

| Provider | Rows | Rows that stream today | New paths, by release | Refused in Release 6 |
| --- | --- | --- | --- | --- |
| `alibaba-cloud` | 50 | 25 | Release 5: `regional_rest` (8) | async_only 17 |
| `amivoice` | 21 | 14 | none | none |
| `assemblyai` | 8 | 2 | Release 3: `assemblyai_sync` (3) | async_only 1 |
| `aws-transcribe` | 5 | 2 | none | none |
| `azure_openai` | 9 | 0 | Release 1: `azure_openai_transcriptions` (7); Release 4: `openai_realtime_transcription` (2) | none |
| `baidu` | 11 | 7 | Release 5: `regional_rest` (7) | stt_model_retired 2 |
| `bhashini` | 13 | 0 | Release 5: `regional_rest` (13) | none |
| `cartesia` | 5 | 5 | Release 4: `cartesia_manual_finalize` (5) | none |
| `deepgram` | 43 | 33 | Release 3: `deepgram_prerecorded` (7) | client_not_implemented 3 |
| `elevenlabs` | 6 | 1 | Release 1: `elevenlabs_batch` (3) | client_not_implemented 1, stt_model_retired 1 |
| `fpt-ai` | 2 | 0 | Release 5: `regional_rest` (2) | none |
| `gladia` | 3 | 1 | none | async_only 1 |
| `gnani` | 4 | 1 | Release 5: `regional_rest` (3) | none |
| `google` | 13 | 13 | Release 5: `google_recognize` (2) | none |
| `groq` | 4 | 0 | Release 1: `groq_transcriptions` (3) | stt_model_retired 1 |
| `huawei-cloud` | 19 | 13 | Release 5: `regional_rest` (11) | async_only 2, client_not_implemented 1 |
| `ibm-watson` | 54 | 52 | none | stt_model_retired 2 |
| `iflytek` | 21 | 11 | none | async_only 10 |
| `microsoft-azure` | 7 | 2 | Release 5: `azure_fast_transcription` (3) | stt_model_retired 1 |
| `naver-clova` | 5 | 0 | Release 5: `regional_rest` (2) | client_not_implemented 3 |
| `nectec` | 3 | 0 | Release 5: `regional_rest` (2) | stt_model_retired 1 |
| `openai` | 12 | 0 | Release 1: `openai_transcriptions` (8); Release 4: `openai_realtime_transcription` (4) | none |
| `phonexia` | 8 | 0 | Release 5: `regional_rest` (7) | stt_model_retired 1 |
| `revai` | 9 | 5 | none | async_only 3, disabled 1 |
| `reverie` | 1 | 1 | none | none |
| `sarvam` | 6 | 4 | none | disabled 1 |
| `sberdevices` | 5 | 0 | Release 5: `regional_rest` (5) | none |
| `self_hosted` | 6 | 0 | Release 1: `openai_transcriptions` (4) | client_not_implemented 2 |
| `speechmatics` | 6 | 3 | Release 5: `speechmatics_batch` (2) | client_not_implemented 1 |
| `tencent` | 37 | 34 | Release 5: `regional_rest` (24) | async_only 1 |
| `tinkoff` | 2 | 2 | none | none |
| `viettel-ai` | 1 | 0 | Release 5: `regional_rest` (1) | none |
| `waav-infer` | 4 | 0 | Release 1: `openai_transcriptions` (4) | none |
| `yandex` | 8 | 0 | Release 5: `regional_rest` (4) | async_only 4 |

Counted for a voice agent with automatic turns. "Rows that stream today" counts rows whose Release 0 outcome is a native streaming client. "New paths" counts rows by the release in which they first get a per-utterance upload or a gateway-driven commit. "Refused in Release 6" counts rows still refused at the end of the plan, by reason (or by code when the code has no reason).

## alibaba-cloud

Aliases `alibaba_cloud`, `alibabacloud`, `alibaba`, `dashscope`, `aliyun`, `阿里云`, `qwen-asr`; model string: wire model; no model named: `qwen3-asr-flash-realtime`; a client is registered today.

Prerequisites: The US (Virginia) region needs a workspace id.

| Models | Vendor interface | Today | Plan |
| --- | --- | --- | --- |
| any other id (`*`) | stream; file, one request | native stream (client unverified) | unchanged |
| `*-asr-flash-message*`, `*-asr-flash-streaming*`, `*-realtime*`, `fun-asr-flash-8k-realtime`, `fun-asr-flash-8k-realtime-2026-01-28`, `fun-asr-mtl-realtime` and 6 more | stream | native stream (client unverified) | unchanged |
| `*-filetrans*`, `fun-asr-202*`, `fun-asr-2025-08-25`, `fun-asr-2025-11-07`, `fun-asr-mtl`, `fun-asr-mtl-2025-08-25` and 6 more | file, asynchronous job only | refused `stt_live_unsupported` (`async_only`) | unchanged |
| `fun-asr` | file, asynchronous job only | today's client, streaming fun-asr-realtime instead | Release 3: refused `stt_live_unsupported` (`async_only`) |
| `fun-asr-flash-2*`, `qwen3-asr-flash-2*`, `qwen3-asr-flash-2025-09-08`, `qwen3-asr-flash-2026-02-10` | file, one request | refused `stt_live_unsupported` (`client_not_implemented`) | Release 5: per-utterance upload (`regional_rest`) [after a live probe] |
| `fun-asr-flash-2026-06-15`, `qwen-audio-3.0-asr-flash`, `qwen-audio-3.1-asr-flash` | file, one request | refused `stt_live_unsupported` (`client_not_implemented`) | Release 5: per-utterance upload (`regional_rest`) [after a live probe; regions only beijing, singapore] |
| `fun-asr-realtime`, `fun-asr-realtime-2025-09-15`, `fun-asr-realtime-2025-11-07`, `fun-asr-realtime-2026-02-28`, `paraformer-realtime-8k-v1`, `paraformer-realtime-8k-v2`, `paraformer-realtime-v1`, `paraformer-realtime-v2` | stream | native stream | unchanged |
| `paraformer-8k-v1` | file, asynchronous job only | today's client, streaming paraformer-realtime-8k-v1 instead | Release 3: refused `stt_live_unsupported` (`async_only`) |
| `paraformer-8k-v2` | file, asynchronous job only | today's client, streaming paraformer-realtime-8k-v2 instead | Release 3: refused `stt_live_unsupported` (`async_only`) |
| `paraformer-v1` | file, asynchronous job only | today's client, streaming paraformer-realtime-v1 instead | Release 3: refused `stt_live_unsupported` (`async_only`) |
| `paraformer-v2` | file, asynchronous job only | today's client, streaming paraformer-realtime-v2 instead | Release 3: refused `stt_live_unsupported` (`async_only`) |
| `qwen3-asr-flash` | file, one request | refused `stt_live_unsupported` (`client_not_implemented`) | Release 5: per-utterance upload (`regional_rest`) [after a live probe; regions only beijing, singapore, us-east-1] |
| `qwen3-asr-flash-realtime` (default), `qwen3-asr-flash-realtime*`, `qwen3-asr-flash-realtime-2025-10-27`, `qwen3-asr-flash-realtime-2026-02-10` | stream; socket with client commit | native stream (client known broken) | unchanged |

- `native` (today; figures of `qwen3-asr-flash-realtime`): limits 20 requests per second (scope account), billed per audio second.
- `regional_rest` (from Release 5; figures of `fun-asr-flash-2026-06-15`): at most 10 MB per request, at most 5 min of audio per request, limits 600 requests per minute (scope account), billed per audio second, only uploaded speech billed.
- Other rows of this provider carry different limits or billing; the map has each one.
- Today's client, defect found by reading: The gateway's Qwen realtime client does not match the vendor's documented protocol. The gateway sends this id to the wrong protocol.
- Unverified (55 distinct items across the rows), for example: `qwen3-asr-flash-realtime`: lifecycle.status (stable alias that the vendor describes as currently equivalent to a snapshot retiring on 2026-10-10; what it resolves to …; `qwen3-asr-flash-realtime`: transports[0].latency (no figure published; never measured); `qwen3-asr-flash-realtime`: transports[0] and transports[1]: limits.max_session_ms.

## amivoice

Aliases `amivoice-stt`, `ami`, `advanced-media`, `アミボイス`, `acp`; model string: wire model; no model named: `-a-general`; a client is registered today; today's client replaces a model id it does not know.

| Models | Vendor interface | Today | Plan |
| --- | --- | --- | --- |
| any other id (`*`) | file, one request; stream | native stream (client known broken) | unchanged |
| `-a-address-input-private`, `-a-bizfinance`, `-a-bizfinance-input`, `-a-bizinsurance`, `-a-bizinsurance-input`, `-a-name-input-private`, `-a-rule-input-private` | socket with client commit; file, one request | today's client, streaming -a-general instead | unchanged |
| `-a-general` (default), `-a-general-en`, `-a-general-input`, `-a-general-ko`, `-a-general-zh`, `-a-medical` and 7 more | stream; socket with client commit; file, one request | native stream (client known broken) | unchanged |

- `native` (today; figures of `-a-general`): a connection lasts at most 24 h, closes after 1 min idle, billed per audio second.
- Today's client, defect found by reading: Known broken: the client does not match the vendor's wire protocol. Known broken: audio frames lack the mandatory 0x70 prefix byte (gateway/src/core/stt/amivoice/client.rs:266-268) and raw PCM is declared with header-format names (gateway/src/core/stt/amivoice/config.rs:419-425).
- Unverified (15 distinct items across the rows), for example: `-a-general`: transports[native].gateway_client (whether the current client really fails against the live service; the protocol mismatch was read on …; `-a-general`: transports[*].latency (no percentile is published for either interface); `-a-general`: transports[*].limits.rates (concurrency and requests) (the vendor publishes no limit and asks for notice before heavy parallel use).

## assemblyai

model string: wire model; no model named: `universal-streaming-english`; a client is registered today; today's client replaces a model id it does not know.

| Models | Vendor interface | Today | Plan |
| --- | --- | --- | --- |
| any other id (`*`) | file, one request | today's client, streaming universal-streaming-english instead | unchanged |
| `u3-pro`, `u3-sync-pro` | file, one request | today's client, streaming universal-streaming-english instead | Release 3: per-utterance upload (`assemblyai_sync`) |
| `universal-2` | file, asynchronous job | today's client, streaming universal-streaming-english instead | Release 3: refused `stt_live_unsupported` (`async_only`) |
| `universal-3-5-pro` | file, one request; file, asynchronous job | today's client, streaming universal-streaming-english instead | Release 3: per-utterance upload (`assemblyai_sync`) |
| `universal-3-6-pro` | no transport recorded in the row | today's client, streaming universal-streaming-english instead | unchanged |
| `universal-streaming-english` (default), `universal-streaming-multilingual` | stream | native stream | unchanged |

- `native` (today; figures of `universal-streaming-english`): a connection lasts at most 3 h, limits 100 requests per minute (scope unknown; plan paid assumed), billed per audio hour, all call audio billed.
- `assemblyai_sync` (from Release 3; figures of `u3-pro`): at most 40 MB per request, at most 2 min of audio per request, billed per audio hour, +11.11% with prompt, only uploaded speech billed.
- Unverified (25 distinct items across the rows), for example: `universal-streaming-english`: transports[0].latency (which model and which end-of-turn settings Pipecat measured are not recorded; the vendor's own benchmark reports a …; `universal-streaming-english`: transports[0].vendor_turn_signal (classified as semantic because the vendor ends turns with an end-of-turn confidence threshold on the …; `universal-streaming-english`: transports[0].limits.rates (requests) (the vendor limits new sessions per minute: 100 is where paid plans start, and they scale up ….

## aws-transcribe

Aliases `aws_transcribe`, `amazon-transcribe`, `transcribe`; model string: gateway alias; no model named: `standard`; a client is registered today; today's client ignores the model id.

| Models | Vendor interface | Today | Plan |
| --- | --- | --- | --- |
| any other id (`*`), `standard` (default) | stream | native stream | unchanged |
| `call-analytics`, `healthscribe`, `medical` | stream | today's client, streaming standard instead | unchanged |

- `native` (today; figures of `standard`): a connection lasts at most 8 h, limits 25 requests per second, 25 open sessions (scope region), billed per audio minute.
- Other rows of this provider carry different limits or billing; the map has each one.
- Unverified (14 distinct items across the rows), for example: `standard`: transports[0].latency (the model and settings behind Pipecat's figure are not stated); `standard`: transports[0].limits.max_session_ms (Amazon's FAQ says a streaming connection lasts up to eight hours; older sources and the gateway's own …; `standard`: transports[0].limits.rates (requests) (Amazon states 25 StartStreamTranscription requests per second and that window is recorded; version ….

## azure_openai

Aliases `azure-openai`; model string: deployment name; no model named: the provider default row; no live client is registered today.

| Models | Vendor interface | Today | Plan |
| --- | --- | --- | --- |
| any other id (`*`), `gpt-4o-mini-transcribe`, `gpt-4o-transcribe`, `gpt-4o-transcribe-diarize`, `gpt-transcribe`, `whisper`, `whisper-*` | file, one request | voice agent, automatic turns: refused `stt_not_streaming`; manual agent, conversation loop or DAG: refused `unsupported_deployment`; plain /ws session: refused `unsupported_deployment` | Release 1: per-utterance upload (`azure_openai_transcriptions`) |
| `gpt-live-transcribe`, `gpt-realtime-whisper` | socket with client commit | voice agent, automatic turns: refused `stt_not_streaming`; manual agent, conversation loop or DAG: refused `unsupported_deployment`; plain /ws session: refused `unsupported_deployment` | Release 1: refused `stt_live_unsupported` (`client_not_implemented`); Release 4: gateway-driven commit (`openai_realtime_transcription`) [after a live probe] |

- `azure_openai_transcriptions` (from Release 1; figures of `gpt-4o-mini-transcribe`): at most 20 MiB per request, one upload per caller turn, limits 3 requests per minute (scope unknown), billing unit unknown, only uploaded speech billed.
- `openai_realtime_transcription` (from Release 4; figures of `gpt-live-transcribe`): a connection lasts at most 1 h, limits 10 open sessions (scope account), billing unit unknown.
- Other rows of this provider carry different limits or billing; the map has each one.
- Unverified (33 distinct items across the rows), for example: `gpt-4o-mini-transcribe`: transports[0].latency (none published); `gpt-4o-mini-transcribe`: transports[0].limits.max_upload_bytes (Microsoft pages disagree: 'Audio files must be 25 MB or smaller' against 'Message size for audio …; `gpt-4o-mini-transcribe`: transports[0].limits.max_audio_ms.

## baidu

Aliases `baidu-ai`, `baidu_ai`, `baiduai`, `百度`, `百度语音`, `baidu-speech`, `baidu_speech`; model string: wire model; no model named: `1537`; a client is registered today; today's client replaces a model id it does not know.

Prerequisites: The pro API needs the token scope brain_enhanced_asr.

| Models | Vendor interface | Today | Plan |
| --- | --- | --- | --- |
| any other id (`*`), `1537` (default), `1637`, `1737`, `1837` | file, one request; stream | native stream (client known broken) | Release 5: per-utterance upload (`regional_rest`) [after a live probe] |
| `15372`, `15376` | stream | native stream (client known broken) | unchanged |
| `1936` | retired | refused `stt_model_retired` | unchanged |
| `19362` | retired | today's client, streaming 1537 instead | Release 3: refused `stt_model_retired` |
| `80001`, `8001` | file, one request | today's client, streaming 1537 instead | Release 5: per-utterance upload (`regional_rest`) [after a live probe] |

- `native` (today; figures of `15372`): closes after 5 s idle, billed per audio hour.
- `regional_rest` (from Release 5; figures of `1537`): at most 1 min of audio per request, limits 2 uploads in flight (scope application), billed per request, only uploaded speech billed.
- Other rows of this provider carry different limits or billing; the map has each one.
- Today's client, defect found by reading: Known broken against the vendor protocol: the START frame sends the API Key and Secret Key where the vendor expects an integer AppID and the API Key, so the client very probably cannot authenticate.
- Unverified (24 distinct items across the rows), for example: `15372`: limits.max_session_ms and limits.rates (concurrent_sessions) of the streaming transport, and billing.bills_silence; `15372`: whether the live service rejects the gateway's START frame (inferred from the official demo code); `15372`: billing.unit (per call for the file API and per hour for the socket, from an undated third-party mirror of the price list).

## bhashini

Aliases `bhashini-stt`, `bhashini_stt`, `ulca`, `ai4bharat`, `ai4bharat-stt`, `meity`, `meity-stt`; model string: wire model; no model named: the provider default row; a client is registered today.

Prerequisites: The service id and the compute address come from the vendor's pipeline configuration call.

| Models | Vendor interface | Today | Plan |
| --- | --- | --- | --- |
| any other id (`*`), `bhashini/bodhan/asr-transcribe-core`, `bhashini/bodhan/asr-transcribe-flex` | file, one request; stream | voice agent, automatic turns: refused `stt_live_unsupported` (`client_not_implemented`); manual agent, conversation loop or DAG: today's buffering client; plain /ws session: today's buffering client | Release 5: voice agent, automatic turns: per-utterance upload (`regional_rest`) [after a live probe]; manual agent, conversation loop or DAG: per-utterance upload (`regional_rest`) [after a live probe]; plain /ws session: today's buffering client |
| `ai4bharat/conformer-hi-gpu--t4` | file, one request; stream | voice agent, automatic turns: refused `stt_live_unsupported` (`client_not_implemented`); manual agent, conversation loop or DAG: today's buffering client; plain /ws session: today's buffering client | Release 5: voice agent, automatic turns: per-utterance upload (`regional_rest`) [after a live probe; languages only hi]; manual agent, conversation loop or DAG: per-utterance upload (`regional_rest`) [after a live probe; languages only hi]; plain /ws session: today's buffering client |
| `ai4bharat/conformer-multilingual-dravidian-gpu--t4`, `bhashini/iitm/asr-dravidian--gpu--t4` | file, one request; stream | voice agent, automatic turns: refused `stt_live_unsupported` (`client_not_implemented`); manual agent, conversation loop or DAG: today's buffering client; plain /ws session: today's buffering client | Release 5: voice agent, automatic turns: per-utterance upload (`regional_rest`) [after a live probe; languages only 4 listed]; manual agent, conversation loop or DAG: per-utterance upload (`regional_rest`) [after a live probe; languages only 4 listed]; plain /ws session: today's buffering client |
| `ai4bharat/conformer-multilingual-indo_aryan-gpu--t4` | file, one request; stream | voice agent, automatic turns: refused `stt_live_unsupported` (`client_not_implemented`); manual agent, conversation loop or DAG: today's buffering client; plain /ws session: today's buffering client | Release 5: voice agent, automatic turns: per-utterance upload (`regional_rest`) [after a live probe; languages only 8 listed]; manual agent, conversation loop or DAG: per-utterance upload (`regional_rest`) [after a live probe; languages only 8 listed]; plain /ws session: today's buffering client |
| `ai4bharat/whisper-medium-en--gpu--t4` | file, one request; stream | voice agent, automatic turns: refused `stt_live_unsupported` (`client_not_implemented`); manual agent, conversation loop or DAG: today's buffering client; plain /ws session: today's buffering client | Release 5: voice agent, automatic turns: per-utterance upload (`regional_rest`) [after a live probe; languages only en]; manual agent, conversation loop or DAG: per-utterance upload (`regional_rest`) [after a live probe; languages only en]; plain /ws session: today's buffering client |
| `bhashini/ai4bharat/conformer-multilingual-asr` | file, one request; stream | voice agent, automatic turns: refused `stt_live_unsupported` (`client_not_implemented`); manual agent, conversation loop or DAG: today's buffering client; plain /ws session: today's buffering client | Release 5: voice agent, automatic turns: per-utterance upload (`regional_rest`) [after a live probe; languages only 23 listed]; manual agent, conversation loop or DAG: per-utterance upload (`regional_rest`) [after a live probe; languages only 23 listed]; plain /ws session: today's buffering client |
| `bhashini/iisc/asr-bho-t4` | file, one request; stream | voice agent, automatic turns: refused `stt_live_unsupported` (`client_not_implemented`); manual agent, conversation loop or DAG: today's buffering client; plain /ws session: today's buffering client | Release 5: voice agent, automatic turns: per-utterance upload (`regional_rest`) [after a live probe; languages only bho]; manual agent, conversation loop or DAG: per-utterance upload (`regional_rest`) [after a live probe; languages only bho]; plain /ws session: today's buffering client |
| `bhashini/iisc/asr-mai-t4` | file, one request; stream | voice agent, automatic turns: refused `stt_live_unsupported` (`client_not_implemented`); manual agent, conversation loop or DAG: today's buffering client; plain /ws session: today's buffering client | Release 5: voice agent, automatic turns: per-utterance upload (`regional_rest`) [after a live probe; languages only mai]; manual agent, conversation loop or DAG: per-utterance upload (`regional_rest`) [after a live probe; languages only mai]; plain /ws session: today's buffering client |
| `bhashini/iitm/asr-indoaryan--gpu--t4` | file, one request; stream | voice agent, automatic turns: refused `stt_live_unsupported` (`client_not_implemented`); manual agent, conversation loop or DAG: today's buffering client; plain /ws session: today's buffering client | Release 5: voice agent, automatic turns: per-utterance upload (`regional_rest`) [after a live probe; languages only 6 listed]; manual agent, conversation loop or DAG: per-utterance upload (`regional_rest`) [after a live probe; languages only 6 listed]; plain /ws session: today's buffering client |
| `bhashini/iitm/asr-misc--gpu--t4` | file, one request; stream | voice agent, automatic turns: refused `stt_live_unsupported` (`client_not_implemented`); manual agent, conversation loop or DAG: today's buffering client; plain /ws session: today's buffering client | Release 5: voice agent, automatic turns: per-utterance upload (`regional_rest`) [after a live probe; languages only bho, ur]; manual agent, conversation loop or DAG: per-utterance upload (`regional_rest`) [after a live probe; languages only bho, ur]; plain /ws session: today's buffering client |

- `regional_rest` (from Release 5; figures of `ai4bharat/conformer-hi-gpu--t4`): at most 30 s of audio per request, billing unit unknown.
- Unverified (11 distinct items across the rows), for example: `ai4bharat/conformer-hi-gpu--t4`: transports[0].limits.max_upload_bytes and the hard maximum audio duration (not published; 30 s is the documented length beyond which the …; `ai4bharat/conformer-hi-gpu--t4`: transports[0].limits.min_audio_ms and rates (no limits are published; the vendor's error page is empty); `ai4bharat/conformer-hi-gpu--t4`: transports[0].latency (no vendor figure and no credentials to measure).

## cartesia

model string: wire model; no model named: `ink-whisper`; a client is registered today.

| Models | Vendor interface | Today | Plan |
| --- | --- | --- | --- |
| any other id (`*`), `ink-2`, `ink-preview` | socket with client commit; stream | native stream | Release 4: gateway-driven commit (`cartesia_manual_finalize`) [after a live probe] |
| `ink-whisper` (default), `ink-whisper-2025-06-04` | socket with client commit; stream; file, one request | native stream | Release 4: gateway-driven commit (`cartesia_manual_finalize`) [after a live probe] |

- `native` (today; figures of `ink-whisper`): closes after 3 min idle, limits 12 open sessions (scope account; plan pro assumed), billed per audio second, all call audio billed.
- `cartesia_manual_finalize` (from Release 4; figures of `ink-whisper`): closes after 3 min idle, limits 12 open sessions (scope account; plan pro assumed), billed per audio second, all call audio billed.
- Unverified (19 distinct items across the rows), for example: `ink-whisper`: the existing streaming client against the live vendor: whether Cartesia still accepts the api_key query parameter, the JSON-quoted …; `ink-whisper`: transports[1].latency (no figure for the socket without finalize); `ink-whisper`: limits.rates (concurrency) on every transport (12 is the Pro plan, the lowest paid plan; Free is 8, Startup 20, Scale 60, Enterprise by ….

## deepgram

model string: wire model; no model named: `base-general`; a client is registered today.

| Models | Vendor interface | Today | Plan |
| --- | --- | --- | --- |
| any other id (`*`) | stream; file, one request | native stream | unchanged |
| `base`, `base-general` (default), `conversationalai`, `enhanced`, `enhanced-finance`, `enhanced-general` and 26 more | stream; socket with client commit; file, one request | native stream | unchanged |
| `flux-*`, `flux-general-en`, `flux-general-multi` | stream | refused `stt_live_unsupported` (`client_not_implemented`) | unchanged |
| `whisper`, `whisper-*`, `whisper-base`, `whisper-large`, `whisper-medium`, `whisper-small`, `whisper-tiny` | file, one request | refused `stt_live_unsupported` (`client_not_implemented`) | Release 3: per-utterance upload (`deepgram_prerecorded`) [regions except eu, au, in] |

- `native` (today; figures of `base-general`): limits 150 open sessions (scope project; plan pay_as_you_go assumed), billed per audio second.
- `deepgram_prerecorded` (from Release 3; figures of `whisper`): at most 2000 MB per request, limits 3 uploads in flight (scope project), billed per audio second, only uploaded speech billed.
- Other rows of this provider carry different limits or billing; the map has each one.
- Unverified (33 distinct items across the rows), for example: `base-general`: whether the streaming socket accepts this id when it is sent explicitly: the streaming specification's closed model list has 'base' and …; `base-general`: transports[0].latency (Pipecat's Deepgram figure was measured on nova-3-general, not on this model); `base-general`: transports[0].limits.max_session_ms (not stated in the evidence).

## elevenlabs

model string: wire model; no model named: `scribe_v2_realtime`; a client is registered today.

| Models | Vendor interface | Today | Plan |
| --- | --- | --- | --- |
| any other id (`*`), `scribe_v2`, `scribe_v2_medical` | file, one request | refused `stt_live_unsupported` (`client_not_implemented`) | Release 1: per-utterance upload (`elevenlabs_batch`) |
| `*realtime*` | no transport recorded in the row | refused `stt_live_unsupported` (`client_not_implemented`) | unchanged |
| `scribe_v1` | retired | refused `stt_model_retired` | unchanged |
| `scribe_v2_realtime` (default) | stream; socket with client commit | native stream | unchanged |

- `native` (today; figures of `scribe_v2_realtime`): closes after 15 s idle, limits 9 open sessions (scope organisation; plan starter assumed), billed per audio hour.
- `elevenlabs_batch` (from Release 1; figures of `scribe_v2`): at most 5000 MB per request, at most 10 h of audio per request, limits 12 uploads in flight (scope organisation; plan starter assumed), billed per audio hour, minimum 20 s with keyterms above 100, minimum 10 s with transcript_edit, +20% with keyterms, +30% with entity_detection, +30% with transcript_edit, only uploaded speech billed.
- Other rows of this provider carry different limits or billing; the map has each one.
- Unverified (34 distinct items across the rows), for example: `scribe_v2_realtime`: transports[0].latency (unmeasured for the vendor voice-activity commit that the gateway uses today); `scribe_v2_realtime`: transports[0].gateway_client.status (verified_live rests on one live run on 2026-06-04; the handshake code changed on 2026-07-12, the test …; `scribe_v2_realtime`: transports[*].limits.max_session_ms (a session_time_limit_exceeded error type exists; no number is in the evidence).

## fpt-ai

Aliases `fpt_ai`, `fptai`, `fpt`, `fpt-stt`, `fpt_ai-stt`; model string: gateway alias; no model named: `general`; a client is registered today.

| Models | Vendor interface | Today | Plan |
| --- | --- | --- | --- |
| any other id (`*`), `general` (default) | file, one request | voice agent, automatic turns: refused `stt_live_unsupported` (`client_not_implemented`); manual agent, conversation loop or DAG: today's buffering client; plain /ws session: today's buffering client | Release 5: voice agent, automatic turns: per-utterance upload (`regional_rest`) [after a live probe]; manual agent, conversation loop or DAG: per-utterance upload (`regional_rest`) [after a live probe]; plain /ws session: today's buffering client |

- `regional_rest` (from Release 5; figures of `general`): billed per audio minute, only uploaded speech billed.
- Unverified (13 distinct items across the rows), for example: `general`: transports[0].upload.accepted and upload.preferred (the vendor says only 'Audio file'; WAV is what the gateway sends and was never …; `general`: transports[0].limits.max_upload_bytes; `general`: transports[0].limits.max_audio_ms (the gateway's 5-minute constant has no vendor source).

## gladia

Aliases `gladia.io`, `gladia-io`, `gladia_io`, `gladia-stt`; model string: wire model; no model named: `solaria-1`; a client is registered today; today's client ignores the model id.

| Models | Vendor interface | Today | Plan |
| --- | --- | --- | --- |
| any other id (`*`) | file, asynchronous job | today's client, streaming solaria-1 instead | unchanged |
| `solaria-1` (default) | stream; file, asynchronous job | native stream | unchanged |
| `solaria-3` | file, asynchronous job | today's client, streaming solaria-1 instead | Release 3: refused `stt_live_unsupported` (`async_only`) |

- `native` (today; figures of `solaria-1`): a connection lasts at most 3 h, limits 30 open sessions (scope account; plan paid assumed), billed per audio hour.
- Unverified (14 distinct items across the rows), for example: `solaria-1`: transports[0].limits.rates (concurrent_sessions) (30 live sessions is the paid-plan figure; the free plan allows 1); `solaria-1`: transports[0].gateway_client (never run against Gladia; whether Gladia rejects or ignores the request fields that have drifted from its …; `solaria-1`: billing.bills_silence and billing.min_billed_ms (not in the evidence for live sessions).

## gnani

Aliases `gnani-ai`, `gnani.ai`, `vachana`; model string: wire model; no model named: `gnani-prisma-v2.5`; a client is registered today; today's client ignores the model id.

Prerequisites: The legacy gRPC service needs the vendor's CA certificate, which today's client reads only from the process environment (GNANI_CERTIFICATE_PATH or GNANI_CERTIFICATE_CONTENT).

| Models | Vendor interface | Today | Plan |
| --- | --- | --- | --- |
| any other id (`*`), `gnani-prisma-v2.5` (default), `vachana-audio-intelligence-v2` | file, one request; stream | today's client (behaviour for this model not established) | Release 5: per-utterance upload (`regional_rest`) [after a live probe; languages only 10 listed] |
| `default` | stream | native stream (client known broken) | unchanged |

- `native` (today; figures of `default`): billing unit unknown.
- `regional_rest` (from Release 5; figures of `gnani-prisma-v2.5`): at most 1 min of audio per request, billing unit unknown.
- Today's client, defect found by reading: The existing gRPC client calls the method path /Listener/DoSpeechToText, while the vendor's published client stub calls /SpeechToText.Listener/DoSpeechToText, so the vendor server is expected to reject the call as unimplemented.
- Unverified (24 distinct items across the rows), for example: `default`: lifecycle.status (the legacy service is absent from the vendor's current documentation; no deprecation notice or date was found and it is …; `default`: transports[0] as a whole (whether the service still accepts a live stream: an unauthenticated probe returned an HTML 403 from Cloudflare, …; `default`: transports[0].vendor_turn_signal (recorded as unknown: the vendor marks results as final; how it decides is not documented).

## google

model string: wire model; no model named: `latest_long`; a client is registered today.

| Models | Vendor interface | Today | Plan |
| --- | --- | --- | --- |
| any other id (`*`), `chirp_2`, `chirp_3`, `chirp_telephony`, `latest_long` (default), `long` and 5 more | stream; file, one request | native stream | unchanged |
| `chirp` | file, one request; stream | native stream (client unverified) | Release 5: per-utterance upload (`google_recognize`) |
| `latest_short` | file, one request; stream | native stream (client known broken) | Release 5: per-utterance upload (`google_recognize`) |

- `native` (today; figures of `latest_long`): a connection lasts at most 5 min, limits 300 open sessions (scope project), billed per audio minute.
- `google_recognize` (from Release 5; figures of `chirp`): at most 10 MB per request, at most 1 min of audio per request, limits 300 requests per minute (scope project), billed per audio minute, only uploaded speech billed.
- Other rows of this provider carry different limits or billing; the map has each one.
- Today's client, defect found by reading: Google closes the stream on this model after every utterance, and the gateway's Google client has no single-utterance handling.
- Unverified (27 distinct items across the rows), for example: `latest_long`: transports[0] (no official per-method statement says Speech-to-Text V2 streams this model; the only evidence is third party: LiveKit's …; `latest_long`: transports[1] (whether V2 Recognize accepts this model is not documented); `latest_long`: what Google does when the model field is empty, which is what a session with no model sends today (the audit inferred a rejection; not ….

## groq

model string: wire model; no model named: `whisper-large-v3-turbo`; a client is registered today.

| Models | Vendor interface | Today | Plan |
| --- | --- | --- | --- |
| any other id (`*`), `whisper-large-v3`, `whisper-large-v3-turbo` (default) | file, one request | voice agent, automatic turns: refused `stt_live_unsupported` (`client_not_implemented`); manual agent, conversation loop or DAG: today's buffering client; plain /ws session: today's buffering client | Release 1: voice agent, automatic turns: per-utterance upload (`groq_transcriptions`); manual agent, conversation loop or DAG: per-utterance upload (`groq_transcriptions`); plain /ws session: today's buffering client |
| `distil-whisper-large-v3-en` | retired | refused `stt_model_retired` | unchanged |

- `groq_transcriptions` (from Release 1; figures of `whisper-large-v3-turbo`): at most 25 MiB per request, limits 400 requests per minute, 400,000 audio seconds per hour (scope organisation; plan developer assumed), billed per audio hour, minimum 10 s per request, only uploaded speech billed.
- Other rows of this provider carry different limits or billing; the map has each one.
- Unverified (15 distinct items across the rows), for example: `whisper-large-v3-turbo`: transports[0].limits.max_audio_ms (only a file size limit is documented); `whisper-large-v3-turbo`: transports[0].limits.rates (concurrent_requests) (none documented); `whisper-large-v3-turbo`: transports[0].limits.rates (plan developer) (Developer plan values, confirmed on the models page only; the free-plan entries come from the ….

## huawei-cloud

Aliases `huawei_cloud`, `huaweicloud`, `huawei`, `华为云`, `华为`, `sis`, `huawei-sis`; model string: wire model; no model named: `chinese_16k_general`; a client is registered today; today's client replaces a model id it does not know.

| Models | Vendor interface | Today | Plan |
| --- | --- | --- | --- |
| any other id (`*`) | file, one request; stream | native stream (client known broken) | Release 5: per-utterance upload (`regional_rest`) [after a live probe] |
| `arabic_16k_general` | socket with client commit; file, one request; stream | native stream (client known broken) | Release 5: per-utterance upload (`regional_rest`) [after a live probe; regions except cn-north-4, cn-east-3] |
| `arabic_8k_general`, `chinese_16k_court`, `chinese_16k_it`, `chinese_8k_general`, `english_16k_general` | stream; socket with client commit | native stream (client known broken) | unchanged |
| `cantonese_16k_common`, `shanghai_16k_common`, `sichuan_16k_common` | socket with client commit; file, one request; stream | native stream (client known broken) | Release 5: per-utterance upload (`regional_rest`) [after a live probe; regions only cn-north-4] |
| `chinese_16k_common`, `chinese_8k_common` | socket with client commit; file, one request; stream | native stream (client known broken) | Release 5: per-utterance upload (`regional_rest`) [after a live probe; regions only cn-north-4, cn-east-3] |
| `chinese_16k_conversation` | no transport recorded in the row | refused `stt_live_unsupported` (`client_not_implemented`) | unchanged |
| `chinese_16k_general` (default) | socket with client commit; file, one request; stream | native stream (client known broken) | Release 5: per-utterance upload (`regional_rest`) [after a live probe] |
| `chinese_16k_media`, `sichuan_8k_common` | file, asynchronous job only | refused `stt_live_unsupported` (`async_only`) | unchanged |
| `chinese_16k_travel` | file, one request | refused `stt_live_unsupported` (`client_not_implemented`) | Release 5: per-utterance upload (`regional_rest`) [after a live probe; regions only cn-north-4, cn-east-3] |
| `english_16k_common` | file, one request | refused `stt_live_unsupported` (`client_not_implemented`) | Release 5: per-utterance upload (`regional_rest`) [after a live probe; regions except cn-north-4] |
| `english_8k_common` | file, one request | refused `stt_live_unsupported` (`client_not_implemented`) | Release 5: per-utterance upload (`regional_rest`) [after a live probe] |

- `native` (today; figures of `arabic_8k_general`): a connection lasts at most 5 h, closes after 20 s idle, billing unit unknown.
- `regional_rest` (from Release 5; figures of `chinese_16k_general`): at most 4 MB per request, at most 1 min of audio per request, billed per request, only uploaded speech billed.
- Other rows of this provider carry different limits or billing; the map has each one.
- Today's client, defect found by reading: Known broken against the vendor protocol: the client dials a host that does not exist for China regions, expects a response schema the vendor does not send, and can only reach the one-minute one-sentence mode.
- Unverified (14 distinct items across the rows), for example: `arabic_8k_general`: limits.rates (concurrent_sessions) of both socket transports (not published); `arabic_8k_general`: billing.unit and billing.bills_silence (billed by audio duration; granularity and price not published); `arabic_8k_general`: whether one WebSocket may carry several START and END cycles (matters for the per-utterance transport).

## ibm-watson

Aliases `ibm_watson`, `watson`, `ibm`; model string: wire model; no model named: `en-US_Multimedia`; a client is registered today.

| Models | Vendor interface | Today | Plan |
| --- | --- | --- | --- |
| any other id (`*`) | stream; file, one request; socket with client commit | native stream | unchanged |
| `*_BroadbandModel`, `*_NarrowbandModel` | retired | refused `stt_model_retired` | unchanged |
| `ar-MS_Telephony`, `cs-CZ_Telephony`, `de-DE_Multimedia`, `de-DE_Telephony`, `en-AU_Multimedia`, `en-AU_Telephony` and 27 more | stream; socket with client commit; file, one request | native stream | unchanged |
| `de-DE`, `en-AU`, `en-GB`, `en-IN`, `en-US`, `es-AR` and 12 more | stream; socket with client commit; file, one request | native stream (client unverified) | unchanged |

- `native` (today; figures of `en-US_Multimedia`): limits 100 open sessions (scope resource; plan plus assumed), billed per audio minute, all call audio billed.
- Unverified (18 distinct items across the rows), for example: `en-US_Multimedia`: transports[*].latency (IBM publishes no latency figure for the WebSocket or the HTTP interface); `en-US_Multimedia`: transports[*].limits.rates (concurrency) and limits.rates[].scope (100 is the Plus plan, counted across WebSocket and HTTP for one service …; `en-US_Multimedia`: transports[planned_file].limits.max_upload_bytes (IBM says 100 MB; recorded as 100,000,000 bytes).

## iflytek

Aliases `iflytek-stt`, `iflytek_stt`, `ifly`, `xfyun`, `xunfei`, `讯飞`, `科大讯飞`; model string: gateway alias; no model named: `iat`; a client is registered today.

| Models | Vendor interface | Today | Plan |
| --- | --- | --- | --- |
| any other id (`*`), `iat` (default), `medical` | socket with client commit; stream | native stream (client known broken) | unchanged |
| `*ist*`, `*realtime*`, `*stream*`, `ist`, `ist_huanyu`, `ist_hy`, `ist_open`, `sp_ist_vais` | stream | native stream (client known broken) | unchanged |
| `iflyrec_voice_*`, `iflyrec_voice_cn_10m_ed`, `iflyrec_voice_de_de_24h`, `iflyrec_voice_es_es_24h`, `iflyrec_voice_fr_fr_24h`, `iflyrec_voice_ja_jp_24h` and 4 more | file, asynchronous job only | today's client, streaming iat instead | Release 3: refused `stt_live_unsupported` (`async_only`) |

- `native` (today; figures of `iat`): a connection lasts at most 1 min, closes after 10 s idle, limits 50 open sessions (scope unknown), billed per request.
- Other rows of this provider carry different limits or billing; the map has each one.
- Today's client, defect found by reading: Known broken for a call: the short-form session ends at the vendor's first final result and is never reopened, so only the first utterance is transcribed, and results are emitted as unassembled fragments. Known broken against the documented protocol of the real-time socket: the client always sends domain 'iat' where the vendor documents ist_open, ist, ist_hy, ist_huanyu and sp_ist_vais, sends punctuation as 'ptt' instead of 'punc', sends 'vad_eos' although … (1 more in the map.)
- Unverified (14 distinct items across the rows), for example: `iat`: limits.rates[].scope (the vendor says 50 channels by default and 5 on the free trial, without saying whether that is per application or …; `iat`: whether languages other than Chinese and English work on this socket URL (the page says minority languages use a different URL but lists …; `iat`: how much faster than real time audio may be sent, and whether ending an utterance without trailing silence clips the last word.

## microsoft-azure

Aliases `azure`, `microsoft_azure`; model string: gateway alias; no model named: `default`; a client is registered today; today's client ignores the model id.

| Models | Vendor interface | Today | Plan |
| --- | --- | --- | --- |
| any other id (`*`), `default` (default) | stream; file, one request | native stream | unchanged |
| `llm-speech`, `MAI-Transcribe-1.5`, `MAI-Transcribe-2` | file, one request | today's client, streaming default instead | Release 5: per-utterance upload (`azure_fast_transcription`) [regions only 6 listed] |
| `MAI-Transcribe-1` | retired | today's client, streaming default instead | Release 3: refused `stt_model_retired` |
| `MAI-Transcribe-2-Streaming` | socket with client commit; stream | today's client, streaming default instead | unchanged |

- `native` (today; figures of `default`): limits 100 open sessions (scope resource; plan s0 assumed), billed per audio hour.
- `azure_fast_transcription` (from Release 5; figures of `llm-speech`): at most 250 MB per request, at most 2 h of audio per request, limits 600 requests per minute (scope resource), billed per audio hour, only uploaded speech billed.
- Other rows of this provider carry different limits or billing; the map has each one.
- Unverified (28 distinct items across the rows), for example: `default`: transports[0].latency (the model and transport behind Pipecat's Azure figure are not stated); `default`: transports[0].limits.max_session_ms (not established by the evidence); `default`: transports[0]: whether the socket honours sample rates other than 16 kHz and 8 kHz, and whether audio sent after turn.end is recognized.

## naver-clova

Aliases `naver_clova`, `naverclova`, `naver`, `clova`, `csr`, `네이버`, `클로바`; model string: gateway alias; no model named: `csr`; a client is registered today; today's client ignores the model id.

| Models | Vendor interface | Today | Plan |
| --- | --- | --- | --- |
| any other id (`*`), `csr` (default) | file, one request | voice agent, automatic turns: refused `stt_live_unsupported` (`client_not_implemented`); manual agent, conversation loop or DAG: today's buffering client; plain /ws session: today's buffering client | Release 5: voice agent, automatic turns: per-utterance upload (`regional_rest`) [after a live probe; languages only 4 listed]; manual agent, conversation loop or DAG: per-utterance upload (`regional_rest`) [after a live probe; languages only 4 listed]; plain /ws session: today's buffering client |
| `clova-speech-long`, `clova-speech-short` | file, one request | voice agent, automatic turns: refused `stt_live_unsupported` (`client_not_implemented`); manual agent, conversation loop or DAG: today's buffering client; plain /ws session: today's buffering client | unchanged |
| `clova-speech-streaming` | stream; socket with client commit | voice agent, automatic turns: refused `stt_live_unsupported` (`client_not_implemented`); manual agent, conversation loop or DAG: today's buffering client; plain /ws session: today's buffering client | unchanged |

- `regional_rest` (from Release 5; figures of `csr`): at most 3 MB per request, at most 1 min of audio per request, limits 300,000 audio seconds per month, 30,000 audio seconds per day (scope application), billed per audio second, minimum 15 s per request, rounded up to 15 s, only uploaded speech billed.
- Unverified (24 distinct items across the rows), for example: `csr`: transports[0].latency (no latency is published for any NAVER file API; nothing was measured); `csr`: billing.min_billed_ms and billing.increment_ms (the 15 s round-up for CSR could not be re-read by the fact-check because the product page …; `csr`: transports[0].limits.max_upload_bytes (the vendor says 3 MB; exact byte count not stated; the lower is recorded).

## nectec

Aliases `aiforthai`, `ai4thai`, `partii`, `partii5`, `partii4`, `nectec-stt`; model string: gateway alias; no model named: `partii4`; a client is registered today.

Availability: evaluation only. Free for study; no commercial use; uploaded audio may be reused for research.

| Models | Vendor interface | Today | Plan |
| --- | --- | --- | --- |
| any other id (`*`) | file, one request | refused `stt_live_unsupported` (`client_not_implemented`) | Release 5: per-utterance upload (`regional_rest`) [after a live probe] |
| `partii4` (default) | file, one request | voice agent, automatic turns: refused `stt_live_unsupported` (`client_not_implemented`); manual agent, conversation loop or DAG: today's buffering client; plain /ws session: today's buffering client | Release 5: voice agent, automatic turns: per-utterance upload (`regional_rest`) [after a live probe]; manual agent, conversation loop or DAG: per-utterance upload (`regional_rest`) [after a live probe]; plain /ws session: today's buffering client |
| `partii5` | retired | refused `stt_model_retired` | unchanged |

- `regional_rest` (from Release 5; figures of `partii4`): at most 1 MB per request, at most 30 s of audio per request, limits 384 requests per minute (scope unknown), billed per request, only uploaded speech billed.
- Unverified (16 distinct items across the rows), for example: `partii4`: whether the endpoint transcribes at all for a valid key (only a quota error to a caller without a key was observed); `partii4`: the success reply of the endpoint for any output option, and what it returns for silence or noise; `partii4`: lifecycle.status (the vendor's public service catalogue lists Partii as inactive with no endpoint; whether that is retirement or an ….

## openai

model string: wire model; no model named: `gpt-transcribe`; a client is registered today.

| Models | Vendor interface | Today | Plan |
| --- | --- | --- | --- |
| any other id (`*`), `gpt-4o-mini-transcribe`, `gpt-4o-mini-transcribe-2025-03-20`, `gpt-4o-mini-transcribe-2025-12-15`, `gpt-4o-transcribe`, `gpt-4o-transcribe-diarize`, `whisper-1` | file, one request | voice agent, automatic turns: refused `stt_live_unsupported` (`client_not_implemented`); manual agent, conversation loop or DAG: today's buffering client; plain /ws session: today's buffering client | Release 1: voice agent, automatic turns: per-utterance upload (`openai_transcriptions`); manual agent, conversation loop or DAG: per-utterance upload (`openai_transcriptions`); plain /ws session: today's buffering client |
| `gpt-live-transcribe`, `gpt-live-transcribe*`, `gpt-realtime-whisper`, `gpt-realtime-whisper*` | socket with client commit | refused `stt_live_unsupported` (`client_not_implemented`) | Release 4: gateway-driven commit (`openai_realtime_transcription`) |
| `gpt-transcribe` (default) | file, one request; socket with client commit | voice agent, automatic turns: refused `stt_live_unsupported` (`client_not_implemented`); manual agent, conversation loop or DAG: today's buffering client; plain /ws session: today's buffering client | Release 1: voice agent, automatic turns: per-utterance upload (`openai_transcriptions`); manual agent, conversation loop or DAG: per-utterance upload (`openai_transcriptions`); plain /ws session: today's buffering client |

- `openai_realtime_transcription` (from Release 4; figures of `gpt-live-transcribe`): limits 500 requests per minute, 60,000 tokens per minute (scope organisation; plan tier_1 assumed), billed per audio minute.
- `openai_transcriptions` (from Release 1; figures of `gpt-transcribe`): at most 25 MiB per request, limits 500 requests per minute, 200,000 tokens per minute (scope organisation; plan tier_1 assumed), traffic must ramp up, billed per audio minute, only uploaded speech billed.
- Other rows of this provider carry different limits or billing; the map has each one.
- Unverified (50 distinct items across the rows), for example: `gpt-live-transcribe`: transports[0].latency (no published figure); `gpt-live-transcribe`: transports[0].limits.max_session_ms (not on the pages fetched); `gpt-live-transcribe`: transports[0].limits.rates (requests and tokens) (the model page gives 500 requests and 60,000 tokens per minute at tier 1; the fact-check ….

## phonexia

Aliases `phonexia-stt`, `phonexia_stt`; model string: wire model; no model named: the provider default row; a client is registered today; today's client ignores the model id.

Prerequisites: An on-premises server needs a trusted way to configure a private address.

| Models | Vendor interface | Today | Plan |
| --- | --- | --- | --- |
| any other id (`*`), `default`, `large_v2`, `large_v3`, `medium`, `speech-to-text`, `speech-to-text-whisper-enhanced` | file, one request; file, asynchronous job | refused `stt_live_unsupported` (`client_not_implemented`) | Release 5: per-utterance upload (`regional_rest`) [after a live probe] |
| `EN_US_6` | retired | refused `stt_model_retired` | unchanged |

- `regional_rest` (from Release 5; figures of `default`): limits 1 uploads in flight (scope deployment), billed per audio second, only uploaded speech billed.
- Other rows of this provider carry different limits or billing; the map has each one.
- Today's client, defect found by reading: The existing WebSocket client matches no Phonexia product and refuses to connect unless an operator sets WAAV_PHONEXIA_ALLOW_UNVERIFIED.
- Unverified (20 distinct items across the rows), for example: `default`: transports[0].latency and transports[1].latency (no latency for short audio is published and no server was available to measure); `default`: transports[0].limits.max_audio_ms (not documented for gRPC); `default`: transports[0].limits.max_upload_bytes (no total size limit is documented for gRPC; the only stated limit is 4 MiB for each message).

## revai

Aliases `rev-ai`, `rev_ai`, `rev.ai`; model string: wire model; no model named: `machine`; a client is registered today; today's client replaces a model id it does not know.

| Models | Vendor interface | Today | Plan |
| --- | --- | --- | --- |
| any other id (`*`), `machine` (default), `machine_v2`, `reverb`, `reverb-foreign-language` | stream | native stream | unchanged |
| `*whisper*` | file, asynchronous job only | refused `stt_live_unsupported` (`async_only`) | unchanged |
| `fusion`, `low_cost` | file, asynchronous job only | today's client, streaming machine instead | Release 3: refused `stt_live_unsupported` (`async_only`) |
| `human` | no transport recorded in the row | today's client (behaviour for this model not established) | Release 3: refused `stt_live_unsupported` (`disabled`) |

- `native` (today; figures of `machine`): a connection lasts at most 3 h, limits 10 open sessions (scope unknown), billed per audio hour, minimum 15 s per request, rounded up to 1 s, all call audio billed.
- Unverified (17 distinct items across the rows), for example: `machine`: transports[0].latency (Rev AI publishes no streaming latency; the only figure found was a competitor's claim and is not used); `machine`: transports[0].limits.rates[].scope (the vendor states a streaming concurrency limit of 10, adjustable by support; the audit reads it as …; `machine`: transports[0].limits.vendor_timeout_ms (whether Rev AI sends WebSocket pings during caller silence is unknown; the gateway treats 60 s ….

## reverie

Aliases `reverie-ai`, `reverie_ai`, `reverie-stt`, `reverieinc`; model string: sensitive; no model named: the provider default row; a client is registered today.

| Models | Vendor interface | Today | Plan |
| --- | --- | --- | --- |
| any other id (`*`) | file, one request; stream | native stream (client known broken) | unchanged |

- `native` (today; figures of `*`): limits 5 open sessions (scope unknown; plan free assumed), billed per audio hour, all call audio billed.
- Today's client, defect found by reading: The existing WebSocket client cannot hold a call with more than one utterance.
- Unverified (17 distinct items across the rows), for example: `*`: transports[0].limits.max_upload_bytes (not published); `*`: transports[0].limits.max_audio_ms (300 s appears only in the official Python SDK docstring, and only for audio given by URL); `*`: transports[0].limits.min_audio_ms (the vendor answers 'audio too short' for very short audio; the threshold is not published).

## sarvam

Aliases `sarvam-ai`, `sarvam.ai`, `saarika`; model string: wire model; no model named: `saarika:v2.5`; a client is registered today.

| Models | Vendor interface | Today | Plan |
| --- | --- | --- | --- |
| any other id (`*`), `saarika:v2.5` (default) | stream; file, one request | native stream | unchanged |
| `saaras:v2.5` | no transport recorded in the row | today's client (behaviour for this model not established) | Release 3: refused `stt_live_unsupported` (`disabled`) |
| `saaras:v3`, `saaras:v4` | stream; socket with client commit; file, one request | native stream | unchanged |
| `saaras:v3-realtime` | socket with client commit | today's client (behaviour for this model not established) | unchanged |

- `native` (today; figures of `saarika:v2.5`): closes after 1 min idle, limits 20 open sessions (scope account; plan starter assumed), billed per audio hour, minimum 1 s per request, rounded up to 1 s.
- Unverified (25 distinct items across the rows), for example: `saarika:v2.5`: lifecycle.deprecated_on (Sarvam calls the model deprecated but publishes no deprecation date; 2026-06-29 is the changelog entry that …; `saarika:v2.5`: lifecycle.shutdown_on (none published); `saarika:v2.5`: whether Sarvam's servers still accept saarika:v2.5 on the legacy socket, on REST or on the batch API today.

## sberdevices

Aliases `sber`, `sber-devices`, `sber_devices`, `salutespeech`, `salute-speech`, `salute_speech`, `smartspeech`; model string: wire model; no model named: `general`; a client is registered today.

Availability: closed to new customers since 2026-07-15. Connection is not available to new customers from 15 July 2026; existing customers keep working and no end date is published.

Prerequisites: The Russian Trusted Root CA must be added for the vendor's hosts before any transport can connect.

| Models | Vendor interface | Today | Plan |
| --- | --- | --- | --- |
| any other id (`*`), `ivr`, `media` | file, one request | today's timed-upload client (known broken) | Release 5: per-utterance upload (`regional_rest`) [after a live probe; languages only 5 listed] |
| `callcenter`, `general` (default) | file, one request; stream | today's timed-upload client (known broken) | Release 5: per-utterance upload (`regional_rest`) [after a live probe; languages only 5 listed] |

- `regional_rest` (from Release 5; figures of `general`): at most 2 MB per request, at most 1 min of audio per request, limits 5 uploads in flight (scope account; plan individual assumed), billed per audio second, only uploaded speech billed.
- Unverified (19 distinct items across the rows), for example: `general`: transports[*].latency (the vendor publishes no latency; no recognition request was made because no credentials were used); `general`: transports[*].limits.rates (the documentation says 10 parallel streams for legal entities and 5 for individuals, both recorded by plan; …; `general`: limits.max_upload_bytes (the vendor says 2 MB; exact byte count not stated; the lower is recorded).

## self_hosted

Aliases `self-hosted`, `waav_self_hosted`, `openai_compatible`; model string: wire model; no model named: the provider default row; no live client is registered today.

Prerequisites: The deployment's own base address, usually a private in-cluster address, must reach the speech-to-text leg through a trusted channel from the deployment record; today only the text-to-speech leg receives it (bud_legs.rs:654-664), and the client-settable endpoint override is refused for private addresses.

| Models | Vendor interface | Today | Plan |
| --- | --- | --- | --- |
| any other id (`*`), `*whisper*` | file, one request | voice agent, automatic turns: refused `stt_not_streaming`; manual agent, conversation loop or DAG: refused `unsupported_deployment`; plain /ws session: refused `unsupported_deployment` | Release 1: per-utterance upload (`openai_transcriptions`) |
| `*nemotron*` | file, one request; stream | voice agent, automatic turns: refused `stt_not_streaming`; manual agent, conversation loop or DAG: refused `unsupported_deployment`; plain /ws session: refused `unsupported_deployment` | Release 1: per-utterance upload (`openai_transcriptions`) |
| `*voxtral*realtime*` | file, one request; socket with client commit | voice agent, automatic turns: refused `stt_not_streaming`; manual agent, conversation loop or DAG: refused `unsupported_deployment`; plain /ws session: refused `unsupported_deployment` | Release 1: per-utterance upload (`openai_transcriptions`) |
| `kyutai/stt-1b-en_fr`, `kyutai/stt-2.6b-en` | stream | voice agent, automatic turns: refused `stt_not_streaming`; manual agent, conversation loop or DAG: refused `unsupported_deployment`; plain /ws session: refused `unsupported_deployment` | Release 1: refused `stt_live_unsupported` (`client_not_implemented`) |

- `openai_transcriptions` (from Release 1; figures of `*`): at most 25 MiB per request, at most 30 s of audio per request, no vendor bill, only uploaded speech billed.
- Other rows of this provider carry different limits or billing; the map has each one.
- Unverified (24 distinct items across the rows), for example: `*`: transports[0].latency (depends on the operator's hardware and model); `*`: transports[0].limits.max_upload_bytes (25 MB is vLLM's default and the operator can change it; WaaV Infer allows 100 MiB; other servers …; `*`: transports[0].limits.max_audio_ms (30 s assumes a Whisper-style window; servers chunk or cut longer audio instead of rejecting it).

## speechmatics

Aliases `speech-matics`, `speech_matics`, `speechmatics-stt`; model string: wire model; no model named: `standard`; a client is registered today.

| Models | Vendor interface | Today | Plan |
| --- | --- | --- | --- |
| any other id (`*`) | stream; file, one request | native stream | unchanged |
| `enhanced`, `standard` (default) | stream; socket with client commit; file, one request | native stream | unchanged |
| `linden-1` | stream | refused `stt_live_unsupported` (`client_not_implemented`) | unchanged |
| `melia-1` | file, one request; stream | refused `stt_live_unsupported` (`client_not_implemented`) | Release 5: per-utterance upload (`speechmatics_batch`) [after a live probe and a deadline measurement; regions only eu1, us1] |
| `oak-1` | file, one request | refused `stt_live_unsupported` (`client_not_implemented`) | Release 5: per-utterance upload (`speechmatics_batch`) [after a live probe and a deadline measurement; regions only eu1, us1] |

- `native` (today; figures of `standard`): a connection lasts at most 48 h, limits 50 open sessions (scope unknown; plan pro assumed), billed per audio hour.
- `speechmatics_batch` (from Release 5; figures of `melia-1`): at most 1000 MB per request, limits 10 requests per second, 20,000 uploads in flight (scope unknown; plan self_serve assumed), billed per audio hour, only uploaded speech billed.
- Other rows of this provider carry different limits or billing; the map has each one.
- Unverified (20 distinct items across the rows), for example: `standard`: transports[0].latency (unmeasured for the gateway's client; Pipecat's 0.74 s figure was taken with a different client and unstated …; `standard`: limits.max_session_ms of the socket transports (48 hours comes from the audit's reading of the realtime limits page; the fact-check did …; `standard`: limits.rates[].scope of the socket transports (the plan-level session limit's counting scope is not stated).

## tencent

Aliases `tencent-cloud`, `tencent_cloud`, `tencentcloud`, `腾讯云`, `腾讯`; model string: wire model; no model named: `16k_zh`; a client is registered today; today's client replaces a model id it does not know.

| Models | Vendor interface | Today | Plan |
| --- | --- | --- | --- |
| any other id (`*`), `16k_ar`, `16k_de`, `16k_en`, `16k_es`, `16k_fil` and 17 more | file, one request; stream | native stream (client known broken) | Release 5: per-utterance upload (`regional_rest`) [after a live probe; regions only china] |
| `16k_en_edu`, `16k_en_game`, `16k_en_large`, `16k_zh-TW`, `16k_zh_court`, `16k_zh_edu` and 3 more | stream | native stream (client known broken) | unchanged |
| `16k_zh-PY` | file, one request | today's client, streaming 16k_zh instead | Release 5: per-utterance upload (`regional_rest`) [after a live probe; regions only china] |
| `16k_zh_dialect` | file, one request | today's client, streaming 16k_zh instead | unchanged |
| `16k_zh_en_meeting` | file, asynchronous job only | today's client, streaming 16k_zh instead | Release 3: refused `stt_live_unsupported` (`async_only`) |
| `16k_zh_medical` | stream; file, one request | native stream (client known broken) | unchanged |
| `Hy-ASR-3.0-preview` | socket with client commit; stream | native stream (client known broken) | unchanged |

- `native` (today; figures of `16k_en_edu`): closes after 15 s idle, limits 200 open sessions (scope account), billed per audio second, minimum 1 s per request.
- `regional_rest` (from Release 5; figures of `16k_zh`): at most 100 MB per request, at most 2 h of audio per request, limits 20 uploads in flight (scope account), billed per audio second, minimum 1 s per request, only uploaded speech billed.
- Other rows of this provider carry different limits or billing; the map has each one.
- Today's client, defect found by reading: Known broken against the vendor protocol: in-progress partial results (slice_type 1) are marked final, the vendor end message is never sent, and a void voice_id is reused on reconnect.
- Unverified (22 distinct items across the rows), for example: `16k_en_edu`: transports[0].limits.max_session_ms; `16k_en_edu`: billing.bills_silence (billed by recognised duration; whether silence sent on the socket counts is not stated); `16k_zh`: transports[1].limits.max_session_ms.

## tinkoff

Aliases `tinkoff-stt`, `tinkoff_stt`, `voicekit`, `tinkoff-voicekit`; model string: gateway alias; no model named: `default`; a client is registered today; today's client ignores the model id.

Prerequisites: The Russian Trusted Root CA must be added for the vendor's hosts before any transport can connect.

| Models | Vendor interface | Today | Plan |
| --- | --- | --- | --- |
| any other id (`*`) | file, one request; socket with client commit; stream | native stream (client known broken) | unchanged |
| `default` (default) | stream; file, one request; socket with client commit | native stream (client known broken) | unchanged |

- `native` (today; figures of `default`): a connection lasts at most 1 h, limits 50 open sessions (scope unknown), billed per audio second.
- Today's client, defect found by reading: The existing streaming client does not match the vendor protocol.
- Unverified (14 distinct items across the rows), for example: `default`: transports[native].gateway_client (the four mismatches are inferred from reading the client, the vendor proto and the certificate chain of …; `default`: transports[*].latency (the vendor publishes no latency figure); `default`: transports[*].limits.rates[].scope (50 simultaneous streams per method for every cloud user; whether the scope is the API key or the ….

## viettel-ai

Aliases `viettel_ai`, `viettelai`, `viettel`, `vtai`, `viettel-stt`, `viettel_ai-stt`; model string: absent; no model named: the provider default row; a client is registered today.

| Models | Vendor interface | Today | Plan |
| --- | --- | --- | --- |
| any other id (`*`) | file, one request; stream | refused `stt_live_unsupported` (`client_not_implemented`) | Release 5: per-utterance upload (`regional_rest`) [after a live probe and a deadline measurement] |

- `regional_rest` (from Release 5; figures of `*`): limits 15 uploads in flight (scope unknown), billed per audio second, only uploaded speech billed.
- Unverified (10 distinct items across the rows), for example: `*`: transports[0].latency (the vendor's figure is an average and every other figure is a single anonymous request; nothing was measured with a …; `*`: transports[0].limits.max_upload_bytes (the web demo states 2 MB; the API documents nothing); `*`: transports[0].limits.max_audio_ms and min_audio_ms.

## waav-infer

Aliases `infer`, `waav_infer`, `waavinfer`; model string: wire model; no model named: the provider default row; no live client is registered today.

Prerequisites: The engine binds loopback with no authentication by default (port 8080), so the gateway reaches it only through a trusted, operator-configured private base address from the deployment record.

| Models | Vendor interface | Today | Plan |
| --- | --- | --- | --- |
| any other id (`*`), `nemotron*`, `voxtral*realtime*`, `whisper*` | file, one request; socket with client commit | refused `stt_live_unsupported` (`client_not_implemented`) | Release 1: per-utterance upload (`openai_transcriptions`) |

- `openai_transcriptions` (from Release 1; figures of `*`): at most 100 MiB per request, limits 64 uploads in flight (scope deployment), no vendor bill, only uploaded speech billed.
- Unverified (5 distinct items across the rows), for example: `*`: transports[0].latency (no end-of-speech-to-final measurement; engine inference time is measured for whisper-base and nemotron-asr only); `*`: transports[0].limits.max_audio_ms; `*`: transports[0].limits.min_audio_ms.

## yandex

Aliases `yandex-speechkit`, `yandex_speechkit`, `speechkit`, `yandex-stt`, `yandex_stt`; model string: wire model; no model named: `general`; a client is registered today; today's client replaces a model id it does not know.

| Models | Vendor interface | Today | Plan |
| --- | --- | --- | --- |
| any other id (`*`) | file, one request | today's timed-upload client (known broken) | Release 5: per-utterance upload (`regional_rest`) [after a live probe] |
| `deferred*` | file, asynchronous job only | today's client (behaviour for this model not established) | Release 3: refused `stt_live_unsupported` (`async_only`) |
| `deferred-general`, `deferred-general:deprecated`, `deferred-general:rc` | file, asynchronous job only | today's timed-upload client (known broken) | Release 3: refused `stt_live_unsupported` (`async_only`) |
| `general` (default), `general:deprecated`, `general:rc` | file, one request; stream; socket with client commit | today's timed-upload client (known broken) | Release 5: per-utterance upload (`regional_rest`) [after a live probe] |

- `regional_rest` (from Release 5; figures of `general`): at most 1 MB per request, at most 30 s of audio per request, limits 20 requests per second (scope unknown), billed per audio second, minimum 15 s per request, rounded up to 15 s, only uploaded speech billed.
- Unverified (22 distinct items across the rows), for example: `general`: transports[*].latency (the vendor publishes no latency for the synchronous endpoint or for streaming; nothing was measured); `general`: transports[0].limits.max_upload_bytes (the vendor says 1 MB; whether that is 1,000,000 or 1,048,576 bytes is not stated; the lower is …; `general`: transports[0].limits.min_audio_ms.

