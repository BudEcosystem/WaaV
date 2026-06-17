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
    let std = &req.config;
    let f = &std.features;
    let b = &req.batch;
    let mut warnings = Vec::new();
    let mut qs: Vec<(String, String)> = Vec::new();

    // Model (empty → Deepgram default nova-2, mirroring the streaming contract).
    let model = if std.base.model.is_empty() {
        "nova-2"
    } else {
        std.base.model.as_str()
    };
    qs.push(("model".into(), model.into()));
    if !std.base.language.is_empty() && std.base.language != "auto" {
        qs.push(("language".into(), std.base.language.clone()));
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
        qs.push(("callback".into(), cb.clone()));
        let method = req
            .callback_method
            .clone()
            .unwrap_or_else(|| "POST".into())
            .to_uppercase();
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

    let (body, content_type) = match &req.audio {
        BatchAudioSource::Url { url } => (
            BatchHttpBody::Json(serde_json::json!({ "url": url })),
            "application/json".to_string(),
        ),
        BatchAudioSource::Bytes {
            audio_base64,
            content_type,
        } => {
            let bytes = b64_decode(audio_base64)?;
            let ct = content_type.clone().unwrap_or_else(|| "audio/wav".into());
            (
                BatchHttpBody::Raw {
                    bytes,
                    content_type: ct.clone(),
                },
                ct,
            )
        }
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
    let std = &req.config;
    let f = &std.features;
    let b = &req.batch;
    let mut warnings = Vec::new();
    let mut body = serde_json::Map::new();
    body.insert("audio_url".into(), serde_json::Value::String(audio_url.into()));

    if !std.base.language.is_empty() && std.base.language != "auto" {
        body.insert(
            "language_code".into(),
            serde_json::Value::String(std.base.language.clone()),
        );
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
                redact.iter().cloned().map(serde_json::Value::String).collect(),
            ),
        );
    }
    if let Some(kw) = &f.keyterms
        && !kw.is_empty()
    {
        body.insert(
            "keyterms_prompt".into(),
            serde_json::Value::Array(
                kw.iter().cloned().map(serde_json::Value::String).collect(),
            ),
        );
    }
    // Webhook (async).
    if let Some(cb) = &req.callback_url {
        body.insert("webhook_url".into(), serde_json::Value::String(cb.clone()));
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
            file: Some((
                "file".into(),
                "audio.wav".into(),
                "audio/wav".into(),
                bytes,
            )),
        },
    };
    Ok(BatchSubmission {
        request,
        is_async: false,
        config_warnings: warnings,
    })
}

/// Whether a provider name is supported by the batch dispatcher.
pub fn batch_provider_supported(provider: &str) -> bool {
    matches!(
        provider.to_lowercase().as_str(),
        "deepgram" | "assemblyai" | "openai"
    )
}

/// Decode a base64 audio payload, tolerating a `data:...;base64,` prefix.
fn b64_decode(s: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    let payload = s.rsplit_once("base64,").map(|(_, b)| b).unwrap_or(s);
    base64::engine::general_purpose::STANDARD
        .decode(payload.trim())
        .map_err(|e| format!("invalid base64 audio: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::stt::base::STTConfig;
    use crate::core::stt::standard::{SttFeatures, StandardSTTConfig};

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
    fn deepgram_builder_enables_streaming_gap_features_on_the_wire() {
        // THE critical assertion: alternatives + detect_language (streaming gaps) AND the
        // batch-exclusive summarize/topics/intents all reach the prerecorded query string.
        let r = req_with(
            "deepgram",
            BatchAudioSource::Url {
                url: "https://example.com/a.wav".into(),
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
        let sub =
            build_deepgram_prerecorded(&r, "test-key", "https://api.deepgram.com").unwrap();
        let url = &sub.request.url;
        assert!(url.contains("/v1/listen?"), "{url}");
        assert!(url.contains("alternatives=4"), "alternatives gap not enabled: {url}");
        assert!(url.contains("detect_language=true"), "detect_language gap not enabled: {url}");
        assert!(url.contains("summarize=v2"), "{url}");
        assert!(url.contains("topics=true"), "{url}");
        assert!(url.contains("intents=true"), "{url}");
        assert!(url.contains("paragraphs=true"), "{url}");
        assert!(url.contains("utterances=true"), "{url}");
        assert!(url.contains("diarize=true"), "reused feature missing: {url}");
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
    fn deepgram_builder_callback_makes_it_async() {
        let mut r = req_with(
            "deepgram",
            BatchAudioSource::Url {
                url: "https://example.com/a.wav".into(),
            },
            Default::default(),
            Default::default(),
        );
        r.callback_url = Some("https://hook.example.com/cb".into());
        r.callback_method = Some("put".into());
        let sub =
            build_deepgram_prerecorded(&r, "k", "https://api.deepgram.com").unwrap();
        assert!(sub.is_async);
        assert!(sub.request.url.contains("callback=https"), "{}", sub.request.url);
        assert!(sub.request.url.contains("callback_method=PUT"), "{}", sub.request.url);
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
        assert!(sub.request.url.ends_with("/v1/audio/transcriptions"), "{}", sub.request.url);
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
    fn job_status_serializes_lowercase() {
        let j = BatchJob::queued("abc", vec!["w".into()]);
        let v = serde_json::to_value(&j).unwrap();
        assert_eq!(v["status"], "queued");
        assert_eq!(v["job_id"], "abc");
        assert_eq!(v["config_warnings"][0], "w");
        // result/error omitted when None.
        assert!(v.get("result").is_none());
        let done = BatchJob::completed("abc", serde_json::json!({"transcript":"hi"}), vec![]);
        assert_eq!(
            serde_json::to_value(&done).unwrap()["status"],
            "completed"
        );
    }

    #[test]
    fn unsupported_provider_is_reported() {
        assert!(batch_provider_supported("deepgram"));
        assert!(batch_provider_supported("assemblyai"));
        assert!(batch_provider_supported("openai"));
        assert!(!batch_provider_supported("gladia"));
    }
}
