use axum::{
    Router,
    extract::DefaultBodyLimit,
    routing::{delete, get, post},
};
use tower_http::trace::TraceLayer;

use crate::core::stt::batch::BATCH_JSON_BODY_LIMIT_BYTES;
use crate::handlers::voices::VOICE_CLONE_JSON_BODY_LIMIT_BYTES;
use crate::handlers::{capabilities, dag, livekit, recording, sip, speak, transcribe, voices};
use crate::state::AppState;
use std::sync::Arc;

/// Ceiling for a `/v1/audio/*` upload when nothing overrides it.
///
/// Axum's own default is 2 MiB, and that is what this route surface ran with until now. It is
/// far too small for the one format the batch transcription path can decode: WAV is
/// uncompressed, so 16 kHz mono 16-bit PCM is ~1.9 MiB per MINUTE, and a one-minute clip was
/// already over the line. Worse, the limit is enforced lazily as the body streams, so it
/// surfaced from `field.bytes()` as multer's generic "Error parsing `multipart/form-data`
/// request" — a message that mentions neither size nor a limit.
///
/// 50 MiB is deliberately double OpenAI's documented 25 MiB upload ceiling. A client that
/// converts a compressed file to WAV to satisfy this gateway arrives with something several
/// times the size of what it would have sent to OpenAI, so matching their number would
/// reproduce the same failure one order of magnitude up.
pub const DEFAULT_MAX_AUDIO_UPLOAD_BYTES: usize = 50 * 1024 * 1024;

/// Environment variable that overrides [`DEFAULT_MAX_AUDIO_UPLOAD_BYTES`], in bytes.
pub const MAX_AUDIO_UPLOAD_BYTES_ENV: &str = "MAX_AUDIO_UPLOAD_BYTES";

/// The configured audio upload ceiling, in bytes.
///
/// Read from the environment on each call rather than cached: the router reads it once at
/// startup and the transcription handler reads it only to explain a refusal, so there is no
/// hot path here, and a `OnceLock` would only make the value harder to exercise in tests.
pub fn max_audio_upload_bytes() -> usize {
    parse_max_audio_upload_bytes(std::env::var(MAX_AUDIO_UPLOAD_BYTES_ENV).ok().as_deref())
}

/// Parse the override, falling back to the default for anything unusable.
///
/// Lenient in one direction only. Missing or blank is simply "not configured". A value that
/// does not parse, or a zero, is an operator mistake that would otherwise refuse every upload
/// including the ones that worked yesterday, so it is logged loudly and ignored. Refusing to
/// start is the other defensible choice, but this knob arrives through a Helm value and a typo
/// in it should not take the audio plane down.
pub fn parse_max_audio_upload_bytes(raw: Option<&str>) -> usize {
    match raw.map(str::trim) {
        None | Some("") => DEFAULT_MAX_AUDIO_UPLOAD_BYTES,
        Some(value) => match value.parse::<usize>() {
            Ok(bytes) if bytes > 0 => bytes,
            Ok(_) => {
                tracing::warn!(
                    "{MAX_AUDIO_UPLOAD_BYTES_ENV}=0 would refuse every upload; using the default of \
                     {DEFAULT_MAX_AUDIO_UPLOAD_BYTES} bytes"
                );
                DEFAULT_MAX_AUDIO_UPLOAD_BYTES
            }
            Err(e) => {
                tracing::warn!(
                    "{MAX_AUDIO_UPLOAD_BYTES_ENV}={value:?} is not a byte count ({e}); using the \
                     default of {DEFAULT_MAX_AUDIO_UPLOAD_BYTES} bytes"
                );
                DEFAULT_MAX_AUDIO_UPLOAD_BYTES
            }
        },
    }
}

/// Create the API router with protected routes
///
/// Note: Authentication middleware should be applied in main.rs after state is available
pub fn create_api_router() -> Router<Arc<AppState>> {
    // Read once, applied to all three audio routes: a per-route read could drift between them,
    // and "transcriptions accepts what speech refuses" is exactly the kind of difference nobody
    // finds until a user does.
    let audio_upload_limit = max_audio_upload_bytes();
    tracing::info!("audio upload ceiling: {audio_upload_limit} bytes");

    Router::new()
        // Protected routes (auth required when AUTH_REQUIRED=true)
        .route("/voices", get(voices::list_voices))
        .route(
            "/voices/clone",
            post(voices::clone_voice)
                .layer(DefaultBodyLimit::max(VOICE_CLONE_JSON_BODY_LIMIT_BYTES)),
        )
        .route("/speak", post(speak::speak_handler))
        // OpenAI-compatible audio surface (FRD-018). These are what the Bud ingress routes
        // /v1/audio/* to, so a WaaV voice endpoint is reachable as an ordinary Bud model.
        .route(
            "/v1/audio/speech",
            post(crate::handlers::openai_audio::speech_handler)
                .layer(DefaultBodyLimit::max(audio_upload_limit)),
        )
        .route(
            "/v1/audio/transcriptions",
            post(crate::handlers::openai_audio::transcription_handler)
                .layer(DefaultBodyLimit::max(audio_upload_limit)),
        )
        .route(
            // translation_handler, NOT transcription_handler: the two differ only by the
            // `translate` flag, and wiring both routes to the same one would make
            // /v1/audio/translations silently return source-language text that looks correct.
            "/v1/audio/translations",
            post(crate::handlers::openai_audio::translation_handler)
                .layer(DefaultBodyLimit::max(audio_upload_limit)),
        )
        .route("/livekit/token", post(livekit::generate_token))
        .route("/livekit/rooms", get(livekit::list_rooms))
        .route("/livekit/rooms/{room_name}", get(livekit::get_room_details))
        .route("/livekit/participant", delete(livekit::remove_participant))
        .route("/livekit/participant/mute", post(livekit::mute_participant))
        .route("/recording/{stream_id}", get(recording::download_recording))
        // SIP hooks management
        .route(
            "/sip/hooks",
            get(sip::list_sip_hooks)
                .post(sip::update_sip_hooks)
                .delete(sip::delete_sip_hooks),
        )
        // SIP call transfer
        .route("/sip/transfer", post(sip::sip_transfer))
        // Capability discovery: the unified-language support matrix (P2).
        .route(
            "/capabilities/languages",
            get(capabilities::list_language_capabilities),
        )
        // DAG routing endpoints
        .route("/dag/templates", get(dag::list_templates))
        .route("/dag/templates/{template_name}", get(dag::get_template))
        .route("/dag/validate", post(dag::validate_dag))
        // P5 batched / async STT: submit a prerecorded job + poll it by id.
        .route(
            "/transcribe/batch",
            post(transcribe::submit_batch)
                .layer(DefaultBodyLimit::max(BATCH_JSON_BODY_LIMIT_BYTES)),
        )
        .route("/transcribe/batch/{job_id}", get(transcribe::get_batch))
        .layer(TraceLayer::new_for_http())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_fifty_mebibytes() {
        // Named rather than derived: the number is a promise to clients, and a test that
        // recomputed it from the constant would pass whatever the constant said.
        assert_eq!(DEFAULT_MAX_AUDIO_UPLOAD_BYTES, 52_428_800);
    }

    #[test]
    fn an_absent_or_blank_override_means_not_configured() {
        assert_eq!(
            parse_max_audio_upload_bytes(None),
            DEFAULT_MAX_AUDIO_UPLOAD_BYTES
        );
        assert_eq!(
            parse_max_audio_upload_bytes(Some("")),
            DEFAULT_MAX_AUDIO_UPLOAD_BYTES
        );
        assert_eq!(
            parse_max_audio_upload_bytes(Some("   ")),
            DEFAULT_MAX_AUDIO_UPLOAD_BYTES
        );
    }

    #[test]
    fn a_byte_count_is_honoured_in_both_directions() {
        // Raising it is the point; lowering it must work too, or an operator cannot tighten
        // the ceiling for an environment that fronts the gateway with a smaller proxy limit.
        assert_eq!(parse_max_audio_upload_bytes(Some("104857600")), 104_857_600);
        assert_eq!(parse_max_audio_upload_bytes(Some("1048576")), 1_048_576);
        assert_eq!(parse_max_audio_upload_bytes(Some(" 2097152 ")), 2_097_152);
    }

    #[test]
    fn an_unusable_value_falls_back_rather_than_refusing_every_upload() {
        // "50MB", "50 MiB" and a negative number are the three things an operator actually
        // types. None of them parses as a byte count, and treating any of them as zero would
        // turn a typo in a Helm value into a total outage of the audio plane.
        for bad in ["50MB", "50 MiB", "-1", "abc", "0"] {
            assert_eq!(
                parse_max_audio_upload_bytes(Some(bad)),
                DEFAULT_MAX_AUDIO_UPLOAD_BYTES,
                "{bad:?} should fall back to the default"
            );
        }
    }
}
