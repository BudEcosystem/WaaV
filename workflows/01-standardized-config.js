/**
 * WaaV implementation workflow 01 — STANDARDIZED CONFIG (the "S1" keystone).
 *
 * Goal: replace the flat 8/12-field STTConfig/TTSConfig (the only struct crossing
 * the dispatch/factory boundary) with a standardized, capability-rich config so that
 * EVERY provider feature/model/param becomes reachable on the live WS/REST path.
 *
 * Run AFTER: `cargo check` is green on the current tree, and the design in
 * PRODUCTION_PLAN.md §2 (Standardized API) is accepted.
 *
 * Invoke:  Workflow({ scriptPath: ".../workflows/01-standardized-config.js" })
 *
 * Strategy (strict TDD, worktree-isolated per stage to avoid clobbering):
 *   Phase 1 Design-freeze : one agent emits the exact Rust types + trait deltas as a spec.
 *   Phase 2 Core          : one agent lands the new types + trait method + registry plumbing
 *                           + WS/REST envelope, with failing-then-passing unit tests.
 *   Phase 3 Migrate       : providers migrated in parallel batches (each batch = its own
 *                           worktree), each writing a contract test that proves an advanced
 *                           param now reaches the wire, then wiring it through.
 *   Phase 4 Verify        : a gate agent runs cargo check/clippy/test and reports red/green.
 *
 * NOTE: model overrides are intentionally omitted — agents inherit the session model.
 */
export const meta = {
  name: 'waav-standardized-config',
  description: 'Implement the standardized capability-rich STT/TTS config (S1 keystone) and migrate all providers to it, TDD-first',
  phases: [
    { title: 'Design-freeze', detail: 'emit exact Rust type + trait spec' },
    { title: 'Core', detail: 'land types, trait method, registry, WS/REST envelope (TDD)' },
    { title: 'Migrate', detail: 'migrate all providers to the new config in parallel batches' },
    { title: 'Verify', detail: 'cargo check/clippy/test gate' },
  ],
}

const GW = '/home/bud/ditto/waav/WaaV/gateway'
const PLAN = '/home/bud/ditto/waav/WaaV/PRODUCTION_PLAN.md'
const REVIEW = '/home/bud/ditto/waav/WaaV/BRUTAL_REVIEW.md'
const COMMON = `Repo: ${GW}. Authoritative design: ${PLAN} (§ Standardized API). Findings: ${REVIEW}. ` +
  `Rules: strict TDD (write the failing test first, then make it pass), no stubs/mocks/hardcoding in production paths, ` +
  `keep backward-compat shims where the plan requires, run \`cargo check\` before declaring done, reference file:line.`

const SPEC_SCHEMA = {
  type: 'object', additionalProperties: false,
  required: ['stt_config_rs', 'tts_config_rs', 'trait_deltas', 'registry_deltas', 'envelope_deltas', 'migration_contract'],
  properties: {
    stt_config_rs: { type: 'string', description: 'final Rust source for the standardized SttConfig (typed common + optional advanced + provider_extras: serde_json::Map passthrough)' },
    tts_config_rs: { type: 'string', description: 'final Rust source for the standardized TtsConfig' },
    trait_deltas: { type: 'string', description: 'exact BaseSTT/BaseTTS trait signature changes (e.g. new() takes the new config; capabilities() method)' },
    registry_deltas: { type: 'string', description: 'exact factory closure type + registry.create_stt/create_tts signature changes' },
    envelope_deltas: { type: 'string', description: 'WS STTWebSocketConfig/TTSWebSocketConfig + REST changes + to_*_config mapping' },
    migration_contract: { type: 'string', description: 'the per-provider migration checklist every batch agent must follow + the contract-test template' },
  },
}

const VERIFY_SCHEMA = {
  type: 'object', additionalProperties: false,
  required: ['cargo_check', 'clippy', 'tests', 'red_items'],
  properties: {
    cargo_check: { type: 'string' }, clippy: { type: 'string' }, tests: { type: 'string' },
    red_items: { type: 'array', items: { type: 'string' }, description: 'concrete remaining failures with file:line and fix hint' },
  },
}

// Provider batches (canonical names). Keep batches small so each worktree compiles fast.
const STT_BATCHES = [
  ['deepgram', 'google', 'azure', 'openai', 'assemblyai'],
  ['cartesia', 'elevenlabs', 'aws_transcribe', 'ibm_watson', 'groq'],
  ['speechmatics', 'gladia', 'revai', 'phonexia', 'sarvam', 'gnani', 'reverie', 'bhashini'],
  ['iflytek', 'alibaba_cloud', 'baidu', 'tencent', 'huawei_cloud', 'naver_clova', 'amivoice'],
  ['fpt_ai', 'viettel_ai', 'prosa_ai', 'nectec', 'yandex', 'tinkoff', 'sberdevices'],
]
const TTS_BATCHES = [
  ['deepgram', 'elevenlabs', 'google', 'azure', 'openai', 'cartesia'],
  ['aws_polly', 'ibm_watson', 'hume', 'lmnt', 'playht', 'murf', 'resemble'],
  ['speechify', 'smallest', 'unrealspeech', 'wellsaid', 'acapela', 'cereproc'],
  ['yandex', 'tinkoff', 'sberdevices', 'bhashini', 'gnani', 'reverie', 'iflytek'],
  ['alibaba_cloud', 'baidu', 'tencent', 'huawei_cloud', 'naver_clova', 'zalo_ai', 'fpt_ai', 'viettel_ai', 'prosa_ai', 'nectec', 'speechmatics'],
]

phase('Design-freeze')
const spec = await agent(
  `${COMMON}\n\nEmit the EXACT, compile-ready Rust spec for the standardized config: the new SttConfig/TtsConfig types ` +
  `(strongly-typed common fields + Option<...> advanced fields covering diarization, interim, endpointing, vad_events, ` +
  `keyterms, redaction, entities, word_timestamps, language_detection, smart_format, custom_vocab; TTS: voice, model, ` +
  `format, sample_rate, speed, stability/similarity/style, emotion, ssml, instructions, streaming, word_timestamps) plus a ` +
  `\`provider_extras: serde_json::Map<String,Value>\` passthrough; the BaseSTT/BaseTTS trait deltas (incl a capabilities() ` +
  `method for graceful degradation); the factory/registry signature deltas; the WS/REST envelope + to_*_config mapping; ` +
  `and the per-provider migration checklist + contract-test template every batch must follow. Be exact — batch agents copy this verbatim.`,
  { label: 'design-freeze', phase: 'Design-freeze', schema: SPEC_SCHEMA })

phase('Core')
const core = await agent(
  `${COMMON}\n\nLand the CORE of the standardized config per this frozen spec (apply verbatim, adjusting only to compile):\n` +
  JSON.stringify(spec) +
  `\n\nSteps: (1) write failing unit tests asserting an advanced STT param (e.g. diarization=true) and an advanced TTS param ` +
  `(e.g. ElevenLabs stability) survive WS-config -> SttConfig/TtsConfig -> provider request; (2) implement the new types, ` +
  `trait method, factory closure type, registry.create_*; (3) update to_stt_config/to_tts_config + the WS/REST envelope; ` +
  `(4) keep a temporary From<old> shim so providers still compile until migrated; (5) cargo check until green. Report what landed with file:line.`,
  { label: 'core', phase: 'Core' })

phase('Migrate')
const migrate = await pipeline(
  [...STT_BATCHES.map((p, i) => ({ kind: 'stt', providers: p, i })),
   ...TTS_BATCHES.map((p, i) => ({ kind: 'tts', providers: p, i }))],
  (batch) => agent(
    `${COMMON}\n\nCore is landed:\n${core}\n\nMigration contract:\n${spec.migration_contract}\n\n` +
    `Migrate these ${batch.kind.toUpperCase()} providers to the standardized config: ${batch.providers.join(', ')}. ` +
    `For EACH provider: (a) write a failing contract test (against tests/mock_providers or a recorded fixture) proving a ` +
    `previously-unreachable advanced param now reaches the provider request/wire; (b) replace its from_base/new so every ` +
    `standardized field + provider_extras is honored; (c) remove dead builder methods that the factory never called; ` +
    `(d) cargo check the crate. Report per-provider: param newly wired, test name, file:line, and any provider whose API ` +
    `genuinely cannot support a field (document the capabilities() gating).`,
    { label: `migrate:${batch.kind}:${batch.i}`, phase: 'Migrate', isolation: 'worktree' }),
)

phase('Verify')
const verify = await agent(
  `${COMMON}\n\nAll batches migrated. Run \`cargo check --all-features\`, \`cargo clippy --all-targets\`, and \`cargo test --lib\` ` +
  `(unit) at ${GW}. Report exact pass/fail and a concrete red-items list (file:line + fix hint) for anything still broken. ` +
  `Do NOT claim green unless cargo actually returns 0.`,
  { label: 'verify', phase: 'Verify', schema: VERIFY_SCHEMA })

return { spec, coreSummary: core, migrate, verify }
