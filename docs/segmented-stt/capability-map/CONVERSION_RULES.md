# Converting the capability map rows from schema version 1 to version 2

Written 2026-10-04 for the integration phase of the segmented speech-to-text plan.

This file tells a converter exactly what to do with each of the 35 provider files under `rows/`
(410 rows, 738 transports) and with `profiles.json`, so that they validate against
`stt_capability_map.schema.json` version 2. It is also the home of the **adapter table**, which
`validate_rows.py` reads: the three tables between `<!-- ...:begin -->` and `<!-- ...:end -->`
markers are parsed by the validator, so keep their column order and keep identifiers in backticks.

Where this file and a workstream design disagree, `design/INTEGRATION_DECISIONS.md` decides; the
rules below were written from its sections 1, 5, 6, 10, 12, 13 and 15. Nothing under `WaaV/` is
changed by the conversion.

Code references marked "opened" were read for this document. Code references inside the three
worked examples are carried over from the version 1 rows unless the text says they were opened.

## 1. Terms

- **Row**: one entry of the map. It matches a provider and either one model id or a pattern.
- **Transport**: one way the gateway can reach the model during a call. A row holds an ordered list.
  Its **input mode** is `live_stream` (the vendor transcribes audio as it arrives on a socket),
  `vendor_segmented` (the vendor holds a buffer and transcribes after a commit) or `file_upload`
  (one complete file per request).
- **Adapter**: the gateway code path that speaks one transport. Version 2 has a closed list of
  sixteen adapter ids (section 3).
- **Native client**: the client `create_stt_standard` builds for a provider today
  (`gateway/src/core/stt/standard.rs:390`, opened). The adapter id `native` means "build exactly
  that, unchanged".
- **Segmented engine**: the new component that cuts caller audio into utterances with the gateway's
  own voice detector and uploads each one. Every file-upload adapter runs inside it.
- **Commit transport**: a vendor socket on which the gateway's detector, not the vendor, decides
  where an utterance ends, and sends the vendor's commit or finalize message.
- **Enabled from release**: the first release in which the resolver may choose a transport. A map
  carries one top-level `release` number; a transport is enabled when its `enabled_from_release`
  is not null and is at most that number.
- **Today's behaviour**: what a live session naming the model gets from the gateway code as it is
  now, when no listed transport is usable. Recorded in `when_unusable.today`.

## 2. Release names

Use these names exactly (integration decisions, section 1).

| Number | Name |
| --- | --- |
| 0 | Release 0, groundwork and honest refusal |
| 1 | Release 1, first working calls |
| 2 | Release 2, dark launch complete |
| 3 | Release 3, default on |
| 4 | Release 4, live-only models and low latency |
| 5 | Release 5, wider vendor coverage and hardening |
| 6 | Release 6, interruption recovery and interim text |

## 3. The adapter table

Sixteen ids. Thirteen name code that exists or that a release builds. Three (`planned_stream`,
`planned_commit`, `planned_file`) are placeholders for a vendor interface that no release builds a
client for: a transport that carries one is never enabled, and its `dialect.wire` is the only
record of which interface is meant. When a later plan schedules such an interface, it adds a real
id here and to the schema's `adapter_id` list, and the rows change in the same commit.

"Enabled from release" is the earliest release a transport of that adapter may be turned on. A
single transport may be later, or never (section 9).

<!-- adapter-table:begin -->
| Final adapter id | Kind | Enabled from release | Serves (canonical provider ids) | What it is |
| --- | --- | --- | --- | --- |
| `native` | existing streaming client | 0 | any provider with a streaming client in the gateway | Today's client, built by the existing factory, unchanged. Buffering and timer-driven REST clients are not transports and are not listed under this id (section 10) |
| `openai_transcriptions` | file-upload transcriber family | 1 | `openai`, `self_hosted`, `waav-infer` | OpenAI-compatible multipart upload. Release 1, first working calls (decisions section 12) |
| `groq_transcriptions` | file-upload transcriber family | 1 | `groq` | Same implementation as `openai_transcriptions`, Groq's host and limits |
| `azure_openai_transcriptions` | file-upload transcriber family | 1 | `azure_openai` | Same implementation, Azure's deployment URL and `api-key` header |
| `elevenlabs_batch` | file-upload transcriber family | 1 | `elevenlabs` | ElevenLabs `scribe_v2` and `scribe_v2_medical` |
| `assemblyai_sync` | file-upload transcriber family | 3 | `assemblyai` | AssemblyAI synchronous endpoint. Release 3, default on (decisions section 6) |
| `deepgram_prerecorded` | file-upload transcriber family | 3 | `deepgram` | Deepgram hosted Whisper; optional for Nova models. Release 3 (decisions section 6) |
| `openai_realtime_transcription` | gateway-driven commit socket | 4 | `openai`, `azure_openai`, `self_hosted` | OpenAI Realtime transcription session with a gateway commit. Release 4 for `openai` only; never enabled for the other two (section 9) |
| `cartesia_manual_finalize` | gateway-driven commit socket | 4 | `cartesia` | Cartesia socket with the gateway sending `finalize`. Release 4 |
| `azure_fast_transcription` | file-upload transcriber family | 5 | `microsoft-azure` | Azure Speech fast transcription. Release 5, wider vendor coverage and hardening |
| `google_recognize` | file-upload transcriber family | 5 | `google` | Google `Recognize` |
| `speechmatics_batch` | file-upload transcriber family | 5 | `speechmatics` | Speechmatics blocking jobs, only after a measurement shows they answer within the deadline |
| `regional_rest` | file-upload transcriber family | 5 | `yandex`, `sberdevices`, `naver-clova`, `bhashini`, `fpt-ai`, `gnani`, `nectec`, `alibaba-cloud`, `tencent`, `huawei-cloud`, `baidu`, `phonexia`, `viettel-ai` | One generic adapter over each vendor's single-request function. It chooses its code path by `dialect.wire`; the interfaces it serves are in the table below. Each transport is enabled only after a live probe with a real key |
| `planned_stream` | streaming client not yet written | none | any | A vendor streaming interface for which the gateway has no client and no release builds one |
| `planned_commit` | gateway-driven commit socket | none | any | A vendor socket the gateway's detector could commit on; no release builds the client |
| `planned_file` | file-upload transcriber family | none | any | A vendor file endpoint no release builds a transcriber for: an asynchronous job, or an optional fallback on a vendor that streams |
<!-- adapter-table:end -->

Interfaces the `regional_rest` adapter serves (the transport's `dialect.wire`):

<!-- regional-rest-wires:begin -->
| Provider | `dialect.wire` values served |
| --- | --- |
| `yandex` | `yandex_stt_v1_rest` |
| `sberdevices` | `sber_salutespeech_rest_v1` |
| `naver-clova` | `naver_csr_rest` |
| `bhashini` | `bhashini_pipeline_json` |
| `fpt-ai` | `fpt_raw_body` |
| `gnani` | `gnani_vachana_multipart` |
| `nectec` | `nectec_partii4_multipart` |
| `alibaba-cloud` | `dashscope_asr_flash_json`, `dashscope_qwen3_asr_json` |
| `tencent` | `tencent_flash` |
| `huawei-cloud` | `huawei_sis_short_audio` |
| `baidu` | `baidu_pro_api`, `baidu_server_api` |
| `phonexia` | `phonexia_grpc` |
| `viettel-ai` | `viettel_recognize_multipart` |
<!-- regional-rest-wires:end -->

`viettel-ai` is listed because decisions section 12 enables it "only if a measurement contradicts
the 15 to 30 s the audit observed"; its transport carries `enable_requires` (section 9).
`nectec` is evaluation only (the provider entry says so, section 6).

## 4. Adapter rename table

Every version 1 adapter id, and the id that replaces it. "When" distinguishes cases of one old id.

<!-- adapter-rename:begin -->
| Version 1 id | When | Final id |
| --- | --- | --- |
| `commit_alibaba_cloud` |  | `planned_commit` |
| `commit_amivoice` |  | `planned_commit` |
| `commit_cartesia` |  | `cartesia_manual_finalize` |
| `commit_deepgram` |  | `planned_commit` |
| `commit_elevenlabs` |  | `planned_commit` |
| `commit_huawei_cloud` |  | `planned_commit` |
| `commit_ibm_watson` |  | `planned_commit` |
| `commit_iflytek` |  | `planned_commit` |
| `commit_microsoft_azure` |  | `planned_commit` |
| `commit_naver_clova` |  | `planned_commit` |
| `commit_openai` |  | `openai_realtime_transcription` |
| `commit_sarvam` |  | `planned_commit` |
| `commit_sarvam_legacy` |  | `planned_commit` |
| `commit_speaches` |  | `planned_commit` |
| `commit_speechmatics` |  | `planned_commit` |
| `commit_tencent` |  | `planned_commit` |
| `commit_tinkoff` |  | `planned_commit` |
| `commit_vllm` |  | `planned_commit` |
| `commit_waav_infer` |  | `planned_commit` |
| `commit_yandex` |  | `planned_commit` |
| `segmented_alibaba_cloud` | `dialect.wire` is `dashscope_asr_flash_json` | `regional_rest` |
| `segmented_alibaba_cloud` | `dialect.wire` is `dashscope_qwen3_asr_json` | `regional_rest` |
| `segmented_amivoice` |  | `planned_file` |
| `segmented_assemblyai` |  | `assemblyai_sync` |
| `segmented_assemblyai_async` |  | `planned_file` |
| `segmented_baidu` | `dialect.wire` is `baidu_pro_api` | `regional_rest` |
| `segmented_baidu` | `dialect.wire` is `baidu_server_api` | `regional_rest` |
| `segmented_bhashini` |  | `regional_rest` |
| `segmented_cartesia` |  | `planned_file` |
| `segmented_deepgram` |  | `deepgram_prerecorded` |
| `segmented_elevenlabs` |  | `elevenlabs_batch` |
| `segmented_fpt_ai` |  | `regional_rest` |
| `segmented_gladia` |  | `planned_file` |
| `segmented_gnani` |  | `regional_rest` |
| `segmented_google` |  | `google_recognize` |
| `segmented_huawei_cloud` |  | `regional_rest` |
| `segmented_ibm_watson` |  | `planned_file` |
| `segmented_microsoft_azure` |  | `azure_fast_transcription` |
| `segmented_naver_clova` | `dialect.wire` is `clova_speech_long_multipart` | `planned_file` |
| `segmented_naver_clova` | `dialect.wire` is `clova_speech_short_rest` | `planned_file` |
| `segmented_naver_clova` | `dialect.wire` is `naver_csr_rest` | `regional_rest` |
| `segmented_nectec` |  | `regional_rest` |
| `segmented_openai_compat` | provider `azure_openai` | `azure_openai_transcriptions` |
| `segmented_openai_compat` | provider `groq` | `groq_transcriptions` |
| `segmented_openai_compat` | provider `openai`, `self_hosted` or `waav-infer`, and the file profiles | `openai_transcriptions` |
| `segmented_phonexia` | `dialect.wire` is `phonexia_rest_task` | `planned_file` |
| `segmented_phonexia` | `dialect.wire` is `phonexia_grpc` | `regional_rest` |
| `segmented_reverie` |  | `planned_file` |
| `segmented_sarvam` |  | `planned_file` |
| `segmented_sberdevices` |  | `regional_rest` |
| `segmented_speechmatics` |  | `speechmatics_batch` |
| `segmented_tencent` | `dialect.wire` is `tencent_sentence_recognition` | `planned_file` |
| `segmented_tencent` | `dialect.wire` is `tencent_flash` | `regional_rest` |
| `segmented_tinkoff` |  | `planned_file` |
| `segmented_viettel_ai` |  | `regional_rest` |
| `segmented_yandex` |  | `regional_rest` |
| `stream_alibaba_cloud` | the client exists (status is not `not_implemented`) | `native` |
| `stream_amivoice` | the client exists (status is not `not_implemented`) | `native` |
| `stream_assemblyai` | the client exists (status is not `not_implemented`) | `native` |
| `stream_aws_transcribe` | the client exists (status is not `not_implemented`) | `native` |
| `stream_aws_transcribe` | `gateway_client.status` is `not_implemented` | `planned_stream` |
| `stream_baidu` | the client exists (status is not `not_implemented`) | `native` |
| `stream_bhashini` | `gateway_client.status` is `not_implemented` | `planned_stream` |
| `stream_cartesia` | the client exists (status is not `not_implemented`) | `native` |
| `stream_cartesia_turns` | `gateway_client.status` is `not_implemented` | `planned_stream` |
| `stream_deepgram` | the client exists (status is not `not_implemented`) | `native` |
| `stream_deepgram_flux` | `gateway_client.status` is `not_implemented` | `planned_stream` |
| `stream_elevenlabs` | the client exists (status is not `not_implemented`) | `native` |
| `stream_gladia` | the client exists (status is not `not_implemented`) | `native` |
| `stream_gnani` | the client exists (status is not `not_implemented`) | `native` |
| `stream_gnani_vachana` | `gateway_client.status` is `not_implemented` | `planned_stream` |
| `stream_google` | the client exists (status is not `not_implemented`) | `native` |
| `stream_huawei_cloud` | the client exists (status is not `not_implemented`) | `native` |
| `stream_ibm_watson` | the client exists (status is not `not_implemented`) | `native` |
| `stream_iflytek` | the client exists (status is not `not_implemented`) | `native` |
| `stream_kyutai_moshi` | `gateway_client.status` is `not_implemented` | `planned_stream` |
| `stream_microsoft_azure` | the client exists (status is not `not_implemented`) | `native` |
| `stream_microsoft_azure` | `gateway_client.status` is `not_implemented` | `planned_stream` |
| `stream_naver_clova` | `gateway_client.status` is `not_implemented` | `planned_stream` |
| `stream_nvidia_nim` | `gateway_client.status` is `not_implemented` | `planned_stream` |
| `stream_phonexia` | the client exists (status is not `not_implemented`) | `native` |
| `stream_registry_passthrough` | the row becomes a pass-through row and has no transports | (removed) |
| `stream_revai` | the client exists (status is not `not_implemented`) | `native` |
| `stream_reverie` | the client exists (status is not `not_implemented`) | `native` |
| `stream_sarvam` | the client exists (status is not `not_implemented`) | `native` |
| `stream_sberdevices` | `gateway_client.status` is `not_implemented` | `planned_stream` |
| `stream_speechmatics` | the client exists (status is not `not_implemented`) | `native` |
| `stream_speechmatics` | `gateway_client.status` is `not_implemented` | `planned_stream` |
| `stream_speechmatics_agent` | `gateway_client.status` is `not_implemented` | `planned_stream` |
| `stream_tencent` | the client exists (status is not `not_implemented`) | `native` |
| `stream_tinkoff` | the client exists (status is not `not_implemented`) | `native` |
| `stream_viettel_ai` | `gateway_client.status` is `not_implemented` | `planned_stream` |
| `stream_yandex` | `gateway_client.status` is `not_implemented` | `planned_stream` |
<!-- adapter-rename:end -->

Rules behind the table, so a new case can be decided the same way:

1. `stream_<provider>` whose `gateway_client.status` is `verified_live`, `wire_tested`,
   `unverified` or `known_broken` is today's client: `native`.
2. `stream_<anything>` whose status is `not_implemented` is a client nobody has written:
   `planned_stream`.
3. `stream_registry_passthrough` disappears: its row becomes a pass-through row (section 7).
4. `commit_openai` and `commit_cartesia` are the two commit transports the plan builds
   (decisions sections 1 and 12). Every other `commit_*` is `planned_commit`.
5. `segmented_openai_compat` splits by provider, because the three ids share an implementation and
   differ in host, authentication and limits.
6. A `segmented_*` id of a vendor that decisions section 12 places in a release takes that
   family's id. For the regional vendors it is `regional_rest` when the interface is one the
   decision names, and `planned_file` otherwise (Tencent `tencent_sentence_recognition`, the two
   NAVER CLOVA Speech interfaces, the Phonexia REST task).
7. Every `segmented_*` id of a vendor that decisions section 12 does not place (AmiVoice, Cartesia's
   file endpoint, IBM Watson, Reverie, Sarvam, T-Bank), and every asynchronous job
   (`file_request_mode: async_poll`), is `planned_file`.

`dialect.wire` is kept in version 2 exactly as written in version 1. With the vendor-specific
adapter ids gone it is what tells two interfaces of one vendor apart.

## 5. Profile name table

`profiles.json` changes shape: each entry becomes `{transport, allowed_providers, provenance}`,
with `fallback_profile` (and `requires` where needed) on socket profiles. Names use hyphens.

| Version 1 name | Final name | Adapter | Enabled from release | `allowed_providers` | `fallback_profile` | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| `openai_compat_file` | `openai-compatible-file` | `openai_transcriptions` | 1 | `self_hosted` | n/a | The self-hosted default. **Content change** (decisions section 15): it now sends `language` in two-letter form and lists it in `dialect.droppable_params`; set `language_param: "language"`, `language_format: "iso639_1"`, `droppable_params: ["language"]` |
| `openai_compat_file_language_prompt` | `vllm-file` and `speaches-file` (two copies) | `openai_transcriptions` | 2 | `self_hosted` | n/a | The generic "language and prompt accepted" profile is retired; its content seeds the two server profiles the capability-map design names (its section 3.11). `vllm-file` needs nothing beyond the new name. `speaches-file`: add to `gateway_client.notes` that the first request may download the model, so it needs a longer deadline |
| `waav_infer_file` | `waav-infer-file` | `openai_transcriptions` | 1 | `self_hosted`, `waav-infer` | n/a | No language, no prompt. Set `provenance.verified_by: "code_reading"` and move the engine source references into `provenance.code_refs` |
| `whisper_cpp_file` | `whispercpp-file` | `openai_transcriptions` | 2 | `self_hosted` | n/a | Set `limits.single_process: true` in place of the concurrency figure of 1 |
| `nvidia_nim_file` | `nim-file` | `openai_transcriptions` | 2 | `self_hosted` | n/a | `language_format: "bcp47"` |
| (none; build from the transport of the `azure_openai` default row) | `azure-openai-file` | `azure_openai_transcriptions` | 1 | `azure_openai` | n/a | `requires: ["underlying_model"]` is **not** set: the default row works without it |
| `openai_realtime_transcription` | `openai-realtime-transcription` | `openai_realtime_transcription` | none | `self_hosted`, `azure_openai` | `openai-compatible-file` | `requires: ["realtime_url"]`. Add `stream_audio` (24 kHz) and `commit` |
| `vllm_realtime` | `vllm-realtime` | `planned_commit` | none | `self_hosted` | `vllm-file` | `requires: ["realtime_url", "underlying_model"]` |
| `nvidia_nim_realtime` | `nim-realtime` | `planned_stream` | none | `self_hosted` | `nim-file` | `requires: ["realtime_url"]` |
| `speaches_realtime` | `speaches-realtime` | `planned_commit` | none | `self_hosted` | `speaches-file` | `requires: ["realtime_url"]` |
| `deepgram_compatible_stream` | `deepgram-compatible-stream` | `planned_stream` | none | `self_hosted` | `openai-compatible-file` | The Deepgram client exists but no factory arm reaches it for a self-hosted deployment, so it is not `native`. Keep `gateway_client.status: "unverified"` |

Release 2, dark launch complete, is where decisions section 1 puts "self-hosted server profiles";
the default profile, the WaaV Infer profile and the Azure OpenAI profile are needed by Release 1
rows and are enabled from Release 1.

Profile provenance: copy `confidence`, `verified_on`, `verified_by` and `owner` from the
`self_hosted` default row's provenance; take `sources` and `code_refs` from the sources named in the
profile's `gateway_client.notes` and `code_ref`. A profile whose only evidence is code gets
`verified_by: "code_reading"`.

After the rename, a row transport that is identical to a profile's transport is written as
`{"profile": "<final name>"}`. In the version 1 files that is four transports: the `self_hosted`
default row (`openai-compatible-file`), the `*voxtral*realtime*` and `*nemotron*` socket transports
of `self_hosted` (`vllm-realtime`, `nim-realtime`) and the `waav-infer` default row
(`waav-infer-file`). The `azure_openai` default row becomes `{"profile": "azure-openai-file"}`.
Other transports stay inline in this pass.

## 6. The provider entry

| Version 1 field | Version 2 | Rule |
| --- | --- | --- |
| `aliases` | `aliases` | Unchanged |
| `default_model` | `default_model` | Unchanged, except: `openai` becomes `gpt-transcribe` (decisions section 15; today's code default is `whisper-1`, `gateway/src/core/stt/openai/config.rs:63-67`, opened). Must be null when `model_string` is `absent` or `sensitive` |
| `registered_in_gateway` | `registered_in_gateway` | Unchanged |
| `notes` | `notes` | Remove the sentences whose fact moves to a field below; keep the rest |
| (new) | `model_string` | Required. From the table below |
| (new) | `key_normalisation` | Only where the table below gives a value |
| (new) | `native_model_handling` | Only where the table gives `substituted` or `ignored` |
| (new) | `deployment_model_source`, `egress`, `deployment_base` | `self_hosted`, `waav-infer`: `table_only`, `in_cluster`, `required`. `azure_openai`: `table_only`, `public_only`, `required`. Everyone else: omit (defaults) |
| (new) | `production_base`, `base_url_convention` | Only for the Release 1 vendors with a vendor host: `openai` `https://api.openai.com`, `origin`; `groq` `https://api.groq.com/openai`, `origin`; `elevenlabs` `https://api.elevenlabs.io`, `origin`. `self_hosted`, `waav-infer`: `base_url_convention: "versioned"` (the base already ends in `/v1`). Others: omit until their transcriber ships |
| (new) | `prerequisites`, `availability` | From the provider notes, per the table below |
| (new) | `regions`, `setup_probe` | Omit in this pass. `regions` is filled with the Release 5 work |

Values per provider. "Evidence" names where the fact is written down; the converter re-reads it.

| Provider | `model_string` | Other new fields | Evidence |
| --- | --- | --- | --- |
| `openai`, `groq` | `wire_model` | `key_normalisation: {strip_provider_prefix: true, placeholder_as_unset: true}` | Capability-map design decision 2.4; the client aliases `whisper`, `whisper1`, `gpt4o-transcribe`, `gpt4o-mini-transcribe` (`gateway/src/core/stt/openai/config.rs:97-104`, opened) become `model_aliases` on their rows |
| `elevenlabs` | `wire_model` | same two flags as OpenAI | The realtime client rejects any id outside its list (`gateway/src/core/stt/elevenlabs/config.rs:391`, opened) |
| `deepgram`, `cartesia`, `google`, `gnani`, `ibm-watson`, `sarvam`, `revai`, `phonexia`, `baidu`, `amivoice`, `yandex`, `speechmatics`, `alibaba-cloud`, `sberdevices` | `wire_model` | `native_model_handling: "substituted"` for `baidu`, `amivoice`, `yandex`. For `sberdevices` add to `notes` that today's client reads the model field as the OAuth scope | Builder gaps (yandex, revai, tencent groups); capability-map design decision 2.7 |
| `tencent` | `wire_model` | `native_model_handling: "substituted"`. Ids are case-sensitive on the wire: keep the vendor's spelling in `match.model` | Builder gap, tencent group ("Key normalisation misses two cases") |
| `huawei-cloud` | `wire_model` | `key_normalisation: {region_suffix_separator: "@"}`; `native_model_handling: "substituted"` | `gateway/src/core/stt/huawei_cloud/config.rs:776-783` (opened) |
| `bhashini` | `wire_model` | `key_normalisation: {strip_provider_prefix: false}`, written out although false is the default, because its ids begin with `bhashini/` and with the alias `ai4bharat/` | Builder gap, gnani group; decisions section 10 |
| `assemblyai` | `wire_model` | `native_model_handling: "substituted"`; `key_normalisation: {placeholder_as_unset: true}` | Auditor for the deepgram group ("nova-3 placeholder") |
| `gladia` | `wire_model` | `native_model_handling: "ignored"`; `key_normalisation: {placeholder_as_unset: true}` | Same |
| `microsoft-azure` | `gateway_alias` | `native_model_handling: "ignored"` | `default` and `llm-speech` are gateway sentinels (auditor, microsoft-azure group) |
| `aws-transcribe`, `naver-clova`, `fpt-ai`, `nectec`, `iflytek`, `tinkoff` | `gateway_alias` | `nectec` only: `availability: {status: "evaluation_only", note: "Free for study; no commercial use; uploaded audio may be reused for research."}` | Builder gaps (alibaba, tencent, yandex groups) |
| `azure_openai` | `deployment_name` | see the row above | Builder gap, extras group |
| `self_hosted`, `waav-infer` | `wire_model` | see the row above; `prerequisites: [{kind: "private_address", ...}]` | `gateway/src/core/tts/self_hosted.rs:49` (opened) |
| `viettel-ai` | `absent` | `default_model: null` | Builder gap, alibaba group |
| `reverie` | `sensitive` | `default_model: null` | `gateway/src/core/stt/reverie/config.rs:515-521` (opened): the model field is the customer's application id |
| `sberdevices`, `tinkoff` | (above) | `prerequisites: [{kind: "trust_root", detail: "The Russian Trusted Root CA must be added for the vendor's hosts before any transport can connect."}]`; `sberdevices` also `availability: {status: "closed_to_new_customers", since: "2026-07-15"}` | Builder gap, yandex group |
| `baidu` | (above) | `prerequisites: [{kind: "token_scope", detail: "The pro API needs the token scope brain_enhanced_asr.", wire: "baidu_pro_api"}]` | Builder gap, tencent group |
| `alibaba-cloud` | (above) | `prerequisites: [{kind: "workspace_id", detail: "The US (Virginia) region needs a workspace id."}]` | Builder gap, alibaba group |
| `bhashini` | (above) | `prerequisites: [{kind: "configuration_call", detail: "The service id and the compute address come from the vendor's pipeline configuration call."}]` | Builder gap, gnani group |
| `phonexia` | (above) | `prerequisites: [{kind: "private_address", detail: "An on-premises server needs a trusted way to configure a private address."}]` | Auditor, gnani group |
| `_global_default` (`*`) | `wire_model` | none | Not a provider |

The top-level value `sdk_placeholder_models` is `["nova-3"]`: the Python kit sends it for every
provider when the customer named no model
(`WaaV/clients_sdk/python/bud_waav/ws/session.py:942`, opened).

## 7. The row

| Version 1 field | Version 2 | Rule |
| --- | --- | --- |
| (new) | `id` | Section 11 |
| `match.provider`, `match.model`, `match.model_glob` | same | Unchanged. Keep the vendor's spelling and case in `match.model` |
| (new) | `match.model_aliases` | Add the spellings today's client rewrites to this row's model, taken from the provider notes: OpenAI (`whisper`, `whisper1` on `whisper-1`; `gpt4o-transcribe` on `gpt-4o-transcribe`; `gpt4o-mini-transcribe` on `gpt-4o-mini-transcribe`; `gateway/src/core/stt/openai/config.rs:97-104`, opened), Groq (`whisper-v3`, `large-v3` on `whisper-large-v3`; `whisper-v3-turbo`, `turbo`, `large-v3-turbo` on `whisper-large-v3-turbo`; `gateway/src/core/stt/groq/config.rs:127-128`, opened), Yandex (`default` on `general`; `rc`, `realtime` on `general:rc`; `gateway/src/core/stt/yandex/config.rs:187-201`, opened; `batch` is not added, because the `deferred*` row already answers it), NECTEC (`partii-4`, `partii_4`, `4`, `v4`, `legacy` on `partii4`; `partii-5`, `partii_5`, `5`, `v5` on `partii5`; `gateway/src/core/stt/nectec/config.rs:108-117`, opened), Speechmatics (`std` on `standard`; `enh`, `high` on `enhanced`; `gateway/src/core/stt/speechmatics/config.rs:100-101`, opened), Alibaba shorthand (`qwen3-asr`, `qwen-asr`, `paraformer`, `paraformer-8k`, `funasr` on the row the client rewrites each to). An alias must not equal another row's model or alias |
| (new) | `passthrough` | `true` on the one row of `_global_default.json`; its `transports` become `[]`, it gets no `when_unusable`, and its `lifecycle.status` becomes `unknown`. Nowhere else in this pass |
| (new) | `applies_to_all_models` | `true` on the `self_hosted` default row only |
| `lifecycle` | `lifecycle` | Unchanged, then apply the re-classification list below |
| `transports` | `transports` | Section 8 for each entry, then the order rule of section 9 |
| `no_live_path_reason` | `when_unusable.refusal.text` | Section 10. The field name disappears |
| `billing` | `billing` | Section 8.9 |
| `provenance` | `provenance` | Section 8.10 |

**Lifecycle re-classification.** Version 1 forced these into `ga`, `deprecated` or `preview`. Set
the status named here; delete a stand-in date or mark it with `dates_basis: "inferred"`.

| Rows | New status | Dates |
| --- | --- | --- |
| Deepgram rows whose notes call the family legacy (the first-generation Nova, Enhanced and Base ids) | `legacy` | none |
| AssemblyAI `u3-sync-pro`, `u3-pro`; Gnani `default`; Huawei `chinese_16k_common`, `chinese_8k_common`; OpenAI `gpt-realtime-whisper`; Yandex `general:deprecated`, `deferred-general:deprecated` | `legacy` | none |
| Huawei `english_8k_common` | `legacy` | remove `deprecated_on` (it was the vendor page's last-updated stamp) |
| Azure OpenAI rows marked `deprecated` only because Azure publishes a retirement date | `retiring` | keep `shutdown_on` |
| Alibaba `gummy-realtime-v1`, `gummy-chat-v1`; Sarvam `saaras:v2.5` | `retiring` | none; add the doubt to `provenance.unverified` |
| NECTEC `partii5` | `retired` | none (the endpoint answers 404 and no retirement was announced); `replacement: ["partii4"]` |
| OpenAI `gpt-4o-mini-transcribe-2025-12-15`; Sarvam `saarika:v2.5`; Phonexia `EN_US_6` | unchanged | add `dates_basis: "inferred"` |
| Alibaba ids that retire on 2026-10-10 | unchanged | add `provenance.recheck_on: "2026-10-10"` |
| Gladia `solaria-3`; Speechmatics `melia-1` | unchanged | add `provenance.recheck_on` one month after `verified_on` |

Ids that the gateway accepts and the vendor does not have (IBM `es-LA_Multimedia`,
`zh-CN_Multimedia`, `ar-MS_Multimedia`, `hi-IN_Multimedia`; six AmiVoice engine ids) have no rows
today. Adding rows with `lifecycle.status: "invalid"` for them is new work, not conversion.

## 8. The transport, field by field

### 8.1 Fields that carry over

| Version 1 | Version 2 | Rule |
| --- | --- | --- |
| `input_mode` | `input_mode` | Unchanged |
| `adapter` | `adapter` | Section 4 |
| `enabled`, `disabled_reason` | `enabled_from_release`, `disabled_reason`, `gateway_client.defect` | Section 9 |
| `interim_results`, `endpointing_owner`, `vendor_turn_signal` | same | Unchanged. Where `provenance.unverified` says the value was a forced guess (AmiVoice batch engines' interims; Yandex and NAVER turn signal), write `unknown` |
| `quality_signals`, `quality_signals_require` | same | Unchanged. Add `no_speech_flag` on FPT.AI rows (the vendor's status 1) and where the notes describe another yes-or-no "no speech" answer |
| `file_request_mode` | same | Unchanged |
| `gateway_client.status`, `.code_ref`, `.notes` | same | Unchanged, except as section 9 says. ElevenLabs `scribe_v2_realtime`: add `last_verified_on: "2026-06-04"` |
| `segment_profile.pre_roll_ms`, `.trailing_silence_ms`, `.max_in_flight`, `.request_timeout_ms`, `.upload_policy`, `.rationale` | same | Unchanged |
| `segment_profile.max_segment_seconds` | `segment_profile.max_segment_ms` | Multiply by 1000 |

### 8.2 Limits

| Version 1 | Version 2 | Rule |
| --- | --- | --- |
| `max_upload_bytes` | `max_upload_bytes` | Unchanged |
| `max_audio_seconds`, `min_audio_seconds`, `vendor_timeout_seconds`, `max_session_seconds`, `context_window_seconds` | `max_audio_ms`, `min_audio_ms`, `vendor_timeout_ms`, `max_session_ms`, `context_window_ms` | Multiply by 1000 and round to an integer; null stays null |
| `requests_per_minute` | one entry of `rates` | `{metric: "requests", per: "minute", value, plan, scope}`. If the notes or `provenance.unverified` say the figure was converted from a per-second quota (Yandex 20 and 40, Alibaba 20, Amazon 25, Speechmatics 10), write the vendor's own window instead: `per: "second"` and the original value. Drop a null |
| `max_concurrent_requests` | one entry of `rates` | `metric: "concurrent_requests"` on a `file_upload` transport, `metric: "concurrent_sessions"` on a socket transport; `per: "none"`. Drop a null. On the whisper.cpp profile set `single_process: true` instead |
| `audio_seconds_per_hour` | one entry of `rates` | `{metric: "audio_seconds", per: "hour", ...}` |
| `limits_scope` | `scope` of each entry | Copy to every entry made from this transport. Where the builders wrote `organisation` or `credential` for lack of a better word, write the true one: `account` (Alibaba, AssemblyAI, Cartesia, Gladia, Tencent, Sarvam), `application` (Baidu), `resource` (Azure Speech resource, IBM service instance, Azure OpenAI), `region` (Amazon account per region) |
| (in notes) | further entries of `rates` | Section 8.3 |
| (new) | `assumed_plan` | The plan whose figures version 1 recorded (it recorded "the lowest paid plan"): `tier_1` for OpenAI, `developer` for Groq, `starter` for Sarvam, and so on. null when every entry has `plan: null` |
| (new) | `ramp` | `true` on every `openai` file transport (the vendor's `slow_down` refusal); omit elsewhere |
| (new) | `idle_timeout_ms` | Where the notes give a socket idle limit (Cartesia 180000, moved out of `vendor_timeout_ms`; Yandex 5000; Tencent 15000; Huawei 20000; iFlytek 10000) |

### 8.3 Rate limits that live in notes today

Read `gateway_client.notes`, `provenance.unverified` and the provider notes of each row, and add one
`rates` entry for every figure found there. The known ones:

| Vendor | Entries to add |
| --- | --- |
| OpenAI | `tokens` per `minute`, plan `tier_1`: 200000 (`gpt-transcribe`), 10000 (`gpt-4o-transcribe`, the diarize model), 50000 (`gpt-4o-mini-transcribe`); `requests` per `minute`, plan `tier_5`: 30000 on `gpt-transcribe` (`verify/group-1.md:45`, `:79`). `gpt-realtime-whisper`: `audio_seconds` per `minute` 6000 (100 audio minutes a minute) |
| Groq | plan `free`: `requests` per `minute` 20, `requests` per `day` 2000, `audio_seconds` per `day` 28800; the existing figures are plan `developer` |
| ElevenLabs | `concurrent_requests` per plan, 8 to 60 as the notes list them |
| Deepgram hosted Whisper | one entry of 3 (the lowest of 3, 5 and 15, plan unknown); say so in the entry's `note` |
| Cartesia | `concurrent_sessions` 8 (plan `free`), 12 (plan `pro`), and 20 and 60 under the plan names its notes give; add `note` that sockets and HTTP requests share the limit |
| Gladia, AssemblyAI | the free and paid job and session figures in the notes |
| Sarvam | `requests` per `minute` 60, 100, 4000 and `concurrent_sessions` 20, 100, by plan |
| IBM | `concurrent_sessions` 100, plan `plus`, scope `resource`, shared with HTTP |
| SberDevices | `concurrent_sessions` 10 (plan `legal_entity`) and 5 (plan `individual`); the FAQ's 50 and 1 go in `note` |
| NAVER CSR | `audio_seconds` per `month` 300000 and per `day` 30000, scope `application` |
| Reverie | `concurrent_sessions` 5, plan `free` |

### 8.4 Latency

| Version 1 | Version 2 | Rule |
| --- | --- | --- |
| `class` | `class` | Unchanged, subject to the two checks below |
| `basis: "unmeasured"` | `measurements: []` | `class` stays `unknown`. `conditions`, if any, becomes `notes` |
| `ttfs_p99_ms`, `basis`, `source`, `measured_on`, `conditions` | one entry of `measurements` | `{percentile: "p99", quantity: "end_of_speech_to_final", value_ms, basis, source, measured_on?, conditions?}`. For the third-party benchmark figures add `pause_ms_assumed: 200` where `conditions` names a 200 ms detector stop |
| `ttfs_p50_ms` | a second entry | Same fields, `percentile: "p50"` |
| figures inside `conditions` on an unmeasured transport | further entries | Vendor claims with no percentile become `percentile: "typical"`, `basis: "vendor_claim"` (Gnani 300 to 500 ms: write 500; Speechmatics 250 ms after its end-of-utterance message). Single observed requests become `percentile: "single_sample"`, `quantity: "request_round_trip"` (Viettel 15700 and 22500). A vendor average becomes `percentile: "mean"` (Viettel 30000). Engine timings become `quantity: "inference_only"` (WaaV Infer 123 and 439) |

Checks: with a p99 `end_of_speech_to_final` entry the class must be the one the thresholds give
(600 and 1200 ms). Without one, the class is `unknown`, or `slow` when some entry is above 1200 ms.
Viettel's file transport therefore changes from the mislabelled "p99 30000, vendor claim" to three
entries (mean 30000, two single samples) with class `slow`.

### 8.5 Upload

| Version 1 | Version 2 | Rule |
| --- | --- | --- |
| `accepted` | `accepted` | Unchanged |
| `preferred.container`, `.encoding`, `.sample_rate_hz`, `.channels` | same | Unchanged. FPT.AI stays WAV with the doubt in `provenance.unverified`; `mp3` is now expressible if a probe shows WAV is rejected |
| `preferred.vendor_param` (`key=value`) | `vendor_params: [{name, value, in}]` | Split at the first `=`. `in` is `form` for a multipart envelope, `query` for a raw body, `json` for base64 in JSON. Exception: Reverie's `format=16k_int16` is a header and goes to `headers` |
| (new) | `envelope` | By `dialect.wire`. Established by the transcriber design or a builder gap: `openai_multipart`, `elevenlabs_multipart`, `assemblyai_sync`, `azure_fast_transcription`, `speechmatics_batch_jobs` are `multipart`; `deepgram_prerecorded`, `fpt_raw_body`, `yandex_stt_v1_rest`, `naver_csr_rest` are `raw_body`; `bhashini_pipeline_json`, `google_speech_v2_recognize` are `json_base64`. For every other interface read the request description in the transport's `gateway_client.notes` and in `providers/<provider>.md`; a name ending in `_multipart` or `_json` says which it is |
| (in notes) | `content_type` | NAVER CSR `application/octet-stream`; SberDevices `audio/x-pcm;bit=16;rate={sample_rate_hz}`; Deepgram `audio/wav` |
| (in notes) | second and third `vendor_params` | Yandex `sampleRateHertz={sample_rate_hz}` beside `format=lpcm`; Alibaba flash `sample_rate` beside `format` |
| (in notes) | `sample_rates_hz` | Where the notes list the accepted rates (Yandex 8000, 16000, 48000) |
| (in notes) | `prefer_call_rate` | `true` on Google `Recognize` transports (the vendor advises against resampling 8 kHz telephone audio), with `sample_rates_hz` including 8000 |

### 8.6 Socket audio (new)

Add `stream_audio` to a socket transport when its notes or the builder gaps state what the socket
accepts. It is required, with `commit`, on every commit transport that a release enables (the
OpenAI rows and the Cartesia rows of Release 4).

| Transport | `stream_audio` |
| --- | --- |
| OpenAI Realtime (`openai_realtime_ga`), Azure OpenAI the same | `encodings: ["pcm_s16le", "mulaw", "alaw"]`, `sample_rates_hz: [24000]` |
| ElevenLabs realtime | `encodings: ["pcm_s16le", "mulaw"]`; `sample_rates_hz`: the PCM rates from 8000 to 48000 that its notes list |
| IBM multimedia models | `sample_rates_hz` starting at 16000 |
| AmiVoice 16 kHz-only engines | `sample_rates_hz: [16000]` |
| Sarvam legacy socket | `encodings: ["pcm_s16le"]` (the client rejects mu-law) |
| vLLM, NVIDIA NIM profiles | `sample_rates_hz: [16000]` |

Also new on commit transports: `commit.audio_gating`, `speech_only` for OpenAI and `continuous` for
Cartesia (performance design, audio gating table).

### 8.7 Dialect

| Version 1 | Version 2 | Rule |
| --- | --- | --- |
| `language_param` (closed list) | `language_param` (the exact wire name, or null) | `none` becomes null. Otherwise write the vendor's spelling. Where `provenance.unverified` or the notes say the listed value was "the nearest", use the real name they give: AssemblyAI Sync `language_codes`; Deepgram Flux `language_hint`; Alibaba `language_hints`; Azure fast transcription `locales`; Google `language_codes`; Reverie `src_lang`; Bhashini `sourceLanguage`; Gnani socket `lang_code`; Speechmatics `melia-1` and `oak-1` `language_hints` (with `language=multi` as an upload `vendor_params` entry). `engine_id` rows keep `engine_id` |
| (new) | `language_shape` | `list` for `languages`, `language_codes`, `locales`, `language_hints`; otherwise `single` |
| (new) | `language_format` | Required on every transport of a scheduled adapter other than `regional_rest`. Unless the vendor audit says otherwise: `iso639_1` for the OpenAI family, ElevenLabs, AssemblyAI and Deepgram Whisper; `bcp47` for Azure, Google, Yandex and the NVIDIA NIM profile; `vendor` for `engine_id` and for Sarvam's tags |
| (new) | `language_required` | `true` where the vendor does not detect and assumes a language: AssemblyAI Sync, Cartesia `ink-whisper`, Gladia `solaria-3`, Yandex REST (its default is `ru-RU`) |
| `context_params: ["prompt", ...]` | `context_params: [{kind, wire_name, ...}]` | `kind` is the old value. `wire_name` is the same word unless the notes give the vendor's: Gnani `bias_list`; Phonexia `preferred_phrases`; Bhashini `hotword_list` (with `note` "Hindi only, one service"); AssemblyAI `keyterms_prompt`; Azure `phraseList.phrases`; Deepgram `keyterm`. Stored vocabularies named in the notes become `kind: "vocabulary_id"` (Rev AI `custom_vocabulary_id`, IBM `language_customization_id`, AmiVoice `profileId`, Huawei `vocabulary_id`, Baidu `lm_id`, Tencent `hotword_id`) |
| (in notes) | `max_tokens`, `max_items` | `prompt` on `whisper-1` and Groq: `max_tokens: 224`. ElevenLabs key terms: `max_items: 100` (the vendor allows 1000; above 100 the bill changes, see 8.9). AssemblyAI `keyterms_prompt`: `max_items: 100` |
| `response_formats` | `response_formats` | Unchanged; put the format the gateway requests first |
| `wire` | `wire` | Unchanged |
| (new) | `auth`, `path` | OpenAI and Groq: `bearer`, `/v1/audio/transcriptions`. Self-hosted profiles: `bearer_optional`, `/audio/transcriptions`. Azure OpenAI profile: `api_key_header`. Deepgram: `token`, `/v1/listen`. Others: omit |
| (new) | `droppable_params` | Only on `openai-compatible-file`: `["language"]` |
| (new) | `usage_reported`, `limit_headers` | Only where the vendor audit states them; omit otherwise |

### 8.8 Constraints (new)

Add `constraints` when a note, a builder gap or a decision says the transport serves only some
languages or regions. Known cases:

| Transport | Constraint |
| --- | --- |
| Google `chirp_2`, `native` | `languages: {mode: "only", codes: [the 16 streaming languages in the row's notes]}` |
| Speechmatics `native` | `languages: {mode: "except", codes: ["auto"]}` (automatic language is batch only) |
| Gladia `solaria-3` | `languages: {mode: "only", codes: [its five languages]}` |
| Amazon `native` | `languages: {mode: "except", codes: [the 18 batch-only codes]}` |
| Deepgram hosted Whisper | `regions: {mode: "except", values: ["eu", "au", "in"]}` |
| Azure fast transcription | `regions: {mode: "only", values: [the regions in the row's notes]}` |
| Speechmatics `melia-1`, `oak-1` | `regions: {mode: "only", values: ["eu1", "us1"]}` |
| Tencent flash | `regions: {mode: "only", values: ["china"]}` |
| Huawei | per row, the regions its notes name |
| Alibaba | per row, the regions its notes name |

A constraint is written only from a fact the fact-check confirmed. A doubtful one goes to
`provenance.unverified` instead, because a wrong constraint refuses a session that works.

### 8.9 Billing

The row keeps one `billing` block: the billing of the **first transport that the plan enables**
(the file transport on a segmented row, the native transport on a streaming row). Any other
transport whose billing differs gets its own complete `billing` block.

| Version 1 | Version 2 | Rule |
| --- | --- | --- |
| `unit` | `unit` | Unchanged; `self_hosted` and `waav-infer` rows and the file profiles change `unknown` to `none` |
| `min_billed_seconds` | `min_billed_ms` | Multiply by 1000 |
| (in notes) | `increment_ms` | Yandex and NAVER CSR: 15000, and `min_billed_ms: 15000` |
| (in notes) | `conditional_minimums` | ElevenLabs file rows: `{feature: "keyterms", above: 100, min_billed_ms: 20000}` and `{feature: "transcript_edit", min_billed_ms: 10000}` |
| (in notes) | `surcharges` | ElevenLabs file rows: `{feature: "keyterms", percent: 20}`, `{feature: "entity_detection", percent: 30}` |
| `bills_silence` | `bills_silence` | Unchanged on the row. On a transport block: `true` for a socket that is sent all call audio, `false` for uploads |
| (new) | `bills_failed_requests` | Only where the audit states it (Yandex bills an empty request: leave null and keep the sentence in `notes`) |
| `pricing_key`, `notes` | same | Unchanged; remove sentences that describe another transport once that transport has its own block |

Rows that need a transport-level block (each named by a builder or an auditor): Viettel, Reverie,
T-Bank, Rev AI, Sarvam, IBM, Tencent, Huawei, Baidu, Azure Speech, Speechmatics, AssemblyAI
`universal-3-5-pro`, Cartesia `ink-whisper`, OpenAI `gpt-transcribe`.

### 8.10 Provenance

| Version 1 | Version 2 | Rule |
| --- | --- | --- |
| `confidence`, `verified_on`, `verified_by`, `owner`, `sources`, `notes` | same | Unchanged |
| `unverified` | `unverified` | Keep each entry; rewrite the field paths that changed name (`limits.max_audio_seconds` to `limits.max_audio_ms`, `limits.requests_per_minute` and `limits.max_concurrent_requests` to `limits.rates`, `limits.limits_scope` to `limits.rates[].scope`, `billing.min_billed_seconds` to `billing.min_billed_ms`) |
| (new) | `code_refs` | Every repository path with a line number that the row cites as evidence for a capability fact. For `waav-infer` rows and profiles this allows `confidence: "documented"`, `verified_by: "code_reading"` |
| (new) | `probe_ref` | On the two rows verified by a live probe (NECTEC `partii5`, Viettel default): the probe note under `verify/` |
| (new) | `recheck_on` | Section 7 |

## 9. Setting `enabled_from_release`, and the order of transports

Apply in this order to each transport.

1. **Adapter `native`.** `enabled_from_release: 0`, whatever version 1's `enabled` said
   (decisions section 15: a native client found broken is reported, not refused).
   - Version 1 `enabled: true`: no other change.
   - Version 1 `enabled: false` with status `known_broken`: keep the status, move the text of
     `disabled_reason` to `gateway_client.defect`, delete `disabled_reason`.
   - Version 1 `enabled: false` with any other status (one case, Google `chirp`): set the status to
     `unverified`, move `disabled_reason` to `gateway_client.defect`.
   - **Exception, substitution.** Where the `known_broken` text says the client serves a different
     model than the one named, the transport is removed from the row and the fact moves to
     `when_unusable` (section 10): AssemblyAI `universal-3-6-pro`, `universal-3-5-pro` and the
     default row; Gladia default row.
2. **Adapter `planned_stream`, `planned_commit` or `planned_file`.** `enabled_from_release: null`.
   Keep `disabled_reason` unchanged; the adapter id already says that no release builds the client.
3. **A scheduled adapter.** `enabled_from_release` is the adapter's release from section 3, and
   `disabled_reason` is kept (it describes the state before that release). Exceptions:
   - `openai_realtime_transcription` on `azure_openai` rows and in the profile: null, with
     "Decisions section 12 places the commit transport for OpenAI only." appended.
   - a file profile listed for Release 2 in section 5: 2.
4. **`enable_requires`.**
   - Every `regional_rest` transport: `["live_probe"]`.
   - `viettel-ai`, and every `speechmatics_batch` transport: `["live_probe", "latency_within_deadline"]`.
   - Nothing else.

**Order.** The first usable transport is what a session gets by default, so the order decides
whether a later release moves a session from streaming to uploads. Two rules, applied after the
renames:

1. **A working native client stays the default.** If the row has a `native` transport that
   version 1 had `enabled: true`, move it to the front. This changes the default rows of
   `alibaba-cloud`, `deepgram`, `google`, `ibm-watson`, `microsoft-azure`, `sarvam` and
   `speechmatics`, and five Google rows (`chirp_telephony`, `telephony_short`, `short`,
   `medical_conversation`, `medical_dictation`), which version 1 listed file-first. Their file
   transport stays in the list and can be chosen with `transcription_mode: "segmented"`.
   **Exception: Cartesia.** `cartesia_manual_finalize` stays first, as version 1 has it, so
   Cartesia sessions move to gateway-driven finalize in Release 4 (decisions section 12).
2. **A broken native client is the last resort.** A `native` transport that version 1 had
   `enabled: false` (status `known_broken`, or Google `chirp`) moves behind every transport whose
   `enabled_from_release` is a number from 1 to 6. Until that release the broken client is the only
   usable entry and the session gets it with a warning; from that release the working transport
   wins. This changes 22 Tencent rows, 7 Huawei rows, one Baidu row and Google `latest_short`,
   where version 1 listed the broken client first.

Every other transport keeps its version 1 position relative to its neighbours.

## 10. Setting `when_unusable`: today's behaviour, and the refusal code and reason

A row needs `when_unusable` when, after sections 8 and 9, no transport has
`enabled_from_release: 0`. (A pass-through row never has one.) Rows whose first transport is an
enabled `native` do not need it, and get none in this pass.

**Step 1: `today`.** From this table. "Opened" references were read for this document; the rest are
frame fact F7 of `design/DESIGN_FRAME.md` or the named auditor's note.

| Provider and rows | `today` | `code_ref` | Source |
| --- | --- | --- | --- |
| `openai`, `groq`: every row with a file transport | `buffers_until_hangup` | `gateway/src/core/stt/openai/config.rs:651` (opened: the default flush strategy is `OnDisconnect`); the Groq client's equivalent | F7 |
| `openai` `gpt-live-transcribe`, `gpt-realtime-whisper` | `fails` | same | The id is sent to the file endpoint at hang-up, which does not serve it |
| `elevenlabs` `scribe_v2`, `scribe_v2_medical`, default row | `refused_at_setup` | `gateway/src/core/stt/elevenlabs/config.rs:391` (opened) | F7 |
| `self_hosted`, `azure_openai`: every row | `refused_at_setup` | `gateway/src/handlers/ws/bud_legs.rs:122-134` and `:815-826` (opened) | F7 |
| `waav-infer`: every row | `fails` | the stub named in the provider notes | Provider notes |
| `deepgram` `whisper*` and `flux*` rows | `fails` | `gateway/src/core/stt/deepgram.rs:486-491` (opened: the id is sent to the socket unchecked) | Row notes |
| `yandex`, `sberdevices`: every row with a file transport | `blind_timed_uploads` | `gateway/src/core/stt/yandex/client.rs:219-332` (opened); the Sber client's equivalent | F7; decisions section 5 |
| `bhashini`, `fpt-ai`, `naver-clova`, `nectec`, `viettel-ai`: every row | `buffers_until_hangup` | each client's buffer path, from the provider notes | F7; decisions section 5 |
| `assemblyai` `universal-3-6-pro`, `universal-3-5-pro`, `universal-2`, `u3-sync-pro`, `u3-pro`, default row | `streams_substituted_model`, `substituted_model: "universal-streaming-english"` | from the removed transport's `code_ref` | Auditor, deepgram group |
| `gladia` `solaria-3`, default row | `streams_substituted_model`, `"solaria-1"` | same | Auditor, deepgram group |
| `microsoft-azure` `MAI-Transcribe-2`, `MAI-Transcribe-1.5`, `MAI-Transcribe-2-Streaming`, `llm-speech` | `streams_substituted_model`, `"default"` | the client never reads the model | Auditor, microsoft-azure group |
| `aws-transcribe` `medical`, `call-analytics`, `healthscribe` | `streams_substituted_model`, `"standard"` | provider notes | Auditor, microsoft-azure group |
| `revai` `low_cost`, `fusion` | `streams_substituted_model`, `"machine"` | row notes | Auditor, revai group |
| `alibaba-cloud` `paraformer-v2`, `paraformer-8k-v2`, `paraformer-v1`, `paraformer-8k-v1`, `fun-asr` | `streams_substituted_model`, the streaming model the client rewrites each to (provider notes) | provider notes | Builder gap, alibaba group |
| `iflytek` `iflyrec_voice_*` rows | `streams_substituted_model`, `"iat"` | the client always sends domain `iat` | Auditor, tencent group |
| `phonexia`: rows without a `native` transport | `fails` | row notes (every session fails at setup today) | Auditor, gnani group |
| Any other row with no usable transport (for example `gnani` Vachana rows, `sarvam` `saaras:v3-realtime` and `saaras:v2.5`, `speechmatics` `melia-1`, `oak-1`, `linden-1`, `revai` `human` and `*whisper*`, retired rows) | `unknown` | omit | Nobody established what the vendor does with the id |

**Step 2: `refuse_from_release`.**

| Case | Value |
| --- | --- |
| The row has `lifecycle.status: "retired"` | `0` (the schema requires it) |
| Version 1 had `no_live_path_reason`, the row has no transport enabled from release 0, and `today` is not `streams_substituted_model` | `0` |
| `today` is `streams_substituted_model` and the reason of step 3 is `async_only` (the vendor's only file interface for the model is a slow asynchronous job): AssemblyAI `universal-2`, Gladia `solaria-3`, and the Rev AI, Alibaba and iFlytek rows above | `3` (decisions section 12: reported with `stt_model_substituted` in Releases 1 and 2, refused from Release 3) |
| Everything else | null. `today` then decides in every release: a buffering client is refused for a voice-agent session whose turn detection is not manual and kept, with the warning `stt_buffered_until_commit`, for every other session (decisions section 5 as narrowed by addendum A1); a blind uploader, a substituting client and an unknown case keep today's client with a warning; `refused_at_setup` and `fails` are refused until a transport is enabled |

One version 1 row has `no_live_path_reason` beside a native transport that stays usable (Gnani
`default`): append its text to that transport's `gateway_client.defect` and write no `when_unusable`.

**Step 3: `refusal`.** Required when `today` is `buffers_until_hangup`, `refused_at_setup` or
`fails`, and whenever `refuse_from_release` is a number.

- `code`: `stt_model_retired` when the lifecycle status is `retired`; otherwise
  `stt_live_unsupported`.
- `reason` (with `stt_live_unsupported` only), the first that applies:

| Condition | `reason` |
| --- | --- |
| `lifecycle.status` is `invalid` | `model_not_served` |
| The provider is not registered in the gateway and no transport is ever enabled | `provider_not_built` |
| The vendor's only file interface for the model is an asynchronous job (the version 1 reason says so, or the only transports are `async_poll`) | `async_only` |
| The model is not a transcription product for calls (Sarvam `saaras:v2.5` translates; Rev AI `human`), or the vendor's terms bar the use | `disabled` |
| Anything else: a transport exists or is planned and is not enabled yet | `client_not_implemented` |

  These five values are the customer contract's wire list. The capability-map design's older names
  map onto them: `async_job_only` to `async_only`, `realtime_only_no_client` to
  `client_not_implemented`, `vendor_unreachable` to `provider_not_built`, `not_usable_on_calls` to
  `disabled`; `retired` is the code `stt_model_retired`.
- `text`: the version 1 `no_live_path_reason` when there was one. Otherwise two or three plain
  sentences: what the model is, why it cannot be used on this call, and what to choose instead
  (a streaming model of the same provider, manual turn detection, or asking the operator to enable
  segmented speech-to-text). No URL, no internal identifier, no release number.

## 11. The row identifier

`id` is `<provider>:<key>`.

- `<provider>` is the file's `provider_id`; `global` in `_global_default.json`.
- `<key>` for an exact row: the model id in lowercase, with every run of characters other than
  `a-z`, `0-9`, `.`, `_` and `-` replaced by one `-`, and leading and trailing `-` removed.
  `general:rc` gives `general-rc`; `kyutai/stt-1b-en_fr` gives `kyutai-stt-1b-en_fr`.
- `<key>` for a pattern row: replace each `*` by `-any-`, apply the same rule, then collapse every
  run of `-` to a single `-` and remove a leading or trailing `-`. `*` gives `any`; `whisper-*`
  gives `whisper-any`; `*voxtral*realtime*` gives `any-voxtral-any-realtime-any`.
- At most 64 characters. The longest version 1 key is 51.
- If two rows of a provider would get the same key, append `-2` to the later one.

The id is assigned once. It does not change when `match` is edited later; the validator checks the
provider prefix and uniqueness, not the derivation. It is the row part of the breaker key, the
`model` label of the new metrics, an entry of the rollout allow-list (`provider:key` is the id
itself) and of the rollback list.

## 12. Three worked examples

Long prose strings are cut with "…" to keep the examples readable; in the real files they are
carried over whole. The three "after" rows were built in full and validated against the version 2
schema.

### 12.1 OpenAI `gpt-transcribe`

Before (`rows/openai.json`, first row):

```json
{
  "match": { "provider": "openai", "model": "gpt-transcribe" },
  "lifecycle": { "status": "ga" },
  "transports": [
    {
      "input_mode": "file_upload",
      "adapter": "segmented_openai_compat",
      "enabled": false,
      "disabled_reason": "The file-upload adapter for live calls (segmented_openai_compat) is not built yet. Today …",
      "interim_results": "post_commit",
      "endpointing_owner": "gateway",
      "vendor_turn_signal": "none",
      "latency": {
        "class": "unknown",
        "basis": "unmeasured",
        "conditions": "No published figure for this model on the file endpoint. The only OpenAI file-route …"
      },
      "limits": {
        "max_upload_bytes": 26214400,
        "max_audio_seconds": null,
        "min_audio_seconds": null,
        "requests_per_minute": 500,
        "max_concurrent_requests": null,
        "limits_scope": "organisation"
      },
      "upload": {
        "accepted": [ "flac", "mp3", "mp4", "mpeg", "mpga", "m4a", "ogg", "wav", "webm" ],
        "preferred": { "container": "wav", "encoding": "pcm_s16le", "sample_rate_hz": 16000, "channels": 1 }
      },
      "quality_signals": [ "detected_languages" ],
      "dialect": {
        "language_param": "languages",
        "context_params": [ "prompt", "keywords" ],
        "response_formats": [ "json" ],
        "wire": "openai_multipart"
      },
      "gateway_client": {
        "status": "not_implemented",
        "code_ref": "gateway/src/core/stt/openai/client.rs:374 (flush_buffer, takes &mut self); gateway/src/core/stt/openai/config.rs:848-906 (transcription_text_fields)",
        "notes": "No adapter exists. The request and response code lives inside the buffering client: …"
      },
      "file_request_mode": "sync",
      "segment_profile": {
        "request_timeout_ms": 10000,
        "rationale": "Only the HTTP request timeout deviates from the gateway defaults. The existing client …"
      }
    },
    {
      "input_mode": "vendor_segmented",
      "adapter": "commit_openai",
      "enabled": false,
      "disabled_reason": "No gateway speech-to-text client speaks the OpenAI Realtime transcription socket (adapter …",
      "interim_results": "post_commit",
      "endpointing_owner": "gateway",
      "vendor_turn_signal": "none",
      "latency": {
        "class": "unknown",
        "basis": "unmeasured",
        "conditions": "No figure for gpt-transcribe on the socket. Pipecat measured a different model, …"
      },
      "limits": { "max_session_seconds": null, "requests_per_minute": null, "limits_scope": "organisation" },
      "quality_signals": [ "detected_languages" ],
      "dialect": {
        "language_param": "languages",
        "context_params": [ "prompt", "keywords" ],
        "wire": "openai_realtime_ga"
      },
      "gateway_client": {
        "status": "not_implemented",
        "code_ref": "gateway/src/handlers/openai_realtime/upstream.rs:113-141 (pass-through facade, dials …",
        "notes": "The socket accepts only 24 kHz PCM or G.711 (mu-law or A-law), base64 inside …"
      }
    }
  ],
  "billing": {
    "unit": "audio_minute",
    "min_billed_seconds": null,
    "bills_silence": false,
    "notes": "Describes the file transport: only uploaded segments are billed. Vendor price in the …"
  },
  "provenance": {
    "confidence": "documented",
    "verified_on": "2026-10-03",
    "verified_by": "docs",
    "owner": "waav-gateway",
    "sources": [ "https://developers.openai.com/api/docs/models/gpt-transcribe", "… (11 more entries)" ],
    "unverified": [
      "transports[0].limits.max_audio_seconds (no maximum duration is documented; community reports of about 1500 s are not official)",
      "transports[0].limits.min_audio_seconds (none documented)",
      "transports[0].limits.max_concurrent_requests (none documented; limits are per minute)",
      "transports[0].limits.requests_per_minute (500 is usage tier 1, the lowest paid tier; higher tiers allow more)",
      "transports[0].upload.preferred (OpenAI names no latency-preferred format; WAV is accepted)",
      "billing.min_billed_seconds (no minimum or rounding rule is documented)",
      "transports[0].latency (no published figure for this model)",
      "transports[0].dialect.response_formats (json is safe; the fact-check could not confirm the exact per-model restrictions)",
      "transports[0].dialect.language_param (whether the API rejects or ignores a singular `language` for this model is unknown)",
      "transports[1] endpointing_owner and vendor_turn_signal (whether this model accepts server …",
      "transports[1].limits.max_session_seconds and requests_per_minute (not on the pages fetched)",
      "transports[1] socket URL (?intent=transcription is absent from official pages)",
      "billing for transports[1] (the costs guide says only that a different rate card applies)",
      "transports[*].limits.limits_scope (the vendor defines rate limits at organisation level and at project level; the row records organisation)"
    ],
    "notes": "The vendor's recommended model for recorded speech. On the Realtime socket it starts …"
  }
}
```

After:

```json
{
  "id": "openai:gpt-transcribe",
  "match": { "provider": "openai", "model": "gpt-transcribe" },
  "lifecycle": { "status": "ga" },
  "transports": [
    {
      "input_mode": "file_upload",
      "adapter": "openai_transcriptions",
      "enabled_from_release": 1,
      "disabled_reason": "The file-upload adapter for live calls (segmented_openai_compat) is not built yet. Today … (text unchanged from version 1)",
      "interim_results": "post_commit",
      "endpointing_owner": "gateway",
      "vendor_turn_signal": "none",
      "latency": {
        "class": "unknown",
        "measurements": [],
        "notes": "No published figure for this model on the file endpoint. The only OpenAI file-route … (text unchanged from version 1)"
      },
      "limits": {
        "max_upload_bytes": 26214400,
        "max_audio_ms": null,
        "min_audio_ms": null,
        "rates": [
          { "metric": "requests", "per": "minute", "value": 500, "plan": "tier_1", "scope": "organisation" },
          { "metric": "tokens", "per": "minute", "value": 200000, "plan": "tier_1", "scope": "organisation" },
          { "metric": "requests", "per": "minute", "value": 30000, "plan": "tier_5", "scope": "organisation" }
        ],
        "assumed_plan": "tier_1",
        "ramp": true
      },
      "upload": {
        "accepted": [ "flac", "mp3", "mp4", "mpeg", "mpga", "m4a", "ogg", "wav", "webm" ],
        "preferred": { "container": "wav", "encoding": "pcm_s16le", "sample_rate_hz": 16000, "channels": 1 },
        "envelope": "multipart"
      },
      "quality_signals": [ "detected_languages" ],
      "dialect": {
        "language_param": "languages",
        "language_shape": "list",
        "language_format": "iso639_1",
        "context_params": [
          { "kind": "prompt", "wire_name": "prompt" },
          { "kind": "keywords", "wire_name": "keywords" }
        ],
        "response_formats": [ "json" ],
        "wire": "openai_multipart",
        "auth": "bearer",
        "path": "/v1/audio/transcriptions"
      },
      "gateway_client": {
        "status": "not_implemented",
        "code_ref": "gateway/src/core/stt/openai/client.rs:374 (flush_buffer, takes &mut self); gateway/src/core/stt/openai/config.rs:848-906 (transcription_text_fields)",
        "notes": "No adapter exists. The request and response code lives inside the buffering client: … (text unchanged from version 1)"
      },
      "file_request_mode": "sync",
      "segment_profile": {
        "request_timeout_ms": 10000,
        "rationale": "Only the HTTP request timeout deviates from the gateway defaults. The existing client … (text unchanged from version 1)"
      }
    },
    {
      "input_mode": "vendor_segmented",
      "adapter": "openai_realtime_transcription",
      "enabled_from_release": 4,
      "disabled_reason": "No gateway speech-to-text client speaks the OpenAI Realtime transcription socket (adapter … (text unchanged from version 1)",
      "interim_results": "post_commit",
      "endpointing_owner": "gateway",
      "vendor_turn_signal": "none",
      "latency": {
        "class": "unknown",
        "measurements": [],
        "notes": "No figure for gpt-transcribe on the socket. Pipecat measured a different model, … (text unchanged from version 1)"
      },
      "limits": {
        "max_session_ms": null,
        "rates": []
      },
      "stream_audio": {
        "encodings": [ "pcm_s16le", "mulaw", "alaw" ],
        "sample_rates_hz": [ 24000 ],
        "channels": 1
      },
      "commit": { "audio_gating": "speech_only" },
      "quality_signals": [ "detected_languages" ],
      "dialect": {
        "language_param": "languages",
        "language_shape": "list",
        "language_format": "iso639_1",
        "context_params": [
          { "kind": "prompt", "wire_name": "prompt" },
          { "kind": "keywords", "wire_name": "keywords" }
        ],
        "wire": "openai_realtime_ga"
      },
      "gateway_client": {
        "status": "not_implemented",
        "code_ref": "gateway/src/handlers/openai_realtime/upstream.rs:113-141 (pass-through facade, dials … (text unchanged from version 1)",
        "notes": "The socket accepts only 24 kHz PCM or G.711 (mu-law or A-law), base64 inside … (text unchanged from version 1)"
      },
      "billing": {
        "unit": "unknown",
        "min_billed_ms": null,
        "bills_silence": null,
        "notes": "The vendor's cost guide says only that a different rate card applies on the Realtime socket; whether this model is billed at the file price there is not documented."
      }
    }
  ],
  "when_unusable": {
    "today": "buffers_until_hangup",
    "code_ref": "gateway/src/core/stt/openai/config.rs:651 (the default flush strategy is OnDisconnect)",
    "refuse_from_release": null,
    "refusal": {
      "code": "stt_live_unsupported",
      "reason": "client_not_implemented",
      "text": "This model transcribes uploaded files and has no streaming interface. On this gateway it is not yet served to a voice agent that decides when the caller has finished speaking. Use a streaming transcription model, set the agent's turn detection to manual, or ask the operator to enable segmented speech-to-text for this deployment."
    }
  },
  "billing": {
    "unit": "audio_minute",
    "min_billed_ms": null,
    "bills_silence": false,
    "notes": "Only uploaded segments are billed. Vendor price in the evidence is $0.0045 per audio minute. gateway/src/config/pricing.rs has no openai:gpt-transcribe entry yet (only openai:whisper-1), so no pricing_key is set. The response carries a usage object (seconds or tokens) that the gateway does not parse today."
  },
  "provenance": {
    "confidence": "documented",
    "verified_on": "2026-10-03",
    "verified_by": "docs",
    "owner": "waav-gateway",
    "sources": [ "https://developers.openai.com/api/docs/models/gpt-transcribe", "… (11 more entries, unchanged)" ],
    "code_refs": [
      "gateway/src/core/stt/openai/config.rs:63-67",
      "gateway/src/core/stt/openai/config.rs:97-104",
      "gateway/src/core/stt/openai/config.rs:651"
    ],
    "unverified": [
      "transports[0].limits.max_audio_ms (no maximum duration is documented; community reports of about 1500 s are not official)",
      "transports[0].limits.min_audio_ms (none documented)",
      "transports[0].limits.rates (concurrent_requests) (none documented; limits are per minute)",
      "transports[0].limits.rates (requests) (500 is usage tier 1, the lowest paid tier; higher tiers allow more)",
      "transports[0].upload.preferred (OpenAI names no latency-preferred format; WAV is accepted)",
      "billing.min_billed_ms (no minimum or rounding rule is documented)",
      "transports[0].latency (no published figure for this model)",
      "transports[0].dialect.response_formats (json is safe; the fact-check could not confirm the exact per-model restrictions)",
      "transports[0].dialect.language_param (whether the API rejects or ignores a singular `language` for this model is unknown)",
      "transports[1] endpointing_owner and vendor_turn_signal (whether this model accepts server … (text unchanged from version 1)",
      "transports[1].limits.max_session_ms and rates (requests) (not on the pages fetched)",
      "transports[1] socket URL (?intent=transcription is absent from official pages)",
      "billing for transports[1] (the costs guide says only that a different rate card applies)",
      "transports[*].limits.rates[].scope (the vendor defines rate limits at organisation level and at project level; the row records organisation)"
    ],
    "notes": "The vendor's recommended model for recorded speech. On the Realtime socket it starts … (text unchanged from version 1)"
  }
}
```

What was done, rule by rule:

1. `id`: `openai:gpt-transcribe` (section 11).
2. First transport: `segmented_openai_compat` on provider `openai` becomes `openai_transcriptions`
   (section 4, rule 5). It is a scheduled adapter, so `enabled_from_release: 1`, and the old
   `disabled_reason` stays as the description of today (section 9, step 3).
3. Second transport: `commit_openai` becomes `openai_realtime_transcription`, enabled from Release 4
   (decisions section 1 lists "`gpt-transcribe` on the socket" there). It keeps second place, so the
   default stays the file upload. `stream_audio` and `commit.audio_gating` are added because the
   adapter is a scheduled commit socket (section 8.6); the 24 kHz and G.711 fact was in its notes.
4. Limits: `requests_per_minute: 500` and `limits_scope: "organisation"` become one `rates` entry
   with `plan: "tier_1"`; the 200,000 tokens a minute came from `gateway_client.notes`, the tier 5
   figure from the fact-check (`verify/group-1.md:45`); `assumed_plan: "tier_1"`; `ramp: true`
   (section 8.2). The socket transport's null request rate is dropped, and with no entry there is
   no scope left to record.
5. Latency: `basis: "unmeasured"` becomes `measurements: []` and `conditions` becomes `notes`.
   No figure is invented: decisions section 15 says the 2,010 ms seed belongs to
   `gpt-4o-mini-transcribe` only.
6. Upload: `envelope: "multipart"` (section 8.5).
7. Dialect: `languages` is already the wire name; shape `list`, format `iso639_1`; the two context
   fields gain their wire names; `auth` and `path` are added (section 8.7).
8. Billing: the row block describes the file transport. The sentence about the Realtime socket's
   rate card leaves the row notes and becomes the socket transport's own `billing` block
   (section 8.9).
9. `when_unusable`: OpenAI's client buffers until hang-up, so `today: "buffers_until_hangup"`,
   `refuse_from_release: null`, and a refusal with reason `client_not_implemented` (section 10).
   In Release 0 a voice-agent session with automatic turn detection on this model is refused with that
   text, and every other session keeps working with a warning; from Release 1 a session the allow-list covers uses the file transport.
10. Provenance: `unverified` paths renamed; `code_refs` added from the lines opened for this
    document.

### 12.2 Deepgram `whisper-large`

Before (`rows/deepgram.json`):

```json
{
  "match": { "provider": "deepgram", "model": "whisper-large" },
  "lifecycle": { "status": "ga" },
  "transports": [
    {
      "input_mode": "file_upload",
      "adapter": "segmented_deepgram",
      "enabled": false,
      "disabled_reason": "Not implemented. No per-utterance file adapter exists for live sessions, and this model …",
      "interim_results": "none",
      "endpointing_owner": "gateway",
      "vendor_turn_signal": "none",
      "latency": {
        "class": "unknown",
        "basis": "unmeasured",
        "conditions": "Deepgram publishes no latency for Whisper Cloud; it says only that its other models …"
      },
      "limits": {
        "max_upload_bytes": 2000000000,
        "max_audio_seconds": null,
        "min_audio_seconds": null,
        "vendor_timeout_seconds": 600,
        "requests_per_minute": null,
        "max_concurrent_requests": 3,
        "limits_scope": "project"
      },
      "upload": {
        "accepted": [ "wav", "raw_pcm", "flac", "mp3", "mp4", "m4a", "aac", "ogg", "opus", "webm" ],
        "preferred": { "container": "wav", "encoding": "pcm_s16le", "sample_rate_hz": 16000, "channels": 1 }
      },
      "quality_signals": [ "utterance_confidence", "word_confidence" ],
      "dialect": {
        "language_param": "language",
        "context_params": [],
        "wire": "deepgram_prerecorded"
      },
      "gateway_client": {
        "status": "not_implemented",
        "code_ref": "gateway/src/core/stt/batch.rs:336-353,421-425; …",
        "notes": "No code takes one finished utterance and returns one transcript during a live session. …"
      },
      "file_request_mode": "sync",
      "segment_profile": {
        "max_in_flight": 1,
        "request_timeout_ms": 10000,
        "rationale": "One upload in flight per session instead of two, because Whisper Cloud allows as few as 3 …"
      }
    }
  ],
  "billing": {
    "unit": "audio_second",
    "min_billed_seconds": 0,
    "bills_silence": false,
    "pricing_key": "deepgram:whisper",
    "notes": "Billed per second of uploaded audio with no minimum, so short segments need not be …"
  },
  "provenance": {
    "confidence": "documented",
    "verified_on": "2026-10-03",
    "verified_by": "docs",
    "owner": "waav-gateway",
    "sources": [ "https://developers.deepgram.com/docs/deepgram-whisper-cloud", "… (10 more entries)" ],
    "unverified": [
      "transports[0].latency (no published figure)",
      "transports[0].limits.max_concurrent_requests (Deepgram publishes 3, 5 and 15 on three …",
      "transports[0].limits.vendor_timeout_seconds (20 minutes in the pre-recorded guide, 10 minutes on the Whisper Cloud page; the lower is recorded)",
      "transports[0].limits.max_audio_seconds and min_audio_seconds (no duration bounds are stated)",
      "transports[0].dialect.language_param (the vendor documents bare Whisper codes such as en; …",
      "whether Whisper Cloud rejects or ignores unsupported parameters (keyterm, …",
      "lifecycle.status (no notice about the future of hosted Whisper Cloud was found)"
    ],
    "notes": "Deepgram Whisper Cloud is file-only: the vendor states that live streaming is not …"
  }
}
```

After:

```json
{
  "id": "deepgram:whisper-large",
  "match": { "provider": "deepgram", "model": "whisper-large" },
  "lifecycle": { "status": "ga" },
  "transports": [
    {
      "input_mode": "file_upload",
      "adapter": "deepgram_prerecorded",
      "enabled_from_release": 3,
      "disabled_reason": "Not implemented. No per-utterance file adapter exists for live sessions, and this model … (text unchanged from version 1)",
      "interim_results": "none",
      "endpointing_owner": "gateway",
      "vendor_turn_signal": "none",
      "constraints": {
        "regions": {
          "mode": "except",
          "values": [ "eu", "au", "in" ],
          "note": "Whisper Cloud is offered on Deepgram's North America host only, not on the Europe, Australia or India hosts."
        }
      },
      "latency": {
        "class": "unknown",
        "measurements": [],
        "notes": "Deepgram publishes no latency for Whisper Cloud; it says only that its other models … (text unchanged from version 1)"
      },
      "limits": {
        "max_upload_bytes": 2000000000,
        "max_audio_ms": null,
        "min_audio_ms": null,
        "vendor_timeout_ms": 600000,
        "rates": [
          {
            "metric": "concurrent_requests",
            "per": "none",
            "value": 3,
            "plan": null,
            "scope": "project",
            "note": "Deepgram publishes 3, 5 and 15 on three pages without saying which plan each applies to; the lowest is recorded."
          }
        ],
        "assumed_plan": null
      },
      "upload": {
        "accepted": [ "wav", "raw_pcm", "flac", "mp3", "mp4", "m4a", "aac", "ogg", "opus", "webm" ],
        "preferred": { "container": "wav", "encoding": "pcm_s16le", "sample_rate_hz": 16000, "channels": 1 },
        "envelope": "raw_body",
        "content_type": "audio/wav"
      },
      "quality_signals": [ "utterance_confidence", "word_confidence" ],
      "dialect": {
        "language_param": "language",
        "language_shape": "single",
        "language_format": "iso639_1",
        "context_params": [],
        "wire": "deepgram_prerecorded",
        "auth": "token",
        "path": "/v1/listen"
      },
      "gateway_client": {
        "status": "not_implemented",
        "code_ref": "gateway/src/core/stt/batch.rs:336-353,421-425; … (text unchanged from version 1)",
        "notes": "No code takes one finished utterance and returns one transcript during a live session. … (text unchanged from version 1)"
      },
      "file_request_mode": "sync",
      "segment_profile": {
        "max_in_flight": 1,
        "request_timeout_ms": 10000,
        "rationale": "One upload in flight per session instead of two, because Whisper Cloud allows as few as 3 … (text unchanged from version 1)"
      }
    }
  ],
  "when_unusable": {
    "today": "fails",
    "code_ref": "gateway/src/core/stt/deepgram.rs:486-491 (the id is sent to the streaming socket unchecked)",
    "refuse_from_release": null,
    "refusal": {
      "code": "stt_live_unsupported",
      "reason": "client_not_implemented",
      "text": "Deepgram's hosted Whisper models transcribe uploaded files only; Deepgram does not stream them. This gateway serves them on calls from the release that adds the Deepgram file transcriber. Until then choose a Deepgram streaming model such as nova-3."
    }
  },
  "billing": {
    "unit": "audio_second",
    "min_billed_ms": 0,
    "bills_silence": false,
    "pricing_key": "deepgram:whisper",
    "notes": "Billed per second of uploaded audio with no minimum, so short segments need not be … (text unchanged from version 1)"
  },
  "provenance": {
    "confidence": "documented",
    "verified_on": "2026-10-03",
    "verified_by": "docs",
    "owner": "waav-gateway",
    "sources": [ "https://developers.deepgram.com/docs/deepgram-whisper-cloud", "… (10 more entries, unchanged)" ],
    "code_refs": [ "gateway/src/core/stt/deepgram.rs:486-491" ],
    "unverified": [
      "transports[0].latency (no published figure)",
      "transports[0].limits.rates (concurrent_requests) (Deepgram publishes 3, 5 and 15 on three pages; the lowest is recorded and the plan each applies to is unclear)",
      "transports[0].limits.vendor_timeout_ms (20 minutes in the pre-recorded guide, 10 minutes on the Whisper Cloud page; the lower is recorded)",
      "transports[0].limits.max_audio_ms and min_audio_ms (no duration bounds are stated)",
      "transports[0].dialect.language_param (the vendor documents bare Whisper codes such as en; … (text unchanged from version 1)",
      "whether Whisper Cloud rejects or ignores unsupported parameters (keyterm, … (text unchanged from version 1)",
      "lifecycle.status (no notice about the future of hosted Whisper Cloud was found)"
    ],
    "notes": "Deepgram Whisper Cloud is file-only: the vendor states that live streaming is not … (text unchanged from version 1)"
  }
}
```

What was done:

1. `id`: `deepgram:whisper-large`.
2. `segmented_deepgram` becomes `deepgram_prerecorded`, enabled from Release 3 (decisions section 6).
3. `max_concurrent_requests: 3` with `limits_scope: "project"` becomes one `rates` entry; the
   conflict between 3, 5 and 15 moves from `provenance.unverified` into the entry's `note`, and stays
   listed as unverified. `vendor_timeout_seconds: 600` becomes `vendor_timeout_ms: 600000`.
4. The North America restriction, which version 1 could only say in notes, becomes
   `constraints.regions` (section 8.8).
5. Upload: `envelope: "raw_body"`, `content_type: "audio/wav"`. Dialect: `language_format:
   "iso639_1"`, `auth: "token"`, `path: "/v1/listen"`; no context fields, as before.
6. `when_unusable`: the row has no native transport, because Deepgram does not stream this model.
   Today the id is sent to the streaming socket unchecked and fails at the vendor, so `today:
   "fails"`. Decisions section 6 says such a session is refused with `stt_live_unsupported` until
   Release 3: `refuse_from_release` stays null (the refusal follows from `fails`) and ends by itself
   when the transport becomes enabled.
7. Billing and lifecycle are unchanged apart from `min_billed_seconds: 0` becoming
   `min_billed_ms: 0`.

### 12.3 Yandex `general`

Before (`rows/yandex.json`):

```json
{
  "match": { "provider": "yandex", "model": "general" },
  "lifecycle": { "status": "ga" },
  "transports": [
    {
      "input_mode": "file_upload",
      "adapter": "segmented_yandex",
      "enabled": false,
      "disabled_reason": "The per-utterance file-upload adapter (segmented_yandex) is not implemented. The Yandex …",
      "interim_results": "none",
      "endpointing_owner": "gateway",
      "vendor_turn_signal": "none",
      "latency": { "class": "unknown", "basis": "unmeasured" },
      "limits": {
        "max_upload_bytes": 1000000,
        "max_audio_seconds": 30,
        "min_audio_seconds": null,
        "requests_per_minute": 1200,
        "max_concurrent_requests": null,
        "limits_scope": "unknown"
      },
      "upload": {
        "accepted": [ "raw_pcm", "ogg" ],
        "preferred": {
          "container": "raw_pcm",
          "encoding": "pcm_s16le",
          "sample_rate_hz": 16000,
          "channels": 1,
          "vendor_param": "format=lpcm"
        }
      },
      "quality_signals": [],
      "dialect": {
        "language_param": "lang",
        "context_params": [],
        "wire": "yandex_stt_v1_rest"
      },
      "gateway_client": {
        "status": "not_implemented",
        "code_ref": "gateway/src/core/stt/yandex/client.rs:45-58 (request builder), :160-216 (recognize_sync); …",
        "notes": "The request and response code for the synchronous endpoint (POST …"
      },
      "file_request_mode": "sync",
      "segment_profile": {
        "request_timeout_ms": 8000,
        "rationale": "Only the HTTP request timeout deviates from the gateway defaults. 8000 ms in total, with …"
      }
    },
    {
      "input_mode": "live_stream",
      "adapter": "stream_yandex",
      "enabled": false,
      "disabled_reason": "No streaming client for Yandex exists in the gateway. The vendor streams only over gRPC …",
      "interim_results": "live",
      "endpointing_owner": "vendor",
      "vendor_turn_signal": "silence",
      "latency": { "class": "unknown", "basis": "unmeasured" },
      "limits": {
        "max_session_seconds": 300,
        "requests_per_minute": 2400,
        "max_concurrent_requests": null,
        "limits_scope": "unknown"
      },
      "quality_signals": [ "language_probability" ],
      "dialect": {
        "language_param": "language_code",
        "context_params": [],
        "wire": "yandex_stt_v3_grpc"
      },
      "gateway_client": {
        "status": "not_implemented",
        "code_ref": "gateway/src/core/stt/yandex/messages.rs:33-42 (unused placeholder types); gateway/Cargo.toml:282 (tonic 0.11 already a dependency)",
        "notes": "Vendor endpointing through the default end-of-utterance classifier (sensitivity DEFAULT …"
      }
    },
    {
      "input_mode": "live_stream",
      "adapter": "commit_yandex",
      "enabled": false,
      "disabled_reason": "No streaming client for Yandex exists in the gateway (see stream_yandex). This entry is …",
      "interim_results": "live",
      "endpointing_owner": "gateway",
      "vendor_turn_signal": "none",
      "latency": { "class": "unknown", "basis": "unmeasured" },
      "limits": {
        "max_session_seconds": 300,
        "requests_per_minute": 2400,
        "max_concurrent_requests": null,
        "limits_scope": "unknown"
      },
      "quality_signals": [ "language_probability" ],
      "dialect": {
        "language_param": "language_code",
        "context_params": [],
        "wire": "yandex_stt_v3_grpc"
      },
      "gateway_client": {
        "status": "not_implemented",
        "code_ref": "gateway/Cargo.toml:282 (tonic 0.11 already a dependency)",
        "notes": "Same RPC as stream_yandex, opened with the ExternalEouClassifier option ('Use EOU …"
      }
    }
  ],
  "billing": {
    "unit": "audio_second",
    "min_billed_seconds": 15,
    "bills_silence": false,
    "notes": "The vendor bills in 15 s units of single-channel audio, rounded up per request: a 2 s …"
  },
  "provenance": {
    "confidence": "documented",
    "verified_on": "2026-10-03",
    "verified_by": "docs",
    "owner": "waav-gateway",
    "sources": [ "https://aistudio.yandex.ru/docs/en/speechkit/stt/models", "… (9 more entries)" ],
    "unverified": [
      "transports[*].latency (the vendor publishes no latency for the synchronous endpoint or for streaming; nothing was measured)",
      "transports[0].limits.max_upload_bytes (the vendor says 1 MB; whether that is 1,000,000 or 1,048,576 bytes is not stated; the lower is recorded)",
      "transports[0].limits.min_audio_seconds",
      "transports[*].limits.max_concurrent_requests (no concurrency limit is published apart from the requests-per-second quota)",
      "transports[*].limits.limits_scope (whether quotas apply per folder or per cloud is not stated)",
      "transports[1].vendor_turn_signal (the proto describes an end-of-utterance classifier with …",
      "transports[1].quality_signals and transports[2].quality_signals (language probabilities are in the v3 response message; never exercised)",
      "the YaCloud-Billing-Units response header, which would let the adapter read billed units …",
      "vendor documentation pages were read through a summarising fetch tool, not raw HTML; only the proto files were read verbatim",
      "transports[0].segment_profile.request_timeout_ms (8000 ms is the vendor audit's provisional value; no latency is published and nothing was measured)",
      "transports[2].interim_results (that partial results keep arriving when the external …",
      "transports[1].dialect.language_param and transports[2].dialect.language_param (the …",
      "transports[*].limits.requests_per_minute (derived, not published: the vendor states …"
    ],
    "notes": "Default model. The id is sent as the 'topic' query parameter on the synchronous endpoint …"
  }
}
```

After:

```json
{
  "id": "yandex:general",
  "match": {
    "provider": "yandex",
    "model": "general",
    "model_aliases": [ "default" ]
  },
  "lifecycle": { "status": "ga" },
  "transports": [
    {
      "input_mode": "file_upload",
      "adapter": "regional_rest",
      "enabled_from_release": 5,
      "disabled_reason": "The per-utterance file-upload adapter (segmented_yandex) is not implemented. The Yandex … (text unchanged from version 1)",
      "enable_requires": [ "live_probe" ],
      "interim_results": "none",
      "endpointing_owner": "gateway",
      "vendor_turn_signal": "none",
      "latency": {
        "class": "unknown",
        "measurements": []
      },
      "limits": {
        "max_upload_bytes": 1000000,
        "max_audio_ms": 30000,
        "min_audio_ms": null,
        "rates": [
          {
            "metric": "requests",
            "per": "second",
            "value": 20,
            "plan": null,
            "scope": "unknown",
            "note": "Synchronous recognition requests. Whether the quota is per folder or per cloud is not stated."
          }
        ],
        "assumed_plan": null
      },
      "upload": {
        "accepted": [ "raw_pcm", "ogg" ],
        "preferred": { "container": "raw_pcm", "encoding": "pcm_s16le", "sample_rate_hz": 16000, "channels": 1 },
        "sample_rates_hz": [ 8000, 16000, 48000 ],
        "envelope": "raw_body",
        "vendor_params": [
          { "name": "format", "value": "lpcm", "in": "query" },
          {
            "name": "sampleRateHertz",
            "value": "{sample_rate_hz}",
            "in": "query"
          }
        ]
      },
      "quality_signals": [],
      "dialect": {
        "language_param": "lang",
        "language_shape": "single",
        "language_format": "bcp47",
        "language_required": true,
        "context_params": [],
        "wire": "yandex_stt_v1_rest"
      },
      "gateway_client": {
        "status": "not_implemented",
        "code_ref": "gateway/src/core/stt/yandex/client.rs:45-58 (request builder), :160-216 (recognize_sync); … (text unchanged from version 1)",
        "notes": "The request and response code for the synchronous endpoint (POST … (text unchanged from version 1)"
      },
      "file_request_mode": "sync",
      "segment_profile": {
        "request_timeout_ms": 8000,
        "rationale": "Only the HTTP request timeout deviates from the gateway defaults. 8000 ms in total, with … (text unchanged from version 1)"
      }
    },
    {
      "input_mode": "live_stream",
      "adapter": "planned_stream",
      "enabled_from_release": null,
      "disabled_reason": "No streaming client for Yandex exists in the gateway. The vendor streams only over gRPC … (text unchanged from version 1)",
      "interim_results": "live",
      "endpointing_owner": "vendor",
      "vendor_turn_signal": "silence",
      "latency": {
        "class": "unknown",
        "measurements": []
      },
      "limits": {
        "max_session_ms": 300000,
        "idle_timeout_ms": 5000,
        "rates": [
          {
            "metric": "requests",
            "per": "second",
            "value": 40,
            "plan": null,
            "scope": "unknown",
            "note": "Streaming requests (new streams)."
          }
        ]
      },
      "quality_signals": [ "language_probability" ],
      "dialect": {
        "language_param": "language_code",
        "language_shape": "single",
        "context_params": [],
        "wire": "yandex_stt_v3_grpc"
      },
      "gateway_client": {
        "status": "not_implemented",
        "code_ref": "gateway/src/core/stt/yandex/messages.rs:33-42 (unused placeholder types); gateway/Cargo.toml:282 (tonic 0.11 already a dependency)",
        "notes": "Vendor endpointing through the default end-of-utterance classifier (sensitivity DEFAULT … (text unchanged from version 1)"
      }
    },
    {
      "input_mode": "live_stream",
      "adapter": "planned_commit",
      "enabled_from_release": null,
      "disabled_reason": "No streaming client for Yandex exists in the gateway (see stream_yandex). This entry is … (text unchanged from version 1)",
      "interim_results": "live",
      "endpointing_owner": "gateway",
      "vendor_turn_signal": "none",
      "latency": {
        "class": "unknown",
        "measurements": []
      },
      "limits": {
        "max_session_ms": 300000,
        "idle_timeout_ms": 5000,
        "rates": [
          {
            "metric": "requests",
            "per": "second",
            "value": 40,
            "plan": null,
            "scope": "unknown",
            "note": "Streaming requests (new streams)."
          }
        ]
      },
      "quality_signals": [ "language_probability" ],
      "dialect": {
        "language_param": "language_code",
        "language_shape": "single",
        "context_params": [],
        "wire": "yandex_stt_v3_grpc"
      },
      "gateway_client": {
        "status": "not_implemented",
        "code_ref": "gateway/Cargo.toml:282 (tonic 0.11 already a dependency)",
        "notes": "Same RPC as stream_yandex, opened with the ExternalEouClassifier option ('Use EOU … (text unchanged from version 1)"
      }
    }
  ],
  "when_unusable": {
    "today": "blind_timed_uploads",
    "code_ref": "gateway/src/core/stt/yandex/client.rs:219-332",
    "refuse_from_release": null
  },
  "billing": {
    "unit": "audio_second",
    "min_billed_ms": 15000,
    "increment_ms": 15000,
    "bills_silence": false,
    "bills_failed_requests": null,
    "notes": "An empty request is billed one 15 s unit. Streaming is billed by the same rule from the moment the settings message is sent. No pricing key exists for this provider in gateway/src/config/pricing.rs."
  },
  "provenance": {
    "confidence": "documented",
    "verified_on": "2026-10-03",
    "verified_by": "docs",
    "owner": "waav-gateway",
    "sources": [ "https://aistudio.yandex.ru/docs/en/speechkit/stt/models", "… (9 more entries, unchanged)" ],
    "code_refs": [
      "gateway/src/core/stt/yandex/client.rs:45-58",
      "gateway/src/core/stt/yandex/client.rs:160-216",
      "gateway/src/core/stt/yandex/client.rs:219-332"
    ],
    "unverified": [
      "transports[*].latency (the vendor publishes no latency for the synchronous endpoint or for streaming; nothing was measured)",
      "transports[0].limits.max_upload_bytes (the vendor says 1 MB; whether that is 1,000,000 or 1,048,576 bytes is not stated; the lower is recorded)",
      "transports[0].limits.min_audio_ms",
      "transports[*].limits.rates (concurrent_requests) (no concurrency limit is published apart from the requests-per-second quota)",
      "transports[*].limits.rates[].scope (whether quotas apply per folder or per cloud is not stated)",
      "transports[1].vendor_turn_signal (the proto describes an end-of-utterance classifier with … (text unchanged from version 1)",
      "transports[1].quality_signals and transports[2].quality_signals (language probabilities are in the v3 response message; never exercised)",
      "the YaCloud-Billing-Units response header, which would let the adapter read billed units … (text unchanged from version 1)",
      "vendor documentation pages were read through a summarising fetch tool, not raw HTML; only the proto files were read verbatim",
      "transports[0].segment_profile.request_timeout_ms (8000 ms is the vendor audit's provisional value; no latency is published and nothing was measured)",
      "transports[2].interim_results (that partial results keep arriving when the external … (text unchanged from version 1)",
      "transports[1].dialect.language_param and transports[2].dialect.language_param (the … (text unchanged from version 1)",
      "transports[*].limits.rates (requests) (derived, not published: the vendor states quotas per second, 20 for synchronous requests and 40 for streaming; the recorded figures are those multiplied by 60, and a limiter must hold the per-second rate, because a burst above it is answered with HTTP 429, which today counts against the breaker shared by every Yandex session)"
    ],
    "notes": "Default model. The id is sent as the 'topic' query parameter on the synchronous endpoint … (text unchanged from version 1)"
  }
}
```

What was done:

1. `id`: `yandex:general`. `model_aliases: ["default"]`, because today's client reads `default` as
   `general` (provider notes).
2. `segmented_yandex` with `dialect.wire: "yandex_stt_v1_rest"` becomes `regional_rest`, enabled
   from Release 5 with `enable_requires: ["live_probe"]` (decisions section 12: "each is enabled
   only after a live probe with a real key").
3. `stream_yandex` (status `not_implemented`) becomes `planned_stream` and `commit_yandex` becomes
   `planned_commit`; both get `enabled_from_release: null` and keep their `disabled_reason`.
   `dialect.wire` keeps the fact that both are the gRPC version 3 interface. No `language_format`
   is written for them: it is required only on scheduled adapters, and the name of the language
   field on that interface is itself unverified.
4. Limits: `requests_per_minute: 1200` and `2400` were per-second quotas multiplied by 60. They go
   back to the vendor's window: 20 and 40 `requests` per `second`. `max_audio_seconds: 30` becomes
   `max_audio_ms: 30000`; `max_session_seconds: 300` becomes `max_session_ms: 300000`; the 5 s idle
   rule from the notes becomes `idle_timeout_ms: 5000`.
5. Upload: `envelope: "raw_body"`; `vendor_param: "format=lpcm"` becomes two `vendor_params`
   (`format` and `sampleRateHertz`) in the query; the accepted rates go to `sample_rates_hz`.
6. Dialect: `lang`, format `bcp47`, `language_required: true`: the synchronous interface takes one
   code, defaults to `ru-RU` and does not detect a language (`providers/yandex.md:183-187`).
7. Billing: the 15 s unit, which version 1 could only put in `min_billed_seconds` and explain in
   notes, becomes `min_billed_ms: 15000` and `increment_ms: 15000`; the sentence that said the
   schema could not express it is removed (`verify/group-7.md:54` confirms the unit).
8. `when_unusable`: today's Yandex client uploads on a 500 ms timer and marks every piece as the end
   of a turn (`gateway/src/core/stt/yandex/client.rs:219-332`, opened), so `today:
   "blind_timed_uploads"`. No refusal: decisions section 5 keeps that client until Release 5 and
   reports it as known broken in `ready.stt`.

## 13. After converting

Run `python3 validate_rows.py --all`. It checks every file against the schema and then:

- row ids are unique across the map, and each begins with its provider;
- every provider file has exactly one default row, and one global default file exists;
- no model, alias or pattern is used twice in a provider, and no alias of a provider is claimed
  by two providers;
- no two patterns of equal specificity (the same number of literal characters) can match the same id;
- the provider's `default_model` has an exact row;
- every adapter id is in the adapter table above, fits the transport's input mode, serves the
  row's provider, and is not enabled before the release it ships in;
- a transport that is not implemented today is enabled from Release 1 or later, or carries
  `enabled_from_release: null` with a `disabled_reason`;
- a row with no transport usable today has `when_unusable`;
- the latency class agrees with the measurements;
- a `regional_rest` transport names an interface from the table in section 3;
- `profiles.json` is valid, and every profile reference resolves and allows the row's provider.

Warnings (they do not fail the run): a `lifecycle.replacement` that does not resolve to a row of
the same provider that is not retired; a live-probe row without `probe_ref`; a provider default
row whose `min_billed_ms` is below the largest among the provider's file rows; several plans in
`rates` with no `assumed_plan`.

**What a purely mechanical pass gives.** To check that the schema can hold the real rows, the
renames, unit changes, release numbers, order rules and the parts of the `when_unusable` tables
that need no reading were applied to all 35 files and `profiles.json` in a scratch copy (nothing
under `rows/` was changed). That copy validated with 0 errors and 4 warnings:

- the default rows of `alibaba-cloud` and `tencent` carry `min_billed_ms: 0` while a file row of the
  same provider carries 1000; raise the default row's value to 1000;
- NECTEC `partii5` and the Viettel default row are marked as verified by a live probe and need a
  `probe_ref`.

So the remaining work of the conversion is the part that needs reading: sections 6, 7 (aliases and
lifecycle), 8.3, 8.4 (figures in prose), 8.5 to 8.9 and the refusal texts of section 10.

## 14. Points this conversion cannot settle

These are decisions, not conversion steps. Each has the value the rules above use until someone
decides otherwise.

1. **Azure OpenAI live-only models.** Decisions section 12 names OpenAI for the Release 4 commit
   transport and does not mention the Azure OpenAI rows `gpt-live-transcribe` and
   `gpt-realtime-whisper`. The rules leave them never enabled.
2. **Optional file fallbacks on streaming rows.** Deepgram Nova rows, Google rows and others carry a
   file transport behind their native one. The rules enable it in its adapter's release, reachable
   only by `transcription_mode: "segmented"`. If the plan wants no such fallback, set those
   transports to null.
3. **Rows whose today behaviour is `unknown`.** They keep today's client with a warning. A live
   probe should replace `unknown` with a fact before Release 3.
4. **Provider default rows of providers whose client substitutes the model** (AssemblyAI, Gladia).
   The rules warn in every release and never refuse an unknown id; decisions section 12 names only
   `universal-2` and `solaria-3` for the Release 3 refusal.
5. **Rows that version 1 refused on documentation alone.** The 48 rows with an empty transport
   list keep a refusal: from Release 0, or from Release 3 where today's client substitutes another
   model (section 10, step 2). For some of the Release 0 ones a session starts today and nobody has
   probed what the vendor then does (Yandex `deferred*`, Rev AI `human`, Sarvam `saaras:v2.5`). The capability-map design's caution, "refuse a session that produces
   transcripts today only on the evidence of a live probe", would give those rows
   `refuse_from_release: null` with `today: "unknown"` until a probe exists.
6. **The 48-character cut of the metric label** in the performance design. Row keys are allowed 64
   characters; the two Bhashini keys of 51 characters stay distinct after a cut at 48, so either
   limit works.
