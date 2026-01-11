# WaaV Gateway Codebase Review Scratchpad

Scope: /home/bud/Desktop/bud_waav/WaaV/gateway

Notes:
- Goal is file-by-file line review (excluding build artifacts such as target/ unless explicitly requested).
- Each file entry should capture: what it does, where used, architectural role, use-cases, integration points, and any issues/stubs/hardcoded parts.

## docs/integrations.md
- Purpose: provider catalog + configuration guide for all STT/TTS/realtime providers, env vars, YAML config, use-case selection, pricing references.
- Architecture integration: documents provider selection; points to `src/config/pricing.rs` for cost estimation utilities.
- Notes/issues: coming-soon providers are documentation-only; ensure status matches actual code. No obvious bugs; mostly reference data.

## docs/plugin_architecture_analysis.md
- Purpose: analysis + recommended plugin architecture with registry, processors, middleware, and migration plan.
- Architecture integration: describes desired registry-based provider system; indicates prior hardcoded factories and enum coupling.
- Notes/issues: appears to be a proposal/roadmap; may be out-of-sync with current code if plugin system already implemented. Needs verification against `src/plugin/*` and current factories.

## docs/plugins.md
- Purpose: authoritative guide to current plugin system (capability-based, inventory + PHF + DashMap, lifecycle + isolation).
- Architecture integration: maps directly to `src/plugin/*` modules, capability traits, and global registry; documents built-in providers, aliases, and use of inventory registration.
- Notes/issues: claims compile-time registration and O(1) lookup; verify actual code matches. Mentions plugin-enabled ServerConfig with `plugins` field; confirm in `src/config/*`.

## docs/livekit_integration.md
- Purpose: detailed LiveKit + SIP integration guide, webhook verification, SIP auto-provisioning, and downstream webhook forwarding/signing.
- Architecture integration: corresponds to `src/handlers/livekit/*`, `src/utils/sip_hooks.rs`, `src/utils/sip_api_client.rs`, `src/livekit/*`, config in `src/config/sip.rs`.
- Notes/issues: doc includes explicit security rules (HTTPS hooks, min secret length, webhook verification, timestamp window). Must verify enforcement in code (URL validation + secret length).

## docs/authentication.md
- Purpose: describes dual auth modes (API secrets + JWT external validation), config, request flow, and error mapping.
- Architecture integration: corresponds to `src/middleware/auth.rs`, `src/auth/*`, `src/config/*`, `src/errors/auth_error.rs`, and routes for protected endpoints.
- Notes/issues: explicitly states WebSocket auth is not implemented; verify if still true and if risk documented/mitigated. Ensure auth-required config validation matches doc.

## docs/websocket.md
- Purpose: comprehensive WebSocket protocol doc; message types, config flow, LiveKit integration, timeouts, and best practices.
- Architecture integration: maps to `src/handlers/ws/*`, `src/handlers/realtime/*`, `src/livekit/*`, `src/core/voice_manager/*`.
- Notes/issues: doc states WS is unauthenticated and idle timeout 10s; verify these behaviors in code (keep-alive handling, timeouts, allow/deny logic). Mentions connection kept open on config errors.

## Cross.toml
- Purpose: cross-rs config for Linux glibc targets; explains musl incompatibility due to LiveKit/libwebrtc.
- Architecture integration: build tooling only; no runtime impact.
- Notes/issues: none; explicitly says features like turn-detect/noise-filter handled via Docker builds.

## examples/sign_webhook.rs
- Purpose: CLI example to generate LiveKit webhook JWT signature with body hash; prints curl command.
- Architecture integration: corresponds to LiveKit webhook verification in `src/handlers/livekit/webhook.rs` and docs.
- Notes/issues: uses default test API key/secret if env vars missing (safe for example). No functional issues.

## .cargo/config.toml
- Purpose: per-target rustflags (static CRT on Windows, ObjC link arg on Apple) and crates.io sparse protocol.
- Architecture integration: build tooling only.
- Notes/issues: none.

## CLAUDE.md
- Purpose: internal dev guidance; lists commands, feature flags, architecture components, patterns, and endpoints.
- Architecture integration: points to core modules, config precedence, provider addition steps.
- Notes/issues: references factory-based provider registration; may be outdated if plugin registry is current. Mentions endpoints `/recording/{stream_id}` and `/sip/hooks`—verify routes exist.

## .env
- Purpose: local runtime env vars for dev (API keys, port, CORS).
- Architecture integration: loaded by config/env loader for provider credentials and server settings.
- Notes/issues: contains what appears to be concrete API keys; if real, this is a security risk in repo. Ensure `.env` is gitignored and keys are rotated.

## Cargo.toml
- Purpose: crate metadata, features, dependencies, profiles, benches.
- Architecture integration: defines feature-gated subsystems (`turn-detect`, `noise-filter`, `dag-routing`, `openapi`, `plugins-dynamic`), LiveKit/Google/AWS SDK versions, plugin system deps (`inventory`, `phf`, `dashmap`).
- Notes/issues: Git dependency on DeepFilterNet; requires network to build and may impact reproducibility. LiveKit/Google/prost versions pinned for compatibility. Panic=abort in release (could limit backtraces). Verify `plugins-dynamic` feature usage in code.

## memory.md
- Purpose: internal development journal with implementation notes, historical changes, and known issues per provider.
- Architecture integration: references many gateway modules; useful for identifying known bugs and expected behavior.
- Notes/issues: contains a list of previously identified issues (notably Hume voice cloning ignores audio samples, Hume EVI hardcoded sample rate, ProsodyScores mismatch, emotion type inconsistencies). Need to verify if these are still present in current code and if docs reflect them.

## CHANGELOG.md
- Purpose: release notes via git-cliff.
- Architecture integration: none (documentation).
- Notes/issues: changelog versions (0.1.x) appear behind Cargo.toml version (1.0.0). Potential versioning mismatch.

## README.md
- Purpose: main project overview, features, architecture diagrams, configuration, and usage.
- Architecture integration: describes plugin system, VoiceManager, providers, WebSocket/REST flows, LiveKit integration, security features.
- Notes/issues: claims plugin architecture with O(1) lookup and panic isolation; verify matches `src/plugin/*`. Mentions audio-disabled mode and auth; confirm behavior in handlers.

## config.example.yaml
- Purpose: exhaustive configuration sample for server, providers, cache, auth, SIP, recording.
- Architecture integration: feeds `src/config/*` loading/validation; documents runtime SIP hooks persistence.
- Notes/issues: priority note says YAML > env > .env > defaults. Need to verify actual merge order in code. SIP hook runtime merge behavior should be checked.

## config.yaml
- Purpose: default local config (subset of example).
- Architecture integration: same config loader.
- Notes/issues: contains concrete API key in repo; security risk if committed. Comment says priority: env > YAML > defaults (contradicts config.example). Verify actual precedence.

## perf_test_config.yaml
- Purpose: minimal config for performance testing (auth disabled, in-memory cache).
- Architecture integration: uses standard config loader.
- Notes/issues: none.

## perf_test_no_ratelimit.yaml
- Purpose: perf baseline config with extreme rate limits to remove throttling.
- Architecture integration: standard config; expects `security` section to map to rate limiting config (verify in `src/config/*`).
- Notes/issues: ensure `security` section is actually parsed; if not, file is ineffective.

## .cursor/rules/core.mdc
- Purpose: dev guidelines for core STT/TTS abstractions and config examples.
- Architecture integration: documents `BaseSTT`/`BaseTTS` traits and config shapes.
- Notes/issues: provider list is outdated (only early providers). Trait signatures listed should be cross-checked with actual code.

## .cursor/rules/axum.mdc
- Purpose: Axum 0.8+ best practices and patterns.
- Architecture integration: guidance only.
- Notes/issues: none; general doc.

## .cursor/rules/livekit.mdc
- Purpose: LiveKit integration guidance and desired WS message behavior.
- Architecture integration: references `src/livekit/*` and WS config handling.
- Notes/issues: may be outdated (mentions token in WS config; current system may use room creation + REST token endpoint). Verify actual integration.

## .cursor/rules/rust.mdc
- Purpose: Rust best practices document.
- Architecture integration: guidance only.
- Notes/issues: claims Rust Edition 2024, but Cargo.toml uses edition 2021. Doc is out-of-sync.

## .cursor/rules/openapi.mdc
- Purpose: guidelines for OpenAPI generation using utoipa.
- Architecture integration: maps to `src/docs/openapi.rs` and feature-gated docs.
- Notes/issues: none, but ensure version numbers (0.1.0) match current release/versioning.

## .dockerignore
- Purpose: exclude build artifacts, docs/tests, and local caches from Docker context.
- Architecture integration: build tooling only.
- Notes/issues: excludes docs/tests/examples, which means image build can’t run tests or include docs.

## Dockerfile
- Purpose: multi-stage Docker build using cargo-chef, ONNX runtime download, and runtime deps for LiveKit/webrtc.
- Architecture integration: builds binary with `turn-detect` + `noise-filter` features; includes init stage to download models.
- Notes/issues: depends on network to download ONNX runtime and Git deps; requires LiveKit/webrtc system libs. `RUN_GATEWAY_INIT` default true in image build.

## .gitignore
- Purpose: ignore build artifacts, env files, config, caches, and local assets.
- Architecture integration: repo hygiene.
- Notes/issues: `.env` and `config.yaml` are ignored, but both exist in repo; check if committed.

## LICENSE
- Purpose: Apache-2.0 license.
- Architecture integration: legal.
- Notes/issues: none.

## .vscode/launch.json
- Purpose: VS Code LLDB launch configs for binary and tests.
- Architecture integration: developer tooling only.
- Notes/issues: none.

## benches/gateway_benchmarks.rs
- Purpose: Criterion benchmarks for WS message parsing/validation, cache operations, phone validation, audio frame size checks.
- Architecture integration: exercises `handlers/ws/messages`, `core/cache`, `utils/phone_validation`.
- Notes/issues: uses `MAX_SPEAK_TEXT_SIZE` and `MAX_AUDIO_FRAME_SIZE`; ensure these constants align with docs. Benchmarks do not gate prod.

## .env.example
- Purpose: sample env vars for local dev.
- Architecture integration: config/env loading.
- Notes/issues: none (placeholder values).

## docs/provider_integration_status.md
- Purpose: provider rollout playbook + status tracking for 70 providers.
- Architecture integration: documentation/process only.
- Notes/issues: marks LMNT/Play.ht as COMPLETE; verify code + config coverage. Might be outdated vs actual provider list in code. Contains long-term plan for many providers not implemented.

## docs/deployment.md
- Purpose: deployment guide for Docker/Compose/Kubernetes with LiveKit.
- Architecture integration: mirrors Dockerfile and config env vars.
- Notes/issues: references distroless runtime and Rust 2024, but Dockerfile uses debian runtime and edition is 2021. Build args mention `RUN_SAYNA_INIT` and `--all-features`, while Dockerfile uses `RUN_GATEWAY_INIT` and `--no-default-features --features turn-detect,noise-filter`.

## docs/docker.md
- Purpose: quick Docker usage doc.
- Architecture integration: runtime env vars and endpoints.
- Notes/issues: provider list limited to early providers; likely outdated. Mentions `audio_disabled` config schema; verify current WS config supports it.

## docs/openai-stt.md
- Purpose: OpenAI Whisper STT usage/config guide.
- Architecture integration: maps to `src/core/stt/openai/*` and WS config fields.
- Notes/issues: ensure WS config field names match `IncomingMessage` schema (uses `stt` vs `stt_config` in docs).

## docs/openai-tts.md
- Purpose: OpenAI TTS usage/config guide.
- Architecture integration: `src/core/tts/openai/*` and `/speak`.
- Notes/issues: verify voice/model defaults and `voice_id` naming match actual config in code.

## docs/openai-realtime.md
- Purpose: OpenAI realtime WebSocket `/realtime` protocol guide.
- Architecture integration: `src/core/realtime/openai/*`, `src/handlers/realtime/*`.
- Notes/issues: confirm supported fields align with `RealtimeSessionConfig` and handler behavior.

## docs/google-stt.md
- Purpose: Google STT gRPC streaming guide and auth patterns.
- Architecture integration: `src/core/stt/google/*`, config loader (google_credentials).
- Notes/issues: uses `api_key` field in WS config to pass JSON; confirm code supports inline JSON in `api_key` or uses server-side credentials only.

## docs/google-tts.md
- Purpose: Google TTS usage guide.

## src/core/tts/cartesia/mod.rs
- Purpose: module-level docs and re-exports for Cartesia TTS.
- Architecture integration: exposes Cartesia TTS config + provider; used by `create_tts_provider`.
- Notes/issues: docs claim language support and voice usage; actual request builder always uses language "en" and sends voice id (empty string if missing).

## src/core/tts/cartesia/provider.rs
- Purpose: Cartesia TTS REST request builder + provider using generic `TTSProvider` (HTTP pool, queue, cache).
- Architecture integration: `CartesiaRequestBuilder` builds POST to `https://api.cartesia.ai/tts/bytes` with `Cartesia-Version` header; `CartesiaTTS` plugs into generic provider pipeline.
- Notes/issues:
  - `get_language()` hardcodes `"en"` (no config/voice-based language), which can mislabel non-English voices.
  - `build_voice_json` sends empty id when voice_id missing (no validation), may fail API-side without clear error.
  - Config hash excludes pronunciations/emotion (consistent with other providers but means cache collisions if pronunciations change).

## src/core/tts/aws_polly/config.rs
- Purpose: Polly engine/output formats/voice list + Polly-specific config wrapper over `TTSConfig`.
- Architecture integration: used by `AwsPollyTTS` provider (AWS SDK).
- Notes/issues:
  - `MAX_TOTAL_LENGTH` exists but not enforced in provider (only `MAX_TEXT_LENGTH` is checked).
  - `speaking_rate` in base defaults to 1.0 but Polly provider does not use it (no SSML generation).
  - Some non-ASCII voice IDs (e.g., "Léa") are baked; ok if AWS requires.

## src/core/tts/aws_polly/provider.rs
- Purpose: AWS Polly TTS via AWS SDK; synchronous request -> ByteStream -> chunking for PCM.
- Architecture integration: standalone provider (does not use `TTSProvider`), but implements `BaseTTS`.
- Notes/issues:
  - `speak` ignores `flush` (expected for sync provider but mismatch with interface semantics).
  - No caching or queueing; `audio_bytes.to_vec()` copies full response (can be heavy for long responses).
  - Pronunciations from `TTSConfig` unused; no SSML generation despite `TextType` support.

## src/core/tts/aws_polly/mod.rs
- Purpose: module wrapper + re-exports.
- Architecture integration: exposes Polly types + provider.
- Notes/issues: docs mention caching but provider doesn’t implement any.

## src/core/tts/aws_polly/tests.rs
- Purpose: unit/integration tests for Polly config and provider.
- Notes/issues: integration tests require AWS creds and are ignored by default.

## src/core/tts/gnani/config.rs
- Purpose: Gnani TTS config and language/gender enums; maps base config to Gnani fields.
- Architecture integration: used by `GnaniTTS` provider.
- Notes/issues:
  - `from_base` always reads `GNANI_TOKEN`/`GNANI_ACCESS_KEY` from env and ignores any token/access_key present in config, so WS-configured creds can’t be passed through.
  - `voice_name` is always `None` (no mapping from base config), so only default voice “gnani” is used.
  - Gender derived from `model` string containing “MALE” (non-obvious, no explicit field).

## src/core/tts/gnani/provider.rs
- Purpose: Gnani TTS REST provider (base64 audio response).
- Architecture integration: synchronous HTTP request + callback delivery; no generic `TTSProvider`.
- Notes/issues:
  - `speak` only calls `on_complete` when `flush == true`; if caller uses `flush=false`, completion may never fire (potentially hangs downstream).
  - Hardcodes `audio_encoding` to `pcm16` and ignores base `audio_format`/`pronunciations`.
  - Uses `tokio::spawn` to set callbacks, so a race may occur if `speak` is invoked immediately after `on_audio`.

## src/core/tts/gnani/mod.rs
- Purpose: module wrapper + docs for Gnani TTS.
- Architecture integration: exposes config enums + provider.
- Notes/issues: docs claim multi-speaker support; current provider never sets `voice_name` (always default).

## src/core/tts/ibm_watson/config.rs
- Purpose: IBM Watson TTS config, voice list, output formats, IAM constants.
- Architecture integration: used by `IbmWatsonTTS` provider with IAM token flow (shared IAM URL with STT).
- Notes/issues: `build_query_params` uses dynamic vector -> static keys mapping; OK but extra keys default to `"voice"` if unexpected (safe).

## src/core/tts/ibm_watson/provider.rs
- Purpose: IBM Watson TTS REST provider with IAM token caching, SSML for rate/pitch, chunking for PCM.
- Architecture integration: uses its own reqwest client, not `TTSProvider`; IAM token fetch shared with STT.
- Notes/issues:
  - On 401 responses, cached IAM token is not cleared (comment says it should be). This can cause repeated auth failures until token expiry.
  - Pronunciations in `TTSConfig` are not applied (only customizations via IDs are supported).

## src/core/tts/ibm_watson/mod.rs
- Purpose: module wrapper + re-exports for IBM Watson TTS.
- Architecture integration: exposes provider, config, and constants.
- Notes/issues: docs imply “automatic refresh” (true) but 401 invalidation gap remains.

## src/core/tts/ibm_watson/tests.rs
- Purpose: comprehensive unit tests for IBM Watson TTS config/provider.
- Notes/issues: no integration tests (credentials required).

## src/core/tts/hume/config.rs
- Purpose: Hume Octave TTS config (emotion via natural language description), audio formats, validation.
- Architecture integration: used by Hume request builder/provider; describes instant mode and generation_id.
- Notes/issues:
  - `voice_description` field is defined but never used in provider request body.
  - `generation_id`, `trailing_silence`, `num_generations` affect audio but are not included in cache hash (see provider).

## src/core/tts/hume/messages.rs
- Purpose: Hume TTS request/response structs for REST API.
- Architecture integration: used by Hume request builder.
- Notes/issues: supports voice by name or ID, but current provider always uses `by_name` (custom voice ID path unused).

## src/core/tts/hume/provider.rs
- Purpose: Hume TTS request builder + provider using generic `TTSProvider`.
- Architecture integration: HTTP POST to `https://api.hume.ai/v0/tts/stream/file` with X-Hume-Api-Key header.
- Notes/issues:
  - `HumeRequestBuilder::build_voice_spec` always uses `ByName`; custom voice IDs are not supported.
  - `previous_text` field exists and `build_http_request_with_context` supports context, but generic `TTSProvider` may not call it (likely unused); context continuity may be incomplete.
  - Config hash omits `generation_id`, `trailing_silence`, `num_generations`, `voice_description`, and pronunciations/emotion_config (cache collision risk).

## src/core/tts/hume/mod.rs
- Purpose: module wrapper + documentation; re-exports config/messages/provider.
- Architecture integration: exposes Hume types and provider.
- Notes/issues: docs mention voice cloning and context continuity; actual request builder doesn’t accept voice IDs or voice_description yet.

## src/core/tts/lmnt/config.rs
- Purpose: LMNT TTS config and audio format mapping.
- Architecture integration: used by LMNT provider request builder.
- Notes/issues: LMNT supports non-streamable formats (AAC/WAV) but provider uses generic streaming flow; no guard preventing use in real-time streaming scenarios.

## src/core/tts/lmnt/messages.rs
- Purpose: LMNT API request/response structs (voice list, cloning, WS types).
- Architecture integration: not currently wired into provider; prepared for future features (voice list/clone/WS).
- Notes/issues: no provider integration for WS or voice cloning yet (types exist only).

## src/core/tts/lmnt/provider.rs
- Purpose: LMNT TTS request builder + provider using generic `TTSProvider`.
- Architecture integration: HTTP POST to `https://api.lmnt.com/v1/ai/speech/bytes` with X-API-Key.
- Notes/issues:
  - Config hash omits pronunciations/emotion_config (cache collision risk).
  - No enforcement of streaming-safe formats (AAC/WAV may be used with chunking expectations).

## src/core/tts/lmnt/mod.rs
- Purpose: module wrapper + constants and re-exports.
- Architecture integration: exposes LMNT provider/config/messages.
- Notes/issues: none beyond provider limits.

## src/core/tts/playht/config.rs
- Purpose: Play.ht TTS config, model and format enums, validation.
- Architecture integration: used by PlayHt provider; mapping from base config.
- Notes/issues:
  - Many advanced parameters are supported but validation only checks a subset (no bounds for guidance fields, repetition_penalty, etc.).
  - Uses `voice_id` as required manifest URL; no helper to map from friendly voice name.

## src/core/tts/playht/messages.rs
- Purpose: Play.ht API request/response and WS message types.
- Architecture integration: provider uses request types; WS auth/streaming types currently unused.
- Notes/issues: WS support exists in types only; no integration in provider.

## src/core/tts/playht/provider.rs
- Purpose: Play.ht request builder + provider using generic `TTSProvider`.
- Architecture integration: POST to `https://api.play.ht/api/v2/tts/stream` with `X-USER-ID` + `AUTHORIZATION` headers.
- Notes/issues:
  - Requires `PLAYHT_USER_ID` env var for `new()`; this cannot be set via base config (only via env or `with_user_id`).
  - Config hash omits pronunciations/emotion_config (cache collision risk).

## src/core/tts/playht/mod.rs
- Purpose: module wrapper + constants/re-exports for Play.ht TTS.
- Architecture integration: exposes provider/config/messages.
- Notes/issues: docs suggest 30s min for voice cloning; no actual cloning endpoint integration in provider.
- Architecture integration: `src/core/tts/google/*`, provider config via google_credentials.
- Notes/issues: verify voice_id/audio_format sample rate handling aligns with code.

## docs/azure-stt.md
- Purpose: Azure STT guide.
- Architecture integration: `src/core/stt/azure/*`.
- Notes/issues: doc mentions `auto_detect_languages` field; verify in code.

## docs/azure-tts.md
- Purpose: Azure TTS guide.
- Architecture integration: `src/core/tts/azure/*`.
- Notes/issues: verify SSML generation and speaking_rate handling align with code.

## docs/cartesia-stt.md
- Purpose: Cartesia STT guide.
- Architecture integration: `src/core/stt/cartesia/*`.
- Notes/issues: mentions `min_volume`/`max_silence_duration_secs` params; verify actual config/query support.

## docs/cartesia-tts.md
- Purpose: Cartesia TTS guide.
- Architecture integration: `src/core/tts/cartesia/*`.
- Notes/issues: doc says speaking_rate reserved for future use; verify code behavior. Ensure format mappings match code enums.

## docs/dag_routing.md
- Purpose: DAG routing feature design and config schema.
- Architecture integration: `src/dag/*` (feature-gated).
- Notes/issues: mentions templates not implemented; verify if templates exist or stubs in code.

## docs/sip_routing.md
- Purpose: SIP header-based webhook routing architecture.
- Architecture integration: `src/handlers/sip/*`, `src/utils/sip_hooks.rs`, `src/livekit/sip_handler.rs`.
- Notes/issues: endpoints `/sip/hooks` and runtime persistence should be verified in code.

## docs/api-reference.md
- Purpose: combined architecture and API documentation.
- Architecture integration: describes REST/WS endpoints and behavior.
- Notes/issues: provider list and REST `/speak` doc mentions only deepgram/elevenlabs; may be outdated. Lists LiveKit room/participant endpoints; verify handlers/routes exist and auth scoping works.

## docs/plugin_architecture_notes.md
- Purpose: analysis notes about plugin architecture and hardcoded factories.
- Architecture integration: references core factories and config structure.
- Notes/issues: may be outdated if plugin system now implemented (`src/plugin/*`).

## docs/hume.md
- Purpose: Hume TTS/EVI/voice cloning guide plus unified emotion mapping.
- Architecture integration: `src/core/tts/hume/*`, `src/core/realtime/hume/*`, `src/core/emotion/*`, `/voices/clone`.
- Notes/issues: claims voice cloning supports audio samples; memory.md says audio samples ignored. Verify implementation.

## docs/new_provider.md
- Purpose: comprehensive provider addition guide.
- Architecture integration: references factory-based design.
- Notes/issues: uses factory pattern; may be outdated given plugin registry. Mentions many examples not in code (dubbing providers).

## docs/waav_integrations.json
- Purpose: provider catalog data.
- Architecture integration: unclear; likely documentation/reference only.
- Notes/issues: check if used in code; otherwise stale.

## src/main.rs
- Purpose: CLI entrypoint, config loading, middleware wiring, routing, TLS setup, and server start.
- Architecture integration: builds AppState, sets up auth/connection-limit middleware on WS and realtime routes, CORS, rate limiting, security headers, and optional OpenAPI generation/turn-detect init.
- Notes/issues: rate limiting disabled when `rate_limit_rps >= 100000`; confirm config defaults. Auth middleware is applied to WS even though docs say WS unauthenticated—needs validation. CORS allows `x-provider-api-key`; verify use. TLS config required only when enabled.

## src/lib.rs
- Purpose: crate module exports and re-exports.
- Architecture integration: exposes core modules, plugin registry, AppState, and error/result types for internal use/tests.
- Notes/issues: `docs` module is always compiled (OpenAPI generation is feature-gated inside it).

## src/init.rs
- Purpose: CLI helper for `waav-gateway init` to download turn-detect assets to cache.
- Architecture integration: uses `ServerConfig::from_env` and `core::turn_detect::assets`.
- Notes/issues: requires `turn-detect` feature and `CACHE_PATH` env var; otherwise returns error.

## src/config/mod.rs
- Purpose: core configuration types, API key accessors, load/validation pipeline entrypoints, and secret zeroization.
- Architecture integration: `ServerConfig` consumed by AppState and handlers; plugin config stub defined here.
- Notes/issues: module doc says priority YAML > ENV > .env > defaults (matches config.example, conflicts with config.yaml comments). `PluginConfig` derives Default (enabled=false) but doc says plugin system enabled by default; verify in merge/env logic. Zeroize covers most secrets but not all optional fields (e.g., playht_user_id is not zeroized; check if needed).

## src/config/env.rs
- Purpose: load ServerConfig from environment vars with defaults; validate TLS/auth/security; parse SIP env config.
- Architecture integration: used by `ServerConfig::from_env` and `init`.
- Notes/issues: plugin system explicitly enabled by default via `PLUGINS_ENABLED` default true—contrasts with `PluginConfig::default` in struct; verify merge behavior. SIP env config requires `SIP_ROOM_PREFIX` if any SIP vars set.

## src/config/yaml.rs
- Purpose: serde types for YAML config file and loader.
- Architecture integration: used by merge layer to populate `ServerConfig`.
- Notes/issues: plugin `enabled` is Option; default behavior depends on merge logic (verify default true/false). Includes security section with rate limits and max connections.

## src/config/merge.rs
- Purpose: merge YAML config with environment vars into ServerConfig; defines merge priorities and merges SIP sub-config.
- Architecture integration: called by ServerConfig::from_yaml/from_env to produce runtime config; critical for auth, TLS, providers, SIP, security, plugin toggles.
- Notes/issues: explicit priority is YAML > ENV > defaults (matches docs in config.example, conflicts with comment in config.yaml). Plugin enabled default true for backward compatibility (reinforces default-on behavior). Auth API secrets merge precedence: YAML non-empty > AUTH_API_SECRETS_JSON > legacy AUTH_API_SECRET/AUTH_API_SECRET_ID. SIP merge supports env JSON hooks; requires room_prefix if any SIP config set. Uses max connections default 100 but max_websocket_connections optional (None = unlimited). Cache TTL defaults 30 days. No validation here beyond parse/required paths.

## src/config/utils.rs
- Purpose: parse_bool helper with multiple accepted values.
- Architecture integration: used by env/merge for config flags (TLS, auth, plugins, etc.).
- Notes/issues: None; returns None on unrecognized strings (caller chooses default).

## src/config/validation.rs
- Purpose: validation for JWT auth, TLS, auth-required config, API secret entries, SIP config, and security rate limit values.
- Architecture integration: called during config load to enforce correctness and security constraints.
- Notes/issues: SIP allowed_addresses validation is regex-based and only checks pattern, not 0-255 ranges; invalid IPs like 999.1.1.1 pass. HTTPS validation for SIP hooks is explicitly not implemented (commented test), but docs require HTTPS for hooks. SIP hook secrets require min length 16; ensures per-hook or global secret present when hooks exist.

## src/config/sip.rs
- Purpose: SIP configuration structures and normalization logic for SIP hooks and room prefixing.
- Architecture integration: used by SIP handlers and utils; hooks implement SipHookInfo for downstream forwarding.
- Notes/issues: Normalizes hooks to lowercase hosts and trims secrets/addresses; naming_prefix default "waav". Does not validate URL scheme or host format; relies on validation.rs (which doesn’t enforce HTTPS).

## src/config/pricing.rs
- Purpose: centralized static pricing map + helper functions for STT/TTS model cost estimation.
- Architecture integration: referenced in docs and potentially in API (cost estimation features).
- Notes/issues: Last updated 2024-01-06; prices may be stale. Estimation returns 0.0 for incompatible unit conversions (time vs char), which can be misleading vs returning None/error.

## src/core/stt/base.rs
- Purpose: shared STTConfig/STTResult/traits and STTStats helper.
- Architecture integration: base contracts for all STT providers and plugin registry.
- Notes/issues: STTConfig default model "nova-3" is Deepgram-specific; can be misleading for other providers if caller relies on defaults. STTStats appears unused (needs verification in other modules).

## src/core/stt/mod.rs
- Purpose: STT provider module hub, re-exports, enum list, and factory wrappers using plugin registry.
- Architecture integration: central provider registry; create_stt_provider delegates to global plugin registry.
- Notes/issues: STTProvider enum and get_supported_stt_providers omit Gnani even though module exists; cannot select gnani via enum or list, and error text omits it. `example` module contains a todo stub (doc-only, but still a stub).

## src/core/stt/openai/config.rs
- Purpose: OpenAI Whisper STT config (model/format/flush/silence detection).
- Architecture integration: used by OpenAISTT REST client.
- Notes/issues: AudioInputFormat suggests non-WAV inputs are supported, but client always packages raw PCM into WAV (see client.rs) and only changes MIME type.

## src/core/stt/openai/messages.rs
- Purpose: response parsing and WAV helper for OpenAI STT.
- Architecture integration: OpenAISTT response parsing + PCM->WAV packaging.
- Notes/issues: WAV helper always assumes 16-bit PCM, so non-PCM input from upstream would be misinterpreted.

## src/core/stt/openai/client.rs
- Purpose: OpenAI Whisper REST client with buffering + optional silence/threshold flush.
- Architecture integration: BaseSTT implementation for REST-based STT.
- Notes/issues:
  - Always creates WAV from raw PCM buffer but uses `audio_input_format` MIME; if config sets mp3/webm/etc, request body is still WAV (mismatch).
  - Silence detection assumes PCM16; ignores `base.encoding` (if caller sends mu-law/opus/etc, silence detection is invalid).
  - No audio format conversion; caller must provide PCM16.

## src/core/stt/groq/config.rs
- Purpose: Groq Whisper STT config (models/format/flush/silence detection, translation endpoint).
- Architecture integration: used by GroqSTT REST client.
- Notes/issues: Same as OpenAI: audio_input_format suggests non-WAV input, but client always wraps PCM as WAV (see client.rs).

## src/core/stt/groq/messages.rs
- Purpose: Groq response parsing, confidence estimation, WAV helper.
- Architecture integration: GroqSTT response parsing + PCM->WAV packaging.
- Notes/issues: WAV helper is PCM16-only; not compatible with non-PCM input.

## src/core/stt/groq/client.rs
- Purpose: Groq REST client with buffering, retry/backoff, and silence-based flushing.
- Architecture integration: BaseSTT implementation for Groq.
- Notes/issues:
  - Always creates WAV from raw PCM buffer but uses `audio_input_format` MIME/extension; non-WAV formats are mislabeled.
  - Silence detection assumes PCM16 regardless of `base.encoding`.
  - No audio format conversion; upstream must deliver PCM16.

## src/core/stt/assemblyai/config.rs
- Purpose: AssemblyAI streaming config and WebSocket URL builder.
- Architecture integration: used by AssemblyAISTT client.
- Notes/issues: `include_word_timestamps` is not used in URL (v3 may always return words). No validation that `pcm_mulaw` uses 8kHz.

## src/core/stt/assemblyai/messages.rs
- Purpose: AssemblyAI v3 WS message structs and parsing.
- Architecture integration: AssemblyAISTT WebSocket handling.
- Notes/issues: none obvious in parsing; UpdateConfiguration types exist but not used by client.

## src/core/stt/assemblyai/client.rs
- Purpose: AssemblyAI v3 streaming WS client.
- Architecture integration: BaseSTT implementation using binary audio frames.
- Notes/issues:
  - No keepalive or ping; relies on inbound messages, may timeout during long silence.
  - No validation for encoding/sample_rate compatibility (e.g., mu-law should be 8kHz).
  - UpdateConfigurationMessage type exists but no client method to send it; only ForceEndpoint is implemented.

## src/core/stt/aws_transcribe/config.rs
- Purpose: AWS Transcribe Streaming config definitions and helpers.
- Architecture integration: used by AwsTranscribeSTT.
- Notes/issues: config exposes many options (language options, content redaction, diarization) that the client does not apply.

## src/core/stt/aws_transcribe/client.rs
- Purpose: AWS SDK streaming client implementation.
- Architecture integration: BaseSTT for Amazon Transcribe.
- Notes/issues:
  - `show_speaker_label` does not set `max_speaker_labels` in request (likely required by AWS).
  - `enable_channel_identification`, `number_of_channels`, `vocabulary_filter_method`, `language_model_name`, `preferred_language`, `enable_content_redaction`, `content_redaction_types`, `pii_entity_types` are never sent.
  - `identify_language` toggled but no language options are provided.
  - `convert_language_code` only supports a small subset; unsupported languages default to en-US (could mis-transcribe).
  - BaseSTT::new only reads AWS creds from env; STTConfig has no fields for access keys, so WS-configured creds can’t be passed unless using new_with_config.
  - `chunk_duration_ms` is not enforced by client (caller must chunk appropriately).

## src/core/stt/ibm_watson/config.rs
- Purpose: IBM Watson STT config, model/encoding enums, URL/message builder.
- Architecture integration: used by IbmWatsonSTT client (WebSocket).
- Notes/issues:
  - from_base ignores `base.model`; user-specified model in STTConfig is discarded (unless updated later via IBM-specific setters).
  - Linear16 content-type always sets `channels=1` (ignores base.channels).

## src/core/stt/ibm_watson/client.rs
- Purpose: IBM Watson STT WebSocket client with IAM token caching and keepalive.
- Architecture integration: BaseSTT for IBM Watson.
- Notes/issues:
  - Keepalive sends a fixed 64-byte silence frame assuming PCM16 16kHz mono; ignores actual encoding/sample_rate/channels and may be invalid for other formats.
  - No validation for encoding/sample_rate compatibility with model (telephony vs multimedia).

## src/core/stt/gnani/config.rs
- Purpose: Gnani STT config with mTLS certificate settings.
- Architecture integration: used by GnaniSTT gRPC client.
- Notes/issues:
  - from_base pulls token/access_key/cert from env only; there’s no way to pass these via STTConfig/WS config (config fields exist but are ignored).

## src/core/stt/gnani/grpc.rs
- Purpose: Gnani gRPC client, metadata headers, and custom codec.
- Architecture integration: used by GnaniSTT streaming session.
- Notes/issues: metadata uses base.encoding/audio_format but no audio conversion; caller must send correctly encoded audio.

## src/core/stt/gnani/client.rs
- Purpose: Gnani gRPC streaming STT client.
- Architecture integration: BaseSTT for Gnani.
- Notes/issues:
  - `interim_results` config is not used to filter results; interim chunks are always emitted.
  - No support for updating credentials at runtime; update_config does not reconnect or revalidate.

## src/core/stt/deepgram.rs
- Purpose: Deepgram WebSocket STT client.
- Architecture integration: BaseSTT for Deepgram.
- Notes/issues: `diarize`, `filler_words`, `profanity_filter`, `redact`, `vad_events`, `utterance_end_ms` exist in config but are not added to WS URL; URL params are not URL-encoded (keywords/tags/language may break).

## src/core/stt/google/provider.rs
- Purpose: Google STT v2 streaming provider using google-cloud-speech.
- Architecture integration: BaseSTT for Google STT.
- Notes/issues: keepalive sends LINEAR16 silence regardless of encoding; wrong for mu-law/alaw. Credentials parsing expects project_id either in model string or JSON; no explicit location/recognizer config.

## src/core/stt/azure/client.rs
- Purpose: Azure STT WebSocket client.
- Architecture integration: BaseSTT for Azure.
- Notes/issues: `auto_detect_languages` is sent alongside `language` param (likely invalid); `word_level_timing` not passed in URL; Content-Type always `audio/wav; codecs=audio/pcm` regardless of encoding; keepalive uses fixed 64-byte PCM silence.

## src/core/stt/cartesia/client.rs
- Purpose: Cartesia STT WebSocket client.
- Architecture integration: BaseSTT for Cartesia.
- Notes/issues: `finalize()` is stub; API key in query string; no keepalive; `channels` config not used.

## src/core/stt/elevenlabs/client.rs
- Purpose: ElevenLabs real-time STT WebSocket client.
- Architecture integration: BaseSTT for ElevenLabs.
- Notes/issues: language code is truncated to primary subtag (e.g., en-US -> en); no keepalive for silence; client never sets `commit` in audio chunks (manual commit strategy unused).

## src/handlers/sip/hooks.rs
- Purpose: REST endpoints to list/update/delete SIP hooks persisted in cache; merge runtime hooks with config hooks.
- Architecture integration: used by `/sip/hooks` routes; relies on `utils::sip_hooks` + cache store; uses `validate_webhook_url` SSRF guard.
- Notes/issues:
  - Requires configured cache path; returns 500 if missing (no in-memory fallback).
  - Disallows overriding hooks for config-managed hosts; uses global secret for runtime hooks.

## src/handlers/sip/transfer.rs
- Purpose: REST endpoint to initiate SIP transfer for a LiveKit room.
- Architecture integration: uses LiveKit handlers to fetch room/participants; validates phone using `utils::phone_validation`.
- Notes/issues: chooses first SIP participant in room (no participant id param); times out to "initiated" after 2s.

## src/handlers/sip/mod.rs
- Purpose: SIP handler module exports.
- Architecture integration: consolidates hooks + transfer routes.
- Notes/issues: none.

## src/handlers/ws/config.rs
- Purpose: WebSocket config structs for STT/TTS/LiveKit and hash helpers.
- Architecture integration: used in WS config handling for VoiceManager and LiveKit setup.
- Notes/issues:
  - `compute_tts_config_hash` omits pronunciations/emotion config; caching may be stale if those change.
  - Assumes mono audio when deriving LiveKit config.

## src/handlers/ws/messages.rs
- Purpose: WS message enums, validation, and size limits (audio and text).
- Architecture integration: parse `auth`, `config`, `speak`, `clear`, `interrupt`, `custom`, etc.
- Notes/issues: `auth` is only allowed as first message; `custom` forwards to plugins.

## src/handlers/ws/state.rs
- Purpose: WS connection state (auth status, optional DAG).
- Architecture integration: shared between handler and message processors.
- Notes/issues: DAG state only available behind feature flag.

## src/handlers/ws/audio_handler.rs
- Purpose: binary audio handling and DAG routing integration.
- Architecture integration: routes audio to DAG or VoiceManager; honors `MAX_AUDIO_FRAME_SIZE`.
- Notes/issues: `handle_clear_message` respects non-interruptible playback (won't clear while playback is locked).

## src/handlers/ws/command_handler.rs
- Purpose: handles commands from WS (interrupt, clear, speak, connect, SIP transfer).
- Architecture integration: pushes operations to VoiceManager/LiveKit operations queue.
- Notes/issues: SIP transfer chooses first SIP participant in room (no participant id).

## src/handlers/ws/config_handler.rs
- Purpose: initializes VoiceManager and LiveKit on config; registers callbacks and sends ready.
- Architecture integration: core WS setup; wires STT/TTS and LiveKit.
- Notes/issues:
  - Calls `voice_manager.on_tts_audio` twice (cached audio + LiveKit). If the callback is overwritten rather than composed, the first is lost.
  - LiveKit data callback uses hardcoded room name "livekit" (TODO in code).

## src/handlers/ws/handler.rs
- Purpose: WS upgrade, auth gating, idle timeout, message loop, cleanup.
- Architecture integration: entrypoint for `/ws`.
- Notes/issues:
  - Idle timeout is 5 min with jitter (docs mention 10s); no ping/pong keepalive.
  - Jitter calculation uses `Instant::now().elapsed()` (near-zero), so idle timeout jitter is effectively 0 across connections.
  - First-message auth supported; if auth is required and missing, sends AuthRequired.

## src/handlers/ws/tests.rs
- Purpose: message serialization/validation tests.
- Architecture integration: covers WS protocol edges.
- Notes/issues: none.

## src/handlers/realtime/mod.rs
- Purpose: module exports for realtime WS.
- Architecture integration: `/realtime` WS handler for OpenAI realtime.
- Notes/issues: none.

## src/handlers/realtime/messages.rs
- Purpose: realtime WS message types and validation.
- Architecture integration: schema for realtime session updates and audio.
- Notes/issues: session update fields exist but handler ignores many.

## src/handlers/realtime/handler.rs
- Purpose: realtime WS handler, provider integration, idle timeout.
- Architecture integration: `/realtime` endpoints; uses realtime provider configs.
- Notes/issues:
  - `handle_session_update` ignores turn_detection/tools/transcribe_input/output formats.
  - `api_key` is forced to empty on update (relies on provider retaining prior key).
  - Drops audio if provider is not ready; no backpressure.

## src/utils/mod.rs
- Purpose: utility module exports.
- Architecture integration: shared helpers for SIP hooks, URL validation, phone validation, noise filter, ReqManager.
- Notes/issues: none.

## src/utils/noise_filter.rs
- Purpose: DeepFilterNet noise reduction worker pool.
- Architecture integration: used when `noise-filter` feature enabled.
- Notes/issues: feature-gated stub if disabled.

## src/utils/phone_validation.rs
- Purpose: permissive phone validation/normalization for SIP transfer.
- Architecture integration: used in SIP handlers.
- Notes/issues: validation is shallow; accepts many formats and only prepends `tel:`.

## src/utils/req_manager.rs
- Purpose: pooled HTTP/2 client with metrics, retry, warmup.
- Architecture integration: shared across TTS providers and other HTTP clients.
- Notes/issues:
  - `OperationQueue::pending_count()` appears inverted (capacity - max_capacity), likely underflows; should be `max_capacity - capacity`.

## src/utils/sip_api_client.rs
- Purpose: raw Twirp client for LiveKit SIP API (trunks, transfer, include_headers).
- Architecture integration: used by LiveKit SIP handlers.
- Notes/issues: transfer treats timeout as "initiated" (2s) and returns success.

## src/utils/sip_hooks.rs
- Purpose: cache-backed storage/merge of SIP hooks.
- Architecture integration: used by SIP hooks handlers and SIP webhook forwarding.
- Notes/issues: uses blocking `Path::exists()`; merge logic is a bit redundant but functional.

## src/utils/url_validation.rs
- Purpose: SSRF protection for webhook URLs.
- Architecture integration: used by SIP hooks and LiveKit webhooks.
- Notes/issues: DNS resolution uses blocking `ToSocketAddrs`; may block async thread.

## src/livekit/mod.rs
- Purpose: LiveKit integration module exports.
- Architecture integration: LiveKit client, ops queue, SIP/room handlers.
- Notes/issues: none.

## src/livekit/types.rs
- Purpose: LiveKit config and error types.
- Architecture integration: shared across LiveKit client and handlers.
- Notes/issues: none.

## src/livekit/operations.rs
- Purpose: operation queue and stats for LiveKit actions.
- Architecture integration: used by LiveKit client and handler queueing.
- Notes/issues: `pending_count()` calculation is inverted (capacity - max_capacity), likely underflow/incorrect.

## src/livekit/manager.rs
- Purpose: LiveKitManager wrapper for managing rooms/connection.
- Architecture integration: used by handlers/VoiceManager.
- Notes/issues: `set_audio_callback` is a stub (warns, not implemented).

## src/livekit/room_handler.rs
- Purpose: room creation, token generation, recording integration.
- Architecture integration: LiveKit REST API and token signing.
- Notes/issues:
  - Token grants for "user" include room_create/record/list (over-permission).
  - `max_participants=3` hardcoded for room creation.

## src/livekit/sip_handler.rs
- Purpose: SIP trunk and dispatch management with LiveKit.
- Architecture integration: called in SIP handler and provisioning.
- Notes/issues: `max_participants` is not applied to SIP rooms (TODO).

## src/livekit/client/mod.rs
- Purpose: LiveKit client module exports.
- Architecture integration: WS client/RTC integration.
- Notes/issues: none.

## src/livekit/client/connection.rs
- Purpose: LiveKit room connection and session lifecycle.
- Architecture integration: used by LiveKit client.
- Notes/issues: operation priorities are not enforced (simple mpsc).

## src/livekit/client/audio.rs
- Purpose: audio publishing/track management.
- Architecture integration: handles TTS audio publishing.
- Notes/issues: expects PCM16; no format conversion if TTS returns mp3/wav.

## src/livekit/client/callbacks.rs
- Purpose: LiveKit event callback registration and dispatch.
- Architecture integration: used by LiveKit client for audio/data events.
- Notes/issues: uses config sample_rate/channels for incoming audio, not track-provided values.

## src/livekit/client/events.rs
- Purpose: LiveKit room event handling and reconnection.
- Architecture integration: used by LiveKit client.
- Notes/issues: re-publishes tracks on reconnect; no resample of remote audio.

## src/livekit/client/messaging.rs
- Purpose: data message sending to LiveKit.
- Architecture integration: used by LiveKit client and DAG livekit endpoint.
- Notes/issues: none.

## src/livekit/client/operation_worker.rs
- Purpose: LiveKit operation worker processing queue.
- Architecture integration: executes queued operations.
- Notes/issues: priorities not enforced in queue.

## src/livekit/client/tests.rs
- Purpose: LiveKit client tests.
- Architecture integration: none.
- Notes/issues: none.

## src/plugin/mod.rs
- Purpose: plugin system exports.
- Architecture integration: provider registration + creation across STT/TTS/realtime/processors.
- Notes/issues: none.

## src/plugin/registry.rs
- Purpose: global plugin registry and capability indexes.
- Architecture integration: central provider registry; used by factory functions.
- Notes/issues:
  - Aliases are registered twice (once in `register_*` and again when iterating `constructor.aliases`), which may insert aliases into capability index and create duplicate provider entries.

## src/plugin/dispatch.rs
- Purpose: PHF dispatch tables for built-in providers.
- Architecture integration: compile-time aliasing and metadata.
- Notes/issues: Gnani appears in dispatch lists but may be missing in other provider lists (mismatch).

## src/plugin/isolation.rs
- Purpose: panic isolation and error capturing for plugins.
- Architecture integration: ensures plugin failures don’t crash server.
- Notes/issues: none.

## src/plugin/capabilities.rs, lifecycle.rs, metadata.rs, macros.rs, builtin/mod.rs
- Purpose: plugin traits, lifecycle hooks, metadata, macro helpers, built-in registrations.
- Architecture integration: powers inventory-based plugin discovery.
- Notes/issues: none beyond alias duplication noted above.

## src/dag/nodes/provider.rs
- Purpose: DAG nodes wrapping STT/TTS/realtime providers with channel bridging.
- Architecture integration: used in DAG routing pipelines.
- Notes/issues:
  - `with_config` fields for STT/TTS/Realtime nodes are stored but never used.
  - Realtime node hardcodes output sample_rate=24000 and format="pcm16" (may not match provider).

## src/dag/nodes/endpoint.rs
- Purpose: DAG endpoints for HTTP/gRPC/WS/IPC/LiveKit.
- Architecture integration: used in DAG definitions for external integrations.
- Notes/issues:
  - No SSRF validation for HTTP/gRPC/WS endpoints; DAG definitions could call internal services.
  - gRPC TLS detection uses `address.contains("localhost")`; raw IPs without scheme default to TLS (might fail).
  - WebSocket endpoint expects a single response frame; ping/pong or streaming responses produce errors.

## src/dag/nodes/router.rs
- Purpose: split/join/router nodes with Rhai selection and merge scripting.
- Architecture integration: DAG flow control.
- Notes/issues:
  - Join selector/merge scripts expose `api_key` and `api_key_id` to user-supplied Rhai scripts (data leak risk if DAG definitions are untrusted).
  - Router uses new default evaluator each execute; compiled conditions must be compatible with that engine.

## src/dag/nodes/transform.rs
- Purpose: Rhai-based transform and passthrough nodes.
- Architecture integration: data shaping between nodes.
- Notes/issues: transform scripts get metadata and api_key_id but not api_key; ok but inconsistent with router/join.

## src/dag/endpoints/http.rs
- Purpose: HTTP endpoint adapter (legacy).
- Architecture integration: used by EndpointAdapter trait (not DAGNode).
- Notes/issues: no SSRF validation; forwards ctx.api_key to external endpoint.

## src/dag/endpoints/webhook.rs
- Purpose: fire-and-forget webhook adapter.
- Architecture integration: used by EndpointAdapter (legacy).
- Notes/issues: no SSRF validation; spawns task without backpressure; uses ctx.api_key_id only.

## src/dag/templates.rs
- Purpose: DAG templates registry with inline/dir loading.
- Architecture integration: template lookup for DAG definitions.
- Notes/issues: no template validation before registration; invalid DAGs are only caught later.

## src/core/tts/mod.rs
- Purpose: TTS provider exports and factory.
- Architecture integration: provider creation via plugin registry; URL helper list.
- Notes/issues: tests assume error messages list providers; ensure registry error formatting matches.

## src/core/tts/base.rs
- Purpose: BaseTTS trait, TTSConfig, AudioData, callbacks, errors.
- Architecture integration: shared across all TTS providers.
- Notes/issues:
  - `prefer_compressed` exists in config; many providers ignore `effective_audio_format`.
  - `effective_audio_format` returns provider-specific strings (e.g., `pcm_{rate}`) that may not map cleanly to each provider's API.

## src/core/tts/provider.rs
- Purpose: generic HTTP-based TTSProvider with queue worker, dispatcher, caching, and pronunciation replacement.
- Architecture integration: used by many HTTP TTS providers.
- Notes/issues:
  - Cache key uses text hash of original text (not post-pronunciation); pronunciations/emotion not in config hash, so caching can serve stale audio.
  - `set_tts_config_hash` is only called by some providers (Deepgram/OpenAI/Azure/Cartesia/Hume); others may not cache.

## src/core/tts/deepgram.rs
- Purpose: Deepgram REST TTS implementation.
- Architecture integration: HTTP TTSProvider with optional caching.
- Notes/issues: speaking_rate is unused; config hash omits pronunciations/emotion.

## src/core/tts/elevenlabs.rs
- Purpose: ElevenLabs REST TTS implementation with voice settings.
- Architecture integration: HTTP TTSProvider.
- Notes/issues:
  - Pronunciation replacements not applied (no PronunciationReplacer).
  - No config hash set (caching unused).

## src/core/tts/openai/config.rs, src/core/tts/openai/provider.rs
- Purpose: OpenAI TTS config enums and provider.
- Architecture integration: HTTP TTSProvider with caching.
- Notes/issues: config hash omits pronunciations/emotion; sample_rate included though OpenAI always 24k.

## src/core/tts/azure/config.rs, src/core/tts/azure/provider.rs
- Purpose: Azure TTS config, SSML builder, provider.
- Architecture integration: HTTP TTSProvider with caching.
- Notes/issues:
  - `use_ssml=false` sends raw text with SSML content-type (may be invalid).
  - Config hash omits pronunciations/emotion.

## src/core/tts/cartesia/config.rs, src/core/tts/cartesia/provider.rs
- Purpose: Cartesia TTS config and provider.
- Architecture integration: HTTP TTSProvider with caching.
- Notes/issues:
  - Language is hardcoded to "en" in request builder; no config for language selection.
  - Config hash omits pronunciations/emotion.

## src/core/tts/gnani/config.rs, src/core/tts/gnani/provider.rs
- Purpose: Gnani REST TTS implementation (Indian languages).
- Architecture integration: BaseTTS direct client (not TTSProvider).
- Notes/issues:
  - Credentials are pulled only from env in `from_base`; YAML/config fields are ignored.
  - on_audio/remove_audio_callback are async spawned; callback may not be set before `speak` (race).
  - `flush` only calls on_complete; `speak` only calls on_complete if `flush=true`, so completion may be missed.
  - Audio format is hardcoded pcm16; base audio_format ignored.

## src/core/tts/google/config.rs, src/core/tts/google/provider.rs
- Purpose: Google Cloud TTS config and provider (OAuth2).
- Architecture integration: BaseTTS direct client with internal caching.
- Notes/issues:
  - Google-specific fields (pitch/volume/effects) are not exposed via base TTSConfig path.
  - Config hash omits pronunciation/emotion but text hash uses processed text (post-pronunciation).
  - Requires project_id extraction from credentials; ADC without project_id may fail.

## src/core/tts/hume/config.rs, src/core/tts/hume/messages.rs, src/core/tts/hume/provider.rs
- Purpose: Hume Octave TTS config, request messages, provider.
- Architecture integration: HTTP TTSProvider with caching and emotion control.
- Notes/issues:
  - `voice_description` exists in config but is never used in request body.
  - Config hash omits pronunciations/emotion.

## src/core/tts/aws_polly/config.rs, src/core/tts/aws_polly/provider.rs
- Purpose: AWS Polly TTS config and provider (AWS SDK).
- Architecture integration: BaseTTS direct client.
- Notes/issues:
  - `MAX_TOTAL_LENGTH` (SSML) is defined but not enforced; hard limit uses 3000 chars for all.
  - Pronunciation replacements from base config are not applied; relies on Polly lexicons.

## src/core/tts/ibm_watson/config.rs, src/core/tts/ibm_watson/provider.rs
- Purpose: IBM Watson TTS config and provider (IAM auth).
- Architecture integration: BaseTTS direct client.
- Notes/issues: base pronunciations are not applied; custom dictionaries require IBM-specific config.

## src/core/tts/lmnt/config.rs, src/core/tts/lmnt/provider.rs
- Purpose: LMNT TTS config and provider (HTTP streaming).
- Architecture integration: HTTP TTSProvider with caching.
- Notes/issues:
  - `validate_text` uses byte length (`text.len()`), so non-ASCII text may be rejected earlier than intended.
  - Voice cloning is referenced in docs but not exposed via provider methods; requires external API use.

## src/core/tts/lmnt/messages.rs
- Purpose: LMNT voice/voice-clone and request/response message types.
- Architecture integration: used by LMNT provider and potential future API helpers.
- Notes/issues: no direct integration with voice clone endpoints.

## src/core/tts/playht/config.rs, src/core/tts/playht/provider.rs, src/core/tts/playht/messages.rs
- Purpose: Play.ht TTS config, request builder/provider, and API message types.
- Architecture integration: HTTP TTSProvider with caching; requires `PLAYHT_USER_ID`.
- Notes/issues:
  - User ID is only provided via env or explicit `with_user_id`; base TTSConfig has no field for it.
  - Config hash omits pronunciations/emotion (same caching caveat as other providers).

## src/core/tts/azure/config.rs, src/core/tts/azure/provider.rs, src/core/tts/azure/mod.rs
- Purpose: Azure TTS config + SSML builder + provider wrapper over generic TTSProvider.
- Architecture integration: HTTP REST synthesis via `TTSProvider` with Azure headers; uses `AzureRegion` and output format mapping.
- Notes/issues:
  - `language_code()` derives only from `voice_id`; if `voice_id` is empty but `model` is used for voice, SSML `xml:lang` defaults to `en-US`, which can mismatch the voice.
  - Speaking rate in SSML is not clamped to Azure’s documented range; extreme values could generate invalid SSML.
  - Config hash ignores pronunciations (and `use_ssml`), so caching can reuse audio across different pronunciation rules.

## src/core/tts/gnani/config.rs, src/core/tts/gnani/provider.rs, src/core/tts/gnani/mod.rs
- Purpose: Gnani TTS config and HTTP provider for Indic languages with gender/voice selection.
- Architecture integration: standalone REST client implementing `BaseTTS` without shared `TTSProvider`/cache.
- Notes/issues:
  - Credentials come only from `GNANI_TOKEN`/`GNANI_ACCESS_KEY`; `TTSConfig.api_key` is unused (can confuse callers).
  - `audio_format` is ignored; requests always set `pcm16` and `sample_rate` from config.
  - Language derives from `voice_id` and gender from `model` string; nonstandard inputs can silently fall back to defaults.

## src/core/tts/google/config.rs, src/core/tts/google/provider.rs, src/core/tts/google/mod.rs
- Purpose: Google Cloud TTS config, auth wrapper, request builder, and REST provider with caching.
- Architecture integration: uses `core/providers/google` auth and `TTSProvider`-style caching + ReqManager.
- Notes/issues:
  - `language_code` is derived only from `voice_id`; if only `model` is set, language defaults to `en-US`, which may mismatch voice.
  - Config hash excludes `pitch`, `volume_gain_db`, `effects_profile_id`, and pronunciations; cache can return audio for mismatched settings.
  - When `sample_rate` is unset, chunking/duration uses 24kHz even if Google returns a different default for the selected voice.

## src/core/tts/openai/config.rs, src/core/tts/openai/provider.rs, src/core/tts/openai/mod.rs
- Purpose: OpenAI TTS config enums and REST provider over generic `TTSProvider`.
- Architecture integration: `TTSProvider` handles HTTP and audio callbacks; provider builds OpenAI JSON requests.
- Notes/issues:
  - If `audio_format` is unset, provider defaults to PCM output, but config hash defaults to `"mp3"`; cache keys can mismatch actual response format.
  - Config hash ignores pronunciations and uses `sample_rate` even though OpenAI output is fixed 24kHz; cache key may differ without changing output.

## src/handlers/ws/processor.rs
- Purpose: WS message router/orchestrator (auth, config, speak, custom, etc.).
- Architecture integration: entrypoint for WS message processing; uses plugin handlers.
- Notes/issues: first-message auth only supports API secrets; JWT path not supported for WS.

## src/handlers/ws/error.rs
- Purpose: WS error types.
- Architecture integration: used by WS handlers and message routing.
- Notes/issues: none.

## src/core/stt/cartesia/config.rs, src/core/stt/cartesia/messages.rs, src/core/stt/cartesia/client.rs, src/core/stt/cartesia/mod.rs, src/core/stt/cartesia/tests.rs
- Purpose: Cartesia WebSocket STT (ink-whisper) config, message types, client, and tests.
- Architecture integration: `BaseSTT` implementation; registered via plugin registry and re-exported in `src/core/stt/mod.rs`.
- Notes/issues:
  - `CartesiaSTT::finalize()` is a stub (warns and returns Ok). Manual finalize/flush isn’t implemented even though API supports finalize.
  - In `start_connection`, initial connection error path uses `error_tx_for_task.send(stt_error)` without awaiting; error likely never sent (future dropped).
  - Idle timeout is inbound-only; if Cartesia sends no messages during long silence, connection may drop after 60s.

## src/core/stt/deepgram.rs
- Purpose: Deepgram WebSocket STT implementation, config, message parsing, keep-alive.
- Architecture integration: `BaseSTT` implementation; uses WS with keep-alive timer.
- Notes/issues:
  - `DeepgramSTTConfig` includes `diarize`, `filler_words`, `profanity_filter`, `redact`, `vad_events`, `utterance_end_ms`, but `build_websocket_url()` never sends these fields; config options are effectively ignored.
  - Keep-alive only sends client messages; `WS_MESSAGE_TIMEOUT` depends on inbound server messages. If Deepgram doesn’t emit responses during silence, connection can still time out.

## src/core/stt/elevenlabs/config.rs, src/core/stt/elevenlabs/messages.rs, src/core/stt/elevenlabs/client.rs, src/core/stt/elevenlabs/mod.rs, src/core/stt/elevenlabs/tests.rs
- Purpose: ElevenLabs real-time WebSocket STT integration (config, messages, client, tests).
- Architecture integration: `BaseSTT` implementation; used by WS handlers via plugin registry.
- Notes/issues:
  - Session ID from `session_started` is never stored in `self.session_id`; `get_session_id()` always returns `None`.
  - `CommitStrategy::Manual` is exposed but there is no API to send commit chunks; `send_audio()` always sends without `commit`, so manual commit is unsupported.
  - `ElevenLabsAudioFormat::Ulaw8000` exists but `from_sample_rate()` never selects it, and no public API lets callers force ulaw; config defaults always PCM.
  - Client `build_websocket_url()` strips language region (e.g., `en-US` -> `en`), while `ElevenLabsSTTConfig::build_websocket_url()` keeps full code; mismatch could cause locale issues.
  - No keep-alive; inbound idle timeout is 60s, so long silence may drop the connection if server emits no messages.

## src/core/stt/gnani/config.rs, src/core/stt/gnani/messages.rs, src/core/stt/gnani/grpc.rs, src/core/stt/gnani/client.rs, src/core/stt/gnani/mod.rs
- Purpose: Gnani gRPC streaming STT (config, manual protobuf encode/decode, gRPC client, `BaseSTT` wrapper).
- Architecture integration: `BaseSTT` implementation using tonic; credentials from env (token/access_key/cert).
- Notes/issues:
  - `TranscriptChunk::decode()` skips unknown length-delimited fields without bounds checks, so malformed data could advance `pos` past buffer end silently.
  - No use of `base.api_key`; credentials are entirely separate (token/access_key), which may be surprising to callers.

## src/core/stt/google/config.rs, src/core/stt/google/streaming.rs, src/core/stt/google/provider.rs, src/core/stt/google/mod.rs, src/core/stt/google/tests/*
- Purpose: Google Speech-to-Text v2 gRPC streaming client with keep-alive silence, config mapping, and tests.
- Architecture integration: Uses `core/providers/google` auth/token plumbing; `BaseSTT` implementation with gRPC channel per session.
- Notes/issues: no obvious functional issues in review; keep-alive and chunking look consistent.

## src/core/stt/groq/config.rs, src/core/stt/groq/messages.rs, src/core/stt/groq/client.rs, src/core/stt/groq/mod.rs, src/core/stt/groq/tests.rs
- Purpose: Groq Whisper STT (REST) with buffering, retry, silence detection, WAV packaging, and tests.
- Architecture integration: `BaseSTT` implementation; batching strategy for non-streaming API.
- Notes/issues:
  - `audio_input_format` suggests multiple formats, but client always builds WAV and only changes MIME/extension; non-WAV formats are not actually encoded.
  - `estimated_cost()` applies minimum billed duration even when buffer is empty, so it returns non-zero cost for zero audio.
  - `flush_buffer()` clears audio bytes but does not reset silence-detection state; subsequent silence detection can be skewed.
  - Silence detection assumes PCM16 input; no resampling/encoding conversion is applied.
  - Safety cap `MAX_BUFFER_SIZE_BYTES` (20MB) is below config max file size; dev tier 100MB never reachable without refactor.

## src/core/stt/ibm_watson/config.rs, src/core/stt/ibm_watson/messages.rs, src/core/stt/ibm_watson/client.rs, src/core/stt/ibm_watson/mod.rs, src/core/stt/ibm_watson/tests.rs
- Purpose: IBM Watson WebSocket STT with IAM auth, start/stop messaging, and rich config.
- Architecture integration: `BaseSTT` implementation; IAM token fetched at connect time.
- Notes/issues:
  - `IbmWatsonMessage` uses `#[serde(untagged)]` with `ListeningMessage` and `StateMessage` sharing the same shape; any `{ "state": ... }` parses as `Listening`, so non-listening state messages can incorrectly signal readiness.
  - IAM token is only fetched on connect; no refresh during long-running sessions, so long sessions may expire.
  - `WS_MESSAGE_TIMEOUT` uses inbound messages only; if IBM sends no messages during silence, the connection can time out despite keep-alive audio.

## src/core/stt/openai/config.rs, src/core/stt/openai/messages.rs, src/core/stt/openai/client.rs, src/core/stt/openai/mod.rs, src/core/stt/openai/tests.rs
- Purpose: OpenAI Whisper STT (REST) with buffering, silence detection, WAV packaging, and tests.
- Architecture integration: `BaseSTT` implementation; batching strategy for non-streaming API.
- Notes/issues:
  - `audio_input_format` supports multiple formats, but client always builds WAV and uses a fixed `audio.wav` filename; non-WAV formats are not actually encoded.
  - `flush_buffer()` clears audio bytes but does not reset silence-detection state; subsequent detection can be skewed after manual/threshold flushes.
  - Silence detection assumes PCM16 input; no resampling/encoding conversion is applied.
  - Safety cap `MAX_BUFFER_SIZE_BYTES` (20MB) below config max (25MB) means you cannot buffer up to OpenAI’s limit.

## src/core/stt/mod.rs
- Purpose: STT module registry/re-exports, provider enum, and factory helpers.
- Architecture integration: provider selection via plugin registry; enum + list used for supported provider discovery.
- Notes/issues:
  - `STTProvider` enum and `get_supported_stt_providers()` omit `gnani` despite having a Gnani implementation; enum and supported list are incomplete.

## src/core/turn_detect/config.rs, src/core/turn_detect/tokenizer.rs, src/core/turn_detect/assets.rs, src/core/turn_detect/detector.rs, src/core/turn_detect/model_manager.rs, src/core/turn_detect/stub.rs, src/core/turn_detect/mod.rs
- Purpose: turn detection (end-of-utterance) model config, asset download/verification, tokenizer, ONNX model inference, and stub fallback.
- Architecture integration: `CoreState::initialize_turn_detector` wires this into `VoiceManager` speech-final logic when `turn-detect` feature enabled.
- Notes/issues:
  - `assets.rs` uses `get_expected_hash()` placeholder `"expected_hash_here"`; integrity check is effectively a stub (warns but doesn’t validate).
  - `TurnDetectorConfig::get_cache_dir` requires `cache_path`; if `ServerConfig.cache_path` is None, init fails and falls back to timer-based.
  - `model_manager.rs` assumes batch size 1 when decoding outputs; ignores additional batches and assumes output ordering is `[1, 0]`.
  - EOS token id is hardcoded to `2`; if the tokenizer/model uses different EOS, probabilities are wrong.

## src/core/voice_manager/config.rs, src/core/voice_manager/errors.rs, src/core/voice_manager/stt_result.rs, src/core/voice_manager/state.rs, src/core/voice_manager/callbacks.rs, src/core/voice_manager/manager.rs, src/core/voice_manager/tests.rs, src/core/voice_manager/mod.rs
- Purpose: core STT/TTS orchestration, buffering, interrupt logic, non-interruptible playback, and turn-final detection.
- Architecture integration: used by WS handler to drive audio in/out; uses STT providers and TTS providers; optionally turn-detect.
- Notes/issues:
  - `stt_result.rs`: `fire_speech_final` sends an empty transcript to callbacks; it stores forced text in `last_forced_text` but never emits it (forced final events are empty).
  - `handle_turn_detection` concatenates `text_buffer` without spacing; words may run together.
  - Default `SpeechFinalConfig` (1800/500/4000) differs from `STTProcessingConfig::default` (2000/100/5000); VoiceManager uses SpeechFinalConfig, but mismatch can confuse config expectations.
  - `on_tts_audio` assumes PCM16 mono and uses configured sample rate to estimate non-interruptible duration even for compressed formats; duration locks can be wrong for mp3/opus/wav.

## src/core/providers/google/auth.rs, src/core/providers/google/client.rs, src/core/providers/google/error.rs, src/core/providers/google/mod.rs
- Purpose: shared Google OAuth2/ADC auth and gRPC channel plumbing for Google STT/TTS.
- Architecture integration: used by `core/stt/google` and `core/tts/google`.
- Notes/issues: ADC without `project_id` in credentials may fail; auth errors are bubbled, but per-request caching is minimal.

## src/core/providers/azure/auth.rs, src/core/providers/azure/region.rs, src/core/providers/azure/mod.rs
- Purpose: shared Azure auth header helpers and region mapping for STT/TTS.
- Architecture integration: used by Azure STT/TTS clients.
- Notes/issues: none beyond per-provider usage gaps.

## src/core/cache/mod.rs, src/core/cache/store.rs
- Purpose: unified cache abstraction with in-memory or filesystem backing; TTL enforcement; used for TTS caching and SIP hooks persistence.
- Architecture integration: `CoreState` creates CacheStore; used across providers and SIP hooks.
- Notes/issues: none obvious; filesystem cache required for SIP hooks and turn-detect assets.

## src/core/emotion/types.rs, src/core/emotion/mapper.rs, src/core/emotion/mod.rs, src/core/emotion/mappers/*
- Purpose: emotion configuration and mapping to provider-specific fields (Azure, ElevenLabs, Hume).
- Architecture integration: used by TTS providers supporting emotion/stability/expressiveness.
- Notes/issues:
  - Mapper coverage is partial; some providers ignore emotion config entirely.
  - Mappers use string matching for delivery style/intensity; may drift from provider enums.

## src/core/realtime/base.rs, src/core/realtime/openai/*, src/core/realtime/hume/*
- Purpose: realtime audio-to-audio abstractions and provider implementations (OpenAI Realtime, Hume EVI).
- Architecture integration: used by `/realtime` handler and DAG realtime node.
- Notes/issues:
  - OpenAI `build_session_config()` ignores `RealtimeConfig.modalities` and `output_audio_format`, always sets `["text","audio"]` and mirrors input format.
  - OpenAI `AudioDelta` events hardcode `sample_rate` 24000 even for g711 formats.
  - OpenAI ignores `response.text.delta`/`response.text.done`; text-only modality yields no transcripts.
  - Hume realtime uses `HUME_EVI_DEFAULT_SAMPLE_RATE` for audio output even if config requests a different sample_rate; reconnection config exists but no reconnection logic is implemented.

## src/core/state.rs
- Purpose: core shared state (cache store, per-provider ReqManager pool, turn detector init, SIP hooks runtime state).
- Architecture integration: constructed by `AppState::new`, used by handlers and providers.
- Notes/issues:
  - Memory cache defaults are large (5,000,000 entries, 500MB) even when no cache_path provided.
  - Turn-detect initialization relies on cache_path (see turn_detect notes); missing cache_path forces fallback.

## src/errors/app_error.rs, src/errors/auth_error.rs, src/errors/mod.rs
- Purpose: common application/auth error responses and HTTP status mapping.
- Architecture integration: used by middleware/handlers for JSON error responses.
- Notes/issues: AuthError accepts 200 responses with invalid JSON as Auth::empty (see auth client) which can break tenant isolation.

## src/routes/api.rs, src/routes/ws.rs, src/routes/realtime.rs, src/routes/webhooks.rs, src/routes/mod.rs
- Purpose: HTTP/WS route wiring and trace middleware.
- Architecture integration: merged in main router with auth middleware and connection limits.
- Notes/issues: none; WS router explicitly uses auth middleware for tenant isolation (docs claiming “WS unauthenticated” are outdated).

## src/handlers/api.rs
- Purpose: health check endpoint.
- Architecture integration: used in root route.
- Notes/issues: none.

## src/handlers/recording.rs
- Purpose: recording download from S3-compatible object store with tenant-scoped key prefix.
- Architecture integration: `/recording/{stream_id}` route; uses `object_store` in AppState.
- Notes/issues:
  - If JWT auth returns no `id`, tenant isolation is bypassed (fallback to legacy path), even when auth is required.
  - Requires S3 credentials; no support for IAM role/ambient credentials.

## src/handlers/speak.rs
- Purpose: REST `/speak` endpoint; synthesizes TTS and returns binary audio.
- Architecture integration: creates TTS provider via plugin registry; uses CoreState ReqManager pool.
- Notes/issues:
  - Applies pronunciation replacements manually, then `TTSProvider` may apply them again (double replacement).
  - Content-Type mapping lacks `pcm16`/`pcm_XXXX` cases; formats returned by providers may be `application/octet-stream`.

## src/handlers/voices.rs
- Purpose: `/voices` list + `/voices/clone` voice cloning for ElevenLabs/Hume/LMNT.
- Architecture integration: uses provider APIs directly and caches voice list (global static).
- Notes/issues:
  - Voice clone endpoints accept arbitrary base64 size; no server-side size limit (memory DoS risk).
  - Hume voice clone rejects `audio_samples` entirely; docs may imply audio-based cloning is supported.
  - LMNT voices are labeled `language="English"` even though LMNT supports multiple languages; inaccurate metadata.
  - `detect_audio_format` defaults unknown content to WAV, which can mislabel uploads.

## src/handlers/livekit/token.rs, src/handlers/livekit/rooms.rs, src/handlers/livekit/participants.rs, src/handlers/livekit/webhook.rs, src/handlers/livekit/mod.rs
- Purpose: LiveKit REST endpoints (token, rooms, participants) + webhook handling + SIP forwarding.
- Architecture integration: relies on LiveKit handlers and SIP hooks state; tenant isolation via `Auth::normalize_room_name`.
- Notes/issues:
  - If Auth has no `id` (JWT auth returns empty), room isolation is bypassed; list/get/remove/mute operate on global rooms.
  - `get_room_details` decides 404 by string-matching "not found" in error text (brittle).
  - SIP forwarding uses `tts_req_managers` cache for webhook HTTP pools; map can grow unbounded per host.
  - `parse_sip_domain` doesn’t support IPv6 host formats.

## src/middleware/auth.rs
- Purpose: auth middleware for REST/WS; supports API secret or external JWT auth service.
- Architecture integration: applied in main router; WS uses pending auth for first-message auth.
- Notes/issues:
  - First-message auth only supports API secrets; JWT auth cannot be completed via WS message.

## src/middleware/connection_limit.rs, src/middleware/mod.rs
- Purpose: global/per-IP connection limits for WebSocket upgrades.
- Architecture integration: applied to `/ws` and `/realtime` (from main).
- Notes/issues:
  - `try_acquire_connection` uses non-atomic check + increment; concurrent accepts can exceed limits.

## src/auth/context.rs, src/auth/api_secret.rs, src/auth/jwt.rs, src/auth/client.rs, src/auth/mod.rs
- Purpose: auth context, API secret matching, JWT signing, and external auth service client.
- Architecture integration: middleware uses these to validate requests and set tenant ID.
- Notes/issues:
  - `AuthClient` accepts HTTP 200 responses with invalid JSON as `Auth::empty`, which can bypass tenant scoping.
  - `normalize_room_name` prefixes empty room names to `auth_` (edge case if clients send empty room).

## src/state/mod.rs, src/state/sip_hooks_state.rs
- Purpose: AppState (config + livekit + cache + auth + ws connection tracking) and SIP hooks runtime state.
- Architecture integration: constructed in main; used by handlers and core modules.
- Notes/issues:
  - Object store for recordings requires explicit access key/secret; no IAM/ambient credentials.
  - `try_acquire_connection` is not atomic; limits can be exceeded under concurrency.
  - SIP provisioning panics on failure (hard fail if LiveKit API unreachable).

## src/docs/mod.rs, src/docs/openapi.rs
- Purpose: OpenAPI generation (feature-gated) and spec metadata.
- Architecture integration: CLI uses for exporting `docs/openapi.yaml`.
- Notes/issues: OpenAPI spec version hardcoded to `0.1.0` (may be out of sync with crate version).

## src/core/providers/mod.rs, src/core/realtime/mod.rs, src/core/mod.rs
- Purpose: module hubs and re-exports for provider utilities, realtime providers, and core types.
- Architecture integration: registry factory functions and type re-exports for handlers/tests.
- Notes/issues: none beyond provider-specific gaps noted elsewhere.

## src/dag/mod.rs, src/dag/error.rs, src/dag/definition.rs, src/dag/compiler.rs, src/dag/executor.rs, src/dag/context.rs, src/dag/routing.rs, src/dag/metrics.rs, src/dag/templates.rs
- Purpose: DAG routing system (definitions, compiler, executor, routing, templates, metrics).
- Architecture integration: feature-gated per-connection DAG pipelines for WS audio routing.
- Notes/issues:
  - `NodeDefinition.config`, `timeout_ms`, `retry_on_failure`, and `max_retries` are not applied in `DAGCompiler`/`DAGExecutor`.
  - `DAGConfig.variables` is never injected into Rhai scopes; global DAG variables are unused.
  - `DAGExecutor` ignores `max_concurrent_branches`; branch concurrency is unbounded.
  - `DAGExecutor` ignores edge priority; all matching edges are passed through equally.
  - `default_timeout` is not enforced unless callers set `DAGContext.deadline`.
  - `templates` registry loads DAGs without validation; invalid DAGs are only caught at compile time.
  - `DAGContext.node_results` are stored but never exposed to routing/transform expressions.

## src/dag/edges/mod.rs, src/dag/edges/condition.rs, src/dag/edges/switch.rs, src/dag/edges/buffer.rs
- Purpose: edge routing conditions, switch matching, and ring-buffer helpers.
- Architecture integration: edge conditions used by executor; buffers are not wired into executor.
- Notes/issues:
  - `EdgeDefinition.buffer_capacity` and `EdgeBuffer` are unused in execution; buffer sizing is ignored.
  - `EdgeBuffer.pop()` uses `Vec::remove(0)` (O(n) per pop), not suitable for high-throughput.
  - `RtrbAudioBuffer` uses `Mutex` around producer/consumer, so it is not truly wait-free as docs claim.

## src/dag/nodes/mod.rs, src/dag/nodes/input.rs, src/dag/nodes/output.rs, src/dag/nodes/provider.rs, src/dag/nodes/processor.rs, src/dag/nodes/endpoint.rs, src/dag/nodes/router.rs, src/dag/nodes/transform.rs
- Purpose: DAG node implementations (I/O, providers, processors, endpoints, routing, transforms).
- Architecture integration: compiled and executed by DAG compiler/executor; connects to STT/TTS/realtime and external endpoints.
- Notes/issues:
  - `AudioInputNode` expected sample rate/format fields are never validated.
  - `TextInputNode` truncates by byte index; can panic on non-ASCII or cut UTF-8 mid-sequence.
  - Output nodes only set metadata (`output_destination`/`message_type`) and do not actually route output; executor doesn’t consume these.
  - Provider nodes ignore `NodeDefinition.config` and do not pass sample_rate/encoding; rely on provider defaults.
  - Provider nodes do not inject API keys from context; realtime/stt/tts nodes may fail unless env-based keys exist.
  - TTS/STT nodes set `model` to empty string when unspecified, overriding provider defaults with invalid model.
  - STT node sends audio once and never calls finalize; streaming providers that require explicit finalize may not return final results.
  - Realtime node hardcodes `sample_rate=24000` and stops after first transcript/audio; can truncate audio or time out when transcript is absent.
  - Processor node text processing is not implemented (logs and passes through).
  - HTTP/gRPC/WS/Webhook nodes do not enforce SSRF validation; DAG definitions can call internal services.
  - gRPC node auto-enables TLS for non-`localhost` addresses, so IP-based endpoints default to TLS and may fail.
  - LiveKit endpoint node ignores `room` setting; `track_type` only affects binary routing.
  - Router/join/merge scripts expose `api_key` and `api_key_id` to Rhai (potential secret leakage if DAG is untrusted).

## src/dag/endpoints/mod.rs, src/dag/endpoints/http.rs, src/dag/endpoints/webhook.rs
- Purpose: legacy endpoint adapters (HTTP, webhook).
- Architecture integration: used by older DAG endpoint adapter path.
- Notes/issues: no SSRF validation; adapters forward API keys/ids to arbitrary URLs.

## src/core/stt/azure/config.rs, src/core/stt/azure/messages.rs, src/core/stt/azure/mod.rs
- Purpose: Azure STT config, message parsing, and module re-exports.
- Architecture integration: used by `AzureSTT` client for WS streaming.
- Notes/issues:
  - `build_websocket_url` always includes `language` even when `auto_detect_languages` is set; Azure expects either language or auto-detect, not both.
  - `word_level_timing` is never added to the query params, so word timings cannot be enabled even when configured.

## src/core/stt/aws_transcribe/messages.rs
- Purpose: response structs and helpers for Amazon Transcribe streaming results.
- Architecture integration: used by AwsTranscribeSTT client for parsing.
- Notes/issues: none obvious; confidence is derived from word-level averages when present.

## src/core/stt/aws_transcribe/tests.rs
- Purpose: unit tests for AWS Transcribe config and message parsing helpers.
- Architecture integration: validates config invariants and helper logic.
- Notes/issues: tests validate `max_speaker_labels` requirement, but client doesn’t send this field (runtime gap noted earlier).

## src/core/stt/aws_transcribe/mod.rs
- Purpose: module docs + re-exports for AWS Transcribe STT.
- Architecture integration: exposes config/constants/client to registry.
- Notes/issues: docs list features (diarization, redaction, vocab filters) that client doesn’t fully implement (see client notes).

## src/core/stt/aws_transcribe/client.rs, src/core/stt/aws_transcribe/config.rs
- Purpose: Amazon Transcribe streaming client and configuration.
- Architecture integration: used when provider is "aws-transcribe".
- Notes/issues:
  - Many config fields are never applied to the AWS request: `max_speaker_labels`, `enable_channel_identification`/`number_of_channels`, `vocabulary_filter_method`, `language_model_name`, `preferred_language`, `enable_content_redaction`, `content_redaction_types`, `pii_entity_types`, and `chunk_duration_ms` (no chunking based on it).
  - `show_speaker_label` is sent without `max_speaker_labels`, which is required by AWS; diarization config may be rejected.
  - `identify_language` ignores `preferred_language` list; only the boolean is sent.
  - `convert_language_code` defaults unknown languages to `en-US`, which can silently mis-route unsupported-but-valid AWS language codes.

## src/core/stt/assemblyai/tests.rs
- Purpose: unit tests for AssemblyAI config and message parsing.
- Architecture integration: verification only; no runtime behavior.
- Notes/issues: none.

## src/livekit/client/audio.rs
- Purpose: audio publishing, TTS audio queuing, frame conversion, reconnect audio re-publish.
- Architecture integration: used by `LiveKitClient` to publish TTS audio and reconnect after keep-alive failures.
- Notes/issues:
  - `process_send_audio` drains queued frames only when queue length <= 5; if backlog > 5, it drains nothing, so queue can grow without being reduced (latency never recovers).
  - `process_reconnect` repopulates audio_source/publication but never sets `has_audio_source_atomic` or `local_audio_track`; `LiveKitClient::has_audio_source()` can remain false after reconnect.

## src/livekit/client/events.rs
- Purpose: LiveKit room event handler for tracks/data/participants.
- Architecture integration: spawns per-track processing tasks and forwards audio/data via callbacks.
- Notes/issues:
  - When noise-filter is disabled, buffers from `convert_frame_to_audio` are never returned to the pool (pool drains after a few frames, leading to continuous allocations).

## src/livekit/operations.rs
- Purpose: queued LiveKit operations + priority metadata + stats.
- Architecture integration: used by `LiveKitClient` operation worker.
- Notes/issues:
  - Operation priority is defined but not used to reorder the queue (all ops FIFO); retry_count fields are unused.
  - `OperationQueue::pending_count()` uses `capacity() - max_capacity()`, which underflows after enqueues; pending count is incorrect once the queue has items.

## src/livekit/manager.rs
- Purpose: high-level LiveKit connection manager with audio forwarding.
- Architecture integration: intended to abstract LiveKit client for other components.
- Notes/issues:
  - `set_audio_callback` is a placeholder (warns, doesn’t actually register subscriber); feature is effectively unimplemented.
  - `participants` and `room_info` are never updated anywhere; getters always return empty/default, so manager APIs are incomplete.

## src/handlers/realtime/handler.rs
- Purpose: realtime WebSocket for audio-to-audio providers (OpenAI, Hume).
- Architecture integration: wraps `core/realtime` providers with WS protocol handling.
- Notes/issues:
  - Idle-timeout jitter uses `Instant::now().elapsed()` (near-zero), so jitter is effectively 0; all connections time out simultaneously (same issue as `/ws` handler).

## src/handlers/voices.rs
- Purpose: `/voices` listing + `/voices/clone` voice cloning for ElevenLabs/Hume/LMNT.
- Architecture integration: calls provider APIs directly; uses ServerConfig API keys, cached responses.
- Notes/issues:
  - Docs in `VoiceCloneRequest` and handler comments claim Hume supports audio-based cloning, but `clone_voice_hume` explicitly rejects `audio_samples`; documentation and schema comments are misleading.

## src/core/realtime/hume/client.rs
- Purpose: Hume EVI realtime WebSocket client implementing `BaseRealtime`.
- Architecture integration: used by `/realtime` handler when provider is "hume".
- Notes/issues:
  - Reconnection config is parsed but not used; no reconnection logic despite docs claiming it.
  - Output audio sample rate is hardcoded to `HUME_EVI_DEFAULT_SAMPLE_RATE` (44100) even if config overrides input sample_rate.
  - `connect_internal` only sends `SessionSettings` when encoding/sample_rate/system_prompt differ; if only `channels` changes, settings are never sent (channels override ignored).

## src/core/tts/base.rs
- Purpose: core TTS trait, config, and audio callback types.
- Architecture integration: base interface for all TTS providers and config schema used across REST/WS handlers.
- Notes/issues:
  - `TTSConfig::effective_audio_format` docs say explicit `audio_format` always wins, but code treats `"linear16"` as non-explicit and will override it when `prefer_compressed=true`.

## src/core/tts/provider.rs
- Purpose: generic HTTP TTS provider (queue, dispatcher, caching, chunking).
- Architecture integration: used by most HTTP-based TTS providers for shared behavior.
- Notes/issues:
  - PCM detection only recognizes exact strings (`linear16`, `pcm`, `mulaw`, `ulaw`, `alaw`); provider-specific PCM formats like `pcm_24000` or Azure `raw-24khz-16bit-mono-pcm` are treated as non-PCM, so chunking/duration are wrong.
  - Uses base `config.audio_format` (not provider-resolved format) for `encoding` and `AudioData.format`; if provider chooses a different output format (e.g., LMNT/PlayHT default mp3 when `audio_format` is None), chunking and `format` metadata can be inconsistent.

## src/core/tts/openai/provider.rs
- Purpose: OpenAI TTS provider using generic HTTP infrastructure.
- Architecture integration: used by `/speak` and WS config `tts_provider=openai`.
- Notes/issues:
  - Config hash defaults `audio_format` to `"mp3"` even though provider defaults response format to PCM when `audio_format` is None; cache keys can mismatch actual output format.

## src/core/tts/hume/provider.rs
- Purpose: Hume Octave TTS provider using generic HTTP infrastructure.
- Architecture integration: used by `/speak` and WS config `tts_provider=hume`.
- Notes/issues:
  - Always builds `HumeVoiceSpec::ByName`; custom voice IDs will be sent as `name` instead of `id`, likely breaking cloned voice usage.
  - `voice_description` in `HumeTTSConfig` is never sent in requests (unused).
  - Config hash ignores `generation_id`, `trailing_silence`, and `num_generations`, so cache keys can collide across different output settings.
  - `HumeRequestBuilder.previous_text` field is unused; context only comes from `build_http_request_with_context`.

## src/core/tts/cartesia/provider.rs
- Purpose: Cartesia TTS provider using generic HTTP infrastructure.
- Architecture integration: used by `/speak` and WS config `tts_provider=cartesia`.
- Notes/issues:
  - Request `language` is hardcoded to `"en"` with no config input; non-English voices likely mis-specified.

## src/core/tts/gnani/provider.rs
- Purpose: Gnani.ai TTS provider (custom HTTP client, synchronous).
- Architecture integration: used by `/speak` and WS config `tts_provider=gnani`.
- Notes/issues:
  - `on_audio` and `remove_audio_callback` update the callback via `tokio::spawn`, so callback registration is racy (speak may run before callback is set).
  - `speak` only calls `on_complete` when `flush=true`; otherwise completion is never signaled for a synthesis result.
  - Voice selection is limited: `voice_id` is parsed as language code only, and `voice_name` is never set from config (defaults to `"gnani"`).

## src/core/stt/deepgram.rs
- Purpose: Deepgram streaming STT client (WS-based).
- Architecture integration: used by `/transcribe` and realtime WS when provider is "deepgram".
- Notes/issues:
  - `DeepgramSTTConfig` exposes `filler_words`, `profanity_filter`, `redact`, `vad_events`, and `utterance_end_ms`, but `build_websocket_url` never sends these parameters, so the settings are ignored.

## src/core/stt/elevenlabs/client.rs, src/core/stt/elevenlabs/messages.rs
- Purpose: ElevenLabs realtime STT client and WS message types.
- Architecture integration: used by `/transcribe` and realtime WS when provider is "elevenlabs".
- Notes/issues:
  - `self.session_id` is never updated; session ID is tracked only inside the connection task, so `get_session_id()` always returns `None`.
  - Manual commit mode is exposed in config, but `InputAudioChunk::with_commit`/`EndOfStream` are never used; there is no API to trigger commit or send EOS, so `CommitStrategy::Manual` cannot finalize transcripts.

## src/core/stt/openai/client.rs, src/core/stt/openai/config.rs
- Purpose: OpenAI Whisper batch STT client (buffers audio, POSTs on flush).
- Architecture integration: used when provider is "openai" (non-streaming STT path).
- Notes/issues:
  - `audio_input_format` is treated as MIME only; the client always wraps PCM bytes into a WAV file and names it `audio.wav`, so non-WAV formats (mp3/webm/etc.) are not actually encoded and may be mislabeled.
  - `FlushStrategy::OnSilence` uses fixed buffer length heuristics (assumes ~16kHz mono) rather than `sample_rate`/`channels`, so silence detection thresholds can misfire for other formats.
  - PCM 16-bit little-endian is assumed throughout (RMS calc + WAV packaging) with no validation against `STTConfig.encoding`; non-PCM inputs will be mis-encoded.

## src/core/stt/google/config.rs, src/core/stt/google/streaming.rs, src/core/stt/google/provider.rs
- Purpose: Google Cloud Speech-to-Text v2 streaming client and config.
- Architecture integration: used when provider is "google" (gRPC streaming path).
- Notes/issues:
  - `single_utterance` is defined in `GoogleSTTConfig` but never applied to the streaming request; the setting is currently ignored.
  - Encoding handling is inconsistent: `GoogleSTTConfig::google_encoding` supports FLAC/Opus/etc., but `build_config_request` uses `map_encoding_to_proto` (linear16/mulaw/alaw only), so non-PCM encodings are silently mapped to LINEAR16.
  - Keep-alive silence generation assumes LINEAR16 (2 bytes/sample); if encoding is mulaw/alaw/opus, the keepalive payload size/content is incorrect.

## src/core/stt/assemblyai/config.rs, src/core/stt/assemblyai/client.rs
- Purpose: AssemblyAI streaming STT client and configuration.
- Architecture integration: used when provider is "assemblyai" (WebSocket streaming path).
- Notes/issues:
  - No validation that `pcm_mulaw` encoding is paired with 8kHz; config accepts any sample rate even though the API expects 8kHz for mu-law.
  - `UpdateConfigurationMessage` exists but the client never uses it; all config updates require reconnects even though the API supports live updates.

## src/core/stt/ibm_watson/config.rs, src/core/stt/ibm_watson/client.rs
- Purpose: IBM Watson STT WebSocket client and configuration.
- Architecture integration: used when provider is "ibm-watson".
- Notes/issues:
  - Linear16 content-type hardcodes `channels=1` and ignores `base.channels`, so multi-channel audio would be mis-declared.
  - Keep-alive silence frame is hardcoded for 16kHz PCM (64 bytes); if sample rate or encoding differs (mulaw/flac), the keepalive payload is invalid.

## src/core/stt/groq/client.rs, src/core/stt/groq/config.rs
- Purpose: Groq Whisper batch STT client (buffers audio, POSTs on flush).
- Architecture integration: used when provider is "groq" (non-streaming STT path).
- Notes/issues:
  - `audio_input_format` is treated as MIME/extension only; the client always wraps PCM into WAV bytes, so non-WAV formats are mislabeled and not actually encoded.
  - Silence detection uses fixed buffer size heuristics (assumes ~16kHz mono) and RMS on PCM16; not adjusted for `sample_rate`/`channels` or non-PCM encodings.

## src/core/stt/cartesia/client.rs
- Purpose: Cartesia STT WebSocket streaming client.
- Architecture integration: used when provider is "cartesia".
- Notes/issues:
  - `finalize()` is explicitly stubbed (warns and does nothing); there’s no implemented path to send the `finalize` command to flush partial audio without disconnecting.

## src/core/stt/gnani/client.rs, src/core/stt/gnani/config.rs
- Purpose: Gnani.ai gRPC streaming STT client and config.
- Architecture integration: used when provider is "gnani".
- Notes/issues:
  - `interim_results` flag in `GnaniSTTConfig` is never used; interim results are always forwarded.
  - `update_config` does not reconnect or restart the gRPC stream when called while connected, so updated settings won’t take effect.
  - No validation of `sample_rate`/encoding compatibility (e.g., AMR-WB vs 16kHz) even though `audio_format` is inferred from `base.encoding`.

## src/core/mod.rs
- Purpose: re-export core subsystems and provide feature-gated turn detector types.
- Architecture integration: centralizes `core` module exports for library consumers and the binary; surfaces STT/TTS/realtime/voice manager traits/configs and AWS turn detect feature.
- Notes/issues: none obvious.

## src/core/cache/store.rs
- Purpose: Provides memory + filesystem cache abstractions with TTL, hashing, and metrics.
- Architecture integration: used by HTTP handlers (cache avatars?), TTS caching, provider results; integrated via `CacheStore` constructed from `CacheConfig`.
- Notes/issues:
  - Filesystem `exists` only checks metadata expiry but never deletes expired files; stale files accumulate until `get` runs, potentially wasting disk space.
  - Global `CompressionLayer` (from `main.rs`) will compress cache responses, but doc says binary audio responses should not be compressed—needs filtering at handler level.

## src/core/emotion/mod.rs
- Purpose: Unified emotion abstraction plus validation helpers; re-export types/mappers.
- Architecture integration: used by TTS providers when handling emotion configs from CLI/WS; referenced by docs for Hume/Elevation.
- Notes/issues: none; coverage includes graceful warnings when providers lack support.

## src/core/providers/mod.rs
- Purpose: shared provider infra for Google/Azure (auth, region helpers, HTTP headers).
- Architecture integration: reused by cross-provider STT/TTS modules to avoid duplicating auth code (e.g., `core/stt/google`, `core/tts/google`, `core/realtime/openai` referencing auth clients).
- Notes/issues: none obvious; ensures consistent credential handling.

## src/core/providers/google/client.rs
- Purpose: Creates TLS gRPC channels with bearer token interception for Google Cloud APIs.
- Architecture integration: Shared by Google STT/TTS modules for authentication and channel reuse; ensures consistent error handling/logging.
- Notes/issues: No issues noted; tests cover header/interceptor creation and token errors.

## src/core/providers/azure/mod.rs
- Purpose: Shared region/auth infrastructure for Azure Speech services.
- Architecture integration: used by both STT and TTS modules to derive endpoints and headers consistently, avoiding duplication.
- Notes/issues: none.

## src/core/realtime/mod.rs
- Purpose: Abstraction layer for real-time audio-to-audio providers (OpenAI, Hume).
- Architecture integration: exposes `BaseRealtime`, provider enums, and factory functions used by `/realtime` routes and plugin system.
- Notes/issues: None; plugin registry handles provider creation with case-insensitive lookup.

## src/core/realtime/openai/client.rs
- Purpose: Implements OpenAI Realtime WebSocket client with reconnection, callbacks, and audio I/O.
- Architecture integration: used when `realtime` provider set to OpenAI, powering `/realtime` WS sessions and LiveKit bridging.
- Notes/issues: Implementation enforces 24kHz PCM audio, but config allows choosing G.711 8k which may not align with silence detection/reconnection heuristics; need to ensure keep-alive and buffer sizes adapt to actual format.

## src/core/realtime/hume/mod.rs, src/core/realtime/hume/client.rs
- Purpose: Hume EVI (Empathic Voice Interface) realtime provider with prosody tracking, reconnection, and emotional analysis.
- Architecture integration: Integrated via realtime plugin registry; used by `/realtime` endpoint when provider is `hume`, supporting chat session resumption via chat group IDs.
- Notes/issues: None spotted; client handles configuration validation, chat metadata, and graceful reconnection.

## src/core/state.rs
- Purpose: Aggregates shared core resources: TTS request managers, cache store, turn detector, SIP hooks state.
- Architecture integration: Constructed during app initialization (`AppState`), injected into handlers for caching, SIP hooks, and turn detection heuristics.
- Notes/issues: None; caches are initialized with env-provided cache path or defaults, optionally pre-warm TTS connections.

## src/core/voice_manager/config.rs
- Purpose: Configuration structs for STT/TTS combos and speech-final timing windows used throughout the voice manager.
- Architecture integration: `VoiceManagerConfig` passed into `VoiceManager::new` (used by WS handlers) to coordinate provider lifecycles.
- Notes/issues: Default speech final timeouts may not match all STT providers (e.g., 1.8s wait for speech_final could be too short for Azure). Consider making these configurable per provider and exposing in docs.

## src/core/voice_manager/manager.rs
- Purpose: Coordinates STT and TTS providers, manages callbacks, speech final timing, interruption, and TTS cache.
- Architecture integration: Primary execution engine behind `/ws` and LiveKit flows; created via `VoiceManagerConfig` and `AppState` and used by `handlers/ws`.
- Notes/issues:
  - Speech final timing hardcoded to default values stored in `SpeechFinalConfig`; may require tuning per provider (e.g., Azure tends to emit `speech_final` later). No runtime adjustment per provider.
  - `set_tts_cache` is exposed for caching, but it's only called from `src/handlers/ws/config_handler.rs` when config changes; other flows (REST /speak) may bypass cache configuration entirely.

## src/core/voice_manager/state.rs
- Purpose: Internal structures for speech-final timing and interruption tracking using atomic fields for low-latency control.
- Architecture integration: Shared between voice manager internals and callbacks to manage turn detection, timers, and interruption windows.
- Notes/issues:
  - `InterruptionState` uses wall-clock time (`SystemTime::now`) to calculate deadlines; on systems with clock jumps/backwards adjustments, `can_interrupt` may misfire. Consider switching to monotonic `Instant`.

## src/core/voice_manager/callbacks.rs
- Purpose: Type definitions for the various async callbacks used by `VoiceManager` and implementation of `AudioCallback` to forward TTS events.
- Architecture integration: Connects STT/TTS providers to WS handlers via shared callback storage; also updates interruption state on TTS complete events.
- Notes/issues: `VoiceManagerTTSCallback::on_complete` uses `SeqCst` ordering to update `is_completed` but downstream reads use `Acquire`; consistent ordering is fine but `SeqCst` may be heavier than needed.

## src/handlers/ws/audio_handler.rs
- Purpose: Handles binary audio ingestion, speak/clear commands, and zero-copy routing to `VoiceManager`.
- Architecture integration: Called from `ws_voice_handler` dispatch loop for incoming WebSocket messages; forwards audio to STT, commands to TTS/LiveKit.
- Notes/issues:
  - Audio frame limit is 5MB per chunk; while generous, large frames may still strain providers (no chunk splitting).
  - `handle_clear_message` fetches LiveKit client with write lock when audio is disabled, which could wait behind long-lived write locks, potentially delaying clear commands during heavy audio flows.

## src/handlers/ws/command_handler.rs
- Purpose: Handles LiveKit-specific commands (send_message, SIP transfer) for `/ws`.
- Architecture integration: Uses operation queue to send data channel messages and directly invokes SIP handler for transfers; tightly coupled to `AppState` LiveKit handlers.
- Notes/issues:
  - `handle_sip_transfer` filters participants for `participant_info::Kind::Sip`, so the transfer fails if the target participant is not flagged as SIP even though the call is the active LiveKit participant; consider broader selection if bridging non-SIP participants.

## src/handlers/ws/config.rs
- Purpose: Defines STT/TTS/LiveKit WebSocket config shapes, conversion to provider configs, and TTS cache hash calculation.
- Architecture integration: Parsed from `/ws` config message, used to create `VoiceManagerConfig`, LiveKit tokens, and request-specific caches.
- Notes/issues:
  - `LiveKitWebSocketConfig::to_livekit_config` hardcodes mono channels and noise-filter enabling whenever the feature is compiled; there’s no per-call control, so turning noise filtering off at runtime requires server rebuild.

## src/handlers/ws/config_handler.rs
- Purpose: Handles `/ws` config messages; initializes voice manager, LiveKit client, callbacks, and readiness handshake.
- Architecture integration: Connects `AppState`, `VoiceManager`, LiveKit operations, and config conversions when a client sends the initial config.
- Notes/issues:
  - `initialize_livekit_client` creates the LiveKit room synchronously and blocks until recording is initiated; failure to create the room aborts the whole WS session even if audio-only mode is fine.
  - `wait_for_livekit_audio` uses repeated polling with sleeps; if the LiveKit client never exposes audio sources, the function simply warns and proceeds, so STT continues but `LiveKit audio` data may be missing without notifying clients.

## src/handlers/ws/handler.rs
- Purpose: WebSocket upgrade handler and main message loop for `/ws`, including idle detection, message routing, and cleanup.
- Architecture integration: Coordinates Axum upgrade, `ConnectionState`, `VoiceManager`, LiveKit, and message/operation channels; ensures connection limit release via `ConnectionGuard`.
- Notes/issues:
  - Idle timeout jitter introduces ±30s variation around 5 min; jitter calculation uses `Instant::now().elapsed()` which is always near zero at startup, so the offset is essentially the nanosecond counter of wall clock, but the result can be negative (clamped). Works but non-deterministic.
  - Idle detection resets `last_activity` only on incoming frames; outgoing traffic (e.g., keep-alive) does not keep connection alive, so some clients that only read audio might be prematurely closed after 5 minutes of no incoming messages.

## src/handlers/ws/messages.rs
- Purpose: Defines WebSocket incoming/outgoing message schemas, validation constants, and unified message format used by `/ws` handler.
- Architecture integration: Parses config/send_message/sip/auth messages; `MessageRoute` orchestrates responses; used by processors/middleware.
- Notes/issues:
  - `MAX_SPEAK_TEXT_SIZE` is 100KB; extremely long transcripts might be truncated earlier in pipeline but there’s no streaming chunking support for STT input text.

## src/handlers/ws/processor.rs
- Purpose: Routes incoming WebSocket messages to command/config/audio handlers and manages first-message auth for browsers.
- Architecture integration: Called from `ws_voice_handler::process_message`; ensures auth gating and delegates to specialized handlers. Handles both default and deprecated `audio_disabled` field.
- Notes/issues: `handle_auth_message` only supports API secret tokens; JWT-based auth (when `AUTH_REQUIRED=true`) cannot authenticate via first-message flow, so browsers must use header-based auth or special endpoint.

## src/handlers/ws/state.rs
- Purpose: Tracks per-connection state (voice manager, LiveKit client/queue, auth, stream ID) with atomic audio flag for fast reads.
- Architecture integration: Instantiated per WebSocket session in `ws_voice_handler`, shared across handlers via `Arc<RwLock<ConnectionState>>`.
- Notes/issues: `audio_enabled` uses relaxed ordering; potential race if config flips audio flag concurrently during streaming (device toggles audio on/off mid-session) but acceptable due to expectation of single config per session.

## src/handlers/api.rs
- Purpose: Provides simple health check endpoint returning `{"status":"OK"}`.
- Architecture integration: Mounted at `/` route in `main.rs`; used by readiness/liveness probes.
- Notes/issues: none.

## src/handlers/speak.rs
- Purpose: REST `/speak` endpoint that synthesizes text to audio by creating a TTS provider, streaming audio via callbacks, and returning binary data with metadata headers.
- Architecture integration: Called via Router defined in `routes/api.rs`; uses `AppState` for per-provider API keys and request manager pooling.
- Notes/issues:
  - When setting response headers, it uses expressions like `audio_data.len().to_string().as_str()` directly inside the array literal. The temporary `String` is dropped immediately, so the header value points to freed memory, leading to headers being empty or invalid. These strings must be stored before constructing the response.

## src/routes/ws.rs
- Purpose: Constructs `/ws` router with tracing for request telemetry and documents tenant-scoped LiveKit room naming.
- Architecture integration: Mounted in `main.rs` as part of the Axum router stack, relying on auth middleware for tenant isolation.
- Notes/issues: TraceLayer logs entire HTTP payloads by default, which may capture sensitive config data; consider filtering or reducing log verbosity for production.

## src/routes/api.rs
- Purpose: Defines protected REST endpoints for voices, speak, LiveKit management, recordings, and SIP hooks/transfer.
- Architecture integration: Mounted under `/api` with auth middleware applied in `main.rs`; uses TraceLayer for observability.
- Notes/issues: All routes share the same TraceLayer; PII (like auth tokens) may appear in logs if `TraceLayer` is not configured to redact headers.

## src/routes/realtime.rs
- Purpose: Configures `/realtime` WebSocket route with tracing for OpenAI/Hume real-time providers.
- Architecture integration: Uses the same auth middleware for tenant isolation and delegates to `handlers/realtime::realtime_handler`.
- Notes/issues: TraceLayer on realtime path could expose high-volume audio metadata; ensure logging level is appropriate for production.

## src/routes/webhooks.rs
- Purpose: Exposes unauthenticated LiveKit webhook endpoint `/livekit/webhook` (secured via LiveKit JWT signature verification).
- Architecture integration: Merged into public router without auth middleware; relies on `handlers/livekit::handle_livekit_webhook`.
- Notes/issues: TraceLayer logs raw POST payloads, which may include LiveKit HMAC signatures or event payloads; consider adding filters to avoid logging secrets.

## src/handlers/livekit/mod.rs
- Purpose: Re-exports LiveKit REST/webhook handlers (token generation, room listing, participants, webhook).
- Architecture integration: Used by `/api/livekit/*` routes and webhook router, sharing common helpers in `rooms`, `participants`, `token`, `webhook`.
- Notes/issues: None; functionality depends on LiveKit client abstractions in `livekit/*`.

## src/config/mod.rs
- Purpose: Aggregates the configuration loader modules (env, yaml, merge, validation, SIP, pricing) and defines `ServerConfig`, credential enums, and plugin configuration.
- Architecture integration: Entry point for `ServerConfig::from_env`/`from_file` used in `main.rs`; provides API key getters consumed by handlers.
- Notes/issues: Doc claims priority is YAML > env > .env > defaults, but actual loader merges env after YAML, so verifying doc alignment is important; also many option fields default to `None`, requiring runtime validation to catch missing keys early.

## src/middleware/auth.rs
- Purpose: Axum middleware that validates API secret or JWT tokens, supports query `token` for WebSocket first-message auth, and injects `Auth` context.
- Architecture integration: Applied to REST/WS routes in `main.rs`; interacts with `AppState.auth_client` for JWT validation and `auth::match_api_secret_id`.
- Notes/issues:
  - JWT branch buffers the entire request body before validating the token; for endpoints with large payloads this may be problematic, though current protected routes (voices/speak/livekit) have small JSON bodies. Consider streaming validation or body cloning to avoid double buffering.

## src/middleware/connection_limit.rs
- Purpose: Limits concurrent `/ws` upgrades globally and per-IP; injects `ClientIp` extension for release in handler.
- Architecture integration: Applied beneath auth middleware for `/ws`; interacts with `AppState::try_acquire_connection` and `ConnectionGuard`.
- Notes/issues: Relies purely on the `Upgrade: websocket` header; if a malicious client omits the header, the request bypasses limits (but upgrade fails). Also, connection slots are released via `ConnectionGuard` drop, so panics in handler could leak slots if guard is dropped before state cleanups in `handle_voice_socket` returns.

## src/state/mod.rs
- Purpose: Application-level shared state (configs, LiveKit handlers, auth client, cache, connection counters, SIP data).
- Architecture integration: Constructed in `main.rs::AppState::new`, passed to all handlers for TLS/auth/caching/LiveKit interactions; manages connection counters and optional AWS S3 object store.
- Notes/issues:
  - `AppState::new` panics if JWT auth client initialization fails; this ensures the server doesn't start with broken auth but makes deployments brittle if auth service is temporarily unavailable during startup.

## src/plugin/mod.rs
- Purpose: Defines the capability-based plugin system, includes registry/isolation/lifecycle helpers, and exposes a prelude for provider authors.
- Architecture integration: Used by built-in STT/TTS/Realtime providers via `inventory` submissions and referenced by docs; central registry is invoked from `core::stt::create_stt_provider`.
- Notes/issues: None; plugin system relies on feature `plugins-dynamic` to enable runtime directory loading, so ensure feature matches deployment plan.
## src/lib.rs
- Purpose: crate root; exposes top-level modules and re-exports common types/config/state.
- Architecture integration: provides public API surface for the gateway library and CLI.
- Notes/issues: none.

## src/main.rs
- Purpose: CLI entrypoint; config loading, middleware setup, routes, and server startup with optimized TCP listener.
- Architecture integration: wires routes, auth, rate limit, CORS, compression, TLS, and app state.
- Notes/issues:
  - Compression layer is applied globally even though comment says binary audio and webhooks must not be compressed; `CompressionLayer` will compress any eligible response based on client `Accept-Encoding`, which can break audio clients and webhook signature validation unless filtering is added.
