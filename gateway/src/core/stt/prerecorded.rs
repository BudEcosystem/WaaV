//! Prerecorded (batch) speech-to-text over HTTP — the transport a file upload actually wants.
//!
//! # Why this exists
//!
//! `POST /v1/audio/transcriptions` takes a file. WaaV used to serve it by decoding that file to
//! PCM and replaying it at a **streaming** provider in 100 ms frames, then waiting for the socket
//! to go quiet — because the STT fleet is 30-odd WebSocket clients and there was nowhere else to
//! send it. That works, and it is the wrong shape in four measurable ways:
//!
//! * **It is slow.** The settle heuristic exists only because a streaming provider never says
//!   "done": after the last frame the driver waits a 2.5 s quiet window before it can answer. A
//!   prerecorded API answers once, when it is finished.
//! * **It silently drops features.** Deepgram's streaming mapper omits `sentiment`; AssemblyAI's
//!   omits `profanity_filter`, `redaction` and `multichannel`. All of them exist on the
//!   prerecorded wire. The gap is documented in each provider's source and was unreachable.
//! * **Streaming-only concerns leak in.** Endpointing, utterance-end and interim results are
//!   parameters of a live conversation. On a file they are noise at best.
//! * **It is not what the vendor optimises.** Deepgram's prerecorded endpoint reads the whole
//!   file at once and can look backwards; the socket cannot.
//!
//! [`crate::core::stt::batch`] has built correct prerecorded requests for these vendors all
//! along — they were reachable only from the separate `POST /transcribe/batch` route. This module
//! is the driver that lets the OpenAI-compatible route use them, by wearing the [`BaseSTT`]
//! interface the upload handler already drives.
//!
//! # Shape
//!
//! `send_audio` buffers PCM; `disconnect` wraps it in a WAV container, asks
//! [`crate::core::stt::batch`] to build the vendor-native request, runs it (including AssemblyAI's
//! upload-then-poll dance), parses the answer, and delivers it through the result callback. One
//! wire surface serves both routes, so a field added for one is present on the other and the
//! field-level tests cover both.
//!
//! [`BaseSTT::is_request_response`] answers `true`, which is what stops the upload driver waiting
//! 45 seconds for a stream that is never going to arrive.
//!
//! # Not every vendor
//!
//! Only vendors WaaV has a prerecorded implementation for. OpenAI is absent on purpose: its STT
//! client already *is* `POST /v1/audio/transcriptions` and needed only the marker. Everything else
//! keeps the streaming replay, which still works.

use bytes::Bytes;
use reqwest::Client;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use super::base::{
    BaseSTT, STTConfig, STTError, STTErrorCallback, STTResult, STTResultCallback, SpeakerInfo,
    WordTiming,
};
use super::batch::{
    BatchFeatures, BatchHttpBody, BatchSubmission, BatchTranscribeRequest, ResolvedAudio,
};
use super::http_resilience::HttpBreaker;
use super::standard::StandardSTTConfig;

// =============================================================================
// Limits
// =============================================================================

/// Ceiling on buffered PCM, matched to the route's own 50 MB upload limit with room for the WAV
/// header. Not a vendor limit: this is what the gateway is willing to hold in memory at once.
const MAX_BUFFER_SIZE_BYTES: usize = 64 * 1024 * 1024;

/// Per-HTTP-call timeout. Long audio takes real time, and killing the request after the vendor
/// has already been paid for the work is the worst of the outcomes.
const HTTP_TIMEOUT: Duration = Duration::from_secs(300);

/// Overall ceiling on an async vendor's submit-then-poll cycle. Matches the prerecorded driver's
/// own `OVERALL_DEADLINE`, so the two bounds cannot disagree about how long a file may take.
const POLL_DEADLINE: Duration = Duration::from_secs(300);

/// How often to ask an async vendor whether it has finished.
///
/// AssemblyAI bills per hour of audio, not per poll, so this trades a little request volume for
/// latency on the short clips that dominate this route. It backs off — see [`poll_interval`].
const POLL_INTERVAL_START: Duration = Duration::from_millis(500);

/// The longest gap between polls, once a job has clearly not finished quickly.
const POLL_INTERVAL_MAX: Duration = Duration::from_secs(3);

/// Back off from [`POLL_INTERVAL_START`] to [`POLL_INTERVAL_MAX`] as a job runs on.
///
/// A three-second clip is usually done on the first or second poll; a thirty-minute recording is
/// not, and asking it twice a second for half an hour is 3,600 pointless requests.
fn poll_interval(elapsed: Duration) -> Duration {
    let scaled = POLL_INTERVAL_START * (1 + (elapsed.as_secs() / 5) as u32);
    scaled.min(POLL_INTERVAL_MAX)
}

// =============================================================================
// Vendors
// =============================================================================

/// A vendor WaaV can serve a prerecorded upload to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrerecordedVendor {
    /// `POST /v1/listen` with the audio as the body.
    Deepgram,
    /// `POST /v2/upload`, then `POST /v2/transcript`, then poll `GET /v2/transcript/{id}`.
    AssemblyAI,
    /// `POST /v1/speech-to-text`, multipart.
    ElevenLabs,
}

impl PrerecordedVendor {
    /// The vendor id a `voice_table` entry carries, if WaaV has a prerecorded path for it.
    ///
    /// Aliases are resolved the same way [`super::standard::create_stt_standard`] resolves them,
    /// so a deployment naming `rev.ai` and one naming `revai` behave identically here too.
    pub fn from_provider(provider: &str) -> Option<Self> {
        match provider.trim().to_lowercase().as_str() {
            "deepgram" => Some(Self::Deepgram),
            "assemblyai" => Some(Self::AssemblyAI),
            "elevenlabs" => Some(Self::ElevenLabs),
            _ => None,
        }
    }

    /// The canonical provider id, for the breaker and for request building.
    pub fn id(&self) -> &'static str {
        match self {
            Self::Deepgram => "deepgram",
            Self::AssemblyAI => "assemblyai",
            Self::ElevenLabs => "elevenlabs",
        }
    }

    /// The vendor's production host, used when the deployment names no override.
    pub fn default_base_url(&self) -> &'static str {
        match self {
            Self::Deepgram => "https://api.deepgram.com",
            Self::AssemblyAI => "https://api.assemblyai.com",
            Self::ElevenLabs => "https://api.elevenlabs.io",
        }
    }

    /// Whether the vendor answers the submit itself, or hands back a job to poll.
    pub fn is_async(&self) -> bool {
        matches!(self, Self::AssemblyAI)
    }

    /// For logs and `get_provider_info`. Names the TRANSPORT as well as the vendor: two clients
    /// serve one vendor, and a line saying only "Deepgram" cannot tell an operator which ran.
    pub fn provider_info(&self) -> &'static str {
        match self {
            Self::Deepgram => "Deepgram STT (prerecorded)",
            Self::AssemblyAI => "AssemblyAI STT (prerecorded)",
            Self::ElevenLabs => "ElevenLabs Scribe STT (batch)",
        }
    }
}

// =============================================================================
// Client
// =============================================================================

type AsyncSTTCallback = Box<
    dyn Fn(STTResult) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

type AsyncErrorCallback = Box<
    dyn Fn(STTError) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

fn http_client() -> Result<Client, reqwest::Error> {
    crate::core::net::ssrf_protected_client_builder(crate::core::net::HTTP_URL_SCHEMES)
        .timeout(HTTP_TIMEOUT)
        .pool_max_idle_per_host(4)
        .pool_idle_timeout(Duration::from_secs(90))
        .build()
}

/// Buffers PCM and transcribes it against a vendor's prerecorded API on close.
pub struct PrerecordedSTT {
    vendor: PrerecordedVendor,
    config: StandardSTTConfig,
    /// Batch-only knobs, read from `extras` — they have no canonical name and reach one vendor.
    batch: BatchFeatures,
    http: Option<Client>,
    buffer: Vec<u8>,
    connected: AtomicBool,
    result_callback: Arc<Mutex<Option<AsyncSTTCallback>>>,
    error_callback: Arc<Mutex<Option<AsyncErrorCallback>>>,
    total_bytes_received: u64,
    resilience: HttpBreaker,
    /// Degrades collected while building the request (`config_warnings`), surfaced as warnings.
    warnings: Vec<String>,
}

impl PrerecordedSTT {
    /// Build for a vendor from the standardized config.
    pub fn new_standard(
        vendor: PrerecordedVendor,
        std: &StandardSTTConfig,
    ) -> Result<Self, STTError> {
        if std.base.api_key.is_empty() {
            return Err(STTError::AuthenticationFailed(format!(
                "API key is required for {} STT",
                vendor.id()
            )));
        }
        super::standard::validate_standard_endpoint_override(std.endpoint_override())?;
        if let Some(base) = std.endpoint_override() {
            super::batch::validate_batch_base_url(vendor.id(), base)
                .map_err(STTError::ConfigurationError)?;
        }
        let http = http_client().map_err(|e| {
            STTError::ConfigurationError(format!("Failed to create HTTP client: {e}"))
        })?;
        Ok(Self {
            vendor,
            batch: batch_features_from(std),
            config: std.clone(),
            http: Some(http),
            // ~30 s of 16 kHz mono PCM, so a short clip never reallocates.
            buffer: Vec::with_capacity(32 * 1024 * 30),
            connected: AtomicBool::new(false),
            result_callback: Arc::new(Mutex::new(None)),
            error_callback: Arc::new(Mutex::new(None)),
            total_bytes_received: 0,
            resilience: HttpBreaker::new(vendor.id()),
            warnings: Vec::new(),
        })
    }

    /// The vendor this client speaks to.
    pub fn vendor(&self) -> PrerecordedVendor {
        self.vendor
    }

    /// Degrades reported while building the request, e.g. a batch knob the vendor cannot express.
    pub fn config_warnings(&self) -> &[String] {
        &self.warnings
    }

    /// The base URL: the deployment's override, else the vendor's production host.
    fn base_url(&self) -> String {
        self.config
            .endpoint_override()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| self.vendor.default_base_url())
            .to_string()
    }

    /// The envelope the shared builders read features from.
    ///
    /// `audio` is a placeholder: the `_with` builders take the audio as a separate argument and
    /// ignore this field. It is an EMPTY byte source rather than a plausible URL so that a builder
    /// which regressed into reading `req.audio` would post nothing and fail loudly, rather than
    /// quietly transcribe someone else's file.
    fn envelope(&self) -> BatchTranscribeRequest {
        BatchTranscribeRequest {
            audio: super::batch::BatchAudioSource::Bytes {
                audio_base64: String::new(),
                content_type: Some("audio/wav".to_string()),
            },
            config: self.config.clone(),
            batch: self.batch.clone(),
            // A deployment has nowhere to put a callback URL, and this route answers inline.
            callback_url: None,
            callback_method: None,
            translation: self.config.translation.clone(),
        }
    }

    /// POST the buffered audio and deliver the transcript.
    async fn flush_buffer(&mut self) -> Result<(), STTError> {
        if self.buffer.is_empty() {
            debug!(vendor = self.vendor.id(), "no audio buffered");
            return Ok(());
        }
        let wav = match super::wav::encode_pcm16_wav(
            &self.buffer,
            self.config.base.sample_rate,
            self.config.base.channels,
        ) {
            Ok(w) => w,
            Err(e) => {
                return Err(self
                    .report(STTError::AudioProcessingError(format!(
                        "Invalid WAV parameters: {e}"
                    )))
                    .await);
            }
        };

        let Some(http) = self.http.clone() else {
            return Err(self
                .report(STTError::ConfigurationError(
                    "prerecorded HTTP client is unavailable".to_string(),
                ))
                .await);
        };
        let api_key = self.config.base.api_key.clone();
        let base_url = self.base_url();
        let envelope = self.envelope();
        let vendor = self.vendor;

        info!(
            vendor = vendor.id(),
            bytes = wav.len(),
            model = %self.config.base.model,
            "prerecorded transcription"
        );

        // The shared per-provider breaker, consulted before paying the round trip. Reported, not
        // just returned: an open breaker that reached nobody is an empty transcript and a 200.
        if let Err(e) = self.resilience.check() {
            return Err(self.report(e).await);
        }

        let body = match self
            .run(&http, vendor, &envelope, wav, &api_key, &base_url)
            .await
        {
            Ok(b) => b,
            Err(e) => return Err(self.report(e).await),
        };

        let results = match parse_response(vendor, &body) {
            Ok(r) => r,
            Err(e) => {
                return Err(self
                    .report(STTError::ProviderError(format!(
                        "could not read the {} response: {e}",
                        vendor.id()
                    )))
                    .await);
            }
        };

        self.buffer.clear();
        for result in results {
            info!(
                vendor = vendor.id(),
                chars = result.transcript.len(),
                "prerecorded transcription complete"
            );
            if let Some(callback) = self.result_callback.lock().await.as_ref() {
                callback(result).await;
            }
        }
        Ok(())
    }

    /// Build the vendor-native request, run it, and (for an async vendor) poll it to completion.
    async fn run(
        &mut self,
        http: &Client,
        vendor: PrerecordedVendor,
        envelope: &BatchTranscribeRequest,
        wav: Vec<u8>,
        api_key: &str,
        base_url: &str,
    ) -> Result<serde_json::Value, STTError> {
        let submission = match vendor {
            PrerecordedVendor::Deepgram => super::batch::build_deepgram_prerecorded_with(
                envelope,
                ResolvedAudio::wav(wav),
                api_key,
                base_url,
            ),
            PrerecordedVendor::ElevenLabs => super::batch::build_elevenlabs_transcription_with(
                envelope,
                ResolvedAudio::wav(wav),
                api_key,
                base_url,
            ),
            PrerecordedVendor::AssemblyAI => {
                // The only two-step vendor: bytes have to become a URL before a transcript can be
                // requested for them.
                let upload_url = self.upload_assemblyai(http, wav, api_key, base_url).await?;
                super::batch::build_assemblyai_transcript(envelope, api_key, base_url, &upload_url)
            }
        }
        .map_err(STTError::ConfigurationError)?;

        self.warnings = submission.config_warnings.clone();
        for w in &self.warnings {
            warn!(vendor = vendor.id(), "config degraded: {w}");
        }

        let submit = self.execute(http, &submission).await?;
        if vendor.is_async() {
            self.poll_assemblyai(http, &submit, api_key, base_url).await
        } else {
            Ok(submit)
        }
    }

    /// Run one built request and return its JSON body, classifying the status for the breaker.
    async fn execute(
        &mut self,
        http: &Client,
        submission: &BatchSubmission,
    ) -> Result<serde_json::Value, STTError> {
        let r = &submission.request;
        let mut builder = match r.method.as_str() {
            "POST" => http.post(&r.url),
            "PUT" => http.put(&r.url),
            m => {
                return Err(STTError::ConfigurationError(format!(
                    "unsupported method {m}"
                )));
            }
        };
        for (k, v) in &r.headers {
            // Let reqwest own Content-Type for the two body shapes that decide it: multipart
            // needs the boundary appended (a builder-supplied value would have none, and the
            // vendor could not parse the body), and for JSON `.json()` sets it anyway — passing
            // a second one risks a duplicate header rather than an override.
            //
            // The raw shape keeps the builder's value: it is the only source of truth for
            // `audio/wav` vs `audio/mpeg`, and reqwest will not guess it.
            if k.eq_ignore_ascii_case("content-type")
                && matches!(
                    r.body,
                    BatchHttpBody::Multipart { .. } | BatchHttpBody::Json(_)
                )
            {
                continue;
            }
            builder = builder.header(k, v);
        }
        builder = match &r.body {
            BatchHttpBody::Empty => builder,
            BatchHttpBody::Json(v) => builder.json(v),
            BatchHttpBody::Raw { bytes, .. } => builder.body(bytes.clone()),
            BatchHttpBody::Multipart { fields, file } => {
                let mut form = reqwest::multipart::Form::new();
                for (name, value) in fields {
                    form = form.text(name.clone(), value.clone());
                }
                if let Some((field, filename, ct, bytes)) = file {
                    let part = reqwest::multipart::Part::bytes(bytes.clone())
                        .file_name(filename.clone())
                        .mime_str(ct)
                        .map_err(|e| {
                            STTError::ConfigurationError(format!("bad multipart mime: {e}"))
                        })?;
                    form = form.part(field.clone(), part);
                }
                builder.multipart(form)
            }
        };

        let response = match builder.send().await {
            Ok(r) => r,
            Err(e) => {
                self.resilience.record_send_error();
                return Err(STTError::NetworkError(format!("Request failed: {e}")));
            }
        };
        let status = response.status();
        self.resilience.record_status(status);
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(classify_vendor_status(
                status,
                describe_error(self.vendor, status, &text),
            ));
        }
        serde_json::from_str(&text).map_err(|e| {
            STTError::ProviderError(format!(
                "{} returned a body that is not JSON: {e}",
                self.vendor.id()
            ))
        })
    }

    /// `POST /v2/upload` — AssemblyAI transcribes URLs, not uploads, so bytes become a URL first.
    async fn upload_assemblyai(
        &mut self,
        http: &Client,
        wav: Vec<u8>,
        api_key: &str,
        base_url: &str,
    ) -> Result<String, STTError> {
        let url = format!("{}/v2/upload", base_url.trim_end_matches('/'));
        let response = match http
            .post(&url)
            .header("Authorization", api_key)
            .header("Content-Type", "application/octet-stream")
            .body(wav)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                self.resilience.record_send_error();
                return Err(STTError::NetworkError(format!(
                    "assemblyai upload failed: {e}"
                )));
            }
        };
        let status = response.status();
        self.resilience.record_status(status);
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(STTError::ProviderError(format!(
                "assemblyai upload rejected ({status}): {text}"
            )));
        }
        serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| {
                v.get("upload_url")
                    .and_then(|u| u.as_str())
                    .map(str::to_string)
            })
            .ok_or_else(|| {
                STTError::ProviderError(format!("assemblyai upload returned no upload_url: {text}"))
            })
    }

    /// Poll `GET /v2/transcript/{id}` until it completes, errors, or the deadline fires.
    async fn poll_assemblyai(
        &mut self,
        http: &Client,
        submit: &serde_json::Value,
        api_key: &str,
        base_url: &str,
    ) -> Result<serde_json::Value, STTError> {
        // A submit that is already terminal needs no poll at all. Real AssemblyAI answers
        // `queued`, but a mock or a future fast path may not, and polling a finished job would
        // cost a round trip to learn what is already in hand.
        if let Some(status) = submit.get("status").and_then(|s| s.as_str())
            && matches!(status, "completed" | "error")
        {
            return terminal_or_err(submit);
        }
        let id = submit
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                STTError::ProviderError(format!("assemblyai submit returned no id: {submit}"))
            })?
            .to_string();
        let url = format!("{}/v2/transcript/{id}", base_url.trim_end_matches('/'));
        let started = Instant::now();

        loop {
            tokio::time::sleep(poll_interval(started.elapsed())).await;
            if started.elapsed() >= POLL_DEADLINE {
                return Err(STTError::ProviderError(format!(
                    "assemblyai transcript {id} did not finish within {}s",
                    POLL_DEADLINE.as_secs()
                )));
            }
            let response = match http.get(&url).header("Authorization", api_key).send().await {
                Ok(r) => r,
                Err(e) => {
                    // A single failed poll is not a failed job — the transcript is still running
                    // on their side. Keep polling until the deadline decides.
                    warn!(%id, error = %e, "assemblyai poll failed; retrying");
                    continue;
                }
            };
            let status = response.status();
            self.resilience.record_status(status);
            let text = response.text().await.unwrap_or_default();
            if !status.is_success() {
                return Err(STTError::ProviderError(format!(
                    "assemblyai poll rejected ({status}): {text}"
                )));
            }
            let body: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
                STTError::ProviderError(format!("assemblyai poll body is not JSON: {e}"))
            })?;
            match body.get("status").and_then(|s| s.as_str()) {
                Some("completed") | Some("error") => return terminal_or_err(&body),
                Some(_) | None => continue,
            }
        }
    }

    /// Hand an error to the registered callback and give it back for the return path.
    ///
    /// Both, always. `disconnect` swallows the return value (it must still close) and the upload
    /// driver reads only the callbacks, so an unreported failure reaches the caller as an empty
    /// transcript and HTTP 200 — the worst of the three outcomes, because it looks like silence.
    async fn report(&self, err: STTError) -> STTError {
        if let Some(callback) = self.error_callback.lock().await.as_ref() {
            callback(err.clone()).await;
        }
        err
    }
}

/// Decide whose problem a vendor's non-2xx is.
///
/// The distinction decides whether the caller sees **400** ("fix your request") or **502** ("the
/// vendor is having a bad day"), and getting it wrong sends an operator to read a status page
/// about a value they typed themselves.
///
/// A 4xx means the vendor rejected the request WaaV built out of the deployment's configuration —
/// an unsupported language for the chosen model, a model id that does not exist, a feature this
/// tier cannot serve. All fixable, and all named in the body. Three exceptions:
///
/// * **401 / 403** — the credential is WaaV's, not the caller's. A caller who is told "bad
///   request" will go looking at their own request and find nothing wrong with it.
/// * **408 / 429** — "too slow" and "too many" are not malformed requests; they are the upstream
///   being unable to serve this one right now, which is what 502 means here.
fn classify_vendor_status(status: reqwest::StatusCode, message: String) -> STTError {
    match status.as_u16() {
        401 | 403 => STTError::AuthenticationFailed(message),
        408 | 429 => STTError::ProviderError(message),
        // `ConfigurationError` is the variant the transcription driver reads as "the caller's
        // problem"; see `TranscribeFailure` in `handlers::transcribe`.
        code if (400..500).contains(&code) => STTError::ConfigurationError(message),
        _ => STTError::ProviderError(message),
    }
}

/// Render a vendor's error body as one line that names what was wrong.
///
/// Each of these vendors reports failure in its own shape, and handing the raw body to the caller
/// buries the one fact they need. ElevenLabs (a FastAPI service) answers a list of
/// `{loc, msg, type}` under `detail`; Deepgram uses `err_msg`; AssemblyAI uses `error`. Rendered
/// naively, a fixable "unknown model" reaches the caller looking like an internal fault.
///
/// Falls back to the raw body — never swallows it — because an unrecognised shape is still the
/// most information available.
fn describe_error(vendor: PrerecordedVendor, status: reqwest::StatusCode, body: &str) -> String {
    let prefix = format!("{} API error ({status})", vendor.id());
    match crate::core::vendor_error::vendor_message(body) {
        Some(msg) => format!("{prefix}: {msg}"),
        None => format!("{prefix}: {body}"),
    }
}

/// An AssemblyAI job in a terminal state: the body, or its `error` rendered as a failure.
fn terminal_or_err(body: &serde_json::Value) -> Result<serde_json::Value, STTError> {
    if body.get("status").and_then(|s| s.as_str()) == Some("error") {
        let message = body
            .get("error")
            .and_then(|e| e.as_str())
            .unwrap_or("assemblyai reported an error with no message");
        return Err(STTError::ProviderError(format!(
            "assemblyai transcription failed: {message}"
        )));
    }
    Ok(body.clone())
}

/// Read the batch-only knobs out of `extras`.
///
/// `summarize`, `topics`, `intents`, `paragraphs` and `utterances` are prerecorded-only and have
/// no canonical name; inventing one for each would put controls on every deployment's settings
/// page that most vendors ignore. `detect_language` and `alternatives` DO have canonical names
/// and are read from `features` by the builders themselves, so they are not duplicated here.
fn batch_features_from(std: &StandardSTTConfig) -> BatchFeatures {
    let get = |k: &str| std.extras.0.get(k).and_then(|v| v.as_bool());
    BatchFeatures {
        paragraphs: get("paragraphs"),
        utterances: get("utterances"),
        summarize: get("summarize"),
        topics: get("topics"),
        intents: get("intents"),
        // `detect_language` is left None because the builders read BOTH
        // `batch.detect_language` and `features.language_detection`, so setting it here would be
        // two sources for one answer.
        detect_language: None,
        // `alternatives` is NOT the same case, though it looks like it. The Deepgram builder
        // reads `batch.alternatives` and never `features.alternatives`, so leaving this None
        // meant a deployment's `stt.alternatives` reached nothing: the request went out without
        // the parameter and Deepgram returned one hypothesis, exactly as if the setting were
        // absent. Verified by asking the live gateway for three and getting none back.
        alternatives: std.features.alternatives,
    }
}

// =============================================================================
// Response parsing
// =============================================================================

/// Turn a vendor's 2xx body into canonical results — one per channel.
pub fn parse_response(
    vendor: PrerecordedVendor,
    body: &serde_json::Value,
) -> Result<Vec<STTResult>, String> {
    match vendor {
        PrerecordedVendor::Deepgram => parse_deepgram(body),
        PrerecordedVendor::AssemblyAI => parse_assemblyai(body),
        PrerecordedVendor::ElevenLabs => parse_elevenlabs(body),
    }
}

/// Deepgram prerecorded: `results.channels[].alternatives[0]`.
///
/// One result per channel rather than a concatenation, so a multi-channel response keeps each
/// channel's own word timeline instead of merging two into one nonsensical sequence.
fn parse_deepgram(body: &serde_json::Value) -> Result<Vec<STTResult>, String> {
    let channels = body
        .pointer("/results/channels")
        .and_then(|c| c.as_array())
        .ok_or("no results.channels in the response")?;
    let duration = body
        .pointer("/metadata/duration")
        .and_then(serde_json::Value::as_f64);
    let detected_language = body
        .pointer("/results/channels/0/detected_language")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let mut out = Vec::with_capacity(channels.len());
    for channel in channels {
        let Some(alt) = channel.pointer("/alternatives/0") else {
            continue;
        };
        let transcript = alt
            .get("transcript")
            .and_then(|t| t.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        let confidence = alt
            .get("confidence")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(1.0) as f32;

        let words: Option<Vec<WordTiming>> =
            alt.get("words").and_then(|w| w.as_array()).map(|ws| {
                ws.iter()
                    .map(|w| WordTiming {
                        // `punctuated_word` is what `smart_format` produces; falling back to the
                        // bare `word` means a punctuated transcript and unpunctuated word list,
                        // which is the kind of mismatch nobody notices until they rebuild the
                        // text from the words.
                        word: w
                            .get("punctuated_word")
                            .or_else(|| w.get("word"))
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        start: w
                            .get("start")
                            .and_then(serde_json::Value::as_f64)
                            .unwrap_or(0.0),
                        end: w
                            .get("end")
                            .and_then(serde_json::Value::as_f64)
                            .unwrap_or(0.0),
                        confidence: w
                            .get("confidence")
                            .and_then(serde_json::Value::as_f64)
                            .map(|c| c as f32),
                        // Deepgram numbers its speakers; the canonical form is a label.
                        speaker_id: w
                            .get("speaker")
                            .and_then(serde_json::Value::as_u64)
                            .map(|n| format!("speaker_{n}")),
                        logprob: None,
                    })
                    .collect()
            });

        let mut result = STTResult::new(transcript, true, true, confidence);
        result.speakers = speakers_from(words.as_deref());
        result.words = words;
        result.detected_language = detected_language.clone();
        result.audio_duration = duration;
        // The RUNNER-UP hypotheses. `alternatives=3` asked Deepgram for three, Deepgram returned
        // three, and keeping only `[0]` meant the setting reached the vendor, was honoured, and
        // could not be seen — the same defect as the word timings above it.
        let runners_up: Vec<String> = channel
            .get("alternatives")
            .and_then(|a| a.as_array())
            .map(|alts| {
                alts.iter()
                    .skip(1)
                    .filter_map(|a| a.get("transcript").and_then(|t| t.as_str()))
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        if !runners_up.is_empty() {
            result.alternatives = Some(runners_up);
        }
        out.push(result);
    }
    if out.is_empty() {
        return Err("results.channels was empty".to_string());
    }
    Ok(out)
}

/// AssemblyAI: a completed transcript object.
fn parse_assemblyai(body: &serde_json::Value) -> Result<Vec<STTResult>, String> {
    let transcript = body
        .get("text")
        .and_then(|t| t.as_str())
        .ok_or("no text in the transcript")?
        .trim()
        .to_string();
    let confidence = body
        .get("confidence")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(1.0) as f32;

    let words: Option<Vec<WordTiming>> = body.get("words").and_then(|w| w.as_array()).map(|ws| {
        ws.iter()
            .map(|w| WordTiming {
                word: w
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                // AssemblyAI reports MILLISECONDS. Every other provider in this module reports
                // seconds, and a word list silently a thousand times too long is the kind of bug
                // that survives review because nothing about it looks wrong.
                start: ms_to_secs(w.get("start")),
                end: ms_to_secs(w.get("end")),
                confidence: w
                    .get("confidence")
                    .and_then(serde_json::Value::as_f64)
                    .map(|c| c as f32),
                speaker_id: w
                    .get("speaker")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                logprob: None,
            })
            .collect()
    });

    let mut result = STTResult::new(transcript, true, true, confidence);
    result.speakers = speakers_from(words.as_deref());
    result.words = words;
    result.detected_language = body
        .get("language_code")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    // `audio_duration` is in seconds here, unlike the word timings.
    result.audio_duration = body
        .get("audio_duration")
        .and_then(serde_json::Value::as_f64);
    Ok(vec![result])
}

/// ElevenLabs: `text` + `words`, or a `transcripts` array when multi-channel was requested.
fn parse_elevenlabs(body: &serde_json::Value) -> Result<Vec<STTResult>, String> {
    let channels: Vec<&serde_json::Value> = match body.get("transcripts").and_then(|t| t.as_array())
    {
        Some(list) => list.iter().collect(),
        None => vec![body],
    };
    let mut out = Vec::with_capacity(channels.len());
    for channel in channels {
        let transcript = channel
            .get("text")
            .and_then(|t| t.as_str())
            .ok_or("no text in the response")?
            .trim()
            .to_string();

        let words: Option<Vec<WordTiming>> =
            channel.get("words").and_then(|w| w.as_array()).map(|ws| {
                ws.iter()
                    // `spacing` and `audio_event` tokens are not words; keeping them would put
                    // bare whitespace entries in an array every consumer indexes by word.
                    .filter(|w| w.get("type").and_then(|t| t.as_str()).unwrap_or("word") == "word")
                    .map(|w| WordTiming {
                        word: w
                            .get("text")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        start: w
                            .get("start")
                            .and_then(serde_json::Value::as_f64)
                            .unwrap_or(0.0),
                        end: w
                            .get("end")
                            .and_then(serde_json::Value::as_f64)
                            .unwrap_or(0.0),
                        confidence: None,
                        speaker_id: w
                            .get("speaker_id")
                            .and_then(|v| v.as_str())
                            .map(str::to_string),
                        logprob: w.get("logprob").and_then(serde_json::Value::as_f64),
                    })
                    .collect()
            });

        let mut result = STTResult::new(
            transcript, true, true,
            // ElevenLabs reports a LANGUAGE probability, not a transcript confidence. Using it as
            // one would be a different number wearing this field's name.
            1.0,
        );
        result.speakers = speakers_from(words.as_deref());
        result.words = words;
        result.detected_language = channel
            .get("language_code")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        result.audio_duration = channel
            .get("audio_duration_secs")
            .and_then(serde_json::Value::as_f64);
        out.push(result);
    }
    Ok(out)
}

/// Milliseconds to seconds, for the one vendor that reports them.
fn ms_to_secs(v: Option<&serde_json::Value>) -> f64 {
    v.and_then(serde_json::Value::as_f64).unwrap_or(0.0) / 1000.0
}

/// The distinct speakers a word list mentions, in first-appearance order.
fn speakers_from(words: Option<&[WordTiming]>) -> Option<Vec<SpeakerInfo>> {
    let words = words?;
    let mut ids: Vec<String> = Vec::new();
    for w in words {
        if let Some(id) = &w.speaker_id
            && !ids.contains(id)
        {
            ids.push(id.clone());
        }
    }
    (!ids.is_empty()).then(|| ids.into_iter().map(SpeakerInfo::new).collect())
}

// =============================================================================
// BaseSTT
// =============================================================================

#[async_trait::async_trait]
impl BaseSTT for PrerecordedSTT {
    /// Not constructible from the flat config: which vendor to speak to is not in it, and
    /// guessing from `provider` would silently reroute a live session. Use
    /// [`PrerecordedSTT::new_standard`], which the prerecorded factory calls.
    fn new(_config: STTConfig) -> Result<Self, STTError> {
        Err(STTError::ConfigurationError(
            "PrerecordedSTT is built from the standardized config by \
             create_stt_standard_prerecorded, not from the flat factory"
                .to_string(),
        ))
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        self.connected.store(true, Ordering::Release);
        info!(
            vendor = self.vendor.id(),
            "prerecorded STT ready to receive audio"
        );
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
        if !self.connected.load(Ordering::Acquire) {
            return Ok(());
        }
        if !self.buffer.is_empty()
            && let Err(e) = self.flush_buffer().await
        {
            // Logged, not returned: the connection must still close, and the error already
            // reached the caller through the error callback in `flush_buffer`.
            warn!(vendor = self.vendor.id(), error = %e, "prerecorded transcription failed");
        }
        self.connected.store(false, Ordering::Release);
        self.buffer.clear();
        *self.result_callback.lock().await = None;
        *self.error_callback.lock().await = None;
        info!(
            vendor = self.vendor.id(),
            bytes = self.total_bytes_received,
            "prerecorded STT disconnected"
        );
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }

    async fn send_audio(&mut self, audio_data: Bytes) -> Result<(), STTError> {
        if !self.is_ready() {
            return Err(STTError::ConnectionFailed(format!(
                "{} prerecorded STT not connected",
                self.vendor.id()
            )));
        }
        // Refused rather than flushed-and-continued: splitting one upload into several requests
        // would bill twice and lose context across the seam, and a silently halved transcript is
        // worse than a named refusal.
        if self.buffer.len() + audio_data.len() > MAX_BUFFER_SIZE_BYTES {
            return Err(STTError::AudioProcessingError(format!(
                "audio exceeds the {MAX_BUFFER_SIZE_BYTES}-byte ceiling this gateway holds in \
                 memory for a single upload"
            )));
        }
        self.total_bytes_received += audio_data.len() as u64;
        self.buffer.extend_from_slice(&audio_data);
        Ok(())
    }

    async fn on_result(&mut self, callback: STTResultCallback) -> Result<(), STTError> {
        *self.result_callback.lock().await = Some(Box::new(move |result| {
            let cb = callback.clone();
            Box::pin(async move {
                cb(result).await;
            })
        }));
        Ok(())
    }

    async fn on_error(&mut self, callback: STTErrorCallback) -> Result<(), STTError> {
        *self.error_callback.lock().await = Some(Box::new(move |error| {
            let cb = callback.clone();
            Box::pin(async move {
                cb(error).await;
            })
        }));
        Ok(())
    }

    fn get_config(&self) -> Option<&STTConfig> {
        Some(&self.config.base)
    }

    async fn update_config(&mut self, config: STTConfig) -> Result<(), STTError> {
        if self.is_ready() {
            self.disconnect().await?;
        }
        self.config.base = config;
        self.connect().await
    }

    fn get_provider_info(&self) -> &'static str {
        self.vendor.provider_info()
    }

    /// See [`BaseSTT::is_request_response`]. This is what stops the upload driver waiting out a
    /// 45-second first-result timeout for a stream that never arrives.
    fn is_request_response(&self) -> bool {
        true
    }

    fn set_resilience(&mut self, resilience: crate::core::resilience::ResilienceHandles) {
        self.resilience.set_handles(resilience);
    }

    /// The `config_warnings` the request builder produced — a translation target list handed to a
    /// vendor that cannot translate, a batch knob with no equivalent. Logged since this driver
    /// was written; now also returned, so the caller learns their setting was dropped.
    fn config_warnings(&self) -> Vec<String> {
        self.warnings.clone()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::stt::standard::{StandardSTTConfig, SttFeatures};
    use serde_json::json;

    fn cfg(provider: &str, model: &str) -> StandardSTTConfig {
        StandardSTTConfig::from_base(STTConfig {
            provider: provider.into(),
            api_key: "k".into(),
            language: "en-US".into(),
            sample_rate: 16_000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".into(),
            model: model.into(),
        })
    }

    // -------------------------------------------------------------------------------------
    // Which vendors have a prerecorded path
    // -------------------------------------------------------------------------------------

    #[test]
    fn the_three_wired_vendors_resolve_and_nothing_else_does() {
        assert_eq!(
            PrerecordedVendor::from_provider("deepgram"),
            Some(PrerecordedVendor::Deepgram)
        );
        assert_eq!(
            PrerecordedVendor::from_provider("  AssemblyAI "),
            Some(PrerecordedVendor::AssemblyAI)
        );
        assert_eq!(
            PrerecordedVendor::from_provider("elevenlabs"),
            Some(PrerecordedVendor::ElevenLabs)
        );
        // OpenAI and Groq are absent ON PURPOSE: their STT clients already post to
        // `/v1/audio/transcriptions`, so a second client would be a second thing to keep correct.
        assert_eq!(PrerecordedVendor::from_provider("openai"), None);
        assert_eq!(PrerecordedVendor::from_provider("groq"), None);
        assert_eq!(PrerecordedVendor::from_provider("azure"), None);
    }

    #[test]
    fn only_assemblyai_hands_back_a_job_to_poll() {
        assert!(PrerecordedVendor::AssemblyAI.is_async());
        assert!(!PrerecordedVendor::Deepgram.is_async());
        assert!(!PrerecordedVendor::ElevenLabs.is_async());
    }

    #[test]
    fn provider_info_names_the_transport_not_just_the_vendor() {
        // Two clients serve each of these vendors. A log line saying only "Deepgram" cannot tell
        // an operator which of the two ran, which is exactly the question when latency changes.
        for v in [
            PrerecordedVendor::Deepgram,
            PrerecordedVendor::AssemblyAI,
            PrerecordedVendor::ElevenLabs,
        ] {
            let info = v.provider_info().to_lowercase();
            assert!(
                info.contains("prerecorded") || info.contains("batch"),
                "{info}"
            );
        }
    }

    #[tokio::test]
    async fn every_prerecorded_client_answers_on_close() {
        for (provider, model) in [
            ("deepgram", "nova-3"),
            ("assemblyai", ""),
            ("elevenlabs", "scribe_v2"),
        ] {
            let vendor = PrerecordedVendor::from_provider(provider).unwrap();
            let client = PrerecordedSTT::new_standard(vendor, &cfg(provider, model)).unwrap();
            assert!(
                BaseSTT::is_request_response(&client),
                "{provider} must skip the settle wait"
            );
        }
    }

    #[tokio::test]
    async fn construction_requires_a_credential() {
        let mut c = cfg("deepgram", "nova-3");
        c.base.api_key = String::new();
        assert!(matches!(
            PrerecordedSTT::new_standard(PrerecordedVendor::Deepgram, &c),
            Err(STTError::AuthenticationFailed(_))
        ));
    }

    #[tokio::test]
    async fn an_unusable_endpoint_override_is_refused_before_any_request() {
        // Construction is where a bad configuration is caught, while the offending value is
        // still in hand — which is what lets the upload handler answer 400 rather than 502.
        //
        // Only the scheme and parse cases are asserted, and that is a statement about the
        // TEST BINARY rather than about production. `WAAV_ALLOW_LOOPBACK_ENDPOINTS` is a
        // process-global escape hatch that other tests in this binary set so they can point a
        // provider at a local mock — and `validate_url_for_ssrf_inner` answers `Ok(())` for ANY
        // host once it is on, not merely for loopback. So an address assertion here passes alone
        // and fails in a full parallel run, depending on which test got there first.
        //
        // The address rules are exercised where they belong, in `core::net`.
        for bad in ["file:///etc/passwd", "not a url", "ftp://example.com/x"] {
            let mut c = cfg("deepgram", "nova-3");
            c.extras.0.insert("endpoint_override".into(), json!(bad));
            assert!(
                PrerecordedSTT::new_standard(PrerecordedVendor::Deepgram, &c).is_err(),
                "{bad} should be refused"
            );
        }
    }

    #[tokio::test]
    async fn sending_audio_before_connecting_is_refused() {
        let mut client =
            PrerecordedSTT::new_standard(PrerecordedVendor::Deepgram, &cfg("deepgram", "nova-3"))
                .unwrap();
        assert!(
            client
                .send_audio(Bytes::from_static(&[0u8; 16]))
                .await
                .is_err()
        );
        client.connect().await.unwrap();
        assert!(
            client
                .send_audio(Bytes::from_static(&[0u8; 16]))
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn a_failed_flush_reaches_the_error_callback() {
        // `disconnect` must still close, so it LOGS the flush error rather than returning it —
        // and the upload driver reads only the callbacks. An unreported failure therefore
        // surfaces as an empty transcript and HTTP 200, the worst of the three outcomes because
        // it looks like silence.
        // The failure is forced at REQUEST-BUILD time rather than over the network: a realtime
        // model id on the batch transport. That keeps the test hermetic — no host to dial, and
        // nothing that could pass for a reason unrelated to the one under test.
        let c = cfg("elevenlabs", "scribe_v2_realtime");
        let mut client = PrerecordedSTT::new_standard(PrerecordedVendor::ElevenLabs, &c).unwrap();

        let errors = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink = Arc::clone(&errors);
        let on_error: STTErrorCallback = Arc::new(move |e: STTError| {
            let sink = Arc::clone(&sink);
            Box::pin(async move { sink.lock().await.push(e.to_string()) })
        });
        client.on_error(on_error).await.unwrap();
        client.connect().await.unwrap();
        client
            .send_audio(Bytes::from(vec![0u8; 3200]))
            .await
            .unwrap();
        client.disconnect().await.unwrap();

        assert_eq!(errors.lock().await.len(), 1, "exactly one reported error");
    }

    // -------------------------------------------------------------------------------------
    // Batch-only knobs
    // -------------------------------------------------------------------------------------

    #[test]
    fn prerecorded_only_knobs_come_from_extras() {
        // They have no canonical name and reach one vendor each; inventing canonical names would
        // put controls on every deployment's settings page that most vendors ignore.
        let mut c = cfg("deepgram", "nova-3");
        for k in ["summarize", "topics", "intents", "paragraphs", "utterances"] {
            c.extras.0.insert(k.into(), json!(true));
        }
        let b = batch_features_from(&c);
        assert_eq!(b.summarize, Some(true));
        assert_eq!(b.topics, Some(true));
        assert_eq!(b.intents, Some(true));
        assert_eq!(b.paragraphs, Some(true));
        assert_eq!(b.utterances, Some(true));
    }

    #[test]
    fn the_canonical_alternatives_setting_reaches_the_request() {
        // It did not. The Deepgram builder reads `batch.alternatives` and never
        // `features.alternatives`, so a deployment asking for three hypotheses sent no parameter
        // at all and got one back — the setting was configurable, publishable, and inert.
        let mut c = cfg("deepgram", "nova-3");
        c.features = SttFeatures {
            alternatives: Some(3),
            ..Default::default()
        };
        assert_eq!(batch_features_from(&c).alternatives, Some(3));

        // And it must reach the wire, not merely the envelope.
        let req = crate::core::stt::batch::BatchTranscribeRequest {
            audio: crate::core::stt::batch::BatchAudioSource::Bytes {
                audio_base64: String::new(),
                content_type: None,
            },
            config: c.clone(),
            batch: batch_features_from(&c),
            callback_url: None,
            callback_method: None,
            translation: None,
        };
        let sub = crate::core::stt::batch::build_deepgram_prerecorded_with(
            &req,
            ResolvedAudio::wav(vec![0u8; 8]),
            "k",
            "https://api.deepgram.com",
        )
        .unwrap();
        assert!(
            sub.request.url.contains("alternatives=3"),
            "alternatives never reached the URL: {}",
            sub.request.url
        );
    }

    #[test]
    fn the_canonical_knobs_are_not_duplicated_into_batch_features() {
        // `detect_language` has a canonical name AND is read off `features` by the builders, so
        // setting it here too would be two sources for one answer. `alternatives` is not that
        // case — see `the_canonical_alternatives_setting_reaches_the_request`.
        let mut c = cfg("deepgram", "nova-3");
        c.features = SttFeatures {
            language_detection: Some(true),
            alternatives: Some(3),
            ..Default::default()
        };
        let b = batch_features_from(&c);
        assert_eq!(b.detect_language, None);
    }

    // -------------------------------------------------------------------------------------
    // Deepgram parsing
    // -------------------------------------------------------------------------------------

    #[test]
    fn deepgram_prerecorded_body_parses() {
        let body = json!({
            "metadata": {"duration": 2.5},
            "results": {"channels": [{
                "alternatives": [{
                    "transcript": "hello world",
                    "confidence": 0.98,
                    "words": [
                        {"word": "hello", "punctuated_word": "Hello,", "start": 0.1, "end": 0.4,
                         "confidence": 0.99, "speaker": 0},
                        {"word": "world", "start": 0.5, "end": 0.9, "confidence": 0.97, "speaker": 1}
                    ]
                }]
            }]}
        });
        let out = parse_response(PrerecordedVendor::Deepgram, &body).unwrap();

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].transcript, "hello world");
        assert!((out[0].confidence - 0.98).abs() < 1e-6);
        assert_eq!(out[0].audio_duration, Some(2.5));
        let words = out[0].words.as_ref().unwrap();
        // `punctuated_word` wins: a punctuated transcript with an unpunctuated word list is the
        // kind of mismatch nobody notices until they rebuild the text from the words.
        assert_eq!(words[0].word, "Hello,");
        assert_eq!(words[1].word, "world");
        assert_eq!(words[0].speaker_id.as_deref(), Some("speaker_0"));
        assert_eq!(out[0].speakers.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn deepgram_runner_up_hypotheses_survive() {
        // `alternatives=3` asked Deepgram for three, Deepgram returned three, and keeping only
        // `[0]` meant the setting reached the vendor, was honoured, and could not be seen.
        let body = json!({"results": {"channels": [{
            "alternatives": [
                {"transcript": "life moves pretty fast", "confidence": 0.99},
                {"transcript": "life moves pretty vast", "confidence": 0.71},
                {"transcript": "life move pretty fast", "confidence": 0.64}
            ]
        }]}});
        let out = parse_response(PrerecordedVendor::Deepgram, &body).unwrap();

        assert_eq!(
            out[0].transcript, "life moves pretty fast",
            "the BEST stays the transcript"
        );
        assert_eq!(
            out[0].alternatives.as_deref(),
            Some(
                &[
                    "life moves pretty vast".to_string(),
                    "life move pretty fast".to_string()
                ][..]
            ),
            "the runners-up, in the provider's order"
        );
    }

    #[test]
    fn one_hypothesis_means_no_alternatives_rather_than_an_empty_list() {
        // The default. An empty array would read as "asked for, and there were none".
        let body = json!({"results": {"channels": [{
            "alternatives": [{"transcript": "only one"}]
        }]}});
        let out = parse_response(PrerecordedVendor::Deepgram, &body).unwrap();
        assert_eq!(out[0].alternatives, None);
    }

    #[test]
    fn deepgram_multichannel_yields_one_result_per_channel() {
        let body = json!({"results": {"channels": [
            {"alternatives": [{"transcript": "agent here"}]},
            {"alternatives": [{"transcript": "customer here"}]}
        ]}});
        let out = parse_response(PrerecordedVendor::Deepgram, &body).unwrap();

        assert_eq!(out.len(), 2);
        assert_eq!(out[0].transcript, "agent here");
        assert_eq!(out[1].transcript, "customer here");
    }

    #[test]
    fn deepgram_silence_is_an_empty_transcript_not_an_error() {
        // Deepgram answers a well-formed body with an empty string for silence. Treating that as
        // a parse failure would turn "nobody spoke" into a 502.
        let body = json!({"results": {"channels": [{"alternatives": [{"transcript": ""}]}]}});
        let out = parse_response(PrerecordedVendor::Deepgram, &body).unwrap();
        assert_eq!(out[0].transcript, "");
    }

    #[test]
    fn a_deepgram_body_with_no_channels_is_an_error_not_silence() {
        // The distinction that decides whether the caller sees a 502 or a confident empty string.
        let body = json!({"metadata": {"duration": 1.0}});
        assert!(parse_response(PrerecordedVendor::Deepgram, &body).is_err());
    }

    // -------------------------------------------------------------------------------------
    // AssemblyAI parsing
    // -------------------------------------------------------------------------------------

    #[test]
    fn assemblyai_word_timings_are_converted_from_milliseconds() {
        // THE bug this test exists for. AssemblyAI reports milliseconds; every other provider
        // here reports seconds. A word list silently a thousand times too long survives review
        // because nothing about the numbers looks wrong.
        let body = json!({
            "status": "completed",
            "text": "hello world",
            "confidence": 0.95,
            "audio_duration": 2,
            "language_code": "en",
            "words": [
                {"text": "hello", "start": 100, "end": 400, "confidence": 0.99, "speaker": "A"},
                {"text": "world", "start": 500, "end": 900, "confidence": 0.98, "speaker": "B"}
            ]
        });
        let out = parse_response(PrerecordedVendor::AssemblyAI, &body).unwrap();

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].transcript, "hello world");
        let words = out[0].words.as_ref().unwrap();
        assert!((words[0].start - 0.1).abs() < 1e-9, "{}", words[0].start);
        assert!((words[0].end - 0.4).abs() < 1e-9);
        assert!((words[1].start - 0.5).abs() < 1e-9);
        // `audio_duration` is in SECONDS in the same body — the asymmetry is real.
        assert_eq!(out[0].audio_duration, Some(2.0));
        assert_eq!(out[0].detected_language.as_deref(), Some("en"));
        assert_eq!(out[0].speakers.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn an_assemblyai_body_with_no_text_is_an_error() {
        assert!(
            parse_response(PrerecordedVendor::AssemblyAI, &json!({"status": "queued"})).is_err()
        );
    }

    #[test]
    fn an_errored_assemblyai_job_reports_the_vendors_own_message() {
        let body = json!({"status": "error", "error": "Audio file is not readable"});
        let err = terminal_or_err(&body).unwrap_err();
        assert!(
            format!("{err}").contains("Audio file is not readable"),
            "{err}"
        );
    }

    #[test]
    fn a_completed_assemblyai_job_passes_straight_through() {
        let body = json!({"status": "completed", "text": "done"});
        assert_eq!(terminal_or_err(&body).unwrap(), body);
    }

    // -------------------------------------------------------------------------------------
    // ElevenLabs parsing (moved here with the client it belonged to)
    // -------------------------------------------------------------------------------------

    #[test]
    fn elevenlabs_body_parses_and_drops_non_word_tokens() {
        let body = json!({
            "language_code": "eng",
            "text": "Hello world",
            "audio_duration_secs": 1.5,
            "words": [
                {"text": "Hello", "type": "word", "start": 0.0, "end": 0.4, "speaker_id": "speaker_0"},
                {"text": " ", "type": "spacing", "start": 0.4, "end": 0.45},
                {"text": "world", "type": "word", "start": 0.45, "end": 0.9, "speaker_id": "speaker_1"}
            ]
        });
        let out = parse_response(PrerecordedVendor::ElevenLabs, &body).unwrap();

        assert_eq!(out[0].transcript, "Hello world");
        assert_eq!(out[0].detected_language.as_deref(), Some("eng"));
        assert_eq!(out[0].audio_duration, Some(1.5));
        // `spacing` is not a word: keeping it would put a bare whitespace entry in an array
        // every consumer indexes by word.
        let words = out[0].words.as_ref().unwrap();
        assert_eq!(words.len(), 2);
        assert_eq!(words[1].speaker_id.as_deref(), Some("speaker_1"));
    }

    #[test]
    fn elevenlabs_multi_channel_yields_one_result_per_channel() {
        let body = json!({"transcripts": [
            {"text": "agent speaking"},
            {"text": "customer speaking"}
        ]});
        let out = parse_response(PrerecordedVendor::ElevenLabs, &body).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[1].transcript, "customer speaking");
    }

    #[test]
    fn an_elevenlabs_body_with_no_text_is_an_error() {
        assert!(parse_response(PrerecordedVendor::ElevenLabs, &json!({"detail": "nope"})).is_err());
    }

    // -------------------------------------------------------------------------------------
    // Whose problem is it
    // -------------------------------------------------------------------------------------

    #[test]
    fn a_vendor_4xx_is_the_callers_problem_not_the_vendors() {
        // The live case: `nova-3-medical` is English-only, so `language=de` came back from
        // Deepgram as `400 No such model/language/tier combination found` — and WaaV turned that
        // into a 502, sending the operator to read a status page about a value they chose.
        for code in [400, 404, 409, 413, 422] {
            let status = reqwest::StatusCode::from_u16(code).unwrap();
            assert!(
                matches!(
                    classify_vendor_status(status, "x".into()),
                    STTError::ConfigurationError(_)
                ),
                "{code} should reach the caller as a 400"
            );
        }
    }

    #[test]
    fn a_bad_credential_is_not_reported_as_a_bad_request() {
        // The credential is WaaV's, not the caller's. Telling them "bad request" sends them to
        // look at their own request, where they will find nothing wrong with it.
        for code in [401, 403] {
            let status = reqwest::StatusCode::from_u16(code).unwrap();
            assert!(matches!(
                classify_vendor_status(status, "x".into()),
                STTError::AuthenticationFailed(_)
            ));
        }
    }

    #[test]
    fn slow_and_throttled_stay_upstream() {
        // "Too slow" and "too many" are not malformed requests — they are the upstream being
        // unable to serve this one right now, which is what 502 means here.
        for code in [408, 429, 500, 502, 503] {
            let status = reqwest::StatusCode::from_u16(code).unwrap();
            assert!(
                matches!(
                    classify_vendor_status(status, "x".into()),
                    STTError::ProviderError(_)
                ),
                "{code} should stay upstream"
            );
        }
    }

    // -------------------------------------------------------------------------------------
    // Error rendering
    // -------------------------------------------------------------------------------------

    #[test]
    fn a_fastapi_validation_list_names_the_field() {
        // Rendered naively this reaches the caller as a debug-printed Vec, which is how a fixable
        // "unknown model" ends up looking like an internal fault.
        let rendered = describe_error(
            PrerecordedVendor::ElevenLabs,
            reqwest::StatusCode::UNPROCESSABLE_ENTITY,
            r#"{"detail":[{"loc":["body","model_id"],"msg":"unknown model","type":"value_error"}]}"#,
        );
        assert!(rendered.contains("body.model_id"), "{rendered}");
        assert!(rendered.contains("unknown model"), "{rendered}");
    }

    #[test]
    fn each_vendors_own_error_key_is_read() {
        let dg = describe_error(
            PrerecordedVendor::Deepgram,
            reqwest::StatusCode::BAD_REQUEST,
            r#"{"err_code":"INVALID_AUTH","err_msg":"Invalid credentials."}"#,
        );
        assert!(dg.contains("Invalid credentials."), "{dg}");

        let aai = describe_error(
            PrerecordedVendor::AssemblyAI,
            reqwest::StatusCode::BAD_REQUEST,
            r#"{"error":"not a valid audio_url"}"#,
        );
        assert!(aai.contains("not a valid audio_url"), "{aai}");
    }

    #[test]
    fn an_unrecognised_body_is_passed_through_rather_than_swallowed() {
        let rendered = describe_error(
            PrerecordedVendor::Deepgram,
            reqwest::StatusCode::BAD_GATEWAY,
            "<html>nginx</html>",
        );
        assert!(rendered.contains("nginx"), "{rendered}");
        assert!(rendered.contains("deepgram"), "{rendered}");
    }

    // -------------------------------------------------------------------------------------
    // Polling
    // -------------------------------------------------------------------------------------

    #[test]
    fn the_poll_interval_backs_off_but_stays_bounded() {
        // A three-second clip is done on the first or second poll; a thirty-minute recording is
        // not, and asking it twice a second for half an hour is 3,600 pointless requests.
        assert_eq!(poll_interval(Duration::from_secs(0)), POLL_INTERVAL_START);
        assert!(poll_interval(Duration::from_secs(30)) > POLL_INTERVAL_START);
        assert_eq!(poll_interval(Duration::from_secs(600)), POLL_INTERVAL_MAX);
        assert_eq!(
            poll_interval(Duration::from_secs(86_400)),
            POLL_INTERVAL_MAX
        );
    }

    #[test]
    fn the_poll_deadline_matches_the_drivers_own_overall_deadline() {
        // Two bounds on "how long may one file take" that disagreed would make the effective
        // limit whichever happened to be smaller, which is not a decision anyone made.
        assert_eq!(POLL_DEADLINE, Duration::from_secs(300));
    }
}
