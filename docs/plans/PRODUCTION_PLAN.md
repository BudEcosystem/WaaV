# WaaV Production-Hardening — Master Implementation Plan

**Status:** Execution-ready. **Owner:** Synthesis Lead. **Source of truth:** this document + `/home/bud/ditto/waav/WaaV/BRUTAL_REVIEW.md`.
**Codebase:** `/home/bud/ditto/waav/WaaV` (gateway in `/home/bud/ditto/waav/WaaV/gateway`), ~268k LoC Rust, 60+ STT/TTS providers, DAG engine, VAD/turn-detection, OpenAI/Hume realtime, LiveKit/SIP, Python/TS/Widget SDKs.

> **Read this first.** Every claim below was verified against source. The product's entire value proposition is gated on **one** architectural fact: the flat 8/12-field `STTConfig`/`TTSConfig` is the only struct crossing the dispatch/factory boundary, so every advanced provider feature is unreachable on the live path. Fix that (S1) first, additively, per-provider. Everything else is downstream.

---

## 0. Executive Summary & Definition of "Production Ready"

### 0.1 The verdict in one paragraph
WaaV is a real, ambitious, largely-implemented gateway — **not** vaporware — but it is a technical preview, not a `v1.0.0` production system. The dominant problem is architectural: a flat config that strands ~every advanced provider feature (**S1**, the keystone). Downstream of it: several flagship integrations cannot authenticate/connect (Azure, Cartesia, Tencent, Tinkoff), a default build silently ships VAD/turn-detection stubs (**S2**), TTS codec mismatch emits noise for default callers (**S3**), no streaming reconnection exists (**S4**), emotion control is dead for everyone but Cartesia (**S5**), the parallel-DAG path double-executes and double-bills (**S6**), rate-limiting is bypassable and self-disabling (**S7**), `panic=abort` turns one bad packet into a full outage (**S8**), builds are non-reproducible (**S9**), and ML models are executed with fake integrity verification (**S10**).

### 0.2 Definition of "production ready" for WaaV
WaaV is production-ready when **every advertised capability is either reachable-and-tested on the live path, or removed from the product surface**, and the system survives adversarial input, transient upstream failure, and sustained load without a process-wide outage or silent data corruption. Concretely:

1. **Feature honesty** — A provider feature is "supported" iff (a) the standardized config has a typed field for it, (b) the provider's `from_standard` reads it, and (c) the capability matrix advertises it, **and** an automated test invokes it through `create_stt`/`create_tts`. No silent neutralization (S1/S5).
2. **Wire correctness** — Every provider that ships either connects/authenticates/transcribes against its real API (validated by recorded-cassette wire-contract tests, smoke-validated live before release) or is marked `unsupported` and unselectable (fail-closed, not fail-silent).
3. **Audio integrity** — Emitted audio frames are labeled with their actual codec; no container bytes run through the PCM path (S3).
4. **Availability** — Long-lived sessions survive transient upstream drops via bounded backoff reconnection with storm-control; one malformed network frame cannot abort the process (S4/S8).
5. **Security** — Rate-limiting cannot be bypassed or disabled; DAG webhook nodes are SSRF-validated; client-supplied Rhai cannot DoS the runtime; LiveKit tokens are least-privilege + TTL-scoped; ML models are integrity-verified; no secrets in `Debug` output (S6/S7/S10, sec #1–9).
6. **Correctness of WaaV-specific ML** — VAD/turn-detection produce non-degenerate, accurate outputs against labeled datasets, with the production feature set compiled in (S2).
7. **Reproducibility & supply chain** — `Cargo.lock` committed, toolchain pinned, `--locked` everywhere (S9).
8. **Truthful claims** — Every README marketing claim is backed by a reproducible test/benchmark or deleted (§5 of review).

### 0.3 Measurable exit criteria (the acceptance gate for v1.0.0)

| # | Criterion | Measurement |
|---|---|---|
| E1 | Advanced features reachable | Integration test proves ≥1 feature per category — diarization, keyterms, voice-settings, emotion, instructions — is invoked on the live path through `create_stt`/`create_tts` |
| E2 | No rate-limit bypass | Rotating `X-Forwarded-For` from one peer cannot mint a fresh bucket; no rps value disables the limiter |
| E3 | DAG safety | split/join test: each branch external call fires exactly once; `WebhookOutputNode` rejects `169.254.169.254`/RFC1918; an infinite-loop Rhai script is killed by wall-clock timeout |
| E4 | Build honesty | `cargo build --release` (no flags) either includes a working VAD or refuses to start with a clear error; default-codec TTS for WellSaid/Speechify/UnrealSpeech passes a decode test (no noise) |
| E5 | Reconnection + storm control | Killing each streaming provider's WS mid-session recovers within the backoff budget; 1000 simultaneous reconnects stay under the configured concurrency cap |
| E6 | Process isolation | A fuzzed malformed first-message does not abort the process; an FFI panic kills one plugin call, not the gateway |
| E7 | Model integrity | A tampered ONNX file is rejected at startup |
| E8 | Accuracy | Silero VAD F1 ≥ 0.95 clean / ≥ 0.85 noisy; Smart-Turn F1 ≥ 0.85; ensemble F1 ≥ max(audio,text)+0.02; turn inference p95 ≤ 50 ms |
| E9 | SDK interop | TS + Python SDK streaming tests connect to the gateway and receive a non-empty transcript in CI |
| E10 | Reproducibility | `cargo build --locked` from a committed lockfile + pinned toolchain reproduces the binary |
| E11 | Honest benchmark | Throughput/latency/RSS re-measured with the limiter **ON**, through the real REST path, against the TTL-fixed cache; README quotes only those numbers |
| E12 | Coverage | Patch coverage ≥ 85%; `plugin/registry.rs`, auth/rate-limit/SSRF/HMAC ≥ 90%; every registered provider has ≥1 cassette (`xtask cassette list-missing` green) |

---

## 1. Root-Cause Analysis & Leverage Points

### 1.1 The four highest-leverage edits (do these and most P0s collapse)

1. **The keystone (S1).** Add a typed open-extension field to `STTConfig` (`gateway/src/core/stt/base.rs:420`, 8 fields) and `TTSConfig` (`gateway/src/core/tts/base.rs:181`, ~12 fields), mirror it on the WS wire configs, deserialize it in `to_stt_config`/`to_tts_config` (`gateway/src/handlers/ws/config.rs:92` and `:254`), and change every provider's `from_base(base){ base, ..Default::default() }` to **read** it. This single carrier change simultaneously unlocks S1 (all 60+ providers' advanced features), S5 (emotion/description/instructions), the Deepgram diarize/keyterm bug, the Cartesia `cartesia_version` connect failure, AWS Transcribe diarization/redaction/vocab, and IBM `instance_id` — because they share the identical root cause: `STTFactoryFn = Fn(STTConfig)` / `TTSFactoryFn = Fn(TTSConfig)` (`gateway/src/plugin/registry.rs:41,44`) carry only the flat base, and `from_base` discards everything else.

2. **Codec authority (S3).** Make `AudioData.format` authoritative from the **provider response**, not the request. One reconciliation step in `core/tts/provider.rs` (around the chunker) fixes WellSaid/Speechify/UnrealSpeech and removes a whole class of future codec-mismatch bugs, because today format is stamped from `config.audio_format` while providers hardcode their real codec (e.g. `unrealspeech/provider.rs:294` mp3).

3. **Rate-limit (S7) — two edits in `main.rs`.** Swap `SmartIpKeyExtractor` → `PeerIpKeyExtractor` (`gateway/src/main.rs:214`) and delete the `if rate_limit_rps < 100000` auto-disable branch (`:210`). Closes both halves of S7 (XFF spoofing + auto-disable) and reconciles rate-limit vs connection-limit identity behind a proxy. **Verified:** `:210` literally wraps the `GovernorLayer` in `if rate_limit_rps < 100000`, with a "Rate limiting disabled (rate >= 100000/s)" `println` at `:219`.

4. **DAG split prune (S6) — reuse existing machinery.** The codebase already has the correct pruning primitive `prune_subtree`/`can_reach_target` (`gateway/src/dag/executor.rs:278,319`) wired **only** to the router path (`:270`). Mirror it for the split path (`execute_split_branches` at `:608`) — pruning executed branch nodes from `reachable_nodes` fixes the double-execution without new graph logic.

### 1.2 Fix dependency order (what must land before what)

- **F0 — Config passthrough (S1). MUST LAND FIRST.** It is the carrier all of S5, Deepgram diarize/keyterm, Cartesia `cartesia_version`, AWS/Hume/ElevenLabs advanced flags ride on. Nothing downstream of S1 can be fixed *permanently* without it.
- **F1 — Codec reconcile (S3).** Localized; can land in parallel with F0. Cleanest per-provider "declared output format" rides on F0's passthrough but is not blocked by it.
- **F2 — Provider connect fixes (Azure framing, Tencent signature, Tinkoff JWT).** Independent of F0 — these are protocol-correctness defects, not config-passthrough victims. **Cartesia is the one exception:** its `cartesia_version=None` connect failure *is* the F0 gap, so land Cartesia after F0; Azure/Tencent/Tinkoff land independently.
- **F3 — Rate-limit (S7).** Fully independent. Land **immediately**, before any load/security test or benchmark is re-run.
- **F4 — DAG split/join + SSRF + Rhai timeout (S6 + §2.4).** Independent of F0. Land before enabling parallel DAG or accepting inline client DAGs in production.
- **F5 — Reconnection (S4).** Independent of F0 but land **after** F0/F2 so reconnection re-establishes the *correct, full-feature, authenticating* session rather than faithfully reconnecting a crippled one.
- **F6 — Default-build safety (S2).** Independent. Land before publishing any "neural VAD" build.
- **F7/F8/F9 — Process resilience (S8), model integrity (S10), Cargo.lock (S9).** Independent hardening; land after the correctness fixes (F0–F4) since they change crash/supply-chain/repro semantics but not live-path behavior.

### 1.3 Root-cause summary (the architectural "why")

| ID | Issue | Architectural root cause | Blast radius |
|---|---|---|---|
| **S1** | Flat config strands every advanced feature | Dispatch boundary typed **only** on the flat base; `STTConfig`/`TTSConfig` have no open-extension field; `from_base{ base, ..Default::default() }` structurally throws away everything; every `new()` hardcodes advanced flags (e.g. `deepgram.rs:701,703,706` set diarize/filler_words/keywords off) | All 60+ providers — only language+model+rate+punctuation reach the live path |
| **S5** | Emotion dead for everyone but Cartesia | Same root, one layer down: `emotion_config` *does* cross the boundary but each `from_base` ignores it (Hume `hume/config.rs` hard-sets `description:None`) | Hume (its entire differentiator), ElevenLabs, Azure, OpenAI instructions |
| **S3** | TTS codec mismatch | Audio format treated as a caller *request* not an observed property; chunker reads `config.audio_format`, providers hardcode their real codec, no reconciliation step | WellSaid, Speechify, UnrealSpeech + any future container-format provider |
| **S6** | Split double-exec; join ignores sources | Two execution mechanisms (topo loop + split handler) both own branch nodes with no handoff; correct prune primitive exists but wired only to router | Any split/join DAG — double billing, duplicate webhooks/LiveKit audio (reachable from any authenticated WS client) |
| **S7** | Rate limit bypassable + self-disabling | Identity derived from client-controlled XFF; magic-number kill switch at `<100000` | Complete bypass for any caller setting XFF; total absence at high-rps |
| **S2** | Default build ships stubs | Capability gated behind opt-in features with empty `default=[]`; disabled path returns success-shaped stubs (`Ok(0.0)`/`Ok(false)`/`bail!`) | Anyone building without exact flags ships no turn detection / neural VAD with silent wrong behavior |
| **S4** | No reconnection | Each event loop treats any error/timeout as terminal (`break`); no shared supervisor; OpenAI Realtime is the lone working impl | Every streaming STT/TTS + Hume EVI |
| **BROKEN-connect** | Azure/Tencent/Tinkoff | Protocol-correctness defects in outbound handshake, masked because inbound parser is correct; never validated against live service | Azure STT, Tencent STT, Tinkoff STT/TTS cannot auth/transcribe |
| **S8** | `panic=abort` + hot-path unwraps | No fault-isolation boundary; `panic="abort"` (`Cargo.toml:286`) defeats Tokio per-task unwind | Process-wide outage from one malformed message |
| **S10** | Fake model integrity | `get_expected_hash` returns `"expected_hash_here"` (`turn_detect/assets.rs:189`); `verify_hash` only `warn!`s; Silero/Smart-Turn no check | Supply-chain RCE/integrity exposure on every neural-feature deployment |
| **S9** | No Cargo.lock | App repo treats itself like a library; `Cargo.lock` in `.gitignore` | Non-reproducible builds; unreliable audits/forensics |

---

## 2. The Standardized API — The Keystone (S1)

### 2.1 Where fields die today (three lossy sites + one factory)

1. `STTWebSocketConfig::to_stt_config` — `gateway/src/handlers/ws/config.rs:92` (8 fields).
2. `TTSWebSocketConfig::to_tts_config` — `:254` (12 fields; emotion only reaches Cartesia, S5).
3. DAG node rebuilds flat structs — `gateway/src/dag/nodes/provider.rs`.
4. `registry.create_stt`/`create_tts` (`gateway/src/plugin/registry.rs:303,346`) hand a flat struct to a factory whose `new()` discards anything outside those fields.
   Outbound: `OutgoingMessage::STTResult` carries only ~4 fields; the modeled words/speakers/entities/PII never go on the wire.

### 2.2 The design — typed-common + typed-optional-advanced + typed passthrough

Design principle: common paths stay ergonomic and zero-cost, advanced features are discoverable/validated, brand-new provider params never require a gateway release.

```rust
// gateway/src/core/config/common.rs  (new)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioParams {
    pub sample_rate: u32,
    pub channels: u16,
    pub encoding: AudioEncoding, // Linear16|Mulaw|Alaw|Flac|Opus|Mp3|Ogg|Wav|Raw
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StandardSTTConfig {
    pub provider: String,
    #[serde(skip_serializing)] pub api_key: SecretString, // secrecy::SecretString — fixes "secrets in Debug" (sec #9)
    pub model: String,
    pub language: Option<String>,                 // None => provider default / autodetect
    pub audio: AudioParams,
    pub punctuation: bool,
    #[serde(default)] pub features: SttFeatures,         // typed-optional advanced
    #[serde(default)] pub provider_extras: ProviderExtras, // typed JSON passthrough
    #[serde(default)] pub on_unsupported: DegradationPolicy, // Reject|Warn|BestEffort
    #[serde(default)] pub endpoint_override: Option<String>, // test-seam enabler (see §5)
}
```

Every advanced field is `Option<T>` → `None` means "don't request / provider default", so adding fields is backward compatible and serde-default-friendly.

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SttFeatures {
    pub interim_results: Option<bool>,
    pub diarization: Option<Diarization>,        // { enabled, min/max_speakers }
    pub word_timestamps: Option<bool>,
    pub word_confidence: Option<bool>,
    pub endpointing_ms: Option<u32>,
    pub utterance_end_ms: Option<u32>,
    pub vad_events: Option<bool>,
    pub smart_format: Option<bool>,
    pub numerals: Option<bool>,
    pub profanity_filter: Option<bool>,
    pub filler_words: Option<bool>,
    pub keyterms: Option<Vec<String>>,           // canonical; maps to keyterm= / phrase_hints / boost
    pub custom_vocabulary: Option<Vec<VocabTerm>>,
    pub redaction: Option<Redaction>,            // { categories: Vec<PiiCategory>, replacement }
    pub entity_detection: Option<bool>,
    pub language_detection: Option<bool>,
    pub n_best: Option<u8>,
    pub logprobs: Option<bool>,
    pub sentiment: Option<bool>,
    pub summarization: Option<Summarization>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TtsFeatures {
    pub voice_settings: Option<VoiceSettings>,   // { stability, similarity_boost, style, speaker_boost } — wires ElevenLabs
    pub emotion: Option<EmotionConfig>,          // existing core type — now reaches ALL emotion providers
    pub instructions: Option<String>,            // Hume description / OpenAI gpt-4o-mini-tts instructions
    pub voice_design: Option<String>,            // Hume voice_description
    pub ssml: Option<bool>,                      // text is SSML; unlocks Azure mstts:express-as
    pub pitch: Option<f32>,
    pub volume: Option<f32>,
    pub seed: Option<u64>,
    pub language: Option<String>,                // fixes Cartesia hardcoded "en"
    pub bit_rate: Option<u32>,
    pub word_timestamps: Option<bool>,
    pub normalization: Option<bool>,
    pub streaming: Option<bool>,
}
```

**Typed passthrough** — the escape hatch so a provider can expose any un-modeled param **without** an untyped `HashMap<String,String>`:

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderExtras(pub serde_json::Map<String, serde_json::Value>);

impl ProviderExtras {
    /// Deserialize into the provider's own rich config struct, merging over typed fields.
    pub fn merge_into<T: DeserializeOwned + Serialize>(&self, base: T) -> Result<T, ConfigError> {
        let mut v = serde_json::to_value(base)?;
        if let Value::Object(m) = &mut v { m.extend(self.0.clone()); }
        Ok(serde_json::from_value(v)?)
    }
}
```

**Precedence (last-write-wins):** `provider_extras` > `features` > `common` > provider default. `provider_extras` is deserialized per-provider with `serde_path_to_error` + `deny_unknown_fields`, so typos **fail loudly** instead of silently neutralizing features.

### 2.3 Trait & factory changes — `from_base` → `from_standard`

```rust
// base.rs
fn new(config: StandardSTTConfig) -> Result<Self, STTError> where Self: Sized;
// registry.rs
pub type STTFactoryFn = Arc<dyn Fn(StandardSTTConfig) -> Result<Box<dyn BaseSTT>, STTError> + Send + Sync>;

// Deepgram (replaces the hardcoded block at deepgram.rs:701/703/706):
impl DeepgramSTTConfig {
    fn from_standard(c: &StandardSTTConfig) -> Result<Self, STTError> {
        let f = &c.features;
        let mut dg = DeepgramSTTConfig {
            base: c.into(),
            diarize: f.diarization.as_ref().map_or(false, |d| d.enabled),
            interim_results: f.interim_results.unwrap_or(true),
            keyterms: f.keyterms.clone().unwrap_or_default(), // nova-3 keyterm= (fixes verified bug)
            redact: f.redaction.as_ref().map(Redaction::to_dg).unwrap_or_default(),
            endpointing: f.endpointing_ms,
            vad_events: f.vad_events.unwrap_or(true),
            smart_format: f.smart_format.unwrap_or(true),
            filler_words: f.filler_words.unwrap_or(false),
            profanity_filter: f.profanity_filter.unwrap_or(false),
            ..Default::default()
        };
        dg = c.provider_extras.merge_into(dg)?; // typed passthrough wins
        Ok(dg)
    }
}
```

Keep `From<&StandardSTTConfig> for STTConfig` (and reverse) **shims** so not-yet-migrated providers compile against the old flat struct during the migration window. Result types are unchanged structurally — the rich `STTResult` already exists; the design just makes providers *request* and *emit* it.

### 2.4 Capability matrix — the graceful-degradation engine

Replace the weak `ProviderMetadata.features: HashSet<String>` with a **bitflags descriptor** + a per-field value-range map, computed once at registration and queryable at dispatch:

```rust
bitflags::bitflags! {
    pub struct SttCaps: u64 {
        const STREAMING=1<<0;  const INTERIM=1<<1;        const DIARIZATION=1<<2;
        const WORD_TIMESTAMPS=1<<3; const WORD_CONFIDENCE=1<<4; const KEYTERMS=1<<5;
        const CUSTOM_VOCAB=1<<6; const REDACTION=1<<7;     const ENTITY_DETECTION=1<<8;
        const PROFANITY=1<<9;  const FILLER_WORDS=1<<10;  const LANG_DETECTION=1<<11;
        const VAD_EVENTS=1<<12; const ENDPOINTING=1<<13;  const SMART_FORMAT=1<<14;
        const N_BEST=1<<15;    const LOGPROBS=1<<16;      const SENTIMENT=1<<17;
        const SUMMARIZATION=1<<18; const TOPICS=1<<19;
    }
}
// analogous TtsCaps: STREAMING, SSML, EMOTION, ACTING_INSTRUCTIONS, VOICE_SETTINGS,
// WORD_TIMESTAMPS, SEED, PITCH, VOLUME, NORMALIZATION, VOICE_DESIGN, INSTRUCTIONS, MULAW, OPUS...
```

**Three-mode degradation policy** (caller picks per request via `on_unsupported`):
- `Reject` — return a structured `UnsupportedCapability { feature, provider, supported_by: [...] }` error (default for compliance-critical features like `redact`).
- `Warn` (default) — drop the unsupported field, attach a `degradations: [...]` array to the `ready` envelope. **This is what fixes S5** — the caller now *knows* emotion was dropped.
- `BestEffort` — map to the nearest supported primitive (`emotion:angry` → Azure `mstts:express-as style="angry"` if `SSML` cap present; ElevenLabs `style:0.8`; else drop).

Each provider exposes `fn caps() -> ProviderCaps` (static), validated by a test asserting every advertised cap is actually read by `from_standard` — closing the drift between "advertised" and "reachable" (the S1 root cause).

### 2.5 Field-lossless dispatch flow

```
WS Config / REST POST ──deserialize──► StandardSTTConfig / StandardTTSConfig
        │  (typed common + features + provider_extras, no field dropped)
        ▼
  Capability negotiation  (registry.negotiate(provider, &cfg))
        │   diff cfg vs ProviderCaps → Vec<Degradation>
        │   on_unsupported: Reject → error envelope
        │                   Warn   → strip field, record degradation
        │                   BestEffort → remap (emotion→SSML, ...)
        ▼
  registry.create_stt(provider, cfg)  // factory takes Standard*Config
        ▼
  Provider::new(cfg) → ProviderConfig::from_standard(&cfg)
        │   reads typed advanced fields + provider_extras.merge_into()
        ▼
  Live connection requests EVERY negotiated feature (diarize, keyterms, redact, voice_settings...)
```

Guarantees:
1. **Single config type crosses every boundary** — WS, REST, and DAG all deserialize directly into the standardized config. The three lossy sites (`config.rs:92/254`, `dag/nodes/provider.rs`) are deleted; the WS configs become thin newtypes that `#[serde(flatten)]` the standardized config plus a wire-only `api_key`.
2. **Negotiation is a pure function** `(caps, cfg) -> (effective, Vec<Degradation>)`, the single chokepoint for degradation; providers never carry degradation logic.
3. **`provider_extras` survives untouched** through negotiation (opaque JSON) and merges last.
4. **DAG parity** — DAG provider node deserializes into the *same* standardized config; the DAG and direct paths are guaranteed feature-identical.
5. **Cache-key fix (correctness prerequisite, not optional).** `compute_tts_config_hash` (`config.rs:332`) hashes only ~6 fields, so two requests differing only in emotion/voice_settings/instructions collide and return wrong cached audio once those fields take effect. Extend the hash to the full *effective* (post-negotiation) config — XXH3 over the canonical-serialized `StandardTTSConfig` minus `api_key`.
6. **Degradation is observable** — each dropped/remapped feature increments `waav_capability_degraded_total{provider,feature,policy}`.

### 2.6 The request/response envelope

**WS — inbound `config`** (replaces `IncomingMessage::Config`):
```jsonc
{ "type": "config", "stream_id": "uuid?", "audio": true,
  "stt": { "provider": "deepgram", "model": "nova-3", "language": "en-US",
    "audio": {"sample_rate":16000,"channels":1,"encoding":"linear16"}, "punctuation": true,
    "features": { "diarization": {"enabled":true,"max_speakers":4}, "word_timestamps": true,
                  "keyterms": ["WaaV","Accubits"], "redaction": {"categories":["pii","ssn"]}, "vad_events": true },
    "on_unsupported": "warn", "provider_extras": { "mip_opt_out": true } },
  "tts": { "provider": "hume", "model": "octave", "voice_id": "...",
    "audio": {"sample_rate":24000,"channels":1,"encoding":"linear16"},
    "features": { "instructions": "warm, friendly", "streaming": true,
                  "voice_settings": {"stability":0.6,"similarity_boost":0.8}, "word_timestamps": true } },
  "livekit": {...}, "dag": {...} }
```

**WS — `ready` reports negotiation outcome** (degradation never silent):
```jsonc
{ "type": "ready", "stream_id": "...",
  "effective": { "stt": {"diarization":true,"keyterms_applied":2}, "tts": {"instructions_applied":true} },
  "degradations": [ {"channel":"tts","feature":"seed","reason":"unsupported_by_provider","action":"dropped"} ],
  "provider_capabilities": { "stt": ["streaming","diarization",...], "tts": [...] } }
```

**WS — rich `stt_result`** (replaces 4-field result; keys omitted when `None` via `skip_serializing_if` → backward compatible by construction):
```jsonc
{ "type": "stt_result", "transcript": "call me at 555 1234",
  "is_final": true, "is_speech_final": true, "confidence": 0.94,
  "detected_language": "en", "audio_duration": 1.8, "logprob": -0.21,
  "words": [ {"word":"call","start":0.0,"end":0.2,"confidence":0.99,"speaker":"speaker_0"} ],
  "speakers": [ {"id":"speaker_0","total_speaking_time":1.8} ],
  "entities": [ {"text":"555 1234","category":"phone_number","start":11,"end":19,"confidence":0.97} ],
  "redacted_transcript": "call me at [PHONE]",
  "sensitive_data": [ {"type":"pii","redacted":"[PHONE]","start":11,"end":19} ] }
```

**WS — typed TTS events** (today SDKs invent phantom handlers — §2.7 of review):
```jsonc
{ "type": "tts_chunk", "seq": 12, "format": "linear16", "sample_rate": 24000, "duration_ms": 40, "is_final": false }
{ "type": "tts_word", "word": "hello", "start_ms": 0, "end_ms": 180 }
{ "type": "tts_complete", "request_id": "...", "total_ms": 1840 }
```

**REST (symmetric, OpenAI-style):**
- `POST /v1/audio/transcriptions` — `{ "stt": StandardSTTConfig, "audio": <multipart|url|base64> }` → rich STT result.
- `POST /v1/audio/speech` — `{ "tts": StandardTTSConfig, "input": "..." }` → streams `audio/*` + trailing `x-waav-degradations`, or JSON when `Accept: application/json`.
- `GET /v1/providers`, `GET /v1/providers/{id}/capabilities` — serialize the matrix.

**Error model (uniform, structured enum):**
```jsonc
{ "type": "error", "code": "unsupported_capability", "channel": "stt",
  "feature": "summarization", "provider": "deepgram",
  "message": "deepgram streaming does not support summarization",
  "supported_by": ["assemblyai","gladia"], "retryable": false }
```
`code` ∈ {`unsupported_capability`,`auth_failed`,`provider_unavailable`,`invalid_audio_format`,`rate_limited`,`provider_error`} so every SDK handles errors identically.

### 2.7 Backward compatibility & migration

- **Wire compat (no breaking change).** Old wire shape deserializes unchanged: every old top-level field maps to a common or `features.*` field; add `#[serde(alias=...)]` for renames + `#[serde(flatten)]` a legacy adapter; the only new surface is the optional `features`/`provider_extras`/`on_unsupported` objects (`#[serde(default)]`). Fix the TS-SDK wire bugs *here*: add `#[serde(alias="punctuate")]`, give every field `#[serde(default)]`, keep `transcript`/`stream_id` canonical. Generate TS/Python/Widget clients from the gateway's OpenAPI so drift becomes structurally impossible.
- **Staged, compile-checked migration:**
  - **Phase 0 (additive):** introduce the standardized types alongside the flat structs; provide `From` both directions; add `ProviderCaps` to `ProviderMetadata`.
  - **Phase 1 (negotiation):** add the negotiate step; switch WS/REST/DAG entry points to deserialize into the standardized config, down-convert via `From` for unmigrated providers. Degradations reported even before providers read new fields — immediate observability.
  - **Phase 2 (per-provider, ~60 small independent PRs):** change factory signatures to `Standard*Config`; migrate each `from_base` → `from_standard`. Start with the crippled-by-factory set (Deepgram, ElevenLabs, Hume, AWS, AssemblyAI, Cartesia). Each PR gated by a test asserting `caps()` ⟺ fields actually read.
  - **Phase 3 (cleanup):** delete the flat structs, the three lossy sites, and the `From` shims.
- **Versioning:** emit `protocol_version` in `ready`; **minor** bump (additive); old SDKs ignore unknown fields. `SecretString` for `api_key` is internal-only. Cache-key change is a one-time cold event, not a compat break.

---

## 3. Integration Plan — All Providers + Fixes + Catalog + Realtime/LiveKit/SIP

### 3.1 Provider category → standardized API mapping

**Reusable per-provider migration checklist (every provider PR must satisfy):**
1. **Boundary** — `from_standard` reads typed features + `provider_extras.merge_into` with `serde_path_to_error` + `deny_unknown_fields`; unknown keys → `ConfigurationError`.
2. **Feature wiring** — every builder (`set_*`/`with_*`) reachable; grep proves no builder is dead on the live path.
3. **Codec reconciliation** — provider declares its real output codec via `negotiated_format()`; engine label matches actual bytes.
4. **Result fidelity** — words/speakers/entities/PII parsed into `STTResult` when requested (not dropped); confidence real, not hardcoded `1.0`.
5. **Reconnection** — implements the shared `ReconnectableStream`; no bare `break` that kills the task.
6. **Auth correctness** — signing/auth unit test asserts exact wire bytes (header value, query string-to-sign, JWT claims) against a fixture.
7. **Catalog** — contributes to the generated model/voice catalog; implements `discover_voices()`/`discover_models()` where upstream offers it.
8. **Capability flags** — `streaming`/`diarization`/`emotion`/`interim_results`/`word_timing` declared in `ProviderMetadata`.
9. **Backpressure** — replace `let _ = try_send(...)` (silent drop on bounded-256 channels, repo-wide) with a logged/metered overflow policy.
10. **Tests** — mock-server round-trip proving connect + one real transcript/audio frame; negative test proving bad auth surfaces `AuthenticationFailed`.

**Category map:**
- **A — Tier-1 streaming** (Deepgram, Google v2 gRPC, ElevenLabs, AssemblyAI v3, AWS Transcribe, Azure, Cartesia, IBM): `provider_extras` → existing typed config; builders already exist, just wire `from_standard` + delete the hardcoded block. **Highest priority — unlocks the product.**
- **B — Batch/file-upload** (OpenAI STT, Groq, Google TTS, bhashini, yandex, sber, naver, viettel, fpt, nectec): same plumbing; expose `streaming:false` capability so the engine buffers a full utterance (gated by VAD/turn, not the provider) and reports TTFB honestly. Wire OpenAI `gpt-4o-transcribe-diarize` as a distinct model so `diarized_json` is requestable.
- **C — Regional custom signing** (Tencent HMAC, Tinkoff JWT, iflytek, alibaba, baidu, huawei): signing stays inside the provider; `provider_extras` carries only inputs (secret_id/key/app_id, region). Fix the signing routines (§3.2) + shared `signing` test harness asserting the exact string-to-sign per provider against a known-good vector.
- **D — gRPC streaming** (Google, Tinkoff, Phonexia[stub], Gnani, Sarvam): `provider_extras` → request-message fields. **Phonexia** is a fabricated `/ws` protocol — mark `unsupported` until real `tech.phonexia.spa` gRPC is implemented; do **not** advertise it.
- **E — Realtime** (OpenAI Realtime, Hume EVI): bypass STT/TTS factories via `RealtimeConfig`. Add missing `config_id` + `provider_options` to `RealtimeConfig`.

**Priority order:** A → C broken-signers → E realtime → B → D.

### 3.2 Broken-provider fix specs

**Azure STT** — `gateway/src/core/stt/azure/client.rs:442` streams bare `Message::Binary` with no `speech.config` and no header framing. Fix: implement Azure Speech WS framing — each message = `[2-byte BE header length][ASCII headers `Key:Value\r\n`][\r\n][body]`. (1) On connect, send a TEXT frame `Path:speech.config` with `X-RequestId` (32-hex no dashes), `X-Timestamp` (ISO8601), `Content-Type:application/json`, body `{"context":{"system":{"name":"WaaV","version":"1.0"},"os":{...}}}`. (2) First audio frame TEXT `Path:audio`, `Content-Type` matching codec, carrying the WAV/RIFF header; subsequent audio frames BINARY with the same header-framing prefix. (3) One stable `X-RequestId` per turn; parse `speech.hypothesis`/`speech.phrase` (inbound parser already correct). Until done, mark Azure STT `unsupported`.

**Cartesia STT/TTS** — `cartesia_version` defaults `None` via `from_base` (`gateway/src/core/stt/cartesia/config.rs:97`); `build_websocket_url` only appends `&cartesia_version=` when `Some` → required version omitted → connection rejected. Fix: default `cartesia_version: Some("2025-04-16".to_string())`, surfaced via `provider_extras`. Send **both** the `cartesia_version` query param (STT WS) and `Cartesia-Version` header (REST) to be safe — **flagged for live verification**. TTS: thread `language` from `base.language` (kill the hardcoded `"en"`); select speed schema by model — `__experimental_controls.speed` (-1..1) for sonic-2 vs `generation_config.speed` (0.6–2.0) for sonic-3.

**Tencent STT** — `gateway/src/core/stt/tencent/signature.rs:401-461` HMAC-signs only the sorted query params, omitting the `host+path` prefix → every auth fails. Fix: sign `sign_str = format!("{}/{}?{}", "asr.cloud.tencent.com/asr/v2", app_id, param_string)` (no `wss://`), HMAC-SHA1 with secret_key, base64, then url-encode for the query. Refactor so `app_id` is available in `build()` (the appid is part of the signed path). Unit test asserts the exact `sign_str` against a Tencent reference vector.

**Tinkoff STT/TTS** — `gateway/src/core/stt/tinkoff/grpc.rs:57-73` sends raw `x-api-key`/`x-secret-key` instead of an HS256 JWT Bearer; TTS hardcodes `iss:test_issuer`/`sub:test_user` and does **not** base64-decode the secret. Fix: HS256 JWT, header `{"alg":"HS256","typ":"JWT","kid":<api_key>}`, claims `{"iss":<issuer>,"sub":<sub>,"aud":"tinkoff.cloud.stt"|"tinkoff.cloud.tts","exp":now+~600s}`, signing key = **base64-decode(secret_key)** (signing the raw string is the core bug). Send `authorization: Bearer <jwt>` + keep `x-api-key`. Make `iss`/`sub`/`aud` configurable via `provider_extras`. Verify protobuf field numbers against the official `tinkoff.cloud.stt.v1`/`tts.v1` proto. **Flagged — validate aud strings + exp window + proto against current VoiceKit docs before shipping.**

**WellSaid / Speechify / UnrealSpeech (codec mismatch, S3)** — `TTSConfig.audio_format` defaults `linear16` but these emit MP3/WAV (`unrealspeech/provider.rs:294` mp3/libmp3lame; `speechify/config.rs:363` wav_48000 fallback; wellsaid MP3). Fix: each TTS provider implements `negotiated_format() -> AudioFormat` returning what its bytes actually are; the chunker uses **that** to label frames and decide PCM-aggregate (linear16/raw only) vs container pass-through (mp3/wav/ogg). Either (1) force the provider to honor `linear16` where it can (UnrealSpeech `pcm_mulaw`/PCM, Speechify raw/PCM, WellSaid PCM) + reconcile sample_rate, or (2) for container-only providers set `negotiated_format` to the real codec and require a decode step (or refuse `linear16` with a clear error). For telephony, select `pcm_mulaw` and label frames `mulaw`. Test asserts emitted-frame format == first-4-bytes sniff (RIFF/ID3/0xFFFB).

**Deepgram STT (nova-3 keyterm)** — emits legacy `&keywords=<csv>` (`deepgram.rs:244-246`) ineffective on nova-3; factory hardcodes diarize/filler_words/keywords off (`:701/703/706`); `keyterm` appears nowhere. Fix: add `keyterms: Vec<String>`, emit model-aware — nova-3 (and default): repeated `&keyterm=<term>` per term (URL-encoded, **not** csv); nova-2/older: `&keywords=<term>:<boost>`. Detect via `config.model.starts_with("nova-3")`. Remove the hardcoded block; populate diarize/keyterms/redact/filler_words/profanity from `provider_extras`. Stop dropping words/speaker on parse; surface `UtteranceEnd`/`SpeechStarted`. Add Aura-2 to the TTS voice list.

**Hume TTS + EVI** — `from_base` sets `description:None` because `TTSConfig` has no description field → its differentiator is dead; `emotion_config` read by Cartesia only (S5); EVI `config_id` unreachable because `RealtimeConfig` has no field; reconnection is dead code. Fix: (1) TTS `apply_options` reads `provider_extras.hume.description` (≤100 chars) + speed/instant_mode → `with_description`; when `emotion_config` is `Some`, map to Hume description text (and ElevenLabs style / Azure `mstts:express-as`) so the "emotion (Hume/ElevenLabs/Azure)" claim becomes true. (2) Add `config_id: Option<String>` + `provider_options` to `RealtimeConfig`; populate in `build_realtime_config`; thread into `HumeRealtimeConfig` (the URL already appends it). (3) Reconnection: reuse the shared `ReconnectableStream`; restore `config_id` + resumed session on reconnect.

**Universal reconnection (S4)** — only OpenAI Realtime reconnects (`gateway/src/core/realtime/openai/client.rs:587-746`). Extract that loop into `ReconnectableStream<T>`: outer reconnect loop with `AtomicBool intentional_disconnect`, `ReconnectionConfig{max_attempts, initial_delay_ms, max_delay_ms, backoff_multiplier, jitter}` (already in `base.rs:87-108` with `calculate_delay`/`should_retry`), a `connect()` closure, a `restore_session()` closure (re-send speech.config / config message / Deepgram query / Cartesia version), and counter reset on first successful message. Replace every bare `break` on transport error with reconnect-eligible vs intentional. Batch providers = per-request retry (generalize Groq's). Surface `ConnectionState::Reconnecting` to the SDK. **Acceptance:** kill the upstream WS mid-stream → session recovers within `max_delay` with no caller action.

### 3.3 Models/voices catalog strategy

Three layers:
1. **Generated static catalog (build-time floor).** Replace hand-maintained enums/lists with a checked-in `catalog.json` (one entry per provider: models with {id, streaming, languages, capabilities, deprecated}; voices with {id, name, languages, gender, sample_rate_options, codecs}). A `build.rs` codegen produces the typed enums — AWS Polly/Murf stop needing code edits for new voices.
2. **Live discovery (runtime truth).** Optional trait methods `discover_models`/`discover_voices`; wire the URLs that already exist (Murf, PlayHT, Resemble, Smallest, Speechify, Azure, ElevenLabs `/v1/voices`, Deepgram `/v1/models` incl. Aura-2, OpenAI `/v1/models`, Cartesia `/voices`). Expose `GET /v1/providers/{p}/voices|models` (proxy to discovery, else static) + `GET /v1/catalog`. Cache with short TTL (1h).
3. **Validation + drift detection (CI).** Nightly job calls `discover_*` with CI keys, diffs vs `catalog.json`, opens a PR on drift (this is how Aura-2 would've been caught). `validate_catalog` test asserts every referenced voice_id exists and every model's declared `codecs` matches the provider's `negotiated_format()` set (ties to S3). Capability flags in the catalog are the single source of truth the engine reads — a provider listing `emotion:true` whose `apply_options` doesn't wire emotion **fails a coverage test**.

Fix known-bad defaults: Google STT `latest_long` (v1 name invalid for v2), ElevenLabs `eleven_v3` (non-realtime default), AssemblyAI `universal-streaming-english` vs v3 `universal_streaming` (**flagged**). Catalog entries carry `deprecated`/`replaced_by`; factory warns (not fails) on deprecated; `/catalog` omits deprecated by default.

### 3.4 Realtime / LiveKit / SIP

**OpenAI Realtime** — (1) **Barge-in**: add `conversation.item.truncate` — on barge-in (`input_audio_buffer.speech_started` or gateway VAD), send `{"type":"conversation.item.truncate","item_id":<assistant>,"content_index":0,"audio_end_ms":<ms actually played>}` then `response.cancel`, then flush downstream audio. Track `audio_end_ms` from the **playout** clock (bytes emitted / sample_rate), not what OpenAI sent. (2) **gpt-realtime (GA)**: the enum only knows `gpt-4o-realtime-preview*`; requesting GA `gpt-realtime` silently falls back. Add `gpt-realtime`/`gpt-realtime-mini` as first-class variants, make GA default, **fail loudly** on unknown model. Gate the `session.update` shape on the model (GA uses `session.type:"realtime"`).

**Hume EVI** — (1) `config_id` defined + appended to URL but `build_realtime_config` can't set it (`RealtimeConfig` lacks the field) → add `config_id` to `RealtimeConfig` + thread through. (2) "Automatic reconnection" is dead — reuse the shared `ReconnectableStream`; re-send `config_id` + `resumed_chat_group_id` on reconnect. Register post-connect callbacks **before** connect.

**LiveKit token** — `user_token` and `agent_token` share one `token_permissions` granting `room_record:true, room_list:true, room_create:true` to **every** user, with **no `.with_ttl()`** (relies on ~6h default). Fix: split grants — `user_token` → only `{room_join, can_publish, can_subscribe, can_publish_data, can_update_own_metadata, room:<this room>}` (drop record/list/create); `agent_token`/`admin_token` keep `room_admin`. Add `.with_ttl()` — short (5–15 min) for user join tokens (gateway-refreshed), longer for agents. Scope `room` to the exact name; never wildcard.

**SIP** — `max_participants` parsed but dropped at dispatch creation (`CreateSIPDispatchRuleOptions` has no `room_config`); `create_room` hardcodes `max_participants:3`. Fix: set `max_participants` via `RoomConfiguration` once `livekit-api` exposes it, or (interim) pre-create the room with the configured limit and target it from the dispatch rule. Stop hardcoding 3; read `sip_max_participants` from config (already carried in auth/client + parsed in config/merge). **Acceptance:** a call into a full room is rejected at the SIP layer before media.

**Cross-cutting realtime** — `RealtimeConfig` gains `config_id` + `provider_options`; the per-frame detached `tokio::spawn` (LiveKit ingest) becomes a bounded mpsc with a single consumer (backpressure + ordering); host-endian `&[u8]→&[i16]` reinterpret in `livekit/client/audio.rs` uses `i16::from_le_bytes`/`bytemuck` (UB today on BE hosts). Barge-in (truncate semantics: stop playout, flush buffer, signal upstream) applies to the STT→LLM→TTS DAG path too — wire VAD/turn `speech_started` to a cancel of in-flight TTS.

---

## 4. WaaV-Specific Components (VAD/Silero/Smart-Turn/Turn-Detect/DAG)

### 4.1 Silero VAD
**Bug:** feeds v4 inputs (`input`/`sr`/`h`/`c`, hidden 64) to the **v5** model URL, which expects a unified `state` tensor → `session.run()` fails or silently no-ops state. Also panics (`assert_eq!`) on wrong chunk size; time-based LSTM reset every 5s mid-utterance (should reset on silence); per-frame allocations.
**Fix:** use the unified `state` tensor (not v4 `h`/`c`); reset LSTM on silence, not on a 5s timer; remove the `assert_eq!`-on-chunk-size panic (return error); eliminate per-frame allocations.
**Accuracy targets:** clean F1 ≥ 0.95, onset latency p95 ≤ 150 ms; noisy F1 ≥ 0.85; false-trigger rate per minute reported (barge-in cost). Regression gate: fail if F1 drops > 1.0 absolute pt vs baseline or latency p95 regresses > 20 ms.

### 4.2 Smart-Turn
**Bug:** ONNX inference runs synchronously on the tokio worker (no `spawn_blocking`) so the "<50ms timeout" can't cancel CPU work; streaming mel buffer `clear()`s + recomputes + truncates each call → model fed near-empty, possibly transposed (`[1,80,800]` vs Whisper's `[1,800,80]`); STFT not centered; `add_text_signal` discards the decision → the audio+text ensemble never ensembles.
**Fix:** move inference to `spawn_blocking`; fix the streaming mel buffer (incremental, not clear+recompute+truncate); verify `[1,80,800]` vs `[1,800,80]` layout against the model; center the STFT (reflect-pad); actually combine the text signal in the ensemble.
**Accuracy targets:** audio F1 ≥ 0.85; inference p95 ≤ 50 ms with a concurrent runtime-liveness probe (fail if a single inference blocks the runtime).

### 4.3 Turn-Detect (LiveKit SmolLM)
**Bug:** hand-built chat template (`<|im_start|>user\n…` only, no system/history) is out-of-distribution; configured `max_context_turns`/per-language thresholds never used; silent `Ok(0.3)` fallback masks a broken model; fake hash verification (S10).
**Fix:** correct the chat template (system+history per the model's training distribution); wire `max_context_turns` + per-language thresholds; replace the silent `Ok(0.3)` fallback with an explicit error; real hash verification.
**Accuracy targets:** text F1 ≥ 0.80; each claimed language individually F1 ≥ 0.75 (forces the unused per-language thresholds to matter).

### 4.4 Turn ensemble
**Bug:** mostly-correct FSM, but the text path isn't combined with audio in the same `process()` call, and it double-counts min-speech/min-silence timers against VAD.
**Fix:** combine text+audio in one `process()`; stop double-counting timers.
**Accuracy target:** ensemble F1 ≥ max(audio, text) + 0.02 — proves the ensemble actually combines (directly tests the "never ensembles" bug).

### 4.5 DAG engine (S6 + §2.4 security)
**Bugs/fixes:**
- **Split double-exec** — after `execute_split_branches` (`gateway/src/dag/executor.rs:608`), prune executed branch subtrees from `reachable_nodes` via the existing `prune_subtree`/`can_reach_target` (`:278/:319`), mirroring the router path (`:270`). **Join** must correlate/wait on its declared `sources` instead of firing on first input. Add a split/join template + test.
- **WebhookOutputNode SSRF** — add URL validation (parity with HTTP/WS endpoint nodes) rejecting link-local/RFC1918/DNS-rebind targets.
- **Rhai DoS** — wrap eval in a wall-clock timeout **and** move it off the async thread (`spawn_blocking`) so the timeout can actually preempt CPU work (op-limit alone is insufficient for client-supplied scripts).
- **API-key A/B routing** — replace unanchored `starts_with` over a HashMap with deterministic matching.
- **"Lock-free" claim** — either implement real lock-free data passing or delete the claim (today: `HashMap`+`.clone()` per edge; `rtrb` wrapped in a `Mutex` and dead code).

**Note on accuracy harness gating:** all §4.1–4.4 accuracy tests run **only with the feature flags on** (`silero-vad`, `smart-turn`, `turn-detect`, `turn-ensemble`). Under `default=[]` the stubs return `Ok(0.0)`/`false` and any accuracy number is fiction (S2).

---

## 5. Extreme-TDD & Test Strategy

### 5.1 The blocker that determines the whole strategy
**The live provider path is currently untestable in isolation.** Provider endpoints are compile-time constants (`deepgram.rs:200`, `cartesia/config.rs:104`; **~140 hardcoded URLs** across `src/core/stt` + `src/core/tts`); the only struct crossing dispatch is the flat config; there is **no endpoint override and no provider-options passthrough**. The mock servers in `tests/mock_providers/` (`websocket_mock.rs:262`, `http_mock.rs:158`, `grpc_mock.rs`) already work — but nothing can be pointed at them. **Step 0 of the test strategy = the S1 refactor + an `endpoint_override` field** (already in §2.2). One mechanism, two payoffs: fixes S1 (features reachable) and unblocks the entire mock harness.

### 5.2 The TDD loop (every PR)
1. **RED** — write the failing test first at the lowest sufficient seam: pure logic (URL builder, SSML emitter, HMAC, codec reconcile) → co-located `#[cfg(test)]`; provider wire behavior → cassette-driven contract test in `tests/contract/<provider>.rs`; cross-boundary → integration test in `tests/`. Confirm it fails for the asserted reason, not a compile error.
2. **GREEN** — minimum code to pass. No speculative generality.
3. **REFACTOR** — `cargo fmt`, `cargo clippy -D warnings`, dedupe, with the green test as net.
4. **Gate locally** — `cargo nextest run`, the relevant `--features` build, clippy, `cargo llvm-cov` on changed files.

Every brutal-review bug becomes a RED test before its fix (regression-lock):
- **S6** split double-exec — split→2-branch template asserts each branch's mock STT receives exactly one connect/audio (`connection_count`).
- **S7** XFF spoof — two requests with rotating `X-Forwarded-For` from the same peer → 2nd is 429.
- **S3** codec — TTS via wellsaid `audio_format=linear16` but MP3 output → engine transcodes or errors, never emits MP3 labeled linear16.
- **S10** model hash — `verify_hash` returns `Err` on mismatch (today `warn!`s; `assets.rs:189` returns `"expected_hash_here"`).

### 5.3 Mock / contract / cassette harness
Three layers on the existing mock servers:
- **Layer A — transport engines.** Keep ws/http; add a **real `tonic` gRPC mock** (`tests/mock_providers/grpc_tonic_mock.rs`) for Google v2 `StreamingRecognize` + Tinkoff VoiceKit (the current `grpc_mock` is HTTP/2-simplified).
- **Layer B — cassettes** (`tests/fixtures/cassettes/<provider>/<scenario>.json`) — declarative wire scripts that **assert** client requests (URL params, auth header, frame shapes, finalize message) and emit scripted responses. This makes the mock a **contract test**: if a client regresses (drops `cartesia_version`, sends `keywords=` instead of `keyterm=` on nova-3, omits Azure `speech.config`), a `recv_*` clause fails. Each review bug becomes a cassette assertion.
- **Layer C — recording mode** (`xtask cassette record`) — run a client against the real API once (key-gated), capture frames, scrub secrets (auth headers, base64 audio → length-only). Tag cassettes `"source":"captured"` (trustworthy) vs `"source":"doc"` (hand-written, re-verified live).

A `contract_test!(deepgram_stt, protocol=ws, cassettes=["basic","diarize","keyterms","reconnect","rate_limit"])` macro + `xtask cassette list-missing --fail` (CI fails if any registered provider lacks a cassette) makes "60+ testable" real.

**Required cassette scenarios per class:** STT streaming — `basic_transcript`, `interim_then_final`, `diarization`, `keyterms/redaction`, `mid_stream_reconnect`, `backpressure_drop`, `auth_reject_401`. STT batch — `wav_upload_ok`, `oversize_413`, `retry_backoff`. TTS — `basic_audio`, `codec_declared_vs_actual` (S3), `emotion_applied` (S5), `ssml_style`, `voice_settings`. Protocol-specific asserts the cassette must encode: Azure `speech.config`+framing, Cartesia `cartesia_version`, Deepgram `keyterm` not `keywords`, Tencent host+path string-to-sign, Tinkoff JWT Bearer.

**CAN validate:** request shape, feature reachability (the S1 acid test), result parsing, reconnect state machines, backpressure, codec labeling, chaos resilience. **CANNOT validate:** whether the real provider *accepts* your auth/protocol — that gap is closed only by the gated live job (doc-sourced cassettes flagged for live re-verification).

### 5.4 Live / real e2e
`docker-compose.live.yml` brings up gateway (built `--features turn-ensemble,dag-routing,noise-filter` — never `default`, S2) + LiveKit + Redis + MinIO (egress→S3) + optional SIP endpoint + the Python e2e driver.
- **Keyless tier (every PR):** full DAG STT→LLM→TTS with STT/TTS pointed at cassette mocks via `endpoint_override`, LLM stubbed — exercises the *real* gateway (WS framing, config validation surfacing the TS-SDK `punctuate` vs `punctuation` bug, session lifecycle, voice_manager, DAG topo incl. S6, codec path, backpressure). LiveKit room/token/egress against local self-hosted LiveKit+MinIO (validates token TTL/grants, egress→S3, per-frame backpressure fix). SIP webhook signature verification.
- **Key-gated tier (nightly, `environment: live-secrets`, fork-safe):** real provider auth/protocol acceptance (the only thing the mock can't prove) for the 10 Tier-1 providers + any `"source":"doc"` cassette; OpenAI/Hume Realtime; AWS/Google/Azure SDK paths; WER/MOS accuracy. Reuse the `real_provider_tests.rs` `require_env` pattern (missing key → SKIPPED not FAILED).

**First action:** revoke/rotate the **real Deepgram API key committed** at `tests/live_testing/scripts/waav_test_client.py:49`; replace with `os.environ`; add gitleaks gate. Make each live phase emit JUnit XML + JSON so CI can gate. Live job runs only on `workflow_dispatch`+nightly, only on internal PRs (`github.event.pull_request.head.repo.full_name == github.repository`), keys masked, payloads never logged (shapes/lengths only).

### 5.5 Accuracy testing
Today's `smart_turn_accuracy_test.rs` is Rust-vs-Python **parity**, not labeled accuracy, and no datasets exist. Add vendored, SHA-256-pinned datasets (fetched via `xtask fetch-datasets` with a manifest — mirrors the S10 fix):
- **VAD:** AVA-Speech + LibriSpeech+silence/noise, frame-labeled, 16k mono → `tests/datasets/vad/<set>/{audio,labels.jsonl}`.
- **Turn:** AMI/Switchboard/Fisher cuts at true turn boundaries (complete) vs mid-turn pauses (incomplete), ~500–1000 segments per claimed language; include transcripts for the text path.
- **STT WER:** LibriSpeech test-clean slice + a noisy slice (LibriSpeech+MUSAN).
- **TTS smoke:** fixed phrase set (short/medium/long/numbers/special-chars).

**Metrics & gates** (per §4 targets) output JSON scorecards to `target/accuracy/<component>.json`; baselines committed under `tests/datasets/baselines/`, rebaselined only via reviewed PR. TTS smoke (no MOS model in PR CI): decode bytes, assert format == `audio_format` (catches S3), duration within ±25% of phoneme estimate, no all-silence, no clipping >1%, RMS in band; optional gated MOS (UTMOS/DNSMOS) nightly. STT WER measured live (mock can't produce real transcripts); gate as a per-provider regression (worsen >2 pts = fail). Keep the Rust-vs-Python parity test as a separate numeric-stability check.

### 5.6 CI pipeline (greenfield — none exists)
First: `cargo generate-lockfile` + commit `Cargo.lock`, `--locked` everywhere; pin `rust-toolchain.toml` (channel + clippy/rustfmt/llvm-tools-preview).

- **`.github/workflows/ci.yml`** (PR + push to main; merge gate): `fmt`; `clippy` on the **production** feature set (`turn-ensemble,dag-routing,noise-filter,plugins-dynamic,openapi` `-D warnings`); `cargo-deny` + `cargo-audit`; **gitleaks** (MUST pass — the committed Deepgram key); `typos`; **build-matrix** over `[ "" (default — a test asserts VAD/turn are stubs or fail-loud), silero-vad, smart-turn, turn-detect, turn-ensemble, dag-routing, noise-filter, plugins-dynamic, production set, simd-scalar-only, simd-portable ]`; `unit-and-integration` (`nextest`, excludes `real_*`/`live_*`/`accuracy_*`); **contract** (cassette harness + `cassette list-missing --fail`); **accuracy-fast** (committed small slices); **coverage** (`llvm-cov nextest`, gates below); **sdk** (Python ruff+mypy+pytest, TS eslint+tsc+vitest against the keyless mock gateway — surfaces the protocol bugs as RED).
- **`.github/workflows/cross-platform.yml`** (push to main + nightly): `[ubuntu, macos-14 arm64, windows]` + `x86_64-unknown-linux-musl` (static binary — gate ort/tract/livekit linking under musl) + `aarch64-unknown-linux-gnu`; verify SIMD runtime-detection per arch. musl blocks main.
- **`.github/workflows/nightly.yml`** (schedule + dispatch): full accuracy datasets; criterion benchmarks vs baseline (alert on regression); ASan/miri on the unsafe livekit-audio reinterpret + FFI paths; `cargo-mutants` on security+dispatch modules.
- **`.github/workflows/live.yml`** (dispatch + nightly; `environment: live-secrets`, fork-safe): docker-compose up; real-provider gated suite + live WER/MOS + cassette re-verification for `"source":"doc"`; secrets masked, artifacts scrubbed.

Speed: `nextest` (per-test timeouts; `test_all_providers` needs `--test-threads=1`), `Swatinem/rust-cache` keyed **per feature combo**, `sccache` for ort/tract/livekit. **Required merge checks:** fmt, clippy, deny, secrets, build-matrix (all), unit-and-integration, contract, accuracy-fast, coverage, sdk. cross-platform/nightly/live informational/gated except musl (blocks main).

### 5.7 Coverage gates
`cargo-llvm-cov` measured with the **production** feature set (default would over-report a smaller surface). **Patch coverage ≥ 85%** (the real gate, what TDD produces) + **project non-decreasing ratchet** (measure the honest baseline first — likely 40–60% — don't set an aspirational floor on day one). **Per-component floors:** `plugin/registry.rs` ≥ 90%; auth/rate-limit/SSRF/SIP-webhook/HMAC ≥ 90%; DAG split/join ≥ 85%; codec/reconnect ≥ 85%; provider `from_standard`/URL/auth builders ≥ 80%. Branch coverage tracked + gated on security modules. **Honesty rules:** exclude `#[ignore]`/key-gated tests from the numerator (a line "covered" only by a non-CI test is not covered); `cargo-mutants` nightly on security+dispatch as a coverage-quality backstop (no new surviving mutants in registry/rate-limit/auth/DAG split-join); `cassette list-missing` as a structural provider-completeness gate. **Rollout:** P0 measure+commit baseline → P1 add per-component floors → P2 raise project floor in +5% steps to ~75–80%, enable mutants gate.

---

## 6. Phased Roadmap

> **Sequencing law:** S1 (F0) is additive and lands first; nothing downstream matters until it does. Migrate providers one at a time behind mock-upstream wire tests, never big-bang.

### P0 — Security & Correctness (gate to any production traffic; ~4–6 wks)
**Goal:** make the live path honest and safe.

| WS | Workstream | Depends on | Key files |
|---|---|---|---|
| WS-A | **Config keystone (S1/F0)** — typed `features` + `provider_extras` + `endpoint_override`; `from_base`→`from_standard`; capability matrix; cache-key fix | — | `core/stt/base.rs:420`, `core/tts/base.rs:181`, `plugin/registry.rs:41,303,346`, `handlers/ws/config.rs:92,254,332`, `plugin/metadata.rs` |
| WS-B | **Rate-limit (S7/F3)** — `SmartIpKeyExtractor`→`PeerIpKeyExtractor` (or proxy-CIDR XFF trust); delete `<100000` auto-disable | — | `main.rs:210,214` |
| WS-C | **DAG safety (S6/F4)** — prune branches post-split; join honors `sources`; WebhookOutputNode SSRF; Rhai wall-clock timeout off-thread | — | `dag/executor.rs:270,278,319,608`, `dag/nodes/*` |
| WS-D | **Build correctness (S2/S3)** — VAD/turn in `default` OR stubs fail loudly; codec-reconciliation step | F0 (cleanest) | `Cargo.toml:13`, `core/tts/provider.rs`, `unrealspeech/provider.rs:294` |
| WS-E | **Broken-flagship triage (F2)** — Azure framing+speech.config; Cartesia version (after F0); Tencent host+path; Tinkoff JWT; else mark `unsupported` | Cartesia⇐F0 | `azure/client.rs:442`, `tencent/signature.rs:401`, `tinkoff/grpc.rs:57`, `cartesia/config.rs:97` |
| WS-F | **Plugin loader + FFI** — don't drop loader at startup; `catch_unwind` on FFI init/shutdown/callbacks; path allowlist | — | `main.rs:140-153` |
| WS-G | **SDK contract** — fix TS wire (`punctuate`→`punctuation`, `wire.text`→`transcript`, `session_id`→`stream_id`) or generate from OpenAPI; CI round-trip test | F0 (serde) | `clients_sdk/typescript/*`, `docs/openapi.yaml` |

**Exit:** E1, E2, E3, E4, E6, E9 (above) + Azure/Cartesia/Tencent/Tinkoff either fixed (cassette-validated, live-smoked) or unselectable.

### P1 — Reliability & Availability (~4–6 wks, after P0)
**Goal:** long-lived sessions survive the real world.

| WS | Workstream | Depends on |
|---|---|---|
| WS-H | **Reconnection (S4/F5)** — `ReconnectableStream` everywhere + Hume EVI; storm-control: full-jitter backoff, per-provider circuit breaker, global in-flight-reconnect cap | F0/F2 |
| WS-I | **VAD/turn correctness** — Silero v5 tensor + silence-reset + no panic; Smart-Turn `spawn_blocking` + mel buffer + layout; Turn-Detect template + thresholds; ensemble combines text | S2 features on |
| WS-J | **Model supply-chain (S10/F8)** — pinned SHA-256 per model; `verify_hash` hard-fails; Silero/Smart-Turn pinned; bundle/operator-checksum option | — |
| WS-K | **Lifecycle & leaks** — `voice_manager.stop()` before replace; bounded mpsc for LiveKit frames; LiveKit token TTL + least-privilege grants | — |
| WS-L | **Backpressure** — replace `let _ = try_send(...)` with explicit overflow policy + metric | — |
| WS-M | **Panic isolation (S8/F7)** — `panic="unwind"` (measure cost) + per-task `catch_unwind`/supervision; audit hot-path unwraps (cache SystemTime, JWT AuthClaims, IBM/Alibaba serialize, audio reinterpret) | after F0–F4 |

**Exit:** E5, E7, E8 + a 1-hour soak (repeated voice_manager reconfig + LiveKit traffic) shows flat RSS, no task/FD growth; end-user LiveKit token cannot record/create/list and expires per TTL.

### P2 — Quality, Scale & Truthful Claims (~6–8 wks, overlaps P1 tail)
**Goal:** fast under honest measurement, reproducible, documented, every claim true.

| WS | Workstream |
|---|---|
| WS-N | **Scale/perf (§9)** — replace the 4-permit semaphore; guarantee HTTP/2; fix cache TTL; backpressure; re-benchmark with limiter ON |
| WS-O | **Reproducibility (S9/F9)** — commit `Cargo.lock`; pin toolchain; hermetic Docker for ort/tract/livekit |
| WS-P | **Feature completeness** — Deepgram Aura-2 + keyterm; ElevenLabs/Cartesia WS streaming for TTFB; OpenAI `instructions`; Hume `description`/emotion beyond Cartesia (S5) |
| WS-Q | **Documentation (§8)** — generated API reference; CI-enforced capability matrix; runbooks; architecture; ADRs; doc-tests |
| WS-R | **Truthful claims** — every README line (lock-free DAG, 112k RPS/38MB, auto-reconnect, enum_dispatch 10x, SNR-adaptive/AEC, per-language thresholds) re-validated or deleted; the scorecard becomes a CI-checked status table |

**Exit:** E10, E11, E12 + capability-matrix doc generated/validated in CI (fails build if an advertised feature isn't reachable); every public WS/HTTP message type has a doc-test/schema test; zero README claims unbacked by a test/benchmark.

---

## 7. Risk Register

| Risk | Severity | Mitigation |
|---|---|---|
| **Big-bang S1 refactor** touches the only dispatch struct + ~153 `from_base` sites; one regression silently breaks every provider | Critical | **Do NOT big-bang.** Additive optional field (serde default empty); per-provider migration behind mock-upstream tests asserting the advanced feature is actually invoked; a golden test per provider locks the basic transcript/audio path; flat config keeps working until each provider opts in |
| **Native-build/toolchain** — ort/tract/livekit/AWS SDKs heavy; no lockfile → version float; feature flags change what compiles (green builds that ship stubs, S2) | High | Commit `Cargo.lock` + `rust-toolchain.toml` early; named "production" feature set built in CI; CI fails if default ships VAD/turn stubs; hermetic Docker + cached native artifacts; pin ONNX runtime; startup self-check logs which neural features are compiled in |
| **No-live-keys testing** — P0/P1 fixes can't hit real APIs in CI, so "fixed" may still be wrong on the wire (exactly how Azure/Cartesia shipped broken) | High | Mock upstreams that assert the **exact wire bytes** (would have caught Azure's missing speech.config, Cartesia's missing version); record/replay real traffic; manual key-gated smoke suite before releases; wire-contract tests are the primary gate |
| **`panic=abort` + hot-path unwraps** — one malformed frame aborts the whole process, all tenants | High | P1 WS-M: `catch_unwind`/supervised tasks; audit/remove hot-path unwraps; reconsider unwinding + measure size/perf cost; fuzz the WS parser + audio framing; until isolation lands, deploy with a fast supervisor + readiness probes |
| **Reconnection storms** — once S4 lands, an upstream outage makes thousands of sessions reconnect at once (self-inflicted thundering herd, trips provider rate limits) | High | Build storm-control **into** the design: full-jitter exponential backoff, per-provider circuit breaker, global concurrency cap (token bucket), capped attempts, "degraded" event; load-test 1000+ simultaneous reconnects as a P1 exit |
| **Security fixes subtly wrong** — CIDR allowlist trusts wrong hop; SSRF bypassed by DNS-rebind; Rhai timeout doesn't cancel CPU work | High | Adversarial tests (XFF spoof, SSRF to link-local/RFC1918/rebind, Rhai infinite + busy loop); move Rhai off the async thread so the timeout preempts; run `/security-review` on the rate-limit + DAG changes before merge |
| **Benchmark credibility** — "112k RPS / 38MB" almost certainly ran with the limiter auto-disabled and not through the 4-permit REST path | Medium | Re-benchmark as a P2 deliverable with documented methodology (config dump, hardware, limiter ON, real REST path, cache under churn); publish honest numbers + remove old claims in the **same** PR (no over-promise window) |

---

## 8. Extreme-Documentation Plan

Goal: docs that cannot silently drift from code, with the per-provider capability matrix as the centerpiece (the antidote to S1's "breadth is cosmetic"). Current state: ~30 scattered `.md` files + `docs/openapi.yaml` + partial utoipa wiring; no ADRs, no runbooks, no enforced doc-tests.

1. **Standardized API reference (generated).** Make utoipa/OpenAPI the single source of truth: annotate every wire struct (`STTConfig:420`, `TTSConfig:181`, all WS Config/transcript/audio messages) with `#[derive(ToSchema)]`; generate `docs/openapi.yaml` in CI; **fail CI on drift**. Generate the TS/Python/Widget SDK clients from that spec (structurally prevents the `punctuate`/`transcript` class of bug).
2. **Per-provider capability matrix (CI-enforced — the keystone doc).** Machine-readable table, one row per provider: transport, auth status, and per-feature reachability **on the live path** (diarization, keyterms, redaction, voice-settings, emotion, instructions, SSML, reconnection, codec). "Reachable" derived programmatically from whether `from_standard` consumes the corresponding field — green only if an integration test invokes it through `create_stt`/`create_tts`. Turns the review's §3 scorecard into a living, build-checked artifact; **CI fails if a provider advertises a feature the dispatch path can't reach.**
3. **Runbooks** (`docs/runbooks/`, absent today). One per scenario: provider outage + reconnection storm; rate-limit incident (XFF/proxy CIDR); cache memory growth; voice_manager reconfig leak; LiveKit token revocation/TTL; model-hash-mismatch startup failure; panic/restart triage; SSRF/abuse report. Format: symptom → metric → diagnosis → action → rollback.
4. **Architecture docs.** Top-level `architecture.md`: the dispatch/factory boundary (S1 chokepoint), DAG execution model (incl. split/join post-fix), realtime/LiveKit/SIP, request-manager + cache data path, threading model (async vs dedicated OS threads for noise-filter/ONNX), with a data-flow diagram showing where `provider_extras` crosses the boundary.
5. **ADRs** (`docs/adr/`, absent). Backfill load-bearing decisions: ADR-0001 typed `provider_extras` vs per-provider config enums (S1); ADR-0002 PeerIp vs SmartIp XFF trust; ADR-0003 reconnection + storm-control; ADR-0004 panic=abort vs unwind + isolation; ADR-0005 model integrity/pinning; ADR-0006 cache TTL (moka-native vs lazy expiry).
6. **Changelog.** Adopt Keep-a-Changelog + conventional commits; every provider status change and P0/P1 fix gets an entry; release notes generated.
7. **Doc-tests & sync enforcement.** `cargo test --doc` on every public config builder and wire-message type showing exact JSON accepted/emitted (executable wire-contract examples); a CI "docs-drift" gate regenerates OpenAPI + capability matrix and fails on diff; link-check `docs/`. **A provider is not "done" until its matrix row, its doc-test, and its mock-upstream wire test all pass** — docs are a release gate, not an afterthought.

---

## 9. Scalability / Performance (P2 WS-N; measurement starts in P0)

1. **Replace the 4-permit "pool."** **Verified:** `gateway/src/core/state.rs:83` constructs every TTS provider's request manager with `ReqManager::new(4)` → `Semaphore::new(4)` (`req_manager.rs:402`) — a concurrency cap mislabeled a pool; the 5th REST call queues. Fix: drive `max_concurrent_requests` from config (`ReqManagerConfig` already exposes `low_latency(20)`/`high_throughput(50)`), sized per provider's documented concurrency limit; the underlying `reqwest::Client` already pools connections (`pool_max_idle_per_host=512`). Make the cap a real backpressure signal (429/queue-depth metric), not silent queuing. Load-test to find per-provider values.
2. **Guarantee HTTP/2.** `create_optimized_client` (`req_manager.rs:409`) sets `http2_*` windows but no `.http2_prior_knowledge()`/`.http2_only()` → ALPN fallback to h1 silently makes all h2 tuning dead config. Fix: set `http2_prior_knowledge` for providers known to speak h2 (or assert + metric the negotiated version). Add a per-provider "negotiated protocol" metric.
3. **Fix cache TTL.** `store.rs` uses moka but never sets moka-native `time_to_live`; it stores `expires_at` per entry and evicts **lazily** only on get/exists → expired-but-untouched entries linger (incompatible with the 500MB/5M-entry cache and the "38MB RSS" claim). Fix: `time_to_live()`/`time_to_idle()` so moka evicts proactively, or a periodic sweep; keep the size-weigher. Re-measure RSS under churn.
4. **Backpressure.** (a) Replace silent `let _ = try_send(...)` transcript drops on bounded-256 channels with an explicit overflow policy + metric. (b) Replace the one detached `tokio::spawn` per inbound LiveKit frame with a bounded mpsc + single consumer (backpressure + ordering). The request-manager semaphore should surface queue depth for load shedding.
5. **Honest re-benchmarking.** The "112k RPS / 38MB RSS" ran with the limiter auto-disabled (`main.rs:210`) and not through the 4-permit REST path. Re-benchmark: limiter ON at realistic rps; traffic through the actual TTS REST path; cache under sustained churn; documented config/hardware. Report p50/p95/p99 latency + throughput + steady-state RSS, not a single peak.
6. **Load targets (SLOs, set with product, encoded as CI benchmark thresholds):** N concurrent live voice sessions/node; TTS REST p99 added-latency budget; STT streaming e2e p95; cache hit-rate target; steady-state RSS ceiling under full cache; survive an upstream-outage reconnection storm of K simultaneous sessions within the backoff/concurrency budget.
7. **Profiling.** Flamegraphs (pprof/perf) on the TTS REST path and DAG executor (which rebuilds a fresh Rhai `Engine` per execution and reconnects providers per call — per-call alloc/connection costs to eliminate); `tokio-console` for task/stall visibility (the per-frame spawn + synchronous ONNX/Rhai-on-async-thread); heaptrack for the RSS claim; per-frame allocation hunting in Silero/Smart-Turn. Criterion benches gate regressions in CI.

---

## 10. Implementation Workflows (the executable build plan)

Each workflow is an orchestrated, multi-agent task with a hard validation gate. Run them roughly in order; W1 unblocks all others.

### W1 — `keystone-config-passthrough`
- **Inputs:** `core/stt/base.rs:420`, `core/tts/base.rs:181`, `plugin/registry.rs:41,303,346`, `handlers/ws/config.rs:92,254,332`, `plugin/metadata.rs`, the standardized-type design (§2).
- **Phases:** (1) Add `StandardSTTConfig`/`StandardTTSConfig` + `SttFeatures`/`TtsFeatures` + `ProviderExtras` + `endpoint_override` + `From` shims (additive). (2) Add `ProviderCaps` to `ProviderMetadata` + negotiation function. (3) Switch WS/REST/DAG entry points to deserialize into the standardized config (down-convert via `From`). (4) Extend `compute_tts_config_hash` to the full effective config. (5) Characterization tests for the current flat path (regression net).
- **Agents:** core-types, dispatch/registry, wire/serde-compat, test-author.
- **Gate:** flat-path golden tests unchanged; a negotiation unit test reports degradations; old wire shape deserializes unchanged; cache-key test proves emotion/voice_settings no longer collide.

### W2 — `provider-migration` (parameterized, ~60 runs)
- **Inputs:** per-provider config struct + builders; the migration checklist (§3.1); W1 merged.
- **Phases:** (1) `from_base`→`from_standard` reading typed features + `provider_extras.merge_into`. (2) `caps()` self-report. (3) `negotiated_format()`. (4) Result-fidelity (words/speakers/entities). (5) Wire `ReconnectableStream`. (6) Cassette(s) + contract test.
- **Agents:** provider-engineer, cassette-author, capability-validator.
- **Gate:** `caps()` ⟺ fields actually read (coverage test); contract test green; `list-missing` no longer flags this provider; advanced-feature integration test invokes the feature through `create_stt`/`create_tts`. **Order:** Deepgram, ElevenLabs, Hume, AWS, AssemblyAI, Cartesia first.

### W3 — `broken-provider-repair`
- **Inputs:** §3.2 fix specs; reference vectors/protos.
- **Phases:** per provider — (1) implement the protocol fix; (2) auth/signing unit test vs fixture; (3) cassette (recorded if a key exists, else doc-tagged); (4) live smoke (gated). Until validated, mark `unsupported`.
- **Agents:** protocol-engineer (Azure/Tencent/Tinkoff/Cartesia), security-reviewer.
- **Gate:** signing/handshake unit test passes against the reference vector; live smoke connects + returns one transcript/audio frame, OR the provider is registered `unsupported` and unselectable.

### W4 — `rate-limit-and-dag-security`
- **Inputs:** `main.rs:210,214`; `dag/executor.rs:270,278,319,608`; DAG node files.
- **Phases:** (1) `PeerIpKeyExtractor` + delete auto-disable. (2) Prune split branches; join honors `sources`. (3) WebhookOutputNode SSRF. (4) Rhai wall-clock timeout off-thread. (5) Adversarial tests.
- **Agents:** security-engineer, dag-engineer, test-author.
- **Gate:** XFF-spoof test → 429; split test → each branch fires exactly once; SSRF test rejects link-local/RFC1918/rebind; infinite-loop Rhai killed by wall-clock; `/security-review` clean.

### W5 — `codec-and-build-honesty`
- **Inputs:** `core/tts/provider.rs` chunker; `unrealspeech/provider.rs:294`, speechify/wellsaid; `Cargo.toml:13`; stub modules.
- **Phases:** (1) codec-reconciliation step keyed on `negotiated_format()`. (2) VAD/turn in `default` OR stubs fail loudly at startup. (3) decode/format-sniff tests.
- **Agents:** audio-engineer, build-config-engineer.
- **Gate:** TTS decode test for wellsaid/speechify/unrealspeech passes (no noise); `cargo build --release` (no flags) either has working VAD or refuses to start; emitted-frame format == byte sniff.

### W6 — `reconnection-supervisor`
- **Inputs:** `realtime/openai/client.rs:587-746`; `ReconnectionConfig` (`base.rs:87-108`); all streaming providers + Hume EVI.
- **Phases:** (1) extract `ReconnectableStream<T>` (intentional-disconnect flag, backoff+jitter, `restore_session`). (2) wire into every streaming provider + Hume. (3) storm-control (circuit breaker + global cap). (4) chaos tests.
- **Agents:** reliability-engineer, chaos-test-author.
- **Gate:** kill-mid-stream chaos test recovers within `max_delay` for each provider; 1000-simultaneous-reconnect test stays under the concurrency cap.

### W7 — `ml-correctness-and-integrity`
- **Inputs:** Silero/Smart-Turn/Turn-Detect/ensemble code; `turn_detect/assets.rs:189`; vendored datasets.
- **Phases:** (1) Silero v5 tensor + silence-reset + no panic. (2) Smart-Turn `spawn_blocking` + mel buffer + layout. (3) Turn-Detect template + thresholds. (4) ensemble combines text. (5) pinned SHA-256 + hard-fail `verify_hash`. (6) accuracy harness + baselines.
- **Agents:** ml-engineer, accuracy-test-author, security-reviewer.
- **Gate:** accuracy targets met (§4); tampered-model file rejected at startup; inference p95 ≤ 50 ms with the liveness probe passing.

### W8 — `lifecycle-and-realtime-hardening`
- **Inputs:** voice_manager reconfig; LiveKit token (`room_handler.rs`), SIP (`sip_handler.rs`); per-frame spawn; `panic` profile; hot-path unwraps; plugin loader (`main.rs:140-153`).
- **Phases:** (1) `voice_manager.stop()` before replace. (2) LiveKit least-privilege + TTL; SIP max_participants. (3) bounded mpsc for LiveKit frames + LE byte decode. (4) plugin loader lifecycle + FFI `catch_unwind`. (5) `panic="unwind"` + per-task isolation + unwrap audit. (6) barge-in/truncate (OpenAI + DAG path).
- **Agents:** reliability-engineer, livekit-engineer, security-engineer.
- **Gate:** 1-hour soak shows flat RSS + no task/FD growth; end-user token can't record/create/list + expires per TTL; fuzzed malformed first-message doesn't abort; plugin survives load→use→shutdown.

### W9 — `sdk-and-openapi`
- **Inputs:** `docs/openapi.yaml`; TS/Python/Widget SDKs; W1 wire schema.
- **Phases:** (1) annotate wire structs with `ToSchema`; generate OpenAPI in CI with drift gate. (2) generate TS/Python/Widget clients. (3) SDK contract tests against the keyless mock gateway.
- **Agents:** api-docs-engineer, sdk-engineer.
- **Gate:** OpenAPI drift gate green; TS + Python streaming tests connect and receive a non-empty transcript in CI.

### W10 — `catalog-system`
- **Inputs:** existing `list_voices`/`available_voices` impls; discovery URLs; W2 `discover_*`.
- **Phases:** (1) `catalog.json` + `build.rs` codegen. (2) `discover_models`/`discover_voices` + `/v1/providers/{p}/voices|models` + `/v1/catalog`. (3) `validate_catalog` test + nightly drift PR.
- **Agents:** catalog-engineer, ci-engineer.
- **Gate:** `validate_catalog` green (every referenced voice exists; codecs match `negotiated_format()`); nightly drift job opens a PR on change; Aura-2 present.

### W11 — `ci-coverage-reproducibility`
- **Inputs:** §5.6/§5.7; `Cargo.lock`; `rust-toolchain.toml`.
- **Phases:** (1) commit lockfile + pin toolchain + `--locked`. (2) ci.yml/cross-platform.yml/nightly.yml/live.yml. (3) coverage gates + `cargo-mutants` + `cassette list-missing`. (4) gitleaks + revoke the committed Deepgram key.
- **Agents:** ci-engineer, security-engineer.
- **Gate:** all required checks defined + green on a sample PR; default-build stub-assertion test wired; committed key revoked + gitleaks blocks re-introduction; `cargo build --locked` reproduces.

### W12 — `perf-and-scale`
- **Inputs:** §9; `state.rs:83`, `req_manager.rs:402,409`, `cache/store.rs`.
- **Phases:** (1) config-driven concurrency + queue-depth metric. (2) HTTP/2 guarantee + negotiated-protocol metric. (3) moka-native TTL. (4) backpressure (transcripts + LiveKit frames). (5) honest re-benchmark + SLOs as CI thresholds. (6) profiling wiring.
- **Agents:** perf-engineer, observability-engineer.
- **Gate:** re-benchmark with limiter ON through the real REST path published with methodology; cache RSS stable under churn; benchmark regression gate active; README updated in the same PR (no over-promise window).

### W13 — `documentation-system`
- **Inputs:** §8; W9 OpenAPI; W2/W3 capability data.
- **Phases:** (1) generated API reference. (2) CI-enforced capability matrix. (3) runbooks + architecture + ADRs. (4) doc-tests + docs-drift gate + link-check.
- **Agents:** docs-engineer, capability-validator.
- **Gate:** capability-matrix doc generated/validated in CI (fails build if an advertised feature is unreachable); every public wire type has a doc-test; docs-drift gate green; every retained README claim backed by a test/benchmark.

---

### Bottom line
The engineering is real; the gating problem is architectural. **Run W1 first** (additive, per-provider), close the P0 security/correctness holes (W3–W5), then earn reliability (W6–W8) and honest scale/claims/docs (W11–W13). Every "done" is a passing wire-contract test plus a green, CI-checked row in the capability matrix. Fix S1 and the 60-provider breadth stops being cosmetic and becomes the product.
