# Capability map schema: what changed from version 1 to version 2, and why

Written 2026-10-04 for the integration phase of the segmented speech-to-text plan.

The capability map is the table, keyed by provider and model, that says how the gateway can reach
a speech-to-text model during a live call. Version 1 of its schema was a draft; nine builders
filled 35 provider files (410 rows) against it and reported 107 things they could not express
(`BUILDER_GAPS.md`), and the auditors of those rows added their remaining doubts. Version 2
(`stt_capability_map.schema.json`, `schema_version` 2) is the final form. This file says what
changed, why, which reported gap each change closes, and which gaps are deliberately left open.

Sources, in order of authority: `design/INTEGRATION_DECISIONS.md` (sections 1, 5, 6, 10, 12, 13 and
15), then the capability-map design (`design/W3-capability-map-and-resolution.md`), then what the
other designs read from the map (`design/cross-check-consistency.md`, finding 8). Where the
decisions and a design disagree, the decisions were followed.

How to convert the existing files is in `CONVERSION_RULES.md`. This file does not repeat the
field-by-field rules.

## 1. The changes in one page

| Area | Version 1 | Version 2 | Why |
| --- | --- | --- | --- |
| Releases | A transport was `enabled` or not, which could only describe today | Every transport has `enabled_from_release` (0 to 6, or null for never), the map has a top-level `release`, and `gateway_client.status` stays the truthful state of the code today | One map serves all seven releases, and the expected resolution table of each release is computed by setting one number (decisions sections 1 and 10) |
| No usable transport | `no_live_path_reason`, a sentence, only for an empty list | `when_unusable`: what a session gets from today's code (`today`), the release from which it is refused (`refuse_from_release`), and the refusal (`code`, `reason`, `text`) | The resolver needs it to apply the one rule of decisions section 5, and the warn-then-refuse rule of section 12 |
| Row identity | None | `id`, stable and unique | Breaker key, metric label, rollout allow-list and rollback list (decisions section 10) |
| Adapters | Free strings: `stream_<vendor>`, `commit_<vendor>`, `segmented_<vendor>` (82 ids in the files) | A closed list of sixteen: `native`, twelve that the plan builds, and three placeholders for interfaces no release builds | One vocabulary shared with the transcriber design; an unknown id fails validation |
| Billing | One block per row | A row default plus an optional block per transport; increment, conditional minimums, surcharges | Price, minimum and silence billing differ between a model's socket and its file endpoint |
| Limits | One number per field, per-minute only, one scope | `rates`: a list of `{metric, per, value, plan, scope}`; `assumed_plan`, `ramp`, `single_process`, `idle_timeout_ms` | Vendors publish limits per second, day and month, per plan, and against different things |
| Latency | One p50, one p99, one basis, one source | `measurements`: a list of `{percentile, quantity, value_ms, basis, source, measured_on, ...}` | A vendor median and a third-party 99th percentile can sit side by side; an average or a single sample no longer has to pose as a p99 |
| Upload | Container, encoding, one `key=value` | Envelope (multipart, raw body, base64 in JSON), content type, headers, several parameters, accepted rates | The request builder needs them; four vendors need two parameters or a header |
| Socket audio | Nothing | `stream_audio` (encodings, rates, channels), `commit.audio_gating` | The speech-to-text leg receives caller-rate audio; a socket may accept only 16 or 24 kHz |
| Constraints | Nothing | `constraints.languages`, `constraints.regions` on a transport | "Stream these languages, upload the others"; interfaces that exist in some regions only |
| Dialect | Closed list of language field names | The exact wire name, its shape and code format, "language required", context fields with wire names and limits, authentication, path, droppable fields | The transcriber builds its request from the row, not from the provider's name |
| Lifecycle | `ga`, `preview`, `deprecated`, `retired`; dates required | Adds `legacy`, `retiring`, `unknown`, `invalid`; dates optional; `dates_basis` | Builders had to invent dates or file legacy models as generally available |
| Provenance | Web sources only | Adds `code_refs`, `verified_by: "code_reading"`, `probe_ref`, `recheck_on` | First-party facts (the gateway's and the inference engine's own code) could not back a row |
| Provider entry | Aliases, default model, registered flag | Adds what the model string is (`model_string`), key-normalisation exceptions, how today's client treats the id, prerequisites, availability, addressing | The resolver must know these before it looks a model up; one provider stores a credential in the model field |
| Profiles | Bare transports, underscore names | `{transport, allowed_providers, fallback_profile, requires, provenance}`, hyphenated names; a row may reference one | Profiles had nowhere to keep their evidence |
| Deployment override | Not in the schema | `$defs/deployment_override` | One contract for the gateway and for Bud's control plane |
| Pass-through | Not expressible | `passthrough: true` on a row | "Unclassified: take today's path unchanged" |
| Durations | Seconds, fractional | Integer milliseconds everywhere | Exact comparison; the same names as the resolver's output and the deployment settings |

## 2. Each change, with its reason

### 2.1 Release numbers on transports, beside the truthful state of the code

`enabled` and its default of true are gone. `enabled_from_release` is required: 0 means usable with
today's code, 1 to 6 names the release that turns the transport on, null means no release does and
`disabled_reason` says why. `gateway_client.status` is unchanged in meaning: it describes the code
today. The schema ties the two together: a client that is `not_implemented` cannot be enabled from
release 0, and a `known_broken` client can be enabled only on the `native` adapter, where decisions
section 15 says a broken native client is reported in `ready.stt` and not refused. `known_broken`
then requires `gateway_client.defect`, the text that is listed as a separate defect.

`enable_requires` records the two conditions decisions section 12 attaches to Release 5: a live
probe with a real key, and a latency measurement inside the deadline.

Version 1's validator rejected every enabled transport whose client was broken, so the builders
disabled 100 streaming transports that work badly or not at all; a resolver following those rows
would have refused sessions that start today. That contradiction was the auditors' most repeated
doubt, and this change with the next one resolves it.

### 2.2 What happens when nothing is usable

`when_unusable.today` has seven values: `streams`, `streams_substituted_model`,
`buffers_until_hangup`, `blind_timed_uploads`, `refused_at_setup`, `fails`, `unknown`. They are
the behaviours the code readers found (design frame, fact F7). The resolver's rule per value is
written in the field's description, so decisions section 5 is implemented once: a buffering client
is refused for a voice-agent session with automatic turn detection and kept, with a warning, for
every other session (addendum A1 narrowed the original rule); a blind uploader is kept and
reported as known broken until its transport is enabled.

`refuse_from_release` exists for one rule of decisions section 12: a model that today's client
silently replaces is reported with `stt_model_substituted` in Releases 1 and 2 and refused from
Release 3.

`refusal.code` is `stt_live_unsupported` or `stt_model_retired`. `refusal.reason` uses the
customer contract's five wire values (`async_only`, `client_not_implemented`, `disabled`,
`provider_not_built`, `model_not_served`), not the capability-map design's older list;
`CONVERSION_RULES.md` section 10 gives the mapping. This settles the "reason strings" item of
finding 12 of the consistency cross-check in favour of the design that owns the wire.

A row is required to carry `when_unusable` when its transport list is empty, when its lifecycle is
`retired`, and (checked by the validator) whenever no transport is usable with today's code.

### 2.3 A stable row identifier

`id` is `<provider>:<key>`, at most 64 characters after the colon, assigned once. The cost design
uses "provider:row key" in its allow-list and its metric label, and the transcriber design needs
the row in its breaker key; both now read this field.

### 2.4 A closed adapter list

Sixteen ids, in `$defs/adapter_id` and in the adapter table of `CONVERSION_RULES.md`; the validator
fails when the two differ. `native` replaces every `stream_<vendor>` id of a client that exists.
The file-upload ids are the transcriber design's. `planned_stream`, `planned_commit` and
`planned_file` replace about forty ids that named clients nobody plans to write; such a transport
must be `enabled_from_release: null`.

`dialect.wire` is **kept**, against the capability-map design's proposal to remove it. With one
`regional_rest` adapter serving thirteen vendors, and two interfaces at Baidu and Alibaba, the
adapter chooses its code path by this value; and for a planned transport it is the only record of
which vendor interface is meant. It is required in both cases.

### 2.5 Billing per transport, increments, conditional minimums, surcharges

The row's `billing` is the default; a transport may carry its own complete block (eight of the
nine builders reported that one block per row could not describe a model's socket and its file
endpoint). `min_billed_ms`
is joined by `increment_ms` (Yandex and NAVER bill in 15-second units, so a 16-second upload costs
30), `conditional_minimums` (ElevenLabs bills at least 20 s per request above 100 key terms),
`surcharges` (the customer contract's settings report needs the percentage) and
`bills_failed_requests`. The unit gains `none` for an operator-owned server.

### 2.6 Rate limits as a list

`requests_per_minute`, `max_concurrent_requests`, `audio_seconds_per_hour` and `limits_scope` become
entries of `limits.rates`. The vendor's own window is kept, because a limiter that allows 1,200
requests in one burst breaks a limit of 20 a second. `plan` lets one transport carry the free and
the paid figures; `assumed_plan` says which the gateway uses when the deployment declares none, and
the deployment override may name another. The scope list gains `account`, `application`, `resource`
and `region`. `ramp`, `single_process` and `idle_timeout_ms` are requests of the cost and
performance designs.

### 2.7 Latency as a list of measurements

Each entry says which statistic, what was timed, the value, who measured, where it can be read and
when. The seed the engine's time limits start from is the first p99 entry that timed end of speech
to final transcript. The class label must agree with that entry; without one it is `unknown`, or
`slow` when some other figure is already above the fast threshold. `pause_ms_assumed` is the
performance design's request.

### 2.8 Upload envelope, headers and parameters; socket audio

See the table above. Formats the gateway never uploads are deliberately not added to
`upload.accepted` (section 6).

### 2.9 Constraints on a transport

A transport whose language or region constraint the session does not meet is skipped like a
disabled one. Four of the nine builders reported that availability depends on the language or
the region and could only be written in notes.

### 2.10 Dialect

`language_param` is the vendor's own spelling or null. `language_shape` and `language_format` tell
a shared transcriber how to write the value; the validator requires the format on every transport
of a scheduled adapter that renders the language itself. `language_required` is the transcriber
design's request for vendors that assume English when no language is sent. `context_params` entries
carry the wire name and the caps the request builder must apply. `auth`, `path`,
`droppable_params` and `usage_reported` are the capability-map design's additions for the
OpenAI-compatible family; `limit_headers` is the cost design's.

### 2.11 Lifecycle, provenance, provider entry, profiles, deployment override, pass-through

Described in the table of section 1. Three points need a sentence each.

- `model_string: "sensitive"` is the flag decisions section 10 asks for: the string is never
  looked up, logged, counted, labelled or published. `absent` and `gateway_alias` also end the
  "assumed capability" warning that every session of a vendor without a model parameter would
  have raised.
- `native_model_handling` (`verbatim`, `substituted`, `ignored`) is what lets the resolver warn
  `stt_model_substituted` when an unknown id resolves to a provider's default row and today's
  client then runs a different model.
- `$defs/deployment_override` holds `profile`, `underlying_model`, `realtime_url`, `latency`,
  `limits` (with `plan`) and `deadline_ms` (3,000 to 10,000 ms, decisions sections 2 and 10). It
  has no `merge_below_ms`: nothing reads it (finding 8).

## 3. The 107 builder gaps

"Closed" means the fact now has a field. "Partly" means the part named is closed and the rest is
deferred for the reason given. Totals: 88 closed, 12 partly, 7 deferred.

### Builder for alibaba-cloud and three others

| Gap | Outcome | Where |
| --- | --- | --- |
| Gateway rewrites model strings (asynchronous ids and shorthand become streaming models) | Closed | `match.model_aliases` for shorthand; `when_unusable.today: streams_substituted_model` with `substituted_model`; provider `native_model_handling` |
| No region dimension | Closed | `constraints.regions`; provider `regions`; provider `prerequisites` (workspace id) |
| No value for a list-valued language field (`language_hints`) | Closed | `dialect.language_param` free text, `language_shape` |
| Latency cannot hold an average or a few probe samples | Closed | `latency.measurements` with `mean`, `single_sample`; class rule |
| No signal for a yes-or-no "no speech" status | Closed | `quality_signals: no_speech_flag` |
| MP3 as preferred container; request envelope; two vendor parameters | Closed | `upload.preferred` allows `mp3`; `upload.envelope`; `upload.vendor_params` |
| Vendor terms that bar production use | Closed | provider `availability.status: evaluation_only` |
| Retirement without a date; dead endpoint without a notice; vendor-named replacement versus live-capable one | Partly | `retiring`, `retired` with optional dates. `replacement` is defined as the live-capable ids; the vendor's own named replacement stays in notes |
| Billing cannot differ between transports of one model | Closed | transport `billing` |
| Vendor without a model parameter; `default_model` description conflict | Closed | provider `model_string` (`absent`, `gateway_alias`); new description |
| No `account` scope | Closed | `rates[].scope` |
| Interim results: what the vendor offers versus what the adapter delivers | Deferred | The field is defined as what the adapter delivers; vendor capability stays in notes. Nothing reads the other value |

### Builder for deepgram and three others

| Gap | Outcome | Where |
| --- | --- | --- |
| Billing differs per transport | Closed | transport `billing` |
| No value for `language_codes`, `language_hint` | Closed | `dialect.language_param` |
| Limits vary by plan | Closed | `rates[].plan`, `assumed_plan`, override `limits.plan` |
| No `account` scope | Closed | `rates[].scope` |
| Host or region restriction per transport | Closed | `constraints.regions` |
| No model alias; no language-dependent default | Partly | `match.model_aliases`. A default model that depends on the session language is deferred (section 6) |
| No `unknown` or `legacy` lifecycle | Closed | `lifecycle.status` |
| Per-model language constraint ("required", "one of five") | Closed | `dialect.language_required`; `constraints.languages` |
| One streaming and one file adapter id per provider is too few | Closed | closed adapter list plus `dialect.wire` |
| No status for "connects but serves a different model" | Closed | not a status: `when_unusable.today: streams_substituted_model`; provider `native_model_handling` |
| Idle-socket timeout; poll timing of an asynchronous job | Partly | `limits.idle_timeout_ms`. Poll timing is deferred: no release builds a polled transport |
| One basis and one source per latency | Closed | `latency.measurements` |

### Builder for the extras (self-hosted, Azure OpenAI, WaaV Infer, global default)

| Gap | Outcome | Where |
| --- | --- | --- |
| "Unclassified, take today's path unchanged" | Closed | row `passthrough`; lifecycle `unknown` |
| First-party evidence cannot back a row | Closed | `provenance.code_refs`, `verified_by: code_reading` |
| Deployment override not in the schema | Closed | `$defs/deployment_override` |
| Audio format a socket requires | Closed | `stream_audio` |
| Accepted request fields belong to the server, not the model id | Closed, another way | Named profiles, `droppable_params` and the setup probe (decisions section 15: send it, the probe decides). The override does not carry a dialect |
| Language code format | Closed | `dialect.language_format` |
| Billing unit for an operator-owned server | Closed | `billing.unit: none` |
| "Generally available with a published retirement date" | Closed | `lifecycle.status: retiring` |
| Profiles have no provenance | Closed | profile `provenance` |
| A row whose transports are all disabled | Closed | `when_unusable`, required by the validator when nothing is usable today |
| Adapter-exists check versus adapters not yet written | Closed | the adapter table carries each adapter's release; `planned_` ids |

### Builder for gnani and three others

| Gap | Outcome | Where |
| --- | --- | --- |
| Stripping a `<provider>/` prefix breaks Bhashini ids | Closed | provider `key_normalisation.strip_provider_prefix`, off unless set |
| Vendor without a model parameter is forced to `assumed` | Closed | provider `model_string: absent` makes the default row the declared answer |
| Reverie's model field holds a credential | Closed | provider `model_string: sensitive` |
| Real wire names of the language field | Closed | `dialect.language_param` |
| Vendor names of vocabulary fields | Closed | `context_params[].wire_name` |
| Vendor claim without a percentile | Closed | measurement `percentile: typical` |
| Billing differs per transport | Closed | transport `billing` |
| "Legacy, undocumented" and on-premises end of support | Closed | `legacy`; `retired` with `dates_basis` |
| Second socket client at one vendor | Closed | `planned_stream` plus `dialect.wire` |
| No way to say what today's buffering client does | Closed | `when_unusable.today: buffers_until_hangup` |
| On-premises limits are deployment defaults; model or language fixed per endpoint | Partly | scope `deployment` and the override's `limits`, `underlying_model`. A language bound to an endpoint has no field (section 6) |
| Header, JSON field or prior configuration call for the upload | Closed | `upload.headers`, `upload.vendor_params[].in`, `envelope: json_base64`, provider `prerequisites: configuration_call` |

### Builder for microsoft-azure and three others

| Gap | Outcome | Where |
| --- | --- | --- |
| No language dimension | Closed | `constraints.languages` |
| No region dimension | Closed | `constraints.regions` |
| Billing differs per transport | Closed | transport `billing` |
| `default_model` null versus gateway sentinels | Closed | provider `model_string: gateway_alias`; new description |
| No value for `locales`, `language_codes`; fixed language with separate hints | Closed | `dialect.language_param`; `upload.vendor_params` for the fixed value |
| No per-second rate; no resource or per-region scope | Closed | `rates[].per`, `rates[].scope` |
| Typical latency without a percentile | Closed | measurement `percentile: typical` |
| More accepted formats; "send at the call's own rate" | Partly | `upload.prefer_call_rate`, `upload.sample_rates_hz`. More container names are deferred (section 6) |
| Vendor data retention and clean-up | Partly | transport `privacy.retention`. The duty to delete a Speechmatics job is adapter behaviour and stays in notes |
| Refusal reason for an all-disabled row; re-check date | Closed | `when_unusable.refusal`; `provenance.recheck_on` |
| Gateway-side aliases and substring matching | Closed | `match.model_aliases`; pattern rows; `key_normalisation.placeholder_as_unset` |
| A value returned but not meaningful | Deferred | The signal is left out of `quality_signals`; its description now says so |

### Builder for openai and two others

| Gap | Outcome | Where |
| --- | --- | --- |
| No tokens-per-minute limit | Closed | `rates` with `metric: tokens` |
| No per-day or per-minute audio limit | Closed | `rates` with `metric: audio_seconds` |
| Limits vary by plan or tier | Closed | `rates[].plan`, `assumed_plan` |
| Billing differs per transport | Closed | transport `billing` |
| Audio format a socket accepts | Closed | `stream_audio` |
| Request settings not keyed by signal | Deferred | One case (ElevenLabs audio-event tags), which the live request switches off |
| Conditional minimums and surcharges | Closed | `billing.conditional_minimums`, `billing.surcharges` |
| What a session gets when every transport is disabled | Closed | `when_unusable` |
| Context limits (224-token prompt, key-term caps) | Closed | `context_params[].max_tokens`, `.max_items` |
| Limits at organisation and project level | Closed | one `rates` entry per scope |
| No date on a verified-live claim | Closed | `gateway_client.last_verified_on` |
| Inferred deprecation; served but not recommended | Closed | `lifecycle.dates_basis`; `legacy` |

### Builder for revai and three others

| Gap | Outcome | Where |
| --- | --- | --- |
| Model id known to be invalid | Closed | `lifecycle.status: invalid`; refusal reason `model_not_served` |
| Deprecation without a date | Closed | dates optional; `retiring` |
| Minimum per stream connection; silence billing per transport | Partly | transport `billing`. A minimum per connection has no field: it concerns streaming, which segment metering does not bill |
| Default model depends on the session language | Deferred | Section 6 |
| Plan tiers; account and service-instance scope; limit shared by socket and HTTP | Closed | `rates[].plan`, `.scope`, `.note` |
| Minimum or allowed input sample rate; encodings a socket accepts | Closed | `stream_audio` |
| No-speech indicators; failure reported in a 200 response | Partly | `no_speech_flag`. Failure inside a 200 body is adapter knowledge for a vendor with no scheduled adapter |
| A transport cannot name the vendor endpoint | Closed | `dialect.wire`, kept and required where it matters |
| No `unknown` for interim results | Closed | `interim_results: unknown` |
| Nothing between "wire tested" and "known broken" | Closed | `gateway_client.defect` on any status |
| No row-level refusal text | Closed | `when_unusable.refusal.text` |
| Stored vocabulary ids; more upload formats; minimum upload bytes | Partly | `context_params[].kind: vocabulary_id`. Formats and a byte minimum are deferred |

### Builder for tencent and three others

| Gap | Outcome | Where |
| --- | --- | --- |
| No site or region dimension | Closed | `constraints.regions` |
| Billing differs per transport | Closed | transport `billing` |
| One adapter cannot name two file interfaces; required token scope | Closed | `dialect.wire` with the interface table the validator checks; provider `prerequisites: token_scope` |
| "Legacy, date unknown" | Closed | `legacy` |
| Socket pacing, idle rules, one connection per utterance | Partly | `limits.idle_timeout_ms`. Pacing and per-utterance connections concern commit sockets no release builds |
| No `account` or `application` scope | Closed | `rates[].scope` |
| Narrow lists: formats, registered vocabulary, wire names, duration billing with unknown granularity | Partly | `vocabulary_id`, `wire_name`, `increment_ms: null`. Formats are deferred |
| Region suffix on Huawei ids; case-sensitive Tencent ids | Closed | provider `key_normalisation.region_suffix_separator`; `match.model` keeps the vendor's spelling, which is what is sent |
| `default_model` for a vendor keyed by a gateway product name | Closed | provider `model_string: gateway_alias` |
| "Output is wrong" versus "cannot connect"; no policy when nothing is enabled | Closed | a broken native client is enabled and described in `gateway_client.defect`; `when_unusable` |
| "Merge segments shorter than N seconds" | Deferred | The cost design's merge rules use no threshold; they read `upload_policy`, `min_billed_ms` and `increment_ms` |
| File-only status that depends on unverified socket behaviour | Partly | `enable_requires: live_probe` and `provenance.unverified`; no dedicated field |

### Builder for yandex and three others

| Gap | Outcome | Where |
| --- | --- | --- |
| Today's client is neither a stream nor the planned adapter | Closed | `when_unusable.today: blind_timed_uploads`, `buffers_until_hangup` |
| Billing increment | Closed | `billing.increment_ms` |
| Billing differs per transport | Closed | transport `billing` |
| Per-second, per-day and per-month quotas | Closed | `rates[].per` |
| Provider prerequisites and availability | Closed | provider `prerequisites`, `availability` |
| Content type that differs from the container; two parameters | Closed | `upload.content_type`, `upload.vendor_params` |
| A rolling "previous version" tag | Closed | `legacy` |
| Limits vary by account type | Closed | `rates[].plan` |
| A confidence that is not a probability | Deferred | Left out of `quality_signals`, as for the Azure and Google cases |
| The model key is not a wire string | Closed | provider `model_string: gateway_alias` |
| No `unknown` turn signal | Closed | `vendor_turn_signal: unknown` |
| More accepted upload formats | Deferred | Section 6 |

## 4. The auditors' remaining doubts

The auditors' notes are mostly about facts, not about the schema. Grouped by what settles them:

| Doubt | What version 2 does |
| --- | --- |
| "No transport is enabled, so the map refuses sessions that start today" (OpenAI, Groq, Yandex, Tencent, Huawei, Baidu, iFlytek, AmiVoice, Gnani, Reverie, Bhashini, FPT.AI and others) | Settled. A native client, broken or not, is enabled from release 0; a client that is not a transport is described by `when_unusable.today`; the resolver's rule per value follows decisions sections 5 and 15 |
| "Today's client silently serves another model" (AssemblyAI, Gladia, Azure Speech, Amazon, Rev AI, Alibaba) | Settled. `streams_substituted_model`, `refuse_from_release: 3` where decisions section 12 says so, provider `native_model_handling` |
| "Enabling the file adapter on the default row would move streaming sessions to uploads" (Deepgram, Azure Speech, Speechmatics, Sarvam) | Settled by the order rule of `CONVERSION_RULES.md` section 9: a working native transport stays first |
| "The resolver must rewrite client aliases before lookup" (OpenAI, Groq, Alibaba, NECTEC, Speechmatics, Yandex) | Settled. `match.model_aliases` |
| "Limits are per second, per plan, per account" | Settled. `limits.rates` |
| "Regions and languages are not modelled" | Settled. `constraints` |
| "One convention is needed" for vendors without a model parameter, for two file interfaces under one adapter, for interim results after an upload | Settled. `model_string`; `dialect.wire`; `interim_results` is what the adapter delivers |
| "The date is a stand-in", "legacy with no date" | Settled. Optional dates, `dates_basis`, `legacy`, `retiring` |
| "A re-check date is needed" (Alibaba retirements on 2026-10-10, Gladia, Speechmatics) | Settled. `provenance.recheck_on` |
| Facts nobody has tested (whether a vendor rejects a field, whether a socket survives several utterances, what a file endpoint's latency is, which concurrency figure is enforced) | Not a schema matter. They stay in `provenance.unverified`; `enable_requires` keeps the affected transports off until a probe exists |
| Product decisions (whether the SberDevices adapter is worth building; whether unknown ids on AssemblyAI and Gladia should be refused; names of the Amazon and NAVER product aliases) | Not a schema matter. Listed in `CONVERSION_RULES.md` section 14 where they affect a rule |

## 5. What the other designs read, and where it is now

From finding 8 of the consistency cross-check and decisions section 10.

| Needed by | What | Where in version 2 |
| --- | --- | --- |
| Transcriber, cost and performance designs | An identifier of the matched row | row `id` |
| Cost design | A cold-key ramp flag; per-day windows; the vendor's limit header names | `limits.ramp`; `rates[].per`; `dialect.limit_headers` |
| Cost design | Billing rounding, conditional minimums, whether failed requests are billed | `billing.increment_ms`, `.conditional_minimums`, `.bills_failed_requests` |
| Cost design | A privacy block; a map from region to base address | transport `privacy`; provider `regions` |
| Customer contract | Per-feature surcharges | `billing.surcharges` |
| Customer contract | Refusal code and reason | `when_unusable.refusal` |
| Performance design | A deployment deadline | `$defs/deployment_override.deadline_ms` |
| Performance design | Warm-up request; commit audio gating; single-process mark; the pause a seed was measured with; accepted upload rates | transport `warm`; `commit.audio_gating`; `limits.single_process`; `measurements[].pause_ms_assumed`; `upload.sample_rates_hz` |
| Transcriber design | "Language required"; authentication, path, droppable fields, usage | `dialect.language_required`, `.auth`, `.path`, `.droppable_params`, `.usage_reported` |
| Decisions section 10 | Release per transport; language and region constraints; socket audio; the "not a model, never log" flag; key-normalisation exceptions | `enabled_from_release`; `constraints`; `stream_audio`; provider `model_string: sensitive`; provider `key_normalisation` |

Three things are answered outside the row data:

- **The overload refusal kind and code** (decisions section 10; cost design). Overload is measured
  at run time by the limiter and is the same for every row, so it is a refusal kind of the
  resolver with a code the customer contract assigns. The schema carries its inputs (`limits.rates`,
  `assumed_plan`, `ramp`). If the orchestrator wants the code listed in the schema as well, it is
  one more value in `$defs/refusal.code`.
- **The rollout control record and the allow-list.** Run-time state, not map data. The row `id` is
  the key both use.
- **The refusals of decisions section 13** (`stt_segmentation_unavailable` for a missing detector or
  an audio format the engine cannot decode) and `stt_not_streaming` for a session that demanded
  streaming. They depend on the build and the session, not on a row, so the row's `refusal.code`
  list holds only `stt_live_unsupported` and `stt_model_retired`.

## 6. Deliberately deferred

| Not in version 2 | Reason |
| --- | --- |
| A default model that depends on the session language (AssemblyAI, IBM) | On both providers every release keeps the native client, which chooses the model itself; the map's lookup changes only which row is reported. A single `default_model` stays |
| The vendor's own interim capability beside what the adapter delivers | Nothing reads it; Release 6 interim text is for self-hosted models and is decided per deployment |
| More container names in `upload.accepted` (AMR, WMA, Speex, SILK, AC3, G.711, G.729, MOV) | The gateway uploads WAV or raw PCM; a list of formats it never sends would be read by nothing |
| A marker for a signal that is returned but meaningless (Azure LLM Speech confidence, Google Chirp confidence, T-Bank's relative factor) | Leaving the signal out is the safe behaviour and needs no field |
| Request settings keyed per quality signal | One case, and the live request switches that signal off |
| Poll timing for asynchronous jobs; socket pacing; one connection per utterance; a minimum per stream connection; a minimum upload size in bytes; failure reported inside a 200 response | Each concerns an interface that no release builds a client for (`planned_file`, `planned_commit`) or a streaming meter the engine does not use. They stay in `gateway_client.notes` until that interface is scheduled |
| A merge threshold in the segment profile | The cost design's merge rules use none |
| A language bound to an on-premises endpoint | Phonexia arrives in Release 5 through the generic adapter; the override's `underlying_model` covers the bound model, and the bound language is a deployment setting to design with that work |
| The vendor-named replacement beside the live-capable one | `replacement` is what the refusal shows the customer; the vendor's wording stays in notes |
| The clean-up duty for vendor-stored audio | Adapter behaviour (the transcriber design deletes the Speechmatics job) |
| A separate provenance file per provider, and profiles for every repeated transport | Both are layout steps of the gateway import (capability-map design, section 2.2), made mechanical by the row `id`. They change no field |
| `streaming_sibling` on a row | The resolver derives the streaming alternatives from the provider's rows |
| `limits_source` | A resolver output (map seed, deployment declared, vendor header), not a map fact |

## 7. Differences from the capability-map design's own change list

The design's section 3.2 listed seventeen changes. Version 2 adopts most. The differences:

| Design proposal | Version 2 | Reason |
| --- | --- | --- |
| Remove `dialect.wire` | Kept, required for `regional_rest` and planned adapters | Section 2.4 |
| Store model ids in lowercase | The vendor's spelling is kept; comparison ignores case | Tencent ids are case-sensitive on the wire, and the row's spelling is what is sent |
| A row-level `vendor` block (live audio, synchronous file, asynchronous file) | Not added | The same facts are the row's transports, including the never-enabled planned ones |
| `unset_model` as `{resolves_to}` or an inline row | `default_model` kept, with `model_string` | Every provider already has an exact row or a declared default row for the unnamed case; the validator checks it |
| `no_live_path_reason` as `{code, text}` with the design's code list | `when_unusable.refusal` with the customer contract's reason list | Section 2.2 |
| `retired` requires `shutdown_on` | Dates optional | The builders met retirements and dead endpoints without a date |
| Provider `native_model_handling` with a fourth value `not_a_model` | Three values; "not a model" is `model_string` | One field per question |
| Split provenance into a sibling file; forbid a repeated inline transport | Not in the schema | Section 6 |
| `limits_source` | Not added | Section 6 |

Adopted as proposed: the `native` adapter id; a transport written as a profile reference; model
aliases; integer milliseconds; `applies_to_all_models`; the provider fields for addressing
(`egress`, `deployment_base`, `production_base`, `base_url_convention`, `deployment_model_source`,
`setup_probe`); the two key-normalisation switches; the dialect additions; `fallback_profile`,
`allowed_providers` and `requires` on profiles; `sdk_placeholder_models` and `map_revision` at the
top level; `increment_ms`.

## 8. How version 2 was checked

- The schema passes `jsonschema.Draft202012Validator.check_schema`. Every named property has a
  description.
- The three worked examples of `CONVERSION_RULES.md` (OpenAI `gpt-transcribe`, Deepgram
  `whisper-large`, Yandex `general`) were built in full and validate.
- The mechanical rules of `CONVERSION_RULES.md` were applied to all 35 files and `profiles.json`
  in a scratch copy, without touching `rows/`. The result validated with 0 errors and 4 warnings,
  so the schema can hold every existing row.
- Twenty altered copies of that result were run through the validator, one per rule (duplicate row
  id, missing default row, two patterns of equal specificity, an adapter missing from the table, a
  transport not implemented and enabled from release 0, a default model without a row, and so on).
  Each was rejected with a message naming the rule.
- `example_rows.yaml` in this directory is a version 1 illustration and was not updated; the worked
  examples of `CONVERSION_RULES.md` section 12 replace it.
- `python3 validate_rows.py --all` on the unconverted files exits 1 and reports each row as still
  version 1, naming the final adapter id for each old one: 35 files, 410 rows, 7,503 errors. That
  is expected until the files are converted.

## Later correction (2026-10-04)

A retired model's `refuse_from_release` was fixed at 0. Two retired models (Baidu `19362`, Azure
`MAI-Transcribe-1`) start a session today because today's client silently serves a different model,
so refusing them from Release 0 would break sessions that start today. The schema now allows 0 to 3
for a retired row whose `today` is `streams_substituted_model`, and still requires 0 otherwise. The
reference resolver honours it, and three self-test cases pin it. Three Yandex asynchronous-only rows
(`deferred-general` and its two tags) moved from 0 to 3 for the same reason.
