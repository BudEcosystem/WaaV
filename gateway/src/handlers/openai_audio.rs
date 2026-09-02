//! OpenAI-compatible audio handlers (FRD-018 T3.7).
//!
//! These are what make a WaaV voice endpoint reachable as an ordinary Bud model: budapp
//! registers the endpoint, the ingress routes `/v1/audio/*` here, and callers use the same
//! request shapes they already use against OpenAI.
//!
//! The translation itself lives in the `waav-openai-audio` crate — pure functions, no I/O,
//! exhaustively tested. What remains here is genuinely HTTP: reading the body, resolving the
//! endpoint against the control plane, driving the TTS/STT provider, and writing the response
//! with correct headers.

use axum::{
    body::Bytes,
    extract::{Multipart, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Json, Response},
};
use std::sync::Arc;
use tracing::{info, warn};
use waav_openai_audio::{
    speech::{self, SpeechRequest},
    transcription::{self, TranscriptionRequest, TranscriptionResponseFormat},
    AudioError,
};

use crate::state::AppState;

/// Render a translation failure as OpenAI's error envelope.
///
/// Shape matters: SDKs branch on `error.type` and surface `error.message`, so an ad-hoc body
/// would reach the user as "unknown error" no matter how good the text is.
fn error_response(err: &AudioError) -> Response {
    let (status, kind) = match err {
        AudioError::Missing { .. } => (StatusCode::BAD_REQUEST, "invalid_request_error"),
        AudioError::Unsupported { .. } => (StatusCode::BAD_REQUEST, "invalid_request_error"),
        AudioError::OutOfRange { .. } => (StatusCode::BAD_REQUEST, "invalid_request_error"),
        AudioError::TooLarge { .. } => (StatusCode::PAYLOAD_TOO_LARGE, "invalid_request_error"),
    };
    (
        status,
        Json(serde_json::json!({
            "error": {
                "message": err.to_string(),
                "type": kind,
                "param": serde_json::Value::Null,
                "code": serde_json::Value::Null,
            }
        })),
    )
        .into_response()
}

fn not_found(endpoint: &str, capability: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({
            "error": {
                "message": format!(
                    "Model '{endpoint}' not found or does not support {capability}"
                ),
                "type": "invalid_request_error",
                "param": "model",
                "code": "model_not_found",
            }
        })),
    )
        .into_response()
}

/// `POST /v1/audio/speech`
pub async fn speech_handler(
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let req: SpeechRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": {
                        "message": format!("Invalid request body: {e}"),
                        "type": "invalid_request_error",
                    }
                })),
            )
                .into_response();
        }
    };

    let settings = match speech::translate(req) {
        Ok(s) => s,
        Err(e) => return error_response(&e),
    };

    info!(
        endpoint = %settings.endpoint,
        format = settings.format.as_str(),
        chars = settings.text.chars().count(),
        "openai audio/speech"
    );

    let Some(voice_endpoint) = state.resolve_voice_endpoint(&settings.endpoint, "text_to_speech")
    else {
        return not_found(&settings.endpoint, "text_to_speech");
    };

    match state
        .synthesize(&voice_endpoint, &settings)
        .await
    {
        Ok(audio) => {
            let mut headers = HeaderMap::new();
            headers.insert(
                header::CONTENT_TYPE,
                settings.format.content_type().parse().unwrap_or(
                    "application/octet-stream".parse().expect("static header value"),
                ),
            );
            // Raw samples carry no container, so the rate has to travel out of band or the
            // caller cannot play what they were sent. WaaV's own /speak already does this.
            if matches!(settings.format, speech::AudioFormat::Pcm)
                && let Some(rate) = audio.sample_rate
                && let Ok(v) = rate.to_string().parse()
            {
                headers.insert("x-sample-rate", v);
            }
            (StatusCode::OK, headers, audio.bytes).into_response()
        }
        Err(e) => {
            warn!(endpoint = %settings.endpoint, error = %e, "synthesis failed");
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({
                    "error": { "message": e.to_string(), "type": "api_error" }
                })),
            )
                .into_response()
        }
    }
}

/// `POST /v1/audio/transcriptions`
pub async fn transcription_handler(
    State(state): State<Arc<AppState>>,
    multipart: Multipart,
) -> Response {
    handle_transcription(state, multipart, false).await
}

/// `POST /v1/audio/translations` — the same, with the target fixed to English.
pub async fn translation_handler(
    State(state): State<Arc<AppState>>,
    multipart: Multipart,
) -> Response {
    handle_transcription(state, multipart, true).await
}

async fn handle_transcription(
    state: Arc<AppState>,
    mut multipart: Multipart,
    translate_to_english: bool,
) -> Response {
    let mut model = String::new();
    let mut filename = String::new();
    let mut file: Vec<u8> = Vec::new();
    let mut response_format = None;
    let mut language = None;
    let mut prompt = None;
    let mut temperature = None;

    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or_default().to_string();
        match name.as_str() {
            "file" => {
                filename = field.file_name().unwrap_or_default().to_string();
                file = field.bytes().await.map(|b| b.to_vec()).unwrap_or_default();
            }
            "model" => model = field.text().await.unwrap_or_default(),
            "response_format" => response_format = field.text().await.ok(),
            "language" => language = field.text().await.ok(),
            "prompt" => prompt = field.text().await.ok(),
            "temperature" => temperature = field.text().await.ok().and_then(|t| t.parse().ok()),
            // Forward compatibility: an unknown part must not fail the request, because a
            // newer SDK will send fields this build has never heard of.
            _ => {}
        }
    }

    let settings = match transcription::translate(TranscriptionRequest {
        model,
        filename,
        file_len: file.len(),
        response_format,
        language,
        prompt,
        temperature,
        translate: translate_to_english,
    }) {
        Ok(s) => s,
        Err(e) => return error_response(&e),
    };

    let capability = if translate_to_english {
        "audio_translation"
    } else {
        "audio_transcription"
    };

    info!(
        endpoint = %settings.endpoint,
        bytes = file.len(),
        format = settings.response_format.as_str(),
        "openai audio/{}", if translate_to_english { "translations" } else { "transcriptions" }
    );

    let Some(voice_endpoint) = state.resolve_voice_endpoint(&settings.endpoint, capability) else {
        return not_found(&settings.endpoint, capability);
    };

    match state.transcribe(&voice_endpoint, &settings, file).await {
        Ok(result) => {
            // A vendor that returns no segment timings cannot make subtitles. Emitting one
            // caption spanning the whole recording would be worse than saying so.
            if settings.response_format.requires_timestamps() && result.segments.is_empty() {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(serde_json::json!({
                        "error": {
                            "message": format!(
                                "`{}` requires segment timestamps, which endpoint '{}' does not return",
                                settings.response_format.as_str(), settings.endpoint
                            ),
                            "type": "invalid_request_error",
                            "param": "response_format",
                        }
                    })),
                )
                    .into_response();
            }

            let mut headers = HeaderMap::new();
            if let Ok(ct) = settings.response_format.content_type().parse() {
                headers.insert(header::CONTENT_TYPE, ct);
            }

            let body = match settings.response_format {
                TranscriptionResponseFormat::Text => result.text.clone(),
                TranscriptionResponseFormat::Json => {
                    serde_json::json!({ "text": result.text }).to_string()
                }
                TranscriptionResponseFormat::VerboseJson => serde_json::json!({
                    "task": if translate_to_english { "translate" } else { "transcribe" },
                    "language": result.language,
                    "duration": result.duration,
                    "text": result.text,
                    "segments": result.segments,
                })
                .to_string(),
                TranscriptionResponseFormat::Srt => result.to_srt(),
                TranscriptionResponseFormat::Vtt => result.to_vtt(),
            };
            (StatusCode::OK, headers, body).into_response()
        }
        Err(e) => {
            warn!(endpoint = %settings.endpoint, error = %e, "transcription failed");
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({
                    "error": { "message": e.to_string(), "type": "api_error" }
                })),
            )
                .into_response()
        }
    }
}
