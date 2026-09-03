//! OpenAI-compatible audio handlers (FRD-018 T3.7).
//!
//! These are what make a WaaV voice endpoint reachable as an ordinary Bud model: budapp
//! registers the endpoint, the ingress routes `/v1/audio/*` here, and callers use the request
//! shapes they already use against OpenAI.
//!
//! The translation lives in the `waav-openai-audio` crate — pure functions, no I/O,
//! exhaustively tested. What remains here is genuinely HTTP: reading the body, resolving the
//! endpoint against the control plane, driving the provider, and writing correct headers.
//!
//! Synthesis goes through [`crate::handlers::speak::synthesize_once`], the same function
//! `/speak` uses. Two synthesis paths would drift on timeouts, pronunciation handling and
//! connection pooling, and the drift would show on only one of the two routes.

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Json, Response},
};
use std::sync::Arc;
use tracing::{info, warn};
use waav_openai_audio::{
    AudioError,
    speech::{self, AudioFormat, SpeechRequest},
    transcription,
};

use crate::state::AppState;

/// Render a failure as OpenAI's error envelope.
///
/// The shape matters: SDKs branch on `error.type` and surface `error.message`, so an ad-hoc
/// body reaches the user as "unknown error" however good the text is.
fn openai_error(status: StatusCode, kind: &str, message: String, param: Option<&str>) -> Response {
    (
        status,
        Json(serde_json::json!({
            "error": {
                "message": message,
                "type": kind,
                "param": param,
                "code": serde_json::Value::Null,
            }
        })),
    )
        .into_response()
}

fn translation_error(err: &AudioError) -> Response {
    let status = match err {
        AudioError::TooLarge { .. } => StatusCode::PAYLOAD_TOO_LARGE,
        _ => StatusCode::BAD_REQUEST,
    };
    openai_error(status, "invalid_request_error", err.to_string(), None)
}

fn model_not_found(endpoint: &str, capability: &str) -> Response {
    openai_error(
        StatusCode::NOT_FOUND,
        "invalid_request_error",
        format!("Model '{endpoint}' not found or does not support {capability}"),
        Some("model"),
    )
}

/// `POST /v1/audio/speech`
pub async fn speech_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // The bearer resolves the caller's alias map, which is both the alias -> endpoint id
    // mapping and the authorization boundary. Auth has already passed by the time we get here;
    // this is resolution, not a second check.
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.trim().strip_prefix("Bearer ").unwrap_or(v).to_string());

    let req: SpeechRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            return openai_error(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                format!("Invalid request body: {e}"),
                None,
            );
        }
    };

    let settings = match speech::translate(req) {
        Ok(s) => s,
        Err(e) => return translation_error(&e),
    };

    let Some(endpoint) =
        state.resolve_voice_endpoint(&settings.endpoint, "text_to_speech", bearer.as_deref())
    else {
        return model_not_found(&settings.endpoint, "text_to_speech");
    };

    // A hosted vendor with no credential is a misconfiguration worth naming here, rather than a
    // 401 from the vendor several seconds later that mentions neither Bud nor the endpoint.
    let api_key = endpoint.credential.clone().unwrap_or_default();
    if api_key.is_empty() && !crate::core::tts::self_hosted::is_self_hosted(&endpoint.vendor) {
        return openai_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "api_error",
            format!(
                "Endpoint '{}' has no credential configured for vendor '{}'",
                settings.endpoint, endpoint.vendor
            ),
            None,
        );
    }

    info!(
        endpoint = %settings.endpoint,
        vendor = %endpoint.vendor,
        format = settings.format.as_str(),
        chars = settings.text.chars().count(),
        "openai audio/speech"
    );

    let tts_config = crate::core::tts::TTSConfig {
        provider: endpoint.vendor.clone(),
        api_key,
        voice_id: Some(settings.voice.clone()),
        model: endpoint.model.clone().unwrap_or_default(),
        speaking_rate: settings.speaking_rate,
        audio_format: Some(settings.format.as_waav_format().to_string()),
        // Cleared for every compressed format. TTSConfig defaults to Some(24000), and a vendor
        // rejects a sample rate alongside a container that carries its own — Deepgram answers
        // `sample_rate is not applicable when encoding=mp3` and the whole request 400s.
        sample_rate: settings.format.accepts_sample_rate().then_some(24000),
        // Only the self-hosted provider reads this; every hosted vendor compiles its URL in.
        // It has to be threaded through here because it is per-ENDPOINT data, published by
        // budapp into voice_table, not a property of the vendor.
        api_base: endpoint.api_base.clone(),
        ..Default::default()
    };

    match crate::handlers::speak::synthesize_once(&state, tts_config, &settings.text).await {
        Ok((audio, format, sample_rate)) => {
            let mut headers = HeaderMap::new();
            if let Ok(ct) = settings.format.content_type().parse() {
                headers.insert(header::CONTENT_TYPE, ct);
            }
            if let Ok(v) = format.parse() {
                headers.insert("x-audio-format", v);
            }
            // Raw samples carry no container, so the rate has to travel out of band or the
            // caller cannot play what they were sent.
            if matches!(settings.format, AudioFormat::Pcm)
                && let Ok(v) = sample_rate.to_string().parse()
            {
                headers.insert("x-sample-rate", v);
            }
            (StatusCode::OK, headers, audio).into_response()
        }
        Err(e) => {
            warn!(endpoint = %settings.endpoint, error = %e, "synthesis failed");
            // 502, not 500: the failure is upstream of WaaV, and the distinction is what tells
            // an operator whether to look at the vendor or at us.
            openai_error(StatusCode::BAD_GATEWAY, "api_error", e, None)
        }
    }
}

/// `POST /v1/audio/transcriptions` and `/v1/audio/translations`.
///
/// OpenAI's transcription API is a multipart upload, so this is the one audio route that does
/// not take JSON. The file is decoded to PCM here and driven through a STREAMING provider by
/// [`crate::handlers::transcribe::transcribe_once`] — WaaV has no batch STT provider to call,
/// so the batch shape is synthesised from the streaming one.
pub async fn transcription_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    multipart: axum::extract::Multipart,
) -> Response {
    transcription_inner(state, headers, multipart, false).await
}

/// `POST /v1/audio/translations` — same path, but the target language is always English.
pub async fn translation_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    multipart: axum::extract::Multipart,
) -> Response {
    transcription_inner(state, headers, multipart, true).await
}

async fn transcription_inner(
    state: Arc<AppState>,
    headers: HeaderMap,
    mut multipart: axum::extract::Multipart,
    translate: bool,
) -> Response {
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.trim().strip_prefix("Bearer ").unwrap_or(v).to_string());

    let mut file: Option<Vec<u8>> = None;
    let mut filename = String::new();
    let mut model = String::new();
    let mut response_format: Option<String> = None;
    let mut language: Option<String> = None;
    let mut prompt: Option<String> = None;
    let mut temperature: Option<f32> = None;

    loop {
        let field = match multipart.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => {
                return openai_error(
                    StatusCode::BAD_REQUEST,
                    "invalid_request_error",
                    format!("Malformed multipart body: {e}"),
                    None,
                );
            }
        };
        let name = field.name().unwrap_or_default().to_string();
        if name == "file" {
            filename = field.file_name().unwrap_or_default().to_string();
            match field.bytes().await {
                Ok(b) => file = Some(b.to_vec()),
                Err(e) => {
                    return openai_error(
                        StatusCode::BAD_REQUEST,
                        "invalid_request_error",
                        format!("Could not read the uploaded file: {e}"),
                        Some("file"),
                    );
                }
            }
            continue;
        }
        let value = field.text().await.unwrap_or_default();
        match name.as_str() {
            "model" => model = value,
            "response_format" => response_format = Some(value),
            "language" => language = Some(value),
            "prompt" => prompt = Some(value),
            "temperature" => temperature = value.parse().ok(),
            _ => {}
        }
    }

    let Some(file_bytes) = file else {
        return openai_error(
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            "`file` is required".to_string(),
            Some("file"),
        );
    };

    let req = transcription::TranscriptionRequest {
        model: model.clone(),
        filename: filename.clone(),
        file_len: file_bytes.len(),
        response_format,
        language,
        prompt,
        temperature,
        translate,
    };
    let settings = match transcription::translate(req) {
        Ok(s) => s,
        Err(e) => return translation_error(&e),
    };

    let capability = if translate {
        "audio_translation"
    } else {
        "audio_transcription"
    };
    let Some(endpoint) =
        state.resolve_voice_endpoint(&settings.endpoint, capability, bearer.as_deref())
    else {
        return model_not_found(&settings.endpoint, capability);
    };

    let api_key = endpoint.credential.clone().unwrap_or_default();

    // A self-hosted deployment already speaks this exact API, so the file is FORWARDED whole
    // rather than decoded and replayed through a streaming provider. That is not a shortcut:
    // decoding would impose WaaV's WAV-only limit on a backend that may well accept mp3, and
    // the settle heuristic exists only because streaming providers never say "done" -- an
    // HTTP backend answers once and is finished.
    if crate::core::tts::self_hosted::is_self_hosted(&endpoint.vendor) {
        let Some(api_base) = endpoint.api_base.clone() else {
            return openai_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "api_error",
                format!(
                    "Endpoint '{}' is self-hosted but has no deployment URL configured",
                    settings.endpoint
                ),
                None,
            );
        };
        info!(
            endpoint = %settings.endpoint,
            capability,
            bytes = file_bytes.len(),
            "openai audio/transcriptions -> self-hosted passthrough"
        );
        return match crate::handlers::transcribe::transcribe_self_hosted(
            &api_base,
            &api_key,
            &endpoint.model.clone().unwrap_or_default(),
            file_bytes,
            &filename,
            &settings,
        )
        .await
        {
            Ok(body) => passthrough_response(&settings.response_format, body),
            Err(e) => {
                warn!(endpoint = %settings.endpoint, error = %e, "self-hosted transcription failed");
                openai_error(StatusCode::BAD_GATEWAY, "api_error", e, None)
            }
        };
    }

    // Decode BEFORE touching the provider: a container we cannot read is the caller's problem
    // and must not cost a vendor connection to discover.
    let audio = match waav_openai_audio::pcm::decode(&file_bytes, &filename) {
        Ok(a) => a,
        Err(e) => return translation_error(&e),
    };

    if api_key.is_empty() && !crate::core::tts::self_hosted::is_self_hosted(&endpoint.vendor) {
        return openai_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "api_error",
            format!(
                "Endpoint '{}' has no credential configured for vendor '{}'",
                settings.endpoint, endpoint.vendor
            ),
            None,
        );
    }

    info!(
        endpoint = %settings.endpoint,
        vendor = %endpoint.vendor,
        capability,
        secs = audio.duration_secs(),
        rate = audio.sample_rate,
        "openai audio/transcriptions"
    );

    let stt_config = crate::core::stt::STTConfig {
        provider: endpoint.vendor.clone(),
        api_key,
        language: settings.language.clone().unwrap_or_else(|| "en-US".to_string()),
        sample_rate: audio.sample_rate,
        channels: 1,
        punctuation: true,
        encoding: "linear16".to_string(),
        model: endpoint.model.clone().unwrap_or_default(),
    };

    match crate::handlers::transcribe::transcribe_once(&endpoint.vendor, stt_config, &audio).await
    {
        Ok(t) => {
            if t.truncated {
                warn!(endpoint = %settings.endpoint, "returning a partial transcript");
            }
            let result = transcription::TranscriptionResult {
                text: t.text,
                language: settings.language.clone(),
                duration: Some(audio.duration_secs()),
                segments: Vec::new(),
            };
            render_transcription(&settings.response_format, &result)
        }
        Err(e) => {
            warn!(endpoint = %settings.endpoint, error = %e, "transcription failed");
            openai_error(StatusCode::BAD_GATEWAY, "api_error", e, None)
        }
    }
}

/// Render a result in the format the caller asked for.
///
/// `text`, `srt` and `vtt` are PLAIN BODIES, not JSON — a client that asked for an SRT file
/// and got `{"text": "1\n00:00:00,000 ..."}` cannot feed it to a player.
fn render_transcription(
    format: &transcription::TranscriptionResponseFormat,
    result: &transcription::TranscriptionResult,
) -> Response {
    use transcription::TranscriptionResponseFormat as F;
    let mut headers = HeaderMap::new();
    if let Ok(ct) = format.content_type().parse() {
        headers.insert(header::CONTENT_TYPE, ct);
    }
    match format {
        F::Json => (StatusCode::OK, Json(serde_json::json!({ "text": result.text }))).into_response(),
        F::VerboseJson => (
            StatusCode::OK,
            Json(serde_json::json!({
                "task": "transcribe",
                "language": result.language,
                "duration": result.duration,
                "text": result.text,
                "segments": [],
            })),
        )
            .into_response(),
        F::Text => (StatusCode::OK, headers, result.text.clone()).into_response(),
        F::Srt => (StatusCode::OK, headers, result.to_srt()).into_response(),
        F::Vtt => (StatusCode::OK, headers, result.to_vtt()).into_response(),
    }
}

/// Return a self-hosted backend's response body unchanged, with the content type the caller
/// asked for.
///
/// Re-parsing and re-rendering it would be worse than pointless: the backend was given the
/// same `response_format`, so its body is already correct, and a round trip through our own
/// structs would drop any field we do not model (word-level timestamps, per-segment
/// confidence) from a response that had them.
fn passthrough_response(
    format: &transcription::TranscriptionResponseFormat,
    body: String,
) -> Response {
    let mut headers = HeaderMap::new();
    if let Ok(ct) = format.content_type().parse() {
        headers.insert(header::CONTENT_TYPE, ct);
    }
    (StatusCode::OK, headers, body).into_response()
}
