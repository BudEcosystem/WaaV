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
    if api_key.is_empty() && endpoint.vendor != "self_hosted" {
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
/// Not yet served. WaaV's STT providers are streaming-only — `BaseSTT` connects, receives audio
/// frames and emits callbacks — and there is no batch file-transcription path to drive from a
/// single upload. Building one is M5 work, and the honest answer meanwhile is a 501 that says
/// so, rather than a 500 or a silent empty transcript.
pub async fn transcription_not_implemented() -> Response {
    openai_error(
        StatusCode::NOT_IMPLEMENTED,
        "api_error",
        "Audio transcription is not yet served by this gateway. WaaV's STT providers are \
         streaming-only; the batch file path is in progress."
            .to_string(),
        None,
    )
}
