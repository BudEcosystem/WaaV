//! Span attribute names for a voice turn (FRD-018 M6).
//!
//! WaaV EMITS these; budmetrics' `VoiceTurnFact` READS them. Neither repo can see the other at
//! build time, so both assert against the same checked-in contract
//! (`tests/voice_span_contract.json`, generated from budmetrics' column registry).
//!
//! That contract is not ceremony. A name changed on one side only does not fail anything — the
//! column simply stays NULL, which is indistinguishable from "a feature nobody uses". It is the
//! same failure shape as the `voice_table` wire, and it is caught the same way.
//!
//! **Namespacing is load-bearing.** Every name is under `bud.` or `gen_ai.`, because the
//! collector receives spans from every service in the mesh: an un-namespaced `duration_ms` from
//! WaaV would collide with somebody else's and the materialized view would read whichever
//! arrived.

/// Attributes carried by the SERVER span covering one whole turn.
pub mod turn {
    pub const PROJECT_ID: &str = "bud.project_id";
    pub const ENDPOINT_ID: &str = "bud.endpoint_id";
    pub const MODEL_ID: &str = "bud.model_id";
    pub const API_KEY_ID: &str = "bud.api_key_id";
    pub const USER_ID: &str = "bud.user_id";

    /// `text_to_speech` | `audio_transcription` | `audio_translation` | `realtime_session` …
    ///
    /// NOT derived from the URL path: the same capability is served over HTTP and over a
    /// socket, and a path-derived value would disagree between them for one operation.
    pub const CAPABILITY: &str = "bud.voice.capability";
    /// `http` | `websocket`. Latency means different things across the two, and without this
    /// you cannot tell which reading you are looking at.
    pub const TRANSPORT: &str = "bud.voice.transport";
    pub const SESSION_ID: &str = "bud.voice.session_id";
    pub const TURN_INDEX: &str = "bud.voice.turn_index";

    /// Billing dimensions, in the vendors' own units. TTS bills per character, STT per second
    /// of audio; a token count here would be filled with zeroes.
    pub const CHARACTERS: &str = "bud.voice.characters";
    pub const AUDIO_SECONDS: &str = "bud.voice.audio_seconds";
    pub const COST: &str = "bud.voice.cost";

    /// End of user speech to first audio out — the headline measure. Everything else is a
    /// component of it.
    pub const RESPONSE_LATENCY_MS: &str = "bud.voice.response_latency_ms";
    pub const BARGE_IN: &str = "bud.voice.barge_in";
    pub const TURN_DETECTOR: &str = "bud.voice.turn_detector";
    pub const LANGUAGE: &str = "bud.voice.language";

    /// Content. Carries a shorter retention than the row that holds it.
    pub const TRANSCRIPT: &str = "bud.voice.transcript";
    pub const SYNTHESIS_INPUT: &str = "bud.voice.synthesis_input";

    // ---- FRD-021 §6.1, Phase 1 -------------------------------------------------------------

    /// The `model` string the caller sent — the alias. `ENDPOINT_ID` is the endpoint UUID it
    /// resolved to; before FRD-021 that column held this name instead.
    pub const ENDPOINT_NAME: &str = "bud.voice.endpoint_name";
    /// The project of the API key that made the call. `PROJECT_ID` is the ENDPOINT's project
    /// (from the alias metadata) and falls back to this one when there is no alias entry.
    pub const API_KEY_PROJECT_ID: &str = "bud.api_key_project_id";
    /// What `COST` was computed from: `character` | `second` | `minute` | `request`. Recorded
    /// only together with `COST`.
    pub const PRICING_UNIT: &str = "bud.voice.pricing_unit";
    /// The closed error-class vocabulary of FRD-021 §6.5, on every failed call.
    pub const ERROR_TYPE: &str = "bud.voice.error_type";
    /// The vendor's HTTP status, when a vendor response caused the failure.
    pub const VENDOR_STATUS_CODE: &str = "bud.voice.vendor_status_code";

    // ---- FRD-021 §6.1, Phase 5 -------------------------------------------------------------

    /// Seconds of audio a synthesis produced. Exact for PCM and WAV; for MP3, AAC (ADTS), FLAC,
    /// Ogg and MP4, read from the container without decoding (packet durations, the last Ogg
    /// granule position); absent when the container cannot be read (DEG-4).
    pub const OUTPUT_AUDIO_SECONDS: &str = "bud.voice.output_audio_seconds";
    /// The language the vendor says it heard, where it reports one — normalized to the lowercase
    /// BCP-47 primary language subtag, ISO 639-1 where one exists (`en` for `eng`, `english`,
    /// `en-US`; see `observability::language`).
    pub const DETECTED_LANGUAGE: &str = "bud.voice.detected_language";
    /// TTS: the format served. STT: the upload's container.
    pub const AUDIO_FORMAT: &str = "bud.voice.audio_format";
    /// Samples per second, where known without decoding (PCM, WAV headers, a compressed
    /// container's own headers).
    pub const SAMPLE_RATE: &str = "bud.voice.sample_rate";
    /// STT: the size of the uploaded file.
    pub const INPUT_AUDIO_BYTES: &str = "bud.voice.input_audio_bytes";
    /// The vendor's own id for the request, from its response.
    pub const VENDOR_REQUEST_ID: &str = "bud.voice.vendor_request_id";
}

/// What the deployment's Rate limiting and Resilience settings did to this turn (FRD-022 §6.4).
///
/// Usage is billed to the deployment that SERVED the turn, which is not the one the caller named
/// when the fallback chain served it — hence a column of its own rather than `bud.endpoint_id`.
pub mod resilience {
    /// The deployment that actually served the turn.
    pub const SERVED_ENDPOINT_ID: &str = "bud.voice.served_endpoint_id";
    /// The primary deployment, when a fallback served the turn.
    pub const FALLBACK_FROM: &str = "bud.voice.fallback_from";
    /// Vendor retries performed for the turn, across every hop.
    pub const RETRY_COUNT: &str = "bud.voice.retry_count";
    /// The deployment's rate-limit decision for the admitted request: `allow` | `unlimited`.
    pub const RATE_LIMIT_OUTCOME: &str = "bud.rate_limit.outcome";
}

/// A realtime (speech-to-speech) billed record (FRD-023 §5.10, CONTRACTS C2).
///
/// Realtime is the one place on the audio plane billed in TOKENS — at up to eight per-modality
/// rates — so these are the only token attributes in the vocabulary.
pub mod realtime {
    /// `response` | `input_transcription` | `duration_segment`.
    pub const COMPONENT: &str = "bud.voice.rt.component";
    /// The vendor serving the session (`voice_table.vendor`).
    pub const VENDOR: &str = "bud.voice.rt.vendor";
    /// The vendor's model (`voice_table.model`; the Azure deployment name for Azure).
    pub const MODEL: &str = "bud.voice.rt.model";
    pub const RESPONSE_ID: &str = "bud.voice.rt.response_id";
    /// `completed` | `cancelled` | `incomplete` | `failed`.
    pub const RESPONSE_STATUS: &str = "bud.voice.rt.response_status";
    pub const INPUT_TEXT_TOKENS: &str = "bud.voice.rt.input_text_tokens";
    pub const INPUT_AUDIO_TOKENS: &str = "bud.voice.rt.input_audio_tokens";
    pub const INPUT_IMAGE_TOKENS: &str = "bud.voice.rt.input_image_tokens";
    /// Cached tokens are a SUBSET of their input class, not additional to it.
    pub const CACHED_TEXT_TOKENS: &str = "bud.voice.rt.cached_text_tokens";
    pub const CACHED_AUDIO_TOKENS: &str = "bud.voice.rt.cached_audio_tokens";
    pub const CACHED_IMAGE_TOKENS: &str = "bud.voice.rt.cached_image_tokens";
    pub const OUTPUT_TEXT_TOKENS: &str = "bud.voice.rt.output_text_tokens";
    pub const OUTPUT_AUDIO_TOKENS: &str = "bud.voice.rt.output_audio_tokens";
    /// Seconds billed: an input transcription's audio, or a duration segment.
    pub const BILLED_SECONDS: &str = "bud.voice.billed_seconds";
    /// Components present with no rate — named, never priced at zero.
    pub const UNPRICED_COMPONENTS: &str = "bud.voice.unpriced_components";
    /// The vendor's own session id (`session.created.session.id`).
    pub const VENDOR_SESSION_ID: &str = "bud.voice.vendor_session_id";
    /// `realtime` | `transcription` (the session span).
    pub const SESSION_TYPE: &str = "bud.voice.rt.session_type";
}

/// Attributes of the `voice.session` span, one per realtime session (FRD-023 §5.10).
pub mod session {
    pub const DURATION_MS: &str = "bud.voice.session.duration_ms";
    pub const TURNS: &str = "bud.voice.session.turns";
    /// `client_close` | `idle` | `max_duration` | `revoked` | `drain` | `vendor_close` |
    /// `upstream_error` | `rate_limited` | `client_too_slow` | `client_timeout`.
    ///
    /// `vendor_close` is the vendor closing its socket normally (1000/1001) — the session ended, it
    /// did not fail; `upstream_error` is an abnormal vendor close, a lost connection or a vendor
    /// that stopped answering, and marks the session span ERROR.
    pub const END_REASON: &str = "bud.voice.session.end_reason";
    pub const CLOSE_CODE: &str = "bud.voice.session.close_code";
}

/// Attributes carried by a CLIENT span covering one leg of a turn.
///
/// Per-leg rather than a single `provider`/`duration` pair, because a turn routinely spans two
/// vendors and one field would have to pick a winner and drop the other.
pub mod leg {
    pub const STT_VENDOR: &str = "bud.voice.stt.vendor";
    pub const STT_DURATION_MS: &str = "bud.voice.stt.duration_ms";
    pub const STT_TTFB_MS: &str = "bud.voice.stt.ttfb_ms";

    pub const TTS_VENDOR: &str = "bud.voice.tts.vendor";
    pub const TTS_DURATION_MS: &str = "bud.voice.tts.duration_ms";
    pub const TTS_TTFB_MS: &str = "bud.voice.tts.ttfb_ms";

    /// The LLM leg reuses the OTel semantic convention rather than inventing a `bud.` name, so
    /// a voice turn's model attribution matches every other model call in the mesh.
    pub const LLM_MODEL: &str = "gen_ai.request.model";
    pub const LLM_DURATION_MS: &str = "bud.voice.llm.duration_ms";

    /// The VENDOR's model for the leg (`voice_table.model`), as distinct from the Bud model the
    /// endpoint belongs to (`turn::MODEL_ID`).
    pub const STT_MODEL: &str = "bud.voice.stt.model";
    pub const TTS_MODEL: &str = "bud.voice.tts.model";
    /// A result-level confidence the vendor itself reported. Never a default: a vendor with no
    /// confidence leaves it absent rather than contributing 1.0 (DEG-5).
    pub const STT_CONFIDENCE: &str = "bud.voice.stt.confidence";
    /// The voice a synthesis ran with.
    pub const TTS_VOICE: &str = "bud.voice.tts.voice";
    /// Whether the transcription ran through WaaV's own denoiser (FRD-018 Part III N1).
    pub const STT_NOISE_SUPPRESSION: &str = "bud.voice.stt.noise_suppression";
}

/// Every attribute this crate emits, for the contract test.
pub const ALL: &[&str] = &[
    turn::PROJECT_ID,
    turn::ENDPOINT_ID,
    turn::MODEL_ID,
    turn::API_KEY_ID,
    turn::USER_ID,
    turn::CAPABILITY,
    turn::TRANSPORT,
    turn::SESSION_ID,
    turn::TURN_INDEX,
    turn::CHARACTERS,
    turn::AUDIO_SECONDS,
    turn::COST,
    turn::RESPONSE_LATENCY_MS,
    turn::BARGE_IN,
    turn::TURN_DETECTOR,
    turn::LANGUAGE,
    turn::TRANSCRIPT,
    turn::SYNTHESIS_INPUT,
    turn::ENDPOINT_NAME,
    turn::API_KEY_PROJECT_ID,
    turn::PRICING_UNIT,
    turn::ERROR_TYPE,
    turn::VENDOR_STATUS_CODE,
    turn::OUTPUT_AUDIO_SECONDS,
    turn::DETECTED_LANGUAGE,
    turn::AUDIO_FORMAT,
    turn::SAMPLE_RATE,
    turn::INPUT_AUDIO_BYTES,
    turn::VENDOR_REQUEST_ID,
    leg::STT_VENDOR,
    leg::STT_DURATION_MS,
    leg::STT_TTFB_MS,
    leg::TTS_VENDOR,
    leg::TTS_DURATION_MS,
    leg::TTS_TTFB_MS,
    leg::LLM_MODEL,
    leg::LLM_DURATION_MS,
    leg::STT_MODEL,
    leg::TTS_MODEL,
    leg::STT_CONFIDENCE,
    leg::TTS_VOICE,
    leg::STT_NOISE_SUPPRESSION,
    resilience::SERVED_ENDPOINT_ID,
    resilience::FALLBACK_FROM,
    resilience::RETRY_COUNT,
    resilience::RATE_LIMIT_OUTCOME,
    realtime::COMPONENT,
    realtime::VENDOR,
    realtime::MODEL,
    realtime::RESPONSE_ID,
    realtime::RESPONSE_STATUS,
    realtime::INPUT_TEXT_TOKENS,
    realtime::INPUT_AUDIO_TOKENS,
    realtime::INPUT_IMAGE_TOKENS,
    realtime::CACHED_TEXT_TOKENS,
    realtime::CACHED_AUDIO_TOKENS,
    realtime::CACHED_IMAGE_TOKENS,
    realtime::OUTPUT_TEXT_TOKENS,
    realtime::OUTPUT_AUDIO_TOKENS,
    realtime::BILLED_SECONDS,
    realtime::UNPRICED_COMPONENTS,
    realtime::VENDOR_SESSION_ID,
    realtime::SESSION_TYPE,
    session::DURATION_MS,
    session::TURNS,
    session::END_REASON,
    session::CLOSE_CODE,
];

/// Attributes only the `voice.session` span carries; every other attribute in [`ALL`] is a
/// `voice.turn` attribute and declared by [`voice_turn_span!`].
pub const SESSION_ONLY: &[&str] = &[
    realtime::SESSION_TYPE,
    session::DURATION_MS,
    session::TURNS,
    session::END_REASON,
    session::CLOSE_CODE,
];

/// The attributes a `voice.session` span declares (FRD-023): attribution, the realtime shape, the
/// session's own fields and the totals. Kept beside [`ALL`] so the session macro and its test
/// read one list.
pub const SESSION: &[&str] = &[
    turn::PROJECT_ID,
    turn::ENDPOINT_ID,
    turn::MODEL_ID,
    turn::API_KEY_ID,
    turn::USER_ID,
    turn::API_KEY_PROJECT_ID,
    turn::ENDPOINT_NAME,
    turn::CAPABILITY,
    turn::TRANSPORT,
    turn::SESSION_ID,
    turn::COST,
    turn::PRICING_UNIT,
    realtime::VENDOR,
    realtime::MODEL,
    realtime::SESSION_TYPE,
    realtime::VENDOR_SESSION_ID,
    realtime::INPUT_TEXT_TOKENS,
    realtime::INPUT_AUDIO_TOKENS,
    realtime::INPUT_IMAGE_TOKENS,
    realtime::CACHED_TEXT_TOKENS,
    realtime::CACHED_AUDIO_TOKENS,
    realtime::CACHED_IMAGE_TOKENS,
    realtime::OUTPUT_TEXT_TOKENS,
    realtime::OUTPUT_AUDIO_TOKENS,
    realtime::BILLED_SECONDS,
    session::DURATION_MS,
    session::TURNS,
    session::END_REASON,
    session::CLOSE_CODE,
];

/// Open a `voice.turn` span that declares EVERY attribute in [`ALL`] up front.
///
/// This exists because of a failure mode that produces no error of any kind: `Span::record` on a
/// field the span did not declare at creation is a silent no-op. A leg that measures its vendor
/// and duration correctly, and records them into a span that never declared those fields, emits
/// a trace that looks complete and arrives with the columns empty — and an empty column is
/// indistinguishable from a feature nobody used.
///
/// Declaring the whole vocabulary in ONE place means a new leg cannot be wired to a span that
/// silently ignores it. Fields nobody records stay `Empty` and are simply absent from the span,
/// which costs nothing.
///
/// The caller supplies only what is known at turn start; everything else is recorded later.
///
/// `otel.status_code` / `otel.status_message` are declared too (FRD-021 FR-6): a failed turn
/// records them, and a span built here that did not declare them would end `Unset` — a failure
/// counted as a success. Anything after `transport` is passed to `info_span!` unchanged, so an
/// extra field can be written in any form tracing accepts, `{ CONST } = value` included.
#[macro_export]
macro_rules! voice_turn_span {
    // FRD-023 D-19: a realtime billed record is the ROOT of its own trace.
    (parent: $parent:expr, capability = $capability:expr, transport = $transport:expr $(, $($extra:tt)*)?) => {
        $crate::__voice_turn_span_fields!((parent: $parent,) $capability, $transport $(, $($extra)*)?)
    };
    (capability = $capability:expr, transport = $transport:expr $(, $($extra:tt)*)?) => {
        $crate::__voice_turn_span_fields!(() $capability, $transport $(, $($extra)*)?)
    };
}

/// The field list behind [`voice_turn_span!`], in one place for both of its forms.
#[doc(hidden)]
#[macro_export]
macro_rules! __voice_turn_span_fields {
    (($($parent:tt)*) $capability:expr, $transport:expr $(, $($extra:tt)*)?) => {
        ::tracing::info_span!(
            $($parent)*
            "voice.turn",
                { $crate::observability::voice_attrs::turn::CAPABILITY } = $capability,
                { $crate::observability::voice_attrs::turn::TRANSPORT } = $transport,
                { $crate::observability::voice_attrs::turn::PROJECT_ID } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::turn::ENDPOINT_ID } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::turn::MODEL_ID } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::turn::API_KEY_ID } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::turn::USER_ID } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::turn::SESSION_ID } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::turn::TURN_INDEX } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::turn::CHARACTERS } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::turn::AUDIO_SECONDS } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::turn::COST } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::turn::RESPONSE_LATENCY_MS } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::turn::BARGE_IN } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::turn::TURN_DETECTOR } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::turn::LANGUAGE } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::turn::TRANSCRIPT } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::turn::SYNTHESIS_INPUT } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::leg::STT_VENDOR } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::leg::STT_DURATION_MS } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::leg::STT_TTFB_MS } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::leg::TTS_VENDOR } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::leg::TTS_DURATION_MS } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::leg::TTS_TTFB_MS } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::leg::LLM_MODEL } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::leg::LLM_DURATION_MS } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::turn::ENDPOINT_NAME } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::turn::API_KEY_PROJECT_ID } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::turn::PRICING_UNIT } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::turn::ERROR_TYPE } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::turn::VENDOR_STATUS_CODE } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::turn::OUTPUT_AUDIO_SECONDS } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::turn::DETECTED_LANGUAGE } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::turn::AUDIO_FORMAT } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::turn::SAMPLE_RATE } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::turn::INPUT_AUDIO_BYTES } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::turn::VENDOR_REQUEST_ID } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::leg::STT_MODEL } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::leg::TTS_MODEL } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::leg::STT_CONFIDENCE } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::leg::TTS_VOICE } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::leg::STT_NOISE_SUPPRESSION } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::resilience::SERVED_ENDPOINT_ID } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::resilience::FALLBACK_FROM } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::resilience::RETRY_COUNT } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::resilience::RATE_LIMIT_OUTCOME } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::realtime::COMPONENT } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::realtime::VENDOR } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::realtime::MODEL } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::realtime::RESPONSE_ID } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::realtime::RESPONSE_STATUS } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::realtime::INPUT_TEXT_TOKENS } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::realtime::INPUT_AUDIO_TOKENS } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::realtime::INPUT_IMAGE_TOKENS } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::realtime::CACHED_TEXT_TOKENS } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::realtime::CACHED_AUDIO_TOKENS } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::realtime::CACHED_IMAGE_TOKENS } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::realtime::OUTPUT_TEXT_TOKENS } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::realtime::OUTPUT_AUDIO_TOKENS } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::realtime::BILLED_SECONDS } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::realtime::UNPRICED_COMPONENTS } = ::tracing::field::Empty,
                { $crate::observability::voice_attrs::realtime::VENDOR_SESSION_ID } = ::tracing::field::Empty,
                otel.status_code = ::tracing::field::Empty,
                otel.status_message = ::tracing::field::Empty
                $(, $($extra)*)?
        )
    };
}

/// Open a `voice.session` span (FRD-023 §5.10): the ROOT of its own trace, declaring every
/// attribute in [`SESSION`] up front (the declare-before-record rule of [`voice_turn_span!`]).
#[macro_export]
macro_rules! voice_session_span {
    (capability = $capability:expr, transport = $transport:expr) => {
        ::tracing::info_span!(
            parent: None,
            "voice.session",
            { $crate::observability::voice_attrs::turn::CAPABILITY } = $capability,
            { $crate::observability::voice_attrs::turn::TRANSPORT } = $transport,
            { $crate::observability::voice_attrs::turn::PROJECT_ID } = ::tracing::field::Empty,
            { $crate::observability::voice_attrs::turn::ENDPOINT_ID } = ::tracing::field::Empty,
            { $crate::observability::voice_attrs::turn::MODEL_ID } = ::tracing::field::Empty,
            { $crate::observability::voice_attrs::turn::API_KEY_ID } = ::tracing::field::Empty,
            { $crate::observability::voice_attrs::turn::USER_ID } = ::tracing::field::Empty,
            { $crate::observability::voice_attrs::turn::API_KEY_PROJECT_ID } = ::tracing::field::Empty,
            { $crate::observability::voice_attrs::turn::ENDPOINT_NAME } = ::tracing::field::Empty,
            { $crate::observability::voice_attrs::turn::SESSION_ID } = ::tracing::field::Empty,
            { $crate::observability::voice_attrs::turn::COST } = ::tracing::field::Empty,
            { $crate::observability::voice_attrs::turn::PRICING_UNIT } = ::tracing::field::Empty,
            { $crate::observability::voice_attrs::realtime::VENDOR } = ::tracing::field::Empty,
            { $crate::observability::voice_attrs::realtime::MODEL } = ::tracing::field::Empty,
            { $crate::observability::voice_attrs::realtime::SESSION_TYPE } = ::tracing::field::Empty,
            { $crate::observability::voice_attrs::realtime::VENDOR_SESSION_ID } = ::tracing::field::Empty,
            { $crate::observability::voice_attrs::realtime::INPUT_TEXT_TOKENS } = ::tracing::field::Empty,
            { $crate::observability::voice_attrs::realtime::INPUT_AUDIO_TOKENS } = ::tracing::field::Empty,
            { $crate::observability::voice_attrs::realtime::INPUT_IMAGE_TOKENS } = ::tracing::field::Empty,
            { $crate::observability::voice_attrs::realtime::CACHED_TEXT_TOKENS } = ::tracing::field::Empty,
            { $crate::observability::voice_attrs::realtime::CACHED_AUDIO_TOKENS } = ::tracing::field::Empty,
            { $crate::observability::voice_attrs::realtime::CACHED_IMAGE_TOKENS } = ::tracing::field::Empty,
            { $crate::observability::voice_attrs::realtime::OUTPUT_TEXT_TOKENS } = ::tracing::field::Empty,
            { $crate::observability::voice_attrs::realtime::OUTPUT_AUDIO_TOKENS } = ::tracing::field::Empty,
            { $crate::observability::voice_attrs::realtime::BILLED_SECONDS } = ::tracing::field::Empty,
            { $crate::observability::voice_attrs::session::DURATION_MS } = ::tracing::field::Empty,
            { $crate::observability::voice_attrs::session::TURNS } = ::tracing::field::Empty,
            { $crate::observability::voice_attrs::session::END_REASON } = ::tracing::field::Empty,
            { $crate::observability::voice_attrs::session::CLOSE_CODE } = ::tracing::field::Empty,
            otel.status_code = ::tracing::field::Empty,
            otel.status_message = ::tracing::field::Empty
        )
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_attribute_is_namespaced() {
        // The collector receives spans from every service in the mesh. An un-namespaced name
        // collides with another emitter's and the MV reads whichever arrived last.
        for a in ALL {
            assert!(
                a.starts_with("bud.") || a.starts_with("gen_ai."),
                "{a} is not namespaced"
            );
        }
    }

    #[test]
    fn no_attribute_is_listed_twice() {
        let mut seen = ALL.to_vec();
        seen.sort_unstable();
        let before = seen.len();
        seen.dedup();
        assert_eq!(before, seen.len(), "duplicate attribute in ALL");
    }

    /// Collects the field names a span actually ends up carrying.
    ///
    /// Scoped with `with_default` rather than a global subscriber on purpose: a global one is
    /// process-wide, so under a parallel test runner these assertions would see spans from
    /// whatever else happened to be running and fail intermittently.
    mod capture {
        use std::collections::HashSet;
        use std::sync::{Arc, Mutex};

        use tracing::field::{Field, Visit};
        use tracing::span::{Attributes, Id, Record};
        use tracing::{Event, Metadata, subscriber::Interest};

        #[derive(Default)]
        pub struct Names(pub Arc<Mutex<HashSet<String>>>);

        impl Visit for Names {
            fn record_debug(&mut self, field: &Field, _v: &dyn std::fmt::Debug) {
                self.0.lock().unwrap().insert(field.name().to_string());
            }
        }

        pub struct Sub(pub Arc<Mutex<HashSet<String>>>);

        impl tracing::Subscriber for Sub {
            fn register_callsite(&self, _m: &'static Metadata<'static>) -> Interest {
                Interest::always()
            }
            fn enabled(&self, _m: &Metadata<'_>) -> bool {
                true
            }
            fn new_span(&self, attrs: &Attributes<'_>) -> Id {
                let mut v = Names(self.0.clone());
                attrs.record(&mut v);
                Id::from_u64(1)
            }
            fn record(&self, _id: &Id, values: &Record<'_>) {
                let mut v = Names(self.0.clone());
                values.record(&mut v);
            }
            fn record_follows_from(&self, _s: &Id, _f: &Id) {}
            fn event(&self, _e: &Event<'_>) {}
            fn enter(&self, _id: &Id) {}
            fn exit(&self, _id: &Id) {}
        }
    }

    #[test]
    fn a_field_the_span_never_declared_is_silently_dropped() {
        // The mechanism the macro exists to defeat, pinned rather than described. If tracing
        // ever started surfacing this — panicking, warning, anything — the macro's whole
        // rationale would be obsolete and this test would say so.
        let seen = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));
        tracing::subscriber::with_default(capture::Sub(seen.clone()), || {
            let span = tracing::info_span!("voice.turn", { turn::CAPABILITY } = "text_to_speech");
            span.record(leg::LLM_MODEL, "gpt-4o-mini");
            span.record(leg::LLM_DURATION_MS, 42u64);
        });
        let seen = seen.lock().unwrap();
        assert!(
            seen.contains(turn::CAPABILITY),
            "the declared field should be present"
        );
        assert!(
            !seen.contains(leg::LLM_MODEL) && !seen.contains(leg::LLM_DURATION_MS),
            "recording an undeclared field must be a silent no-op — if this now works, \
             voice_turn_span!'s reason for existing is gone: {seen:?}"
        );
    }

    #[test]
    fn the_turn_span_macro_accepts_every_attribute_in_all() {
        // The guard proper. A leg added later records into a span built by this macro; if its
        // attribute is not declared here the value vanishes with no error, and the column just
        // stays NULL. Exercising the whole vocabulary is what keeps that from shipping.
        let seen = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));
        tracing::subscriber::with_default(capture::Sub(seen.clone()), || {
            let span =
                crate::voice_turn_span!(capability = "conversation", transport = "websocket");
            for name in ALL.iter().filter(|a| !SESSION_ONLY.contains(a)) {
                span.record(*name, "x");
            }
        });
        let seen = seen.lock().unwrap();
        let missing: Vec<_> = ALL
            .iter()
            .filter(|a| !SESSION_ONLY.contains(a) && !seen.contains(**a))
            .collect();
        assert!(
            missing.is_empty(),
            "voice_turn_span! does not declare {missing:?}; recording them is a silent no-op, \
             so those columns would arrive empty with nothing reporting a problem"
        );
    }

    #[test]
    fn the_turn_span_macro_declares_the_otel_status_and_passes_extras_through() {
        // FRD-021 GT-3: the HTTP spans were hand-rolled because the macro declared no
        // `otel.status_*` — so a turn built with it could not be marked failed — and took extras
        // only as plain idents, which a `bud.voice.*` constant is not.
        const EXTRA: &str = "bud.voice.macro_extra_probe";
        let seen = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));
        tracing::subscriber::with_default(capture::Sub(seen.clone()), || {
            let span = crate::voice_turn_span!(
                capability = "text_to_speech",
                transport = "http",
                { EXTRA } = tracing::field::Empty
            );
            span.record("otel.status_code", "ERROR");
            span.record("otel.status_message", "the vendor refused");
            span.record(EXTRA, "x");
        });
        let seen = seen.lock().unwrap();
        for name in ["otel.status_code", "otel.status_message", EXTRA] {
            assert!(
                seen.contains(name),
                "voice_turn_span! does not declare {name}"
            );
        }
    }

    #[test]
    fn the_frd_021_names_are_the_contract_names() {
        // Written out rather than derived: these are the names budmetrics' registry reads
        // (CONTRACTS §1.1), and a constant renamed here alone would move the column to NULL.
        for (constant, name) in [
            (turn::ENDPOINT_NAME, "bud.voice.endpoint_name"),
            (turn::API_KEY_PROJECT_ID, "bud.api_key_project_id"),
            (turn::PRICING_UNIT, "bud.voice.pricing_unit"),
            (turn::ERROR_TYPE, "bud.voice.error_type"),
            (turn::VENDOR_STATUS_CODE, "bud.voice.vendor_status_code"),
            (turn::OUTPUT_AUDIO_SECONDS, "bud.voice.output_audio_seconds"),
            (turn::DETECTED_LANGUAGE, "bud.voice.detected_language"),
            (turn::AUDIO_FORMAT, "bud.voice.audio_format"),
            (turn::SAMPLE_RATE, "bud.voice.sample_rate"),
            (turn::INPUT_AUDIO_BYTES, "bud.voice.input_audio_bytes"),
            (turn::VENDOR_REQUEST_ID, "bud.voice.vendor_request_id"),
            (leg::STT_MODEL, "bud.voice.stt.model"),
            (leg::TTS_MODEL, "bud.voice.tts.model"),
            (leg::STT_CONFIDENCE, "bud.voice.stt.confidence"),
            (leg::TTS_VOICE, "bud.voice.tts.voice"),
            (
                leg::STT_NOISE_SUPPRESSION,
                "bud.voice.stt.noise_suppression",
            ),
        ] {
            assert_eq!(constant, name);
            assert!(ALL.contains(&constant), "{name} is not in ALL");
        }
    }

    #[test]
    fn the_billing_dimensions_are_the_vendors_own_units() {
        // A token count on an HTTP voice turn would be all zeroes: TTS bills per character, STT
        // per second of audio. Realtime (FRD-023) is the one capability billed in tokens, so its
        // `bud.voice.rt.*_tokens` are the only token attributes.
        assert!(ALL.contains(&turn::CHARACTERS));
        assert!(ALL.contains(&turn::AUDIO_SECONDS));
        let tokens: Vec<_> = ALL.iter().filter(|a| a.contains("token")).collect();
        assert_eq!(tokens.len(), 8, "{tokens:?}");
        assert!(
            tokens.iter().all(|a| a.starts_with("bud.voice.rt.")),
            "token counts belong to realtime only: {tokens:?}"
        );
    }

    /// TC-MET-10 🔒: every realtime attribute is declared by BOTH span macros that carry it, so a
    /// record on it is never a silent no-op.
    #[test]
    fn tc_met_10_the_realtime_turn_root_declares_every_attribute_in_all() {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));
        tracing::subscriber::with_default(capture::Sub(seen.clone()), || {
            let span = crate::voice_turn_span!(
                parent: None,
                capability = "realtime_session",
                transport = "websocket"
            );
            for name in ALL.iter().filter(|a| !SESSION_ONLY.contains(a)) {
                span.record(*name, "x");
            }
        });
        let seen = seen.lock().unwrap();
        let missing: Vec<_> = ALL
            .iter()
            .filter(|a| !SESSION_ONLY.contains(a) && !seen.contains(**a))
            .collect();
        assert!(
            missing.is_empty(),
            "the root form does not declare {missing:?}"
        );
    }

    #[test]
    fn tc_met_10_the_session_span_declares_every_session_attribute() {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));
        tracing::subscriber::with_default(capture::Sub(seen.clone()), || {
            let span = crate::voice_session_span!(
                capability = "realtime_session",
                transport = "websocket"
            );
            for name in SESSION {
                span.record(*name, "x");
            }
        });
        let seen = seen.lock().unwrap();
        let missing: Vec<_> = SESSION.iter().filter(|a| !seen.contains(**a)).collect();
        assert!(
            missing.is_empty(),
            "voice_session_span! does not declare {missing:?}"
        );
        assert!(
            SESSION.iter().all(|a| ALL.contains(a)),
            "a session attribute is missing from ALL"
        );
    }
}
