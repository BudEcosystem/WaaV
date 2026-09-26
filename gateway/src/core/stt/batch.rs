//! Batched / async STT (P5, additive).
//!
//! A canonical, provider-agnostic batch envelope for prerecorded transcription. It WRAPS the
//! existing [`StandardSTTConfig`](super::standard::StandardSTTConfig) (so `base` + the full typed
//! [`SttFeatures`](super::standard::SttFeatures) + `extras` are reused VERBATIM — diarization,
//! keyterms, redaction, …) plus a small [`BatchFeatures`] for the batch-exclusive and
//! streaming-only-gap knobs (alternatives / detect_language / summarize / topics / intents /
//! paragraphs / utterances), an audio source (URL or inline bytes), and an optional async
//! `callback_url`.
//!
//! # The CRITICAL batch responsibility
//!
//! The streaming `from_standard` mappers deliberately DROP a handful of features that ARE
//! batch-capable on the prerecorded/async wire (documented in-repo: Deepgram `alternatives` +
//! `detect_language` at `deepgram.rs`; AssemblyAI `sentiment`/`entity_detection`/summarization at
//! `assemblyai/config.rs`). The batch dispatcher RE-ENABLES them on the prerecorded wire — that is
//! the entire reason this envelope exists separately from the streaming path.
//!
//! # Per-provider dispatch (build-only here; the handler performs the HTTP)
//!
//! The request *builders* in this module are pure functions returning the exact provider-native
//! request shape (Deepgram prerecorded `POST /v1/listen` URL+query+body; AssemblyAI async
//! `POST /v2/transcript` JSON; OpenAI `POST /v1/audio/transcriptions` multipart fields). They are
//! unit-testable WITHOUT a key — the wire-assert that the streaming-gap features actually reach the
//! wire. The [`crate::handlers::transcribe`] handler resolves the credential and executes them.
//!
//! Degrade rule: a provider lacking a batch feature emits a `config_warning` and proceeds — NEVER a
//! 400. The canonical [`BatchStatus`] is the AssemblyAI-aligned superset.

use super::standard::{StandardSTTConfig, TranslationConfig};
use serde::{Deserialize, Serialize};

const BATCH_BASE_URL_SCHEMES: &[&str] = &["http", "https"];
const BATCH_AUDIO_SOURCE_URL_SCHEMES: &[&str] = &["http", "https"];
const BATCH_CALLBACK_URL_SCHEMES: &[&str] = &["http", "https"];
/// Maximum decoded inline audio accepted by `POST /transcribe/batch`.
///
/// This keeps in-process base64 decoding bounded and aligns with the OpenAI
/// batch transcription upload ceiling documented in this module.
pub const MAX_BATCH_INLINE_AUDIO_BYTES: usize = 25 * 1024 * 1024;
/// JSON body budget for base64 inline audio plus envelope overhead.
pub const BATCH_JSON_BODY_LIMIT_BYTES: usize =
    ((MAX_BATCH_INLINE_AUDIO_BYTES + 2) / 3) * 4 + (1024 * 1024);

// =============================================================================
// Envelope
// =============================================================================

/// A canonical batch transcription request (the body of `POST /transcribe/batch`).
///
/// (No `ToSchema` derive: it `#[serde(flatten)]`s `StandardSTTConfig` + an `#[serde(untagged)]`
/// audio source, which utoipa's derive cannot compose — same reason `ProviderExtras` hand-writes
/// its schema. The OpenAPI surface documents the leaf types [`BatchFeatures`]/[`BatchJob`].)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchTranscribeRequest {
    /// Audio source: exactly one of a remote `url` or inline `audio_base64` bytes.
    #[serde(flatten)]
    pub audio: BatchAudioSource,
    /// REUSED VERBATIM: provider / api_key / language / model + full typed [`SttFeatures`] + extras.
    #[serde(flatten)]
    pub config: StandardSTTConfig,
    /// Batch-only + streaming-gap knobs the dispatcher MUST enable on the prerecorded/async wire.
    #[serde(default)]
    pub batch: BatchFeatures,
    /// Async webhook: when set, the gateway returns `{job_id,status:"queued"}` and POSTs the result
    /// here on completion. When absent, the result is poll-stored under `GET …/{job_id}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callback_url: Option<String>,
    /// Callback HTTP verb (`POST` default, or `PUT` — mirrors Deepgram `callback_method`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callback_method: Option<String>,
    /// Optional in-band translation (deliverable #1). Batch ENABLES AssemblyAI/Speechmatics/Gladia
    /// translation (which is batch-only on AssemblyAI). Reuses the canonical [`TranslationConfig`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translation: Option<TranslationConfig>,
}

/// Audio source for a batch job: a remote URL or inline base64 bytes (mutually exclusive).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BatchAudioSource {
    /// Remote audio URL (Deepgram `{"url":…}` / AssemblyAI `{"audio_url":…}`; not OpenAI).
    Url {
        /// HTTP(S) URL of the audio file.
        url: String,
    },
    /// Inline audio bytes, base64-encoded (Deepgram raw body / OpenAI multipart `file` /
    /// AssemblyAI `POST /v2/upload`).
    Bytes {
        /// Base64-encoded audio payload.
        audio_base64: String,
        /// MIME type of the audio (e.g. `audio/wav`); defaults to `audio/wav` when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_type: Option<String>,
    },
}

impl BatchAudioSource {
    /// The remote URL, if this is a URL source.
    pub fn url(&self) -> Option<&str> {
        match self {
            BatchAudioSource::Url { url } => Some(url),
            BatchAudioSource::Bytes { .. } => None,
        }
    }
    /// The inline bytes (base64) + content-type, if this is a bytes source.
    pub fn bytes(&self) -> Option<(&str, &str)> {
        match self {
            BatchAudioSource::Bytes {
                audio_base64,
                content_type,
            } => Some((
                audio_base64.as_str(),
                content_type.as_deref().unwrap_or("audio/wav"),
            )),
            BatchAudioSource::Url { .. } => None,
        }
    }
}

/// Batch-capable features the streaming path drops (Deepgram prerecorded / AssemblyAI async /
/// OpenAI verbose_json). Every field `Option`, additive, defaults to "don't request".
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct BatchFeatures {
    /// Paragraph formatting (Deepgram `paragraphs` / AssemblyAI response paragraphs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paragraphs: Option<bool>,
    /// Utterance segmentation (Deepgram `utterances` / AssemblyAI `utterances`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub utterances: Option<bool>,
    /// Summarization (Deepgram `summarize=v2` / AssemblyAI `summarization`). OpenAI: unsupported→warn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summarize: Option<bool>,
    /// Topic detection (Deepgram `topics` / AssemblyAI `iab_categories`). OpenAI: unsupported→warn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topics: Option<bool>,
    /// Intent detection (Deepgram `intents`). AssemblyAI: no equiv→warn. OpenAI: unsupported→warn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intents: Option<bool>,
    /// Spoken-language detection — a STREAMING GAP for Deepgram, enabled here (Deepgram
    /// `detect_language` / AssemblyAI `language_detection` / OpenAI verbose_json returns `language`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detect_language: Option<bool>,
    /// N-best alternatives — a STREAMING GAP for Deepgram, enabled here (Deepgram `alternatives=N`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alternatives: Option<u8>,
}

// =============================================================================
// Job object
// =============================================================================

/// Provider-agnostic batch-job status. The canonical superset is AssemblyAI-aligned; Deepgram
/// (which has no status enum) maps `request_id`→`job_id` and the callback POST flips to `Completed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum BatchStatus {
    /// Accepted, not yet started.
    Queued,
    /// Provider is transcribing.
    Processing,
    /// Done; `result` is populated.
    Completed,
    /// Failed; `error` is populated.
    Error,
}

/// A uniform batch job: the shape returned by `POST /transcribe/batch` (job created) and
/// `GET /transcribe/batch/{job_id}` (poll).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct BatchJob {
    /// Gateway-issued job id (maps to the provider `request_id`/transcript id).
    pub job_id: String,
    /// Current status.
    pub status: BatchStatus,
    /// Canonical transcript + features, present once `Completed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    /// Error detail, present on `Error`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Non-fatal config warnings (degraded/omitted features) surfaced to the caller (never a 400).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub config_warnings: Vec<String>,
}

impl BatchJob {
    /// A freshly-queued job.
    pub fn queued(job_id: impl Into<String>, config_warnings: Vec<String>) -> Self {
        Self {
            job_id: job_id.into(),
            status: BatchStatus::Queued,
            result: None,
            error: None,
            config_warnings,
        }
    }
    /// A completed job carrying the canonical result.
    pub fn completed(
        job_id: impl Into<String>,
        result: serde_json::Value,
        config_warnings: Vec<String>,
    ) -> Self {
        Self {
            job_id: job_id.into(),
            status: BatchStatus::Completed,
            result: Some(result),
            error: None,
            config_warnings,
        }
    }
    /// An errored job.
    pub fn errored(job_id: impl Into<String>, error: impl Into<String>) -> Self {
        Self {
            job_id: job_id.into(),
            status: BatchStatus::Error,
            result: None,
            error: Some(error.into()),
            config_warnings: Vec::new(),
        }
    }
}

// =============================================================================
// Provider-native request builders (pure; HTTP is performed by the handler)
// =============================================================================

/// The concrete HTTP request a builder produces (method + URL + headers + body), provider-agnostic
/// so the handler can execute any of them with one `reqwest` call.
#[derive(Debug, Clone)]
pub struct BatchHttpRequest {
    /// HTTP method (`POST`).
    pub method: String,
    /// Fully-qualified URL including the query string (the streaming-gap features ride here for DG).
    pub url: String,
    /// Header (name, value) pairs (auth + content-type).
    pub headers: Vec<(String, String)>,
    /// The request body.
    pub body: BatchHttpBody,
}

/// The body variants a batch builder can produce.
#[derive(Debug, Clone)]
pub enum BatchHttpBody {
    /// No body (e.g. a URL-source GET would use this; unused for submits here).
    Empty,
    /// A JSON body (Deepgram URL submit, AssemblyAI submit).
    Json(serde_json::Value),
    /// Raw audio bytes with a content-type (Deepgram bytes submit, AssemblyAI `/v2/upload`).
    Raw {
        /// Raw audio bytes.
        bytes: Vec<u8>,
        /// MIME type.
        content_type: String,
    },
    /// Multipart text fields + an audio `file` part (OpenAI). Fields are `(name, value)`; the file
    /// is `(field_name, filename, content_type, bytes)`.
    Multipart {
        /// Non-file text fields.
        fields: Vec<(String, String)>,
        /// The audio file part, if any.
        file: Option<(String, String, String, Vec<u8>)>,
    },
}

/// What a built batch submission needs at execution time: the HTTP request, whether the provider
/// is asynchronous (callback/poll) vs synchronous (inline result), and the JSON pointer to the
/// provider job-id in the submit response (for async providers).
#[derive(Debug, Clone)]
pub struct BatchSubmission {
    /// The submit request.
    pub request: BatchHttpRequest,
    /// True if the provider returns a job handle to poll / callback (Deepgram-with-callback,
    /// AssemblyAI). False if the result is returned inline by the submit call (OpenAI,
    /// Deepgram-without-callback).
    pub is_async: bool,
    /// Non-fatal degrade warnings accumulated while building (unsupported features omitted).
    pub config_warnings: Vec<String>,
}

/// Audio a builder has been handed, already decoded.
///
/// The `/transcribe/batch` route carries audio as base64 inside a JSON envelope; the
/// `/v1/audio/transcriptions` route has the bytes in hand already. Passing the latter through the
/// former would cost two extra copies of every upload AND impose
/// [`MAX_BATCH_INLINE_AUDIO_BYTES`] (25 MB) on a route whose own ceiling is 50 MB — refusing
/// uploads that work today. So each builder takes this instead, and the base64 decode happens
/// exactly once, in the one caller that has base64.
pub enum ResolvedAudio {
    /// A remote URL the vendor fetches itself.
    Url(String),
    /// Decoded bytes plus their media type.
    Bytes {
        /// The audio.
        bytes: Vec<u8>,
        /// Its media type, e.g. `audio/wav`.
        content_type: String,
    },
}

impl ResolvedAudio {
    /// Decode a request's audio source once, applying the inline-size ceiling to base64 only.
    pub fn from_source(provider: &str, audio: &BatchAudioSource) -> Result<Self, String> {
        match audio {
            BatchAudioSource::Url { url } => {
                Ok(Self::Url(validate_batch_audio_source_url(provider, url)?))
            }
            BatchAudioSource::Bytes {
                audio_base64,
                content_type,
            } => Ok(Self::Bytes {
                bytes: b64_decode(audio_base64)?,
                content_type: content_type.clone().unwrap_or_else(|| "audio/wav".into()),
            }),
        }
    }

    /// A WAV the gateway decoded and re-encoded itself.
    pub fn wav(bytes: Vec<u8>) -> Self {
        Self::Bytes {
            bytes,
            content_type: "audio/wav".to_string(),
        }
    }
}

/// Build the Deepgram prerecorded submission (`POST /v1/listen`). Enables the streaming-gap
/// features (`alternatives`, `detect_language`) plus the batch-exclusive ones on the query string —
/// the wire-level proof that batch unlocks what streaming drops.
///
/// `base_url` is the scheme://host (default `https://api.deepgram.com`), `api_key` the resolved
/// credential. Returns the submission; `is_async` is `true` iff `callback_url` is set.
pub fn build_deepgram_prerecorded(
    req: &BatchTranscribeRequest,
    api_key: &str,
    base_url: &str,
) -> Result<BatchSubmission, String> {
    let audio = ResolvedAudio::from_source("deepgram", &req.audio)?;
    build_deepgram_prerecorded_with(req, audio, api_key, base_url)
}

/// As [`build_deepgram_prerecorded`], for a caller that already holds the decoded audio.
pub fn build_deepgram_prerecorded_with(
    req: &BatchTranscribeRequest,
    audio: ResolvedAudio,
    api_key: &str,
    base_url: &str,
) -> Result<BatchSubmission, String> {
    validate_batch_base_url("deepgram", base_url)?;

    let std = &req.config;
    let f = &std.features;
    let b = &req.batch;
    let mut warnings = Vec::new();
    let mut qs: Vec<(String, String)> = Vec::new();

    // Vendor contract: `model` and `language` are both OPTIONAL on `/v1/listen`; Deepgram applies
    // its own default when either is absent. An unset value is therefore omitted rather than
    // replaced by one nobody chose (an empty model used to become `nova-2`), mirroring the
    // streaming client.
    let model = std.base.model.trim();
    if !model.is_empty() {
        qs.push(("model".into(), model.to_string()));
    }
    let language = std.base.language.trim();
    if !language.is_empty() && language != "auto" {
        qs.push(("language".into(), language.to_string()));
    }
    qs.push(("punctuate".into(), std.base.punctuation.to_string()));

    // --- streaming-gap features RE-ENABLED on the prerecorded wire (the critical bit) ----------
    if let Some(n) = b.alternatives {
        qs.push(("alternatives".into(), n.to_string()));
    }
    if b.detect_language == Some(true) || f.language_detection == Some(true) {
        qs.push(("detect_language".into(), "true".into()));
    }
    // --- batch-exclusive features --------------------------------------------------------------
    if b.summarize == Some(true) {
        qs.push(("summarize".into(), "v2".into()));
    }
    if b.topics == Some(true) {
        qs.push(("topics".into(), "true".into()));
    }
    if b.intents == Some(true) {
        qs.push(("intents".into(), "true".into()));
    }
    if b.paragraphs == Some(true) {
        qs.push(("paragraphs".into(), "true".into()));
    }
    if b.utterances == Some(true) {
        qs.push(("utterances".into(), "true".into()));
    }
    // --- reused typed SttFeatures (already batch-valid on Deepgram) -----------------------------
    if f.diarization == Some(true) {
        qs.push(("diarize".into(), "true".into()));
    }
    if f.smart_format == Some(true) {
        qs.push(("smart_format".into(), "true".into()));
    }
    if f.sentiment == Some(true) {
        qs.push(("sentiment".into(), "true".into()));
    }
    if f.entity_detection == Some(true) {
        qs.push(("detect_entities".into(), "true".into()));
    }
    if f.numerals == Some(true) {
        qs.push(("numerals".into(), "true".into()));
    }
    if f.multichannel == Some(true) {
        qs.push(("multichannel".into(), "true".into()));
    }
    if let Some(redact) = &f.redaction {
        for r in redact {
            qs.push(("redact".into(), r.clone()));
        }
    }
    if let Some(kw) = &f.keyterms {
        for k in kw {
            qs.push(("keyterm".into(), k.clone()));
        }
    }
    // Async callback.
    let is_async = req.callback_url.is_some();
    if let Some(cb) = &req.callback_url {
        let callback = validate_batch_callback_url("deepgram", cb)?;
        qs.push(("callback".into(), callback));
        let method = req
            .callback_method
            .as_deref()
            .map(normalize_batch_callback_method)
            .transpose()?
            .unwrap_or_else(|| "POST".into());
        qs.push(("callback_method".into(), method));
    }
    // Translation degrade (Deepgram prerecorded translate is EN-target only via batch; treat an
    // arbitrary-target request as a warn — transcript still returned).
    if let Some(t) = &std.translation {
        warnings.extend(t.warnings_for("deepgram", false));
    }

    let host = base_url.trim_end_matches('/');
    let mut parsed = url::Url::parse(&format!("{host}/v1/listen"))
        .map_err(|e| format!("deepgram base url invalid: {e}"))?;
    {
        let mut pairs = parsed.query_pairs_mut();
        for (k, v) in &qs {
            pairs.append_pair(k, v);
        }
    }
    let url = parsed.to_string();

    let (body, content_type) = match audio {
        ResolvedAudio::Url(audio_url) => (
            BatchHttpBody::Json(serde_json::json!({ "url": audio_url })),
            "application/json".to_string(),
        ),
        ResolvedAudio::Bytes {
            bytes,
            content_type,
        } => (
            BatchHttpBody::Raw {
                bytes,
                content_type: content_type.clone(),
            },
            content_type,
        ),
    };

    let request = BatchHttpRequest {
        method: "POST".into(),
        url,
        headers: vec![
            ("Authorization".into(), format!("Token {api_key}")),
            ("Content-Type".into(), content_type),
        ],
        body,
    };
    Ok(BatchSubmission {
        request,
        is_async,
        config_warnings: warnings,
    })
}

/// The token the shared language mapper emits for AssemblyAI when the canonical language is
/// `auto` (see `core::lang::mappers`). It is a marker for "detect", not a language code, so it
/// must never reach `language_code`.
pub(crate) const ASSEMBLYAI_DETECT_SENTINEL: &str = "__detect__";

/// Whether `model` is one AssemblyAI's prerecorded `speech_models` field can carry.
///
/// The documented values are `universal-3-5-pro` and `universal-2`; the family prefix is accepted
/// so a newly shipped `universal-*` prerecorded model is passed through rather than dropped. The
/// streaming family shares the prefix (`universal-streaming-english`, …) and is excluded: it names
/// a model the prerecorded endpoint does not serve.
fn assemblyai_prerecorded_model(model: &str) -> bool {
    let m = model.trim().to_ascii_lowercase();
    m.starts_with("universal-") && !m.starts_with("universal-streaming")
}

/// Build the AssemblyAI async submission (`POST /v2/transcript`). Enables the batch-only
/// Speech-Understanding models (`summarization`, `iab_categories`, `entity_detection`,
/// `sentiment_analysis`, `language_detection`, …) that the v3 streaming WS cannot do. AssemblyAI is
/// always async (poll `GET /v2/transcript/{id}` or `webhook_url`).
///
/// NOTE: a bytes source requires a prior `POST /v2/upload` (raw) to obtain an `audio_url`; this
/// builder targets the URL form. The handler performs the upload step for a bytes source.
pub fn build_assemblyai_transcript(
    req: &BatchTranscribeRequest,
    api_key: &str,
    base_url: &str,
    audio_url: &str,
) -> Result<BatchSubmission, String> {
    validate_batch_base_url("assemblyai", base_url)?;

    let std = &req.config;
    let f = &std.features;
    let b = &req.batch;
    let mut warnings = Vec::new();
    let mut body = serde_json::Map::new();
    let audio_url = validate_batch_audio_source_url("assemblyai", audio_url)?;
    body.insert("audio_url".into(), serde_json::Value::String(audio_url));

    // Vendor contract: `language_code` is OPTIONAL, and when it is absent AssemblyAI detects the
    // spoken language itself (`language_detection` defaults to on). So an unset language is
    // omitted, and so is the shared mapper's sentinel for the canonical `auto`
    // ([`ASSEMBLYAI_DETECT_SENTINEL`]) — sent verbatim, it is a language code AssemblyAI does not
    // have. Neither adds `language_detection: true`: absence already means detection, and the flag
    // is sent below only when the caller asked for it by name.
    let language = std.base.language.trim();
    if !language.is_empty() && language != "auto" && language != ASSEMBLYAI_DETECT_SENTINEL {
        body.insert(
            "language_code".into(),
            serde_json::Value::String(language.to_string()),
        );
    }
    // Model. `speech_models` (an array, in preference order) is the current field; the singular
    // `speech_model` is deprecated and `best`/`nano` are legacy tiers. Its documented values are
    // `universal-3-5-pro` and `universal-2`, and omitting it lets AssemblyAI choose. So a
    // prerecorded `universal-*` model is sent exactly as chosen, and an unset one is omitted.
    // Anything else — a streaming model (`universal-streaming-*`), a legacy tier — cannot be
    // expressed on this endpoint: it is omitted too, but the omission is reported, never silent.
    let model = std.base.model.trim();
    if !model.is_empty() {
        if assemblyai_prerecorded_model(model) {
            body.insert("speech_models".into(), serde_json::json!([model]));
        } else {
            warnings.push(format!(
                "model '{model}' is not an AssemblyAI prerecorded speech model (`speech_models` \
                 takes universal-3-5-pro or universal-2); omitted, so AssemblyAI chose the model"
            ));
        }
    }
    // --- batch-only Speech-Understanding models ------------------------------------------------
    if b.detect_language == Some(true) || f.language_detection == Some(true) {
        body.insert("language_detection".into(), true.into());
    }
    if b.summarize == Some(true) {
        body.insert("summarization".into(), true.into());
    }
    if b.topics == Some(true) {
        body.insert("iab_categories".into(), true.into());
    }
    if b.intents == Some(true) {
        // AssemblyAI has no intent-detection model → degrade (never a 400).
        warnings.push("intents not supported by assemblyai batch; omitted".into());
    }
    if b.utterances == Some(true) || b.paragraphs == Some(true) {
        // Utterances/paragraphs come back in the result when speaker_labels/punctuation are on;
        // there is no explicit request flag, so this is a no-op request-side (response carries them).
    }
    // --- reused typed SttFeatures (batch-valid on AssemblyAI) ----------------------------------
    if f.diarization == Some(true) {
        body.insert("speaker_labels".into(), true.into());
    }
    if f.sentiment == Some(true) {
        body.insert("sentiment_analysis".into(), true.into());
    }
    if f.entity_detection == Some(true) {
        body.insert("entity_detection".into(), true.into());
    }
    if f.multichannel == Some(true) {
        body.insert("multichannel".into(), true.into());
    }
    if f.profanity_filter == Some(true) {
        body.insert("filter_profanity".into(), true.into());
    }
    if let Some(redact) = &f.redaction
        && !redact.is_empty()
    {
        body.insert("redact_pii".into(), true.into());
        body.insert(
            "redact_pii_policies".into(),
            serde_json::Value::Array(
                redact
                    .iter()
                    .cloned()
                    .map(serde_json::Value::String)
                    .collect(),
            ),
        );
    }
    if let Some(kw) = &f.keyterms
        && !kw.is_empty()
    {
        body.insert(
            "keyterms_prompt".into(),
            serde_json::Value::Array(kw.iter().cloned().map(serde_json::Value::String).collect()),
        );
    }
    // Webhook (async).
    if let Some(cb) = &req.callback_url {
        body.insert(
            "webhook_url".into(),
            serde_json::Value::String(validate_batch_callback_url("assemblyai", cb)?),
        );
    }
    // Translation: AssemblyAI translation is batch-only; the canonical block is honored here (no
    // streaming warning). It rides the Speech-Understanding pipeline.
    if let Some(t) = &std.translation
        && !t.is_noop()
    {
        warnings.extend(t.warnings_for("assemblyai", false));
    }

    let host = base_url.trim_end_matches('/');
    let request = BatchHttpRequest {
        method: "POST".into(),
        url: format!("{host}/v2/transcript"),
        headers: vec![
            ("Authorization".into(), api_key.to_string()),
            ("Content-Type".into(), "application/json".into()),
        ],
        body: BatchHttpBody::Json(serde_json::Value::Object(body)),
    };
    Ok(BatchSubmission {
        request,
        is_async: true,
        config_warnings: warnings,
    })
}

/// Build the OpenAI synchronous submission (`POST /v1/audio/transcriptions`, multipart). OpenAI has
/// no async job and no URL source (file bytes only, ≤25MB). `detect_language` is implicit via
/// `response_format=verbose_json` (the response carries the detected `language`). Summarize/topics/
/// intents are unsupported → warn.
pub fn build_openai_transcription(
    req: &BatchTranscribeRequest,
    api_key: &str,
    base_url: &str,
) -> Result<BatchSubmission, String> {
    validate_batch_base_url("openai", base_url)?;

    let std = &req.config;
    let b = &req.batch;
    let mut warnings = Vec::new();

    let (audio_b64, _ct) = req
        .audio
        .bytes()
        .ok_or_else(|| "openai batch requires inline audio bytes (no URL source)".to_string())?;
    let bytes = b64_decode(audio_b64)?;

    let model = if std.base.model.is_empty() {
        "whisper-1".to_string()
    } else {
        std.base.model.clone()
    };

    let mut fields: Vec<(String, String)> = vec![("model".into(), model)];
    // detect_language / word+segment timestamps → verbose_json.
    let want_verbose = b.detect_language == Some(true)
        || std.features.word_timestamps == Some(true)
        || b.paragraphs == Some(true)
        || b.utterances == Some(true);
    fields.push((
        "response_format".into(),
        if want_verbose { "verbose_json" } else { "json" }.into(),
    ));
    if want_verbose {
        // segment granularity covers paragraphs/utterances; word covers word_timestamps.
        fields.push(("timestamp_granularities[]".into(), "segment".into()));
        if std.features.word_timestamps == Some(true) {
            fields.push(("timestamp_granularities[]".into(), "word".into()));
        }
    }
    if !std.base.language.is_empty() && std.base.language != "auto" {
        // Manual language hint (ISO-639-1). Not allowed on the translations endpoint, but this is
        // the transcriptions endpoint.
        let lang = std
            .base
            .language
            .split('-')
            .next()
            .unwrap_or(&std.base.language)
            .to_string();
        fields.push(("language".into(), lang));
    }
    // Unsupported batch knobs → degrade.
    for (on, name) in [
        (b.summarize == Some(true), "summarize"),
        (b.topics == Some(true), "topics"),
        (b.intents == Some(true), "intents"),
        (b.alternatives.is_some(), "alternatives"),
    ] {
        if on {
            warnings.push(format!("{name} not supported by openai batch; omitted"));
        }
    }
    if let Some(t) = &std.translation {
        warnings.extend(t.warnings_for("openai", false));
    }

    // Translation EN fast-path flips the endpoint.
    let translate = std
        .translation
        .as_ref()
        .map(|t| !t.is_noop())
        .unwrap_or(false);
    let path = if translate {
        "/v1/audio/translations"
    } else {
        "/v1/audio/transcriptions"
    };
    let host = base_url.trim_end_matches('/');

    let request = BatchHttpRequest {
        method: "POST".into(),
        url: format!("{host}{path}"),
        headers: vec![("Authorization".into(), format!("Bearer {api_key}"))],
        body: BatchHttpBody::Multipart {
            fields,
            file: Some(("file".into(), "audio.wav".into(), "audio/wav".into(), bytes)),
        },
    };
    Ok(BatchSubmission {
        request,
        is_async: false,
        config_warnings: warnings,
    })
}

/// Build the ElevenLabs submission (`POST /v1/speech-to-text`, multipart).
///
/// The only provider here that accepts BOTH a file and a remote URL natively, so neither the
/// AssemblyAI upload dance nor OpenAI's bytes-only restriction applies.
///
/// The wire surface comes from [`crate::core::stt::elevenlabs::ElevenLabsBatchConfig`], the same
/// builder the `/v1/audio/transcriptions` path uses. One wire surface, two routes: a field added
/// for one is present on the other, and the field-level tests cover both.
pub fn build_elevenlabs_transcription(
    req: &BatchTranscribeRequest,
    api_key: &str,
    base_url: &str,
) -> Result<BatchSubmission, String> {
    let audio = ResolvedAudio::from_source("elevenlabs", &req.audio)?;
    build_elevenlabs_transcription_with(req, audio, api_key, base_url)
}

/// As [`build_elevenlabs_transcription`], for a caller that already holds the decoded audio.
pub fn build_elevenlabs_transcription_with(
    req: &BatchTranscribeRequest,
    audio: ResolvedAudio,
    api_key: &str,
    base_url: &str,
) -> Result<BatchSubmission, String> {
    validate_batch_base_url("elevenlabs", base_url)?;

    let b = &req.batch;
    let mut warnings = Vec::new();

    let mut cfg = crate::core::stt::elevenlabs::ElevenLabsBatchConfig::from_standard(&req.config);
    cfg.endpoint_override = Some(base_url.trim_end_matches('/').to_string());
    cfg.validate()?;

    // `detect_language` is a batch-only knob here as well as a canonical feature; either asking
    // route means "omit `language_code`".
    let detect_language =
        b.detect_language == Some(true) || req.config.features.language_detection == Some(true);

    let mut fields = cfg.multipart_fields(detect_language);

    // Batch knobs ElevenLabs has no equivalent for. A degrade, never a 400 — the transcript is
    // still the thing the caller asked for.
    for (on, name) in [
        (b.summarize == Some(true), "summarize"),
        (b.topics == Some(true), "topics"),
        (b.intents == Some(true), "intents"),
        (b.alternatives.is_some(), "alternatives"),
        (b.paragraphs == Some(true), "paragraphs"),
        (b.utterances == Some(true), "utterances"),
    ] {
        if on {
            warnings.push(format!("{name} not supported by elevenlabs batch; omitted"));
        }
    }
    if let Some(t) = &req.config.translation {
        warnings.extend(t.warnings_for("elevenlabs", false));
    }

    // A remote URL rides as a form field; inline bytes ride as the file part.
    let file = match audio {
        ResolvedAudio::Url(url) => {
            fields.push(("source_url".into(), url));
            None
        }
        ResolvedAudio::Bytes {
            bytes,
            content_type,
        } => Some((
            "file".to_string(),
            "audio.wav".to_string(),
            content_type,
            bytes,
        )),
    };

    let request = BatchHttpRequest {
        method: "POST".into(),
        url: cfg.api_url(),
        // ElevenLabs authenticates with its own header, not `Authorization: Bearer`.
        headers: vec![("xi-api-key".into(), api_key.to_string())],
        body: BatchHttpBody::Multipart { fields, file },
    };
    Ok(BatchSubmission {
        request,
        // ElevenLabs' async mode needs a workspace webhook configured on THEIR side, which WaaV
        // cannot provision, so the synchronous answer is the only one it can actually collect.
        is_async: false,
        config_warnings: warnings,
    })
}

/// Whether a provider name is supported by the batch dispatcher.
pub fn batch_provider_supported(provider: &str) -> bool {
    matches!(
        provider.to_lowercase().as_str(),
        "deepgram" | "assemblyai" | "openai" | "elevenlabs"
    )
}

/// Validate the batch provider base URL before the gateway dials it.
pub fn validate_batch_base_url(provider: &str, base_url: &str) -> Result<(), String> {
    let base = base_url.trim();
    if base.is_empty() {
        return Err(format!("{provider} batch base url invalid: empty"));
    }
    crate::core::net::validate_url_for_ssrf(base, BATCH_BASE_URL_SCHEMES)
        .map_err(|msg| format!("{provider} batch base url rejected (SSRF protection): {msg}"))
}

fn validate_batch_audio_source_url(provider: &str, audio_url: &str) -> Result<String, String> {
    let audio_url = audio_url.trim();
    if audio_url.is_empty() {
        return Err(format!("{provider} batch audio url invalid: empty"));
    }
    crate::core::net::validate_url_for_ssrf(audio_url, BATCH_AUDIO_SOURCE_URL_SCHEMES)
        .map_err(|msg| format!("{provider} batch audio url rejected (SSRF protection): {msg}"))?;
    Ok(audio_url.to_string())
}

fn validate_batch_callback_url(provider: &str, callback_url: &str) -> Result<String, String> {
    let callback_url = callback_url.trim();
    if callback_url.is_empty() {
        return Err(format!("{provider} batch callback url invalid: empty"));
    }
    crate::core::net::validate_url_for_ssrf(callback_url, BATCH_CALLBACK_URL_SCHEMES).map_err(
        |msg| format!("{provider} batch callback url rejected (SSRF protection): {msg}"),
    )?;
    Ok(callback_url.to_string())
}

fn normalize_batch_callback_method(method: &str) -> Result<String, String> {
    let method = method.trim();
    if method.is_empty() {
        return Err("batch callback_method invalid: empty".to_string());
    }
    let method = method.to_ascii_uppercase();
    match method.as_str() {
        "POST" | "PUT" => Ok(method),
        _ => Err(format!(
            "batch callback_method invalid: expected POST or PUT, got {method}"
        )),
    }
}

/// Decode a base64 audio payload, tolerating a `data:...;base64,` prefix.
fn b64_decode(s: &str) -> Result<Vec<u8>, String> {
    decode_inline_batch_audio(s)
}

pub(crate) fn decode_inline_batch_audio(s: &str) -> Result<Vec<u8>, String> {
    decode_inline_batch_audio_with_limit(s, MAX_BATCH_INLINE_AUDIO_BYTES)
}

pub(crate) fn decode_inline_batch_audio_with_limit(
    s: &str,
    max_decoded_bytes: usize,
) -> Result<Vec<u8>, String> {
    use base64::Engine;
    let payload = s.rsplit_once("base64,").map(|(_, b)| b).unwrap_or(s);
    let payload = payload.trim();
    let padding = payload
        .as_bytes()
        .iter()
        .rev()
        .take_while(|&&b| b == b'=')
        .count()
        .min(2);
    let decoded_upper_bound = ((payload.len() + 3) / 4) * 3 - padding;
    if decoded_upper_bound > max_decoded_bytes {
        return Err(format!(
            "inline batch audio exceeds decoded size limit of {max_decoded_bytes} bytes"
        ));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .map_err(|e| format!("invalid base64 audio: {e}"))?;
    if bytes.len() > max_decoded_bytes {
        return Err(format!(
            "inline batch audio exceeds decoded size limit of {max_decoded_bytes} bytes"
        ));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::stt::base::STTConfig;
    use crate::core::stt::standard::{StandardSTTConfig, SttFeatures};

    fn req_with(
        provider: &str,
        audio: BatchAudioSource,
        features: SttFeatures,
        batch: BatchFeatures,
    ) -> BatchTranscribeRequest {
        let mut cfg = StandardSTTConfig::from_base(STTConfig {
            provider: provider.into(),
            ..Default::default()
        });
        cfg.features = features;
        BatchTranscribeRequest {
            audio,
            config: cfg,
            batch,
            callback_url: None,
            callback_method: None,
            translation: None,
        }
    }

    #[test]
    fn envelope_deserializes_url_source_and_reuses_features_verbatim() {
        // The envelope flattens StandardSTTConfig: a typed SttFeatures (diarization) is reused
        // verbatim alongside batch-only knobs (summarize).
        let json = r#"{
            "url": "https://example.com/a.wav",
            "provider": "deepgram", "api_key": "k", "language": "en-US",
            "sample_rate": 16000, "channels": 1, "punctuation": true,
            "encoding": "linear16", "model": "nova-2",
            "features": { "diarization": true },
            "batch": { "summarize": true, "alternatives": 3, "detect_language": true }
        }"#;
        let r: BatchTranscribeRequest = serde_json::from_str(json).unwrap();
        assert_eq!(r.audio.url(), Some("https://example.com/a.wav"));
        assert_eq!(r.config.features.diarization, Some(true));
        assert_eq!(r.batch.summarize, Some(true));
        assert_eq!(r.batch.alternatives, Some(3));
    }

    #[test]
    fn envelope_deserializes_bytes_source() {
        let json = r#"{
            "audio_base64": "AAAA", "content_type": "audio/mp3",
            "provider": "openai", "api_key": "sk", "language": "auto",
            "sample_rate": 16000, "channels": 1, "punctuation": true,
            "encoding": "linear16", "model": ""
        }"#;
        let r: BatchTranscribeRequest = serde_json::from_str(json).unwrap();
        let (b64, ct) = r.audio.bytes().unwrap();
        assert_eq!(b64, "AAAA");
        assert_eq!(ct, "audio/mp3");
    }

    #[test]
    fn inline_batch_audio_decode_is_size_bounded_before_allocation() {
        let ok = decode_inline_batch_audio_with_limit("data:audio/wav;base64,QUJD", 3)
            .expect("three decoded bytes should fit limit");
        assert_eq!(ok, b"ABC");

        let err = decode_inline_batch_audio_with_limit("QUJD", 2)
            .expect_err("decoded payload above limit must be rejected");
        assert!(
            err.contains("decoded size limit"),
            "unexpected limit error: {err}"
        );
    }

    #[test]
    fn deepgram_builder_enables_streaming_gap_features_on_the_wire() {
        // THE critical assertion: alternatives + detect_language (streaming gaps) AND the
        // batch-exclusive summarize/topics/intents all reach the prerecorded query string.
        let r = req_with(
            "deepgram",
            BatchAudioSource::Url {
                url: " https://example.com/a.wav ".into(),
            },
            SttFeatures {
                diarization: Some(true),
                ..Default::default()
            },
            BatchFeatures {
                alternatives: Some(4),
                detect_language: Some(true),
                summarize: Some(true),
                topics: Some(true),
                intents: Some(true),
                paragraphs: Some(true),
                utterances: Some(true),
            },
        );
        let sub = build_deepgram_prerecorded(&r, "test-key", "https://api.deepgram.com").unwrap();
        let url = &sub.request.url;
        assert!(url.contains("/v1/listen?"), "{url}");
        assert!(
            url.contains("alternatives=4"),
            "alternatives gap not enabled: {url}"
        );
        assert!(
            url.contains("detect_language=true"),
            "detect_language gap not enabled: {url}"
        );
        assert!(url.contains("summarize=v2"), "{url}");
        assert!(url.contains("topics=true"), "{url}");
        assert!(url.contains("intents=true"), "{url}");
        assert!(url.contains("paragraphs=true"), "{url}");
        assert!(url.contains("utterances=true"), "{url}");
        assert!(
            url.contains("diarize=true"),
            "reused feature missing: {url}"
        );
        // URL source → JSON body {"url":…}.
        match &sub.request.body {
            BatchHttpBody::Json(v) => assert_eq!(v["url"], "https://example.com/a.wav"),
            _ => panic!("expected JSON body for URL source"),
        }
        // Auth header is a Deepgram Token.
        assert!(
            sub.request
                .headers
                .iter()
                .any(|(k, v)| k == "Authorization" && v == "Token test-key")
        );
        assert!(!sub.is_async, "no callback → synchronous inline");
    }

    #[test]
    fn deepgram_builder_rejects_unsafe_audio_source_url() {
        let _env = crate::core::net::ssrf_env_lock();
        let mut r = req_with(
            "deepgram",
            BatchAudioSource::Url {
                url: "http://127.0.0.1:9000/a.wav".into(),
            },
            Default::default(),
            Default::default(),
        );
        let err = build_deepgram_prerecorded(&r, "test-key", "https://api.deepgram.com")
            .expect_err("loopback audio source URL must be rejected");
        assert!(err.contains("SSRF protection"), "{err}");

        r.audio = BatchAudioSource::Url {
            url: "file:///tmp/a.wav".into(),
        };
        let err = build_deepgram_prerecorded(&r, "test-key", "https://api.deepgram.com")
            .expect_err("non-HTTP audio source URL must be rejected");
        assert!(err.contains("URL scheme"), "{err}");

        r.audio = BatchAudioSource::Url { url: "   ".into() };
        let err = build_deepgram_prerecorded(&r, "test-key", "https://api.deepgram.com")
            .expect_err("blank audio source URL must be rejected");
        assert!(err.contains("empty"), "{err}");
    }

    #[test]
    fn deepgram_builder_callback_makes_it_async() {
        let mut r = req_with(
            "deepgram",
            BatchAudioSource::Url {
                url: "https://example.com/a.wav".into(),
            },
            Default::default(),
            Default::default(),
        );
        r.callback_url = Some(" https://hook.example.com/cb?x=1&y=two words ".into());
        r.callback_method = Some("put".into());
        let sub = build_deepgram_prerecorded(&r, "k", "https://api.deepgram.com").unwrap();
        assert!(sub.is_async);
        let parsed = url::Url::parse(&sub.request.url).unwrap();
        let callback = parsed
            .query_pairs()
            .find(|(key, _)| key == "callback")
            .map(|(_, value)| value.into_owned());
        assert_eq!(
            callback.as_deref(),
            Some("https://hook.example.com/cb?x=1&y=two words"),
            "{}",
            sub.request.url
        );
        assert!(sub.request.url.contains("callback=https%3A%2F%2F"));
        assert!(
            sub.request.url.contains("callback_method=PUT"),
            "{}",
            sub.request.url
        );
    }

    #[test]
    fn deepgram_builder_rejects_unsafe_callback_url_and_method() {
        let _env = crate::core::net::ssrf_env_lock();
        let mut r = req_with(
            "deepgram",
            BatchAudioSource::Url {
                url: "https://example.com/a.wav".into(),
            },
            Default::default(),
            Default::default(),
        );

        r.callback_url = Some("http://127.0.0.1:9000/cb".into());
        let err = build_deepgram_prerecorded(&r, "k", "https://api.deepgram.com")
            .expect_err("loopback callback URL must be rejected");
        assert!(err.contains("SSRF protection"), "{err}");

        r.callback_url = Some("file:///tmp/cb".into());
        let err = build_deepgram_prerecorded(&r, "k", "https://api.deepgram.com")
            .expect_err("non-HTTP callback URL must be rejected");
        assert!(err.contains("URL scheme"), "{err}");

        r.callback_url = Some("   ".into());
        let err = build_deepgram_prerecorded(&r, "k", "https://api.deepgram.com")
            .expect_err("blank callback URL must be rejected");
        assert!(err.contains("empty"), "{err}");

        r.callback_url = Some("https://hook.example.com/cb".into());
        r.callback_method = Some("DELETE".into());
        let err = build_deepgram_prerecorded(&r, "k", "https://api.deepgram.com")
            .expect_err("unsupported callback method must be rejected");
        assert!(err.contains("expected POST or PUT"), "{err}");
    }

    #[test]
    fn deepgram_builder_bytes_source_uses_raw_body() {
        let r = req_with(
            "deepgram",
            BatchAudioSource::Bytes {
                audio_base64: "AAAA".into(),
                content_type: Some("audio/wav".into()),
            },
            Default::default(),
            Default::default(),
        );
        let sub = build_deepgram_prerecorded(&r, "k", "https://api.deepgram.com").unwrap();
        match &sub.request.body {
            BatchHttpBody::Raw { content_type, .. } => assert_eq!(content_type, "audio/wav"),
            _ => panic!("expected raw body for bytes source"),
        }
    }

    fn query_pairs_of(url: &str) -> Vec<(String, String)> {
        url::Url::parse(url)
            .unwrap()
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect()
    }

    #[test]
    fn deepgram_builder_omits_an_unset_model_and_language() {
        // Both are optional on `/v1/listen`. An unset model used to go out as `model=nova-2` — a
        // model nobody chose — and the upload route now hands over an empty language when neither
        // the request nor the deployment names one.
        let mut r = req_with(
            "deepgram",
            BatchAudioSource::Url {
                url: "https://example.com/a.wav".into(),
            },
            Default::default(),
            Default::default(),
        );
        r.config.base.model = String::new();
        r.config.base.language = String::new();
        let sub = build_deepgram_prerecorded(&r, "k", "https://api.deepgram.com").unwrap();
        let q = query_pairs_of(&sub.request.url);
        assert!(q.iter().all(|(k, _)| k != "model"), "{}", sub.request.url);
        assert!(
            q.iter().all(|(k, _)| k != "language"),
            "{}",
            sub.request.url
        );
        assert!(!sub.request.url.contains("nova-2"), "{}", sub.request.url);
    }

    #[test]
    fn deepgram_builder_sends_a_chosen_model_and_language_verbatim() {
        let mut r = req_with(
            "deepgram",
            BatchAudioSource::Url {
                url: "https://example.com/a.wav".into(),
            },
            Default::default(),
            Default::default(),
        );
        r.config.base.model = "nova-3-medical".into();
        r.config.base.language = "de".into();
        let sub = build_deepgram_prerecorded(&r, "k", "https://api.deepgram.com").unwrap();
        let q = query_pairs_of(&sub.request.url);
        assert!(
            q.contains(&("model".into(), "nova-3-medical".into())),
            "{q:?}"
        );
        assert!(q.contains(&("language".into(), "de".into())), "{q:?}");
    }

    #[test]
    fn assemblyai_builder_enables_batch_only_models_and_degrades_intents() {
        let r = req_with(
            "assemblyai",
            BatchAudioSource::Url {
                url: "https://example.com/a.wav".into(),
            },
            SttFeatures {
                diarization: Some(true),
                sentiment: Some(true),
                entity_detection: Some(true),
                ..Default::default()
            },
            BatchFeatures {
                summarize: Some(true),
                topics: Some(true),
                intents: Some(true), // no AAI equiv → warn
                detect_language: Some(true),
                ..Default::default()
            },
        );
        let sub = build_assemblyai_transcript(
            &r,
            "aai-key",
            "https://api.assemblyai.com",
            "https://example.com/a.wav",
        )
        .unwrap();
        assert!(sub.is_async, "AssemblyAI is always async");
        let body = match &sub.request.body {
            BatchHttpBody::Json(v) => v,
            _ => panic!("expected JSON body"),
        };
        assert_eq!(body["summarization"], true);
        assert_eq!(body["iab_categories"], true);
        assert_eq!(body["language_detection"], true);
        assert_eq!(body["speaker_labels"], true);
        assert_eq!(body["sentiment_analysis"], true);
        assert_eq!(body["entity_detection"], true);
        assert_eq!(body["audio_url"], "https://example.com/a.wav");
        // intents has no AssemblyAI model → degrade (never a 400).
        assert!(
            sub.config_warnings.iter().any(|w| w.contains("intents")),
            "intents should degrade: {:?}",
            sub.config_warnings
        );
    }

    fn assemblyai_body(language: &str, model: &str) -> (serde_json::Value, Vec<String>) {
        let mut r = req_with(
            "assemblyai",
            BatchAudioSource::Url {
                url: "https://example.com/a.wav".into(),
            },
            Default::default(),
            Default::default(),
        );
        r.config.base.language = language.into();
        r.config.base.model = model.into();
        let sub = build_assemblyai_transcript(
            &r,
            "aai-key",
            "https://api.assemblyai.com",
            "https://example.com/a.wav",
        )
        .unwrap();
        match sub.request.body {
            BatchHttpBody::Json(v) => (v, sub.config_warnings),
            other => panic!("expected JSON body, got {other:?}"),
        }
    }

    #[test]
    fn assemblyai_unset_language_is_omitted_so_the_vendor_detects_it() {
        // `language_code` is optional; absent, AssemblyAI detects the language. Detection is not
        // requested by name either — nobody asked for the flag, and absence already means it.
        let (body, _) = assemblyai_body("", "");
        assert!(body.get("language_code").is_none(), "{body}");
        assert!(body.get("language_detection").is_none(), "{body}");
    }

    #[test]
    fn assemblyai_detect_sentinel_never_reaches_language_code() {
        // The shared mapper renders canonical `auto` as `__detect__` for AssemblyAI. Sent verbatim
        // it was a language code AssemblyAI does not have.
        let (body, _) = assemblyai_body(ASSEMBLYAI_DETECT_SENTINEL, "");
        assert!(body.get("language_code").is_none(), "{body}");
        assert!(!body.to_string().contains("__detect__"), "{body}");
    }

    #[test]
    fn assemblyai_chosen_language_is_sent() {
        let (body, _) = assemblyai_body("de", "");
        assert_eq!(body["language_code"], "de");
    }

    #[test]
    fn assemblyai_universal_models_ride_as_speech_models() {
        // The model used to be dropped entirely: every upload ran on AssemblyAI's default.
        for model in ["universal-2", "universal-3-5-pro"] {
            let (body, warnings) = assemblyai_body("en", model);
            assert_eq!(body["speech_models"], serde_json::json!([model]), "{body}");
            // The deprecated singular form is never sent.
            assert!(body.get("speech_model").is_none(), "{body}");
            assert!(warnings.iter().all(|w| !w.contains(model)), "{warnings:?}");
        }
    }

    #[test]
    fn assemblyai_unset_model_is_omitted_silently() {
        let (body, warnings) = assemblyai_body("en", "");
        assert!(body.get("speech_models").is_none(), "{body}");
        assert!(
            warnings.iter().all(|w| !w.contains("model")),
            "{warnings:?}"
        );
    }

    #[test]
    fn assemblyai_model_it_cannot_express_is_omitted_and_reported() {
        // A streaming model shares the `universal-` prefix but is not served here; legacy tiers
        // are gone. Omitted, because sending them fails the job — but never silently.
        for model in ["universal-streaming-english", "best", "nano"] {
            let (body, warnings) = assemblyai_body("en", model);
            assert!(body.get("speech_models").is_none(), "{model}: {body}");
            assert!(
                warnings.iter().any(|w| w.contains(model)),
                "{model}: the omission must be reported: {warnings:?}"
            );
        }
    }

    #[test]
    fn assemblyai_builder_validates_audio_url() {
        let _env = crate::core::net::ssrf_env_lock();
        let r = req_with(
            "assemblyai",
            BatchAudioSource::Url {
                url: "https://example.com/a.wav".into(),
            },
            Default::default(),
            Default::default(),
        );

        let sub = build_assemblyai_transcript(
            &r,
            "aai-key",
            "https://api.assemblyai.com",
            " https://example.com/upload.wav ",
        )
        .unwrap();
        let body = match &sub.request.body {
            BatchHttpBody::Json(v) => v,
            _ => panic!("expected JSON body"),
        };
        assert_eq!(body["audio_url"], "https://example.com/upload.wav");

        let err = build_assemblyai_transcript(
            &r,
            "aai-key",
            "https://api.assemblyai.com",
            "http://127.0.0.1:9000/a.wav",
        )
        .expect_err("loopback audio_url must be rejected");
        assert!(err.contains("SSRF protection"), "{err}");

        let err = build_assemblyai_transcript(
            &r,
            "aai-key",
            "https://api.assemblyai.com",
            "file:///tmp/a.wav",
        )
        .expect_err("non-HTTP audio_url must be rejected");
        assert!(err.contains("URL scheme"), "{err}");
    }

    #[test]
    fn assemblyai_builder_validates_webhook_url() {
        let _env = crate::core::net::ssrf_env_lock();
        let mut r = req_with(
            "assemblyai",
            BatchAudioSource::Url {
                url: "https://example.com/a.wav".into(),
            },
            Default::default(),
            Default::default(),
        );

        r.callback_url = Some(" https://hook.example.com/cb ".into());
        let sub = build_assemblyai_transcript(
            &r,
            "aai-key",
            "https://api.assemblyai.com",
            "https://example.com/a.wav",
        )
        .unwrap();
        let body = match &sub.request.body {
            BatchHttpBody::Json(v) => v,
            _ => panic!("expected JSON body"),
        };
        assert_eq!(body["webhook_url"], "https://hook.example.com/cb");

        r.callback_url = Some("http://127.0.0.1:9000/cb".into());
        let err = build_assemblyai_transcript(
            &r,
            "aai-key",
            "https://api.assemblyai.com",
            "https://example.com/a.wav",
        )
        .expect_err("loopback webhook URL must be rejected");
        assert!(err.contains("SSRF protection"), "{err}");

        r.callback_url = Some("file:///tmp/cb".into());
        let err = build_assemblyai_transcript(
            &r,
            "aai-key",
            "https://api.assemblyai.com",
            "https://example.com/a.wav",
        )
        .expect_err("non-HTTP webhook URL must be rejected");
        assert!(err.contains("URL scheme"), "{err}");
    }

    #[test]
    fn openai_builder_uses_verbose_json_for_detect_language_and_warns_unsupported() {
        let r = req_with(
            "openai",
            BatchAudioSource::Bytes {
                audio_base64: "AAAA".into(),
                content_type: None,
            },
            Default::default(),
            BatchFeatures {
                detect_language: Some(true),
                summarize: Some(true), // unsupported → warn
                topics: Some(true),    // unsupported → warn
                ..Default::default()
            },
        );
        let sub = build_openai_transcription(&r, "sk", "https://api.openai.com").unwrap();
        assert!(!sub.is_async, "OpenAI is synchronous");
        assert!(
            sub.request.url.ends_with("/v1/audio/transcriptions"),
            "{}",
            sub.request.url
        );
        match &sub.request.body {
            BatchHttpBody::Multipart { fields, file } => {
                assert!(
                    fields
                        .iter()
                        .any(|(k, v)| k == "response_format" && v == "verbose_json"),
                    "detect_language should force verbose_json: {fields:?}"
                );
                assert!(file.is_some(), "audio file part missing");
            }
            _ => panic!("expected multipart body"),
        }
        assert!(sub.config_warnings.iter().any(|w| w.contains("summarize")));
        assert!(sub.config_warnings.iter().any(|w| w.contains("topics")));
    }

    #[test]
    fn openai_builder_rejects_url_source() {
        let r = req_with(
            "openai",
            BatchAudioSource::Url {
                url: "https://example.com/a.wav".into(),
            },
            Default::default(),
            Default::default(),
        );
        let err = build_openai_transcription(&r, "sk", "https://api.openai.com").unwrap_err();
        assert!(err.contains("inline audio bytes"), "{err}");
    }

    #[test]
    fn batch_builders_reject_ssrf_base_urls() {
        let _env = crate::core::net::ssrf_env_lock();
        let deepgram = req_with(
            "deepgram",
            BatchAudioSource::Url {
                url: "https://example.com/a.wav".into(),
            },
            Default::default(),
            Default::default(),
        );
        let err = build_deepgram_prerecorded(&deepgram, "k", "http://127.0.0.1:9000").unwrap_err();
        assert!(err.contains("SSRF protection"), "{err}");

        let assemblyai = req_with(
            "assemblyai",
            BatchAudioSource::Url {
                url: "https://example.com/a.wav".into(),
            },
            Default::default(),
            Default::default(),
        );
        let err = build_assemblyai_transcript(
            &assemblyai,
            "k",
            "http://169.254.169.254",
            "https://example.com/a.wav",
        )
        .unwrap_err();
        assert!(err.contains("SSRF protection"), "{err}");

        let openai = req_with(
            "openai",
            BatchAudioSource::Bytes {
                audio_base64: "AAAA".into(),
                content_type: None,
            },
            Default::default(),
            Default::default(),
        );
        let err = build_openai_transcription(&openai, "sk", "file:///tmp/socket").unwrap_err();
        assert!(err.contains("not allowed"), "{err}");
    }

    #[test]
    fn job_status_serializes_lowercase() {
        let j = BatchJob::queued("abc", vec!["w".into()]);
        let v = serde_json::to_value(&j).unwrap();
        assert_eq!(v["status"], "queued");
        assert_eq!(v["job_id"], "abc");
        assert_eq!(v["config_warnings"][0], "w");
        // result/error omitted when None.
        assert!(v.get("result").is_none());
        let done = BatchJob::completed("abc", serde_json::json!({"transcript":"hi"}), vec![]);
        assert_eq!(serde_json::to_value(&done).unwrap()["status"], "completed");
    }

    #[test]
    fn unsupported_provider_is_reported() {
        assert!(batch_provider_supported("deepgram"));
        assert!(batch_provider_supported("assemblyai"));
        assert!(batch_provider_supported("openai"));
        assert!(!batch_provider_supported("gladia"));
    }

    // ---------------------------------------------------------------------------------------
    // ElevenLabs
    // ---------------------------------------------------------------------------------------

    fn eleven_fields(sub: &BatchSubmission) -> Vec<(String, String)> {
        match &sub.request.body {
            BatchHttpBody::Multipart { fields, .. } => fields.clone(),
            other => panic!("expected multipart, got {other:?}"),
        }
    }

    fn eleven_req(audio: BatchAudioSource, features: SttFeatures) -> BatchTranscribeRequest {
        let mut r = req_with("elevenlabs", audio, features, BatchFeatures::default());
        r.config.base.api_key = "xi-test".into();
        r.config.base.model = "scribe_v2".into();
        r
    }

    #[test]
    fn elevenlabs_is_a_supported_batch_provider() {
        assert!(batch_provider_supported("elevenlabs"));
        assert!(batch_provider_supported("ElevenLabs"));
    }

    #[test]
    fn elevenlabs_bytes_ride_as_the_file_part() {
        let sub = build_elevenlabs_transcription(
            &eleven_req(
                BatchAudioSource::Bytes {
                    audio_base64: "AAAA".into(),
                    content_type: Some("audio/wav".into()),
                },
                SttFeatures {
                    diarization: Some(true),
                    ..Default::default()
                },
            ),
            "xi-key",
            "https://api.elevenlabs.io",
        )
        .unwrap();

        assert_eq!(
            sub.request.url,
            "https://api.elevenlabs.io/v1/speech-to-text"
        );
        // Their own header, not `Authorization: Bearer` — a bearer token here is a 401 that
        // reads like a bad credential rather than a bad header name.
        assert_eq!(
            sub.request.headers,
            vec![("xi-api-key".to_string(), "xi-key".to_string())]
        );
        assert!(!sub.is_async);
        let fields = eleven_fields(&sub);
        assert!(
            fields
                .iter()
                .any(|(k, v)| k == "model_id" && v == "scribe_v2")
        );
        assert!(fields.iter().any(|(k, v)| k == "diarize" && v == "true"));
        assert!(fields.iter().all(|(k, _)| k != "source_url"));
        match &sub.request.body {
            BatchHttpBody::Multipart { file, .. } => assert!(file.is_some()),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn elevenlabs_takes_a_remote_url_natively() {
        // The only provider here that needs neither AssemblyAI's upload dance nor OpenAI's
        // bytes-only restriction.
        let sub = build_elevenlabs_transcription(
            &eleven_req(
                BatchAudioSource::Url {
                    url: "https://example.com/a.wav".into(),
                },
                SttFeatures::default(),
            ),
            "xi-key",
            "https://api.elevenlabs.io",
        )
        .unwrap();

        let fields = eleven_fields(&sub);
        assert!(
            fields
                .iter()
                .any(|(k, v)| k == "source_url" && v == "https://example.com/a.wav")
        );
        match &sub.request.body {
            BatchHttpBody::Multipart { file, .. } => assert!(file.is_none()),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn elevenlabs_degrades_on_knobs_it_has_no_equivalent_for() {
        // A degrade, never a 400: the transcript is still the thing the caller asked for.
        let mut r = eleven_req(
            BatchAudioSource::Url {
                url: "https://example.com/a.wav".into(),
            },
            SttFeatures::default(),
        );
        r.batch.summarize = Some(true);
        r.batch.topics = Some(true);
        let sub =
            build_elevenlabs_transcription(&r, "xi-key", "https://api.elevenlabs.io").unwrap();

        assert!(sub.config_warnings.iter().any(|w| w.contains("summarize")));
        assert!(sub.config_warnings.iter().any(|w| w.contains("topics")));
    }

    #[test]
    fn elevenlabs_detect_language_omits_the_language_code() {
        let mut r = eleven_req(
            BatchAudioSource::Url {
                url: "https://example.com/a.wav".into(),
            },
            SttFeatures::default(),
        );
        r.config.base.language = "en-US".into();
        r.batch.detect_language = Some(true);
        let sub =
            build_elevenlabs_transcription(&r, "xi-key", "https://api.elevenlabs.io").unwrap();

        assert!(
            eleven_fields(&sub)
                .iter()
                .all(|(k, _)| k != "language_code")
        );
    }

    #[test]
    fn elevenlabs_unset_language_is_omitted_not_defaulted() {
        // `language_code` is optional; absent, ElevenLabs detects the language. The upload route
        // hands over an empty language when neither the request nor the deployment names one.
        let mut r = eleven_req(
            BatchAudioSource::Url {
                url: "https://example.com/a.wav".into(),
            },
            SttFeatures::default(),
        );
        r.config.base.language = String::new();
        let sub =
            build_elevenlabs_transcription(&r, "xi-key", "https://api.elevenlabs.io").unwrap();

        assert!(
            eleven_fields(&sub)
                .iter()
                .all(|(k, _)| k != "language_code")
        );
    }

    #[test]
    fn elevenlabs_refuses_a_realtime_model_on_the_batch_route() {
        let mut r = eleven_req(
            BatchAudioSource::Url {
                url: "https://example.com/a.wav".into(),
            },
            SttFeatures::default(),
        );
        r.config.base.model = "scribe_v2_realtime".into();
        let err =
            build_elevenlabs_transcription(&r, "xi-key", "https://api.elevenlabs.io").unwrap_err();
        assert!(err.contains("scribe_v2_realtime"), "{err}");
    }

    #[test]
    fn elevenlabs_needs_an_audio_source() {
        // Covered by construction elsewhere, but this is the message an operator sees.
        let mut r = eleven_req(
            BatchAudioSource::Url { url: "  ".into() },
            SttFeatures::default(),
        );
        r.config.base.model = "scribe_v2".into();
        assert!(build_elevenlabs_transcription(&r, "k", "https://api.elevenlabs.io").is_err());
    }
}
