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

use tracing::Instrument;

use crate::observability::voice_attrs;
use crate::state::AppState;

use super::advisories::Advisories;
use super::endpoint_settings as settings_map;

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

    let mut advisories = Advisories::new();

    // FRD-018 Part III C1. The deployment's voice is the default; the request still wins.
    //
    // Before this, an operator picked a voice at deploy time, budapp published it, WaaV parsed
    // it into `VoiceEndpoint.voice` -- and nothing read it. The synthesis voice came only from
    // the request body, so the configured voice was unreachable by construction.
    //
    // When neither names a voice, the deployment may still DESCRIBE one. Before this, the
    // descriptor was written by budadmin, validated by budapp, published, and parsed by
    // `bud-auth` into `VoiceSettings.voice_descriptor` -- and read by nothing on this path, so a
    // deployment configured only by description answered `400 voice is required`. Five controls
    // that published cleanly and could never run.
    //
    // When nothing names or describes a voice, a default is used — the answer budadmin already
    // shows for such a deployment ("Vendor default"). This route answered `400 voice is
    // required` instead, so the Use Model snippet, which cannot know a vendor's voice ids, had no
    // way to produce a request that worked. See `default_voice` for which default.
    // Where the voice came from decides who can change it, and so what a vendor's refusal of it
    // should say — see `rejection_error`.
    let (voice, voice_origin) =
        match settings_map::resolve_voice(settings.voice.as_deref(), endpoint.voice.as_deref()) {
            Some(v) if settings.voice.is_some() => (v, VoiceOrigin::Request),
            Some(v) => (v, VoiceOrigin::Deployment),
            None => match resolve_described_voice(&state, &endpoint, &mut advisories).await {
                Some(v) => (v, VoiceOrigin::Described),
                None => match default_voice(&state, &endpoint, &settings.endpoint, &mut advisories)
                    .await
                {
                    Some(v) => (v, VoiceOrigin::Default),
                    None => return no_voice_error(&settings.endpoint, &endpoint.vendor),
                },
            },
        };

    // FRD-018 M7 exit criterion 3: a bad voice name must say which voices exist.
    //
    // Only checked where WaaV holds the vendor's catalog in process. Every other vendor
    // publishes its voices from a live URL (`handlers::voices::list_voices` fetches them), and
    // calling one here would put vendor I/O on the synthesis path — the thing the auth plane
    // was built to avoid. Those keep forwarding verbatim, which is what `known_voices_for`
    // returning an empty slice means. Giving another vendor an actionable error means
    // hydrating its catalog into the snapshot the way credentials already are, which is a
    // design change and not a line to bolt on here.
    //
    // TC-CFG-03: the ENDPOINT DEFAULT passes through this same gate. A default that went around
    // it would turn a config typo into a vendor 401 several seconds later, naming neither Bud
    // nor the endpoint -- and the operator would be looking at a form that accepted the value.
    if let Err(e) = speech::validate_voice(&voice, known_voices_for(&endpoint.vendor)) {
        return translation_error(&e);
    }

    // Refuse a format the vendor cannot produce BEFORE spending a vendor call on it. The
    // alternative was raw samples served under the codec's content type, which nothing can play.
    if let Some(supported) = vendor_output_formats(&endpoint.vendor)
        && !supported.contains(&settings.format)
    {
        let names: Vec<&str> = supported.iter().map(|f| f.as_str()).collect();
        return openai_error(
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            format!(
                "{} cannot produce {} audio; `response_format` must be one of {}",
                endpoint.vendor,
                settings.format.as_str(),
                names.join(", ")
            ),
            Some("response_format"),
        );
    }

    // Accepted fields this route does not apply, said out loud. Each returned 200 audio that
    // silently ignored what the caller asked for.
    if settings
        .instructions
        .as_deref()
        .is_some_and(|i| !i.trim().is_empty())
    {
        advisories
            .warn("`instructions` is not applied on /v1/audio/speech and was ignored".to_string());
    }
    if settings.speaking_rate.is_some()
        && speed_is_ignored(
            &endpoint.vendor,
            endpoint.model.as_deref().unwrap_or_default(),
        )
    {
        advisories.warn(format!(
            "`speed` is not applied by {} model {} and was ignored",
            endpoint.vendor,
            endpoint.model.as_deref().unwrap_or_default()
        ));
    }

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

    // FRD-018 M6. The field names are the attribute names budmetrics' VoiceTurnFact reads —
    // `tracing_opentelemetry` maps span fields straight onto OTel attributes, so a typo here is
    // a permanently NULL column rather than an error. They come from `voice_attrs`, which the
    // cross-repo contract test pins against budmetrics' own registry.
    //
    // `duration_ms` is declared Empty and recorded after synthesis: a field not declared at
    // span creation cannot be recorded later, and silently does nothing if you try.
    let chars = settings.text.chars().count();
    // Endpoint default, then the tts block's override. There is no request-level language on
    // `/v1/audio/speech`, so precedence has only two levels here.
    let language = settings_map::resolve_language(
        None,
        endpoint.language.as_deref(),
        endpoint.config.tts().language.as_deref(),
    );
    let turn_span = tracing::info_span!(
        "voice.turn",
        { voice_attrs::turn::CAPABILITY } = "text_to_speech",
        { voice_attrs::turn::TRANSPORT } = "http",
        { voice_attrs::turn::ENDPOINT_ID } = %settings.endpoint,
        // Recorded on SUCCESS only. It is the billing dimension for synthesis, and set here at
        // creation it counted every refused request — a voice the account lacks, text the vendor
        // would not speak — as characters synthesised.
        { voice_attrs::turn::CHARACTERS } = tracing::field::Empty,
        // Now describes what the request was actually CONFIGURED with, rather than a field
        // nothing read: C1 makes the endpoint language reach the provider.
        { voice_attrs::turn::LANGUAGE } = language.as_deref().unwrap_or(""),
        { voice_attrs::leg::TTS_VENDOR } = %endpoint.vendor,
        { voice_attrs::turn::PROJECT_ID } = tracing::field::Empty,
        { voice_attrs::turn::USER_ID } = tracing::field::Empty,
        { voice_attrs::turn::API_KEY_ID } = tracing::field::Empty,
        { voice_attrs::leg::TTS_DURATION_MS } = tracing::field::Empty,
        otel.status_code = tracing::field::Empty,
        otel.status_message = tracing::field::Empty,
    );
    // NOT `turn_span.enter()`. A span guard held across an `.await` attaches the span to
    // whatever task the executor resumes next, so the attributes land on someone else's work
    // and this turn's span is missing them. `.instrument()` on the future is the async-correct
    // form; the guard form is the single most common way to produce confidently wrong traces.
    info!(
        endpoint = %settings.endpoint,
        vendor = %endpoint.vendor,
        format = settings.format.as_str(),
        chars,
        "openai audio/speech"
    );

    let started = std::time::Instant::now();

    // Attribution. The identity was always resolvable — `resolve_voice_endpoint` just never
    // asked for it — so project_id, user_id and api_key_id were NULL for every voice turn ever
    // recorded, and VoiceTurnFact could not attribute usage to a project at all.
    //
    // `recordable` filters `Some("")`: an empty string is not NULL, and recording one makes the
    // column look populated to the live check that asks whether a value ever arrived.
    if let Some(p) = state.resolve_principal(bearer.as_deref()).await {
        use waav_openai_audio::recordable;
        if let Some(v) = recordable(p.project_id.as_deref()) {
            turn_span.record(voice_attrs::turn::PROJECT_ID, v);
        }
        if let Some(v) = recordable(p.user_id.as_deref()) {
            turn_span.record(voice_attrs::turn::USER_ID, v);
        }
        if let Some(v) = recordable(p.api_key_id.as_deref()) {
            turn_span.record(voice_attrs::turn::API_KEY_ID, v);
        }
    }

    let mut tts_settings = endpoint.config.tts().into_owned();

    // Deployment settings a model refuses outright. `optimize_streaming_latency` on `eleven_v3`
    // is a 400 from ElevenLabs ("not supported with the 'eleven_v3' model") — saved once in the
    // deployment's settings, it failed EVERY request. Not sent, and said so.
    let model_id = endpoint.model.as_deref().unwrap_or_default();
    if tts_settings.optimize_streaming_latency.is_some()
        && rejects_streaming_latency(&endpoint.vendor, model_id)
    {
        tts_settings.optimize_streaming_latency = None;
        advisories.warn(format!(
            "optimize_streaming_latency is not supported by {} model {model_id}; it was not sent",
            endpoint.vendor
        ));
    }
    // Connections to a vendor are pooled and shared across deployments, so a per-deployment
    // connect timeout cannot be applied on this route. `request_timeout` is: it bounds the whole
    // synthesis below.
    if tts_settings.connection_timeout.is_some() {
        advisories.warn(
            "connection_timeout is not applied: connections to the vendor are pooled and shared"
                .to_string(),
        );
    }
    let deadline = tts_settings
        .request_timeout
        .map(std::time::Duration::from_secs);
    let voice_for_errors = voice.clone();

    let mut tts_config = crate::core::tts::TTSConfig {
        provider: endpoint.vendor.clone(),
        api_key,
        voice_id: Some(voice),
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

    // C2: emotion, pronunciations and the timeouts, all of which the FLAT config already
    // carries -- the handler simply never had anything to fill them from.
    settings_map::apply_tts_flat(
        &tts_settings,
        &mut tts_config,
        settings.format.accepts_sample_rate(),
        &mut advisories,
    );

    // C1/C3: the synthesis language, mapped into the vendor's own notation. `/v1/audio/speech`
    // has no language field of its own, so this is entirely the deployment's choice.
    let mapped_language = language.as_deref().and_then(|canonical| {
        settings_map::map_language_for(
            canonical,
            &endpoint.vendor,
            &tts_config.model,
            &mut advisories,
        )
    });

    // C3: the canonical feature vocabulary, which only reaches a provider through the STANDARD
    // config. Everything the chosen vendor does not map warns rather than failing.
    // `apply_tts_flat` just cleared a sample rate the requested format cannot take, and said so.
    // The canonical feature set carries `sample_rate` too, and ElevenLabs reads it from there —
    // so the rate reached the vendor anyway: the advisory claimed "cleared" over a 16 kHz WAV, a
    // 44.1 kHz WAV hit ElevenLabs' Pro-tier wall, and a vendor that rejects a rate beside a codec
    // would have got exactly the pair the clearing exists to prevent. One answer for both paths.
    if !settings.format.accepts_sample_rate() {
        tts_settings.sample_rate = None;
    }
    let std_config = settings_map::standard_tts(
        tts_config,
        &tts_settings,
        mapped_language.as_deref(),
        &mut advisories,
    );

    let synthesis =
        crate::handlers::speak::synthesize_once_standard(&state, std_config, &settings.text)
            .instrument(turn_span.clone());
    // The deployment's `request_timeout`, as a bound on the whole synthesis. It used to reach
    // only an HTTP client this path never builds (the shared per-vendor pool is used instead),
    // so `request_timeout: 1` let a 2.2 s request through.
    let outcome = match deadline {
        Some(limit) => match tokio::time::timeout(limit, synthesis).await {
            Ok(outcome) => outcome,
            Err(_) => {
                let message = format!(
                    "synthesis exceeded deployment '{}''s request_timeout of {}s",
                    settings.endpoint,
                    limit.as_secs()
                );
                warn!(endpoint = %settings.endpoint, "{message}");
                mark_turn_failed(&turn_span, &message);
                return openai_error(StatusCode::GATEWAY_TIMEOUT, "api_error", message, None);
            }
        },
        None => synthesis.await,
    };
    match outcome {
        Ok((audio, format, sample_rate)) => {
            turn_span.record(
                voice_attrs::leg::TTS_DURATION_MS,
                started.elapsed().as_millis() as u64,
            );
            turn_span.record(voice_attrs::turn::CHARACTERS, chars);
            let served = match serve_as_requested(settings.format, audio, sample_rate) {
                Ok(served) => served,
                Err(e) => {
                    warn!(endpoint = %settings.endpoint, error = %e, "could not package audio");
                    mark_turn_failed(&turn_span, &e);
                    return openai_error(StatusCode::BAD_GATEWAY, "api_error", e, None);
                }
            };
            if let Some(actual) = served.substituted {
                advisories.warn(format!(
                    "requested {} audio, but {} returned {actual}; served as {actual}",
                    settings.format.as_str(),
                    endpoint.vendor
                ));
            }
            let audio = served.bytes;
            let mut headers = HeaderMap::new();
            if let Ok(ct) = served.content_type.parse() {
                headers.insert(header::CONTENT_TYPE, ct);
            }
            if let Ok(v) = served.format_label.unwrap_or(format.as_str()).parse() {
                headers.insert("x-audio-format", v);
            }
            // Raw samples carry no container, so the rate has to travel out of band or the
            // caller cannot play what they were sent.
            if matches!(settings.format, AudioFormat::Pcm)
                && let Ok(v) = sample_rate.to_string().parse()
            {
                headers.insert("x-sample-rate", v);
            }
            // W1. This response is audio bytes, so a header is the ONLY channel there is; a
            // body-carried advisory would be unreachable on this route by construction.
            advisories.apply(&mut headers);
            (StatusCode::OK, headers, audio).into_response()
        }
        Err(crate::handlers::speak::SynthesisError::Rejected(message)) => {
            warn!(endpoint = %settings.endpoint, error = %message, "vendor rejected synthesis");
            mark_turn_failed(&turn_span, &message);
            rejection_error(
                message,
                &voice_for_errors,
                voice_origin,
                &endpoint.vendor,
                &settings.endpoint,
            )
        }
        Err(e) => {
            warn!(endpoint = %settings.endpoint, error = %e, "synthesis failed");
            mark_turn_failed(&turn_span, &e.to_string());
            // 502, not 500: the failure is upstream of WaaV, and the distinction is what tells
            // an operator whether to look at the vendor or at us.
            openai_error(StatusCode::BAD_GATEWAY, "api_error", e.to_string(), None)
        }
    }
}

/// Mark a voice turn as failed on its span.
///
/// Every turn span used to end `Unset` whether the vendor produced audio or refused the request,
/// so a failed turn was told apart from a successful one only by a missing duration. The OTel
/// status is set for 4xx outcomes too: `voice.turn` is the domain operation — "synthesise this"
/// — not the HTTP server span, and a turn that produced nothing failed, whoever's fault it was.
/// The HTTP span still carries the status code that says whose.
fn mark_turn_failed(span: &tracing::Span, message: &str) {
    span.record("otel.status_code", "ERROR");
    span.record("otel.status_message", message);
}

/// The OpenAI output formats a vendor can produce, when WaaV knows; `None` = unknown, allow.
///
/// Derived from the vendor client's own mapping rather than declared beside it: for ElevenLabs,
/// a format is producible when `output_format_for` sends that codec, and `wav`/`pcm` are
/// producible because the vendor returns PCM and this route supplies the RIFF header. A format
/// the mapping sends as PCM — `aac`, `flac` — is one ElevenLabs does not have.
fn vendor_output_formats(vendor: &str) -> Option<Vec<AudioFormat>> {
    const ALL: [AudioFormat; 6] = [
        AudioFormat::Mp3,
        AudioFormat::Opus,
        AudioFormat::Aac,
        AudioFormat::Flac,
        AudioFormat::Wav,
        AudioFormat::Pcm,
    ];
    match vendor {
        "elevenlabs" | "eleven_labs" => Some(
            ALL.into_iter()
                .filter(|f| {
                    let sent = crate::core::tts::elevenlabs::output_format_for(
                        Some(f.as_waav_format()),
                        None,
                    );
                    match f {
                        AudioFormat::Wav | AudioFormat::Pcm => sent.starts_with("pcm_"),
                        other => sent.starts_with(&format!("{}_", other.as_str())),
                    }
                })
                .collect(),
        ),
        _ => None,
    }
}

/// Whether a vendor model refuses `optimize_streaming_latency` outright.
///
/// Measured: ElevenLabs answers `eleven_v3` with 400 "Providing optimize_streaming_latency is not
/// supported with the 'eleven_v3' model" (2026-09-24).
fn rejects_streaming_latency(vendor: &str, model: &str) -> bool {
    matches!(vendor, "elevenlabs" | "eleven_labs") && model == "eleven_v3"
}

/// Whether a vendor model accepts `speed` and does nothing with it.
///
/// Measured, not documented: on `eleven_v3`, speeds 0.7, 1.0 and 1.2 produced 3.20/2.88 s,
/// 2.80 s and 2.88/2.96 s of audio for the same sentence and voice (2026-09-24) — no effect —
/// while out-of-range values were accepted with a 200. WaaV does send `voice_settings.speed`.
fn speed_is_ignored(vendor: &str, model: &str) -> bool {
    matches!(vendor, "elevenlabs" | "eleven_labs") && model == "eleven_v3"
}

/// Audio packaged the way the caller asked for it.
#[derive(Debug)]
struct ServedAudio {
    bytes: Vec<u8>,
    content_type: &'static str,
    /// Overrides the provider's own label for `x-audio-format`, when the bytes are not what the
    /// provider called them.
    format_label: Option<&'static str>,
    /// Set when the vendor returned a different container than was asked for.
    substituted: Option<&'static str>,
}

/// Make the response body the format the caller asked for, or say truthfully what it is.
///
/// Only `wav` needs work. A vendor with no WAV output of its own — ElevenLabs offers mp3, pcm,
/// ulaw, alaw and opus — is sent the `wav` request as raw PCM, and returns bare samples. Those
/// were served as `audio/wav` with no RIFF header: nothing plays them, and the playground's voice
/// panel, which defaults to `wav`, showed a dead player for every ElevenLabs deployment. So:
///
/// * already a WAV container → unchanged;
/// * no container at all → raw PCM, wrapped as 16-bit mono at the rate the provider reported.
///   16-bit little-endian mono is WaaV's canonical PCM (`linear16`) and what every vendor here
///   returns for a PCM request;
/// * some other container → served as what it is, with its own content type, and the caller is
///   told. Wrapping an MP3 in a WAV header would only produce a corrupt file.
///
/// The OpenAI route holds the whole clip before replying, which is what makes a header with a
/// correct length possible here; the streaming paths cannot do this and are unchanged.
fn serve_as_requested(
    requested: AudioFormat,
    audio: Vec<u8>,
    sample_rate: u32,
) -> Result<ServedAudio, String> {
    use crate::core::tts::sniff::{SniffedContainer, sniff_container};

    let as_is = |bytes| ServedAudio {
        bytes,
        content_type: requested.content_type(),
        format_label: None,
        substituted: None,
    };
    if !matches!(requested, AudioFormat::Wav) {
        return Ok(as_is(audio));
    }
    match sniff_container(&audio) {
        Some(SniffedContainer::Wav) => Ok(as_is(audio)),
        None => {
            let wav =
                crate::core::stt::wav::encode_pcm16_wav(&audio, sample_rate, 1).map_err(|e| {
                    format!(
                        "the vendor returned raw audio that could not be packaged as WAV: {e:?}"
                    )
                })?;
            Ok(ServedAudio {
                bytes: wav,
                content_type: "audio/wav",
                format_label: Some("wav"),
                substituted: None,
            })
        }
        Some(other) => Ok(ServedAudio {
            bytes: audio,
            content_type: match other {
                SniffedContainer::Mp3 => "audio/mpeg",
                SniffedContainer::Ogg => "audio/ogg",
                SniffedContainer::Flac => "audio/flac",
                SniffedContainer::Wav => "audio/wav",
            },
            format_label: Some(other.as_format_str()),
            substituted: Some(other.as_format_str()),
        }),
    }
}

/// The vendor's default voice, when it has one.
///
/// `None` for a vendor whose default is unknown here. Guessing one would synthesise in a voice
/// nobody chose, and could pick one the deployment's model does not carry.
fn vendor_default_voice(vendor: &str) -> Option<&'static str> {
    Some(crate::handlers::voices::provider_default_voice(vendor)).filter(|v| !v.is_empty())
}

/// The voice for a request that names none, on a deployment that configures none.
///
/// A fixed per-vendor default is not enough on its own, because what an ACCOUNT may use is not
/// what the vendor ships. ElevenLabs' long-standing default, Rachel, is a library voice for
/// accounts created since ElevenLabs reorganised its catalogue, and a free-tier key is refused it
/// outright (`402 Free users cannot use library voices via the API`). So the account's own voice
/// list — fetched with the deployment's credential and cached, exactly as a described voice is —
/// decides; see [`pick_default_voice`]. The fetch is spent only on requests with nothing else to
/// go on, and a vendor with no catalog here costs nothing: the list comes back empty.
async fn default_voice(
    state: &Arc<AppState>,
    endpoint: &bud_auth::credentials::VoiceEndpoint,
    endpoint_name: &str,
    advisories: &mut Advisories,
) -> Option<String> {
    let catalog = crate::handlers::voices::fetch_provider_catalog_with_key(
        state,
        &endpoint.vendor,
        endpoint.credential.as_deref(),
    )
    .await;
    let picked = pick_default_voice(vendor_default_voice(&endpoint.vendor), &catalog)?;
    if let Some(name) = picked.from_account {
        // Not the vendor's documented default, so say which voice this was and how to choose.
        advisories.warn(format!(
            "deployment '{endpoint_name}' has no voice configured; used '{name}' ({}), the first \
             voice on the {} account. Set a voice in the deployment's audio settings to choose one.",
            picked.id, endpoint.vendor
        ));
    }
    info!(vendor = %endpoint.vendor, voice = %picked.id, catalog = catalog.len(), "defaulted a voice");
    Some(picked.id)
}

/// A default voice, and — when it came from the account's list rather than the vendor's fixed
/// default — the display name to report it by.
#[derive(Debug, PartialEq, Eq)]
struct PickedVoice {
    id: String,
    from_account: Option<String>,
}

/// Choose between the vendor's fixed default and the account's own voices.
///
/// * No catalog (unfetchable, or a vendor WaaV cannot list) → the fixed default, unverified.
///   That is the behaviour before the catalog was consulted, and the vendor's own error still
///   reaches the caller as a 400 if the account cannot use it.
/// * The catalog carries the fixed default → it, so the choice stays stable across accounts.
/// * Otherwise → the catalog's first voice, in the vendor's own order.
fn pick_default_voice(
    fixed: Option<&str>,
    catalog: &[crate::handlers::voices::Voice],
) -> Option<PickedVoice> {
    if catalog.is_empty() || fixed.is_some_and(|f| catalog.iter().any(|v| v.id == f)) {
        return fixed.map(|f| PickedVoice {
            id: f.to_string(),
            from_account: None,
        });
    }
    catalog
        .iter()
        .find(|v| !v.id.trim().is_empty())
        .map(|v| PickedVoice {
            id: v.id.clone(),
            from_account: Some(v.name.clone()),
        })
}

/// Nothing named a voice, nothing described one, and the vendor has no default to fall back on.
fn no_voice_error(endpoint: &str, vendor: &str) -> Response {
    openai_error(
        StatusCode::BAD_REQUEST,
        "invalid_request_error",
        format!(
            "`voice` is required: deployment '{endpoint}' has no voice configured and {vendor} has \
             no default voice. Pass a {vendor} voice in `voice`, or set one in the deployment's \
             audio settings."
        ),
        Some("voice"),
    )
}

/// Where the voice a synthesis ran with came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VoiceOrigin {
    /// The request's `voice` field.
    Request,
    /// The deployment's configured default voice.
    Deployment,
    /// Resolved from the deployment's voice description.
    Described,
    /// Nothing named or described one; WaaV picked the vendor or account default.
    Default,
}

/// A vendor's refusal, as the caller should see it: 400, with the vendor's own sentence.
///
/// When the refusal is about the voice, the message also says whose voice it was, because each
/// origin is fixed in a different place:
///
/// * **Request** — `param: voice`, and what the field takes. Common rather than exotic: OpenAI's
///   API makes `voice` mandatory, so OpenAI clients send `alloy` whatever vendor sits behind it.
/// * **Deployment / Described** — the deployment's own configuration. The caller did not choose it
///   and cannot fix it by editing the request; the vendor's sentence alone ("An invalid ID has
///   been received: 'x'") left them hunting through a body that never mentioned `x`.
/// * **Default** — WaaV picked it; say which, and how to choose another.
///
/// "About the voice" means the refusal quotes the voice back or talks about voices. Matching on
/// that rather than each vendor's error code keeps this vendor-agnostic; a miss only loses the
/// hint, never the vendor's sentence. `voice_settings` is excluded — a refusal of a stability
/// value is not a refusal of the voice.
fn rejection_error(
    message: String,
    voice: &str,
    origin: VoiceOrigin,
    vendor: &str,
    endpoint: &str,
) -> Response {
    let v = voice.trim();
    let about_voice = !v.is_empty()
        && (message.contains(&format!("'{v}'"))
            || message.contains(&format!("\"{v}\""))
            || message
                .to_lowercase()
                .replace("voice_settings", "")
                .contains("voice"));
    let (message, param) = match (about_voice, origin) {
        (false, _) => (message, None),
        (true, VoiceOrigin::Request) => (
            format!(
                "{message} `voice` takes a voice id from {vendor}; omit it to use this deployment's voice."
            ),
            Some("voice"),
        ),
        (true, VoiceOrigin::Deployment | VoiceOrigin::Described) => (
            format!(
                "{message} '{v}' is the voice deployment '{endpoint}' is configured with; change it \
                 in the deployment's audio settings, or pass a voice id in `voice`."
            ),
            None,
        ),
        (true, VoiceOrigin::Default) => (
            format!(
                "{message} No voice was named, so the default '{v}' was used, and this {vendor} \
                 account cannot use it. Pass a voice id in `voice`, or set one in the deployment's \
                 audio settings."
            ),
            Some("voice"),
        ),
    };
    openai_error(
        StatusCode::BAD_REQUEST,
        "invalid_request_error",
        message,
        param,
    )
}

/// Whether this transcription ran through WaaV's own denoiser.
///
/// Deliberately NOT a member of `voice_attrs::ALL`. That registry is a cross-repo contract with
/// budmetrics' `VoiceTurnFact`, and a name added to it without a matching column is a column that
/// is NULL forever. This is a trace attribute an operator reads in the span, which is exactly what
/// N1 asks for: telling a denoised transcription from a raw one after the fact.
const NOISE_SUPPRESSION_ATTR: &str = "bud.voice.stt.noise_suppression";

/// Run the decoded PCM through DeepFilterNet before it reaches the vendor (FRD-018 Part III N1).
///
/// Three properties, each of which is the difference between a useful feature and a confusing one:
///
/// * **Off unless configured.** It costs CPU per request and changes the audio the vendor bills
///   against, so it is an explicit choice, never a default.
/// * **A build without the feature degrades LOUDLY.** `noise-filter` is an optional cargo feature.
///   On a build without it, a configured `noise_suppression: true` must not silently transcribe
///   the original as though it had worked -- the operator would attribute the unchanged accuracy
///   to the denoiser being ineffective rather than absent.
/// * **A failure is not a failed request.** The original audio is still perfectly transcribable.
async fn apply_noise_suppression(
    audio: waav_openai_audio::pcm::PcmAudio,
    requested: bool,
    turn_span: &tracing::Span,
    advisories: &mut Advisories,
) -> waav_openai_audio::pcm::PcmAudio {
    if !requested {
        turn_span.record(NOISE_SUPPRESSION_ATTR, false);
        return audio;
    }

    #[cfg(not(feature = "noise-filter"))]
    {
        turn_span.record(NOISE_SUPPRESSION_ATTR, false);
        advisories.warn(
            "noise_suppression is configured on this deployment but this gateway build does not              include the noise-filter feature; the recording was transcribed unprocessed"
                .to_string(),
        );
        audio
    }

    #[cfg(feature = "noise-filter")]
    {
        let sample_rate = audio.sample_rate;
        let bytes = axum::body::Bytes::from(audio.to_bytes_le());
        match crate::utils::reduce_noise_async(bytes, sample_rate).await {
            Ok(processed) => {
                // The filter returns interleaved little-endian i16, the same shape it was given.
                // An odd length would mean a truncated final sample, so the chunk size is
                // asserted by construction rather than assumed.
                let samples: Vec<i16> = processed
                    .chunks_exact(2)
                    .map(|c| i16::from_le_bytes([c[0], c[1]]))
                    .collect();
                turn_span.record(NOISE_SUPPRESSION_ATTR, true);
                info!(
                    samples = samples.len(),
                    "noise suppression applied before transcription"
                );
                waav_openai_audio::pcm::PcmAudio {
                    samples,
                    sample_rate,
                }
            }
            Err(e) => {
                turn_span.record(NOISE_SUPPRESSION_ATTR, false);
                advisories.warn(format!(
                    "noise suppression failed ({e}); the recording was transcribed unprocessed"
                ));
                audio
            }
        }
    }
}

/// Translate a deployment's described voice into the resolver's own shape.
///
/// `None` when nothing was described: an all-empty descriptor must not trigger a catalog fetch,
/// which would spend vendor I/O to decide nothing.
///
/// The free-text field is carried on the blob as `name_hint` and lands on the resolver's `style`,
/// because that is what the control promises — "words matched against the vendor's own voice
/// metadata: warm, gravelly, bright". `name_hint` in the resolver means something narrower: a
/// substring match against the voice's NAME, scored strongest of all. Sending a timbre word there
/// would let "calm" pick a voice called Calmly over every actually-calm voice in the catalog. The
/// blob key keeps its original spelling so an entry written before this still parses.
fn describing_voice(
    described: Option<&bud_auth::endpoint_config::VoiceDescriptor>,
) -> Option<crate::core::voice::VoiceDescriptor> {
    use crate::core::voice::{Age, Gender, VoiceDescriptor};

    let described = described?;
    if described.is_empty() {
        return None;
    }
    Some(VoiceDescriptor {
        gender: described.gender.as_deref().and_then(Gender::from_str),
        locale: described.locale.clone(),
        accent: described.accent.clone(),
        age: described.age.as_deref().and_then(Age::from_str),
        style: described.name_hint.clone(),
        name_hint: None,
    })
}

/// Resolve a deployment's VOICE DESCRIPTOR against the vendor's catalog.
///
/// Reached only when neither the request nor the deployment names a voice outright, so it costs
/// a catalog fetch on exactly the requests that have nothing else to go on. The catalog is cached
/// for ten minutes and keyed by credential, and an unreachable or uncatalogued vendor returns an
/// empty one — which `resolve_voice` maps to the vendor default plus a warning rather than an
/// error. That degrade is the whole design: a description is a preference, not a requirement.
///
/// The descriptor's free-text field is carried on the blob as `name_hint` and mapped here onto
/// the resolver's `style`, because that is what the control above it promises — "words matched
/// against the vendor's own voice metadata: warm, gravelly, bright". The blob key keeps its
/// original spelling so an entry written before this still parses.
async fn resolve_described_voice(
    state: &Arc<AppState>,
    endpoint: &bud_auth::credentials::VoiceEndpoint,
    advisories: &mut Advisories,
) -> Option<String> {
    let tts = endpoint.config.tts();
    let descriptor = describing_voice(tts.voice_descriptor.as_ref())?;

    let catalog = crate::handlers::voices::fetch_provider_catalog_with_key(
        state,
        &endpoint.vendor,
        endpoint.credential.as_deref(),
    )
    .await;
    let resolved = crate::core::voice::resolve_voice(
        &descriptor,
        &catalog,
        crate::handlers::voices::provider_default_voice(&endpoint.vendor),
    );

    if let Some(warning) = resolved.warning {
        warn!(
            vendor = %endpoint.vendor,
            descriptor = %descriptor.describe(),
            resolved = %resolved.voice_id,
            "voice descriptor advisory: {warning}"
        );
        advisories.warn(warning);
    }
    info!(
        vendor = %endpoint.vendor,
        descriptor = %descriptor.describe(),
        voice = %resolved.voice_id,
        catalog = catalog.len(),
        "resolved a described voice"
    );
    Some(resolved.voice_id).filter(|v| !v.trim().is_empty())
}

/// The voices WaaV can name for a vendor without leaving the process.
///
/// Empty means "no catalog here, forward whatever the caller asked for" — the behaviour every
/// vendor had before, and the only safe default: validating against an empty list would reject
/// every voice and turn a missing catalog into an outage for that endpoint.
fn known_voices_for(vendor: &str) -> &'static [&'static str] {
    match vendor {
        "openai" => speech::OPENAI_VOICES,
        _ => &[],
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
    // OpenAI's SDKs send the array as repeated `timestamp_granularities[]` fields.
    let mut timestamp_granularities: Vec<String> = Vec::new();

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
                    // The body limit is enforced LAZILY, as the body streams, so exceeding it
                    // lands here rather than as a rejection of the `Multipart` extractor -- and
                    // multer renders it as "Error parsing `multipart/form-data` request", which
                    // mentions neither size nor a limit. Four identical attempts against this
                    // gateway produced four log lines saying only "auth succeeded". Naming the
                    // ceiling is the difference between a ten-second diagnosis and an
                    // afternoon.
                    let limit = crate::routes::api::max_audio_upload_bytes();
                    tracing::warn!(
                        "rejecting an upload on `file`: {e} (ceiling {limit} bytes, set \
                         {} to change it)",
                        crate::routes::api::MAX_AUDIO_UPLOAD_BYTES_ENV
                    );
                    return openai_error(
                        StatusCode::BAD_REQUEST,
                        "invalid_request_error",
                        format!(
                            "Could not read the uploaded file: {e}. This gateway accepts uploads \
                             up to {limit} bytes ({:.0} MiB); a larger file is refused here with \
                             exactly this message.",
                            limit as f64 / (1024.0 * 1024.0)
                        ),
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
            "prompt" => prompt = Some(value).filter(|p| !p.trim().is_empty()),
            // Refused rather than dropped: `value.parse().ok()` turned "warm" into "no
            // temperature" and answered 200, which is the ignored-parameter class this route had.
            "temperature" if !value.trim().is_empty() => match value.trim().parse() {
                Ok(t) => temperature = Some(t),
                Err(_) => {
                    return openai_error(
                        StatusCode::BAD_REQUEST,
                        "invalid_request_error",
                        format!("`temperature` must be a number from 0.0 to 1.0, got {value:?}"),
                        Some("temperature"),
                    );
                }
            },
            "timestamp_granularities[]" | "timestamp_granularities" => {
                timestamp_granularities.push(value)
            }
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
        timestamp_granularities,
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

    let mut advisories = Advisories::new();
    // Resolved BEFORE the span so the recorded language is the one that will be used. The
    // endpoint is looked up first for the same reason; it was already being resolved a few
    // lines further down, so nothing new is on the request path.
    let Some(endpoint) =
        state.resolve_voice_endpoint(&settings.endpoint, capability, bearer.as_deref())
    else {
        return model_not_found(&settings.endpoint, capability);
    };
    let canonical_language = settings_map::resolve_language(
        settings.language.as_deref(),
        endpoint.language.as_deref(),
        endpoint.config.stt().language.as_deref(),
    );

    // The STT leg's span, mirroring the TTS one. `audio_seconds` is the billing dimension for
    // transcription exactly as `characters` is for synthesis, and both are declared Empty
    // because they are only known after the work: a field not declared at creation cannot be
    // recorded later, and attempting it silently does nothing.
    let turn_span = tracing::info_span!(
        "voice.turn",
        { voice_attrs::turn::CAPABILITY } = capability,
        { voice_attrs::turn::TRANSPORT } = "http",
        { voice_attrs::turn::ENDPOINT_ID } = %settings.endpoint,
        // The language that will actually be used, not just the one the request named:
        // request > endpoint default > the stt block's override (C1).
        { voice_attrs::turn::LANGUAGE } = canonical_language.as_deref().unwrap_or(""),
        { voice_attrs::turn::AUDIO_SECONDS } = tracing::field::Empty,
        { voice_attrs::turn::PROJECT_ID } = tracing::field::Empty,
        { voice_attrs::turn::USER_ID } = tracing::field::Empty,
        { voice_attrs::turn::API_KEY_ID } = tracing::field::Empty,
        { voice_attrs::leg::STT_VENDOR } = tracing::field::Empty,
        { voice_attrs::leg::STT_DURATION_MS } = tracing::field::Empty,
        // Declared here because a field not declared at span creation cannot be recorded later
        // and silently does nothing if you try.
        { NOISE_SUPPRESSION_ATTR } = tracing::field::Empty,
        otel.status_code = tracing::field::Empty,
        otel.status_message = tracing::field::Empty,
    );

    turn_span.record(voice_attrs::leg::STT_VENDOR, endpoint.vendor.as_str());

    // Attribution. The identity was always resolvable — `resolve_voice_endpoint` just never
    // asked for it — so project_id, user_id and api_key_id were NULL for every voice turn ever
    // recorded, and VoiceTurnFact could not attribute usage to a project at all.
    //
    // `recordable` filters `Some("")`: an empty string is not NULL, and recording one makes the
    // column look populated to the live check that asks whether a value ever arrived.
    if let Some(p) = state.resolve_principal(bearer.as_deref()).await {
        use waav_openai_audio::recordable;
        if let Some(v) = recordable(p.project_id.as_deref()) {
            turn_span.record(voice_attrs::turn::PROJECT_ID, v);
        }
        if let Some(v) = recordable(p.user_id.as_deref()) {
            turn_span.record(voice_attrs::turn::USER_ID, v);
        }
        if let Some(v) = recordable(p.api_key_id.as_deref()) {
            turn_span.record(voice_attrs::turn::API_KEY_ID, v);
        }
    }
    let stt_started = std::time::Instant::now();

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
        // Measured before `file_bytes` is moved into the call below. Header read only: no
        // samples are allocated, so a long upload costs nothing to measure.
        let measured_secs = waav_openai_audio::pcm::wav_duration_secs(&file_bytes);
        return match crate::handlers::transcribe::transcribe_self_hosted(
            &api_base,
            &api_key,
            &endpoint.model.clone().unwrap_or_default(),
            file_bytes,
            &filename,
            &settings,
        )
        .instrument(turn_span.clone())
        .await
        {
            Ok(body) => {
                turn_span.record(
                    voice_attrs::leg::STT_DURATION_MS,
                    stt_started.elapsed().as_millis() as u64,
                );
                // `audio_seconds` is the billing dimension for transcription. This branch
                // forwards the upload verbatim and never decodes it, so the field the span
                // reserves was never recorded and the column was permanently NULL for every
                // self-hosted transcription. Read it from the WAV header instead — no samples
                // are allocated. A container the header read cannot parse stays NULL, which is
                // the honest answer: a guessed number in a billing column is worse than none.
                if let Some(secs) = measured_secs {
                    turn_span.record(voice_attrs::turn::AUDIO_SECONDS, secs);
                }
                passthrough_response(&settings.response_format, body, &advisories)
            }
            Err(e) => {
                warn!(endpoint = %settings.endpoint, error = %e, "self-hosted transcription failed");
                mark_turn_failed(&turn_span, &e);
                openai_error(StatusCode::BAD_GATEWAY, "api_error", e, None)
            }
        };
    }

    // Decode BEFORE touching the provider: a container we cannot read is the caller's problem
    // and must not cost a vendor connection to discover.
    //
    // On a blocking thread: an MP3 is decoded and resampled in full here — seconds of CPU for a
    // long recording — and on the async runtime that would stall every other request scheduled
    // on the same worker for as long as it took.
    let decode_name = filename.clone();
    let decoded = tokio::task::spawn_blocking(move || {
        waav_openai_audio::pcm::decode(&file_bytes, &decode_name)
    })
    .await;
    let audio = match decoded {
        Ok(Ok(a)) => a,
        Ok(Err(e)) => {
            mark_turn_failed(&turn_span, &e.to_string());
            return translation_error(&e);
        }
        Err(e) => {
            warn!(endpoint = %settings.endpoint, error = %e, "audio decode task failed");
            mark_turn_failed(&turn_span, "audio decode task failed");
            return openai_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "api_error",
                "the uploaded audio could not be decoded".to_string(),
                None,
            );
        }
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

    let stt_settings = endpoint.config.stt();

    // FRD-018 Part III C1/C3 step 3. `language`, `channels`, `punctuation` and `encoding` were
    // hardcoded here; they keep the same values as defaults, so a deployment with no `stt`
    // block behaves exactly as it did.
    //
    // The language is routed through the P2 mapper rather than handed to the vendor raw, which
    // is what makes an endpoint default work across vendors instead of only on the ones that
    // happen to speak BCP-47.
    let mapped_language = canonical_language
        .as_deref()
        .and_then(|canonical| {
            settings_map::map_language_for(
                canonical,
                &endpoint.vendor,
                endpoint.model.as_deref().unwrap_or_default(),
                &mut advisories,
            )
        })
        .unwrap_or_else(|| "en-US".to_string());

    let mut stt_config = crate::core::stt::STTConfig {
        provider: endpoint.vendor.clone(),
        api_key,
        language: mapped_language,
        sample_rate: audio.sample_rate,
        channels: 1,
        punctuation: true,
        encoding: "linear16".to_string(),
        model: endpoint.model.clone().unwrap_or_default(),
    };
    settings_map::apply_stt_flat(&stt_settings, &mut stt_config, &mut advisories);

    // N1: decode -> denoise -> transcribe. DeepFilterNet ships in the image and, before this,
    // had exactly one caller -- the LiveKit participant path, which this platform does not
    // deploy. Cleaning a noisy recording before transcription is the one thing it is
    // unambiguously good at, and it needs no vendor support at all.
    let audio = apply_noise_suppression(
        audio,
        stt_settings.noise_suppression == Some(true),
        &turn_span,
        &mut advisories,
    )
    .await;

    // C3: the canonical vocabulary and the translation request reach the provider only through
    // the STANDARD config. `translate` is the ROUTE the caller chose, so it wins over a
    // deployment's configured target list.
    let mut std_config = settings_map::standard_stt(
        stt_config,
        &stt_settings,
        settings_map::translation_for(endpoint.config.translation.as_ref(), translate),
        &mut advisories,
    );
    apply_request_stt_fields(
        &mut std_config,
        &settings,
        &endpoint.vendor,
        &mut advisories,
    );

    match crate::handlers::transcribe::transcribe_once_standard(
        &endpoint.vendor,
        std_config,
        &audio,
    )
    .instrument(turn_span.clone())
    .await
    {
        Ok(t) => {
            turn_span.record(
                voice_attrs::leg::STT_DURATION_MS,
                stt_started.elapsed().as_millis() as u64,
            );
            // The billing dimension, recorded on success only: set before the call, it counted
            // every refused transcription's audio as transcribed.
            turn_span.record(voice_attrs::turn::AUDIO_SECONDS, audio.duration_secs());
            // What the provider could not honour — a translation target list on a vendor that
            // cannot translate, a batch knob with no equivalent. Produced since the prerecorded
            // driver was written and, until now, logged and nothing else: the response was
            // byte-identical to one where the setting had worked.
            advisories.extend(t.config_warnings.clone());
            if t.truncated {
                warn!(endpoint = %settings.endpoint, "returning a partial transcript");
            }
            // The vendor's own measurement where it reports one; the decoder's otherwise.
            let duration = t.audio_duration.or_else(|| Some(audio.duration_secs()));
            let segments = transcript_segments(
                &t.text,
                &t.words,
                duration,
                &settings,
                &endpoint.vendor,
                &mut advisories,
            );
            let result = transcription::TranscriptionResult {
                text: t.text,
                // The DETECTED language wins over the requested one: `language_detection` exists
                // to answer "what was this?", and echoing the request back answers nothing. Falls
                // back to what was asked for, and to `null` when neither is known.
                language: t
                    .detected_language
                    .clone()
                    .or_else(|| settings.language.clone()),
                duration,
                segments,
            };
            let words: &[crate::core::stt::WordTiming] = if settings.wants_words() {
                &t.words
            } else {
                &[]
            };
            render_transcription(
                &settings.response_format,
                settings.translate,
                &result,
                &advisories,
                words,
                &t.speakers,
                &t.alternatives,
            )
        }
        Err(e) => {
            warn!(endpoint = %settings.endpoint, error = %e, "transcription failed");
            mark_turn_failed(&turn_span, &e.to_string());
            match e {
                // The deployment cannot be served as configured: the caller's problem and
                // fixable, so it is a 400 carrying the reason. Returning 502 here — as this
                // path used to for everything — sends an operator to look at the vendor's
                // status page for a model id they typed themselves.
                crate::handlers::transcribe::TranscribeFailure::Configuration(message) => {
                    openai_error(
                        StatusCode::BAD_REQUEST,
                        "invalid_request_error",
                        message,
                        Some("model"),
                    )
                }
                // 502, not 500: the failure is upstream of WaaV, and the distinction is what
                // tells an operator whether to look at the vendor or at us.
                crate::handlers::transcribe::TranscribeFailure::Upstream(message) => {
                    openai_error(StatusCode::BAD_GATEWAY, "api_error", message, None)
                }
            }
        }
    }
}

/// Render a result in the format the caller asked for.
///
/// `text`, `srt` and `vtt` are PLAIN BODIES, not JSON — a client that asked for an SRT file
/// and got `{"text": "1\n00:00:00,000 ..."}` cannot feed it to a player.
///
/// Advisories (W1) ride the HEADERS on every format, because three of the five have no JSON body
/// to put an array in. `verbose_json` additionally carries them in the body, where an SDK finds
/// them without reaching for response headers — which is the natural home, but only covers two
/// of the five cases, so it cannot be the only carrier.
/// Vendors whose upload path sends a request's `prompt` / `temperature` on to the vendor.
///
/// Each entry is a provider config that reads the key from `extras`: OpenAI and Groq
/// (`from_standard`), Sarvam (`prompt` as a streaming parameter), ElevenLabs batch
/// (`temperature`, `apply_extras`). A vendor not listed drops the field, so the caller is told.
/// Self-hosted deployments are not here: their request is forwarded whole.
const PROMPT_VENDORS: &[&str] = &["openai", "groq", "sarvam"];
const TEMPERATURE_VENDORS: &[&str] = &["openai", "groq", "elevenlabs"];

fn uses_request_field(vendors: &[&str], vendor: &str) -> bool {
    let vendor = crate::core::capabilities::normalize_provider(vendor);
    vendors.iter().any(|v| *v == vendor)
}

/// Apply the request-level transcription fields a vendor can act on, and say which it cannot.
///
/// `prompt`, `temperature` and `timestamp_granularities` used to be parsed and then dropped for
/// every hosted vendor with a 200 and no word about it.
fn apply_request_stt_fields(
    std_config: &mut crate::core::stt::standard::StandardSTTConfig,
    settings: &transcription::TranscriptionSettings,
    vendor: &str,
    advisories: &mut Advisories,
) {
    if let Some(prompt) = &settings.prompt {
        if uses_request_field(PROMPT_VENDORS, vendor) {
            std_config
                .extras
                .0
                .insert("prompt".into(), serde_json::Value::String(prompt.clone()));
        } else {
            advisories.warn(format!(
                "`prompt` is not applied by {vendor} and was ignored"
            ));
        }
    }
    if let Some(t) = settings.temperature {
        if uses_request_field(TEMPERATURE_VENDORS, vendor) {
            // Through the decimal text, so 0.2 reaches the vendor as 0.2 and not as the f32's
            // widened 0.20000000298023224.
            let value = t.to_string().parse::<f64>().unwrap_or(f64::from(t));
            if let Some(n) = serde_json::Number::from_f64(value) {
                std_config
                    .extras
                    .0
                    .insert("temperature".into(), serde_json::Value::Number(n));
            }
        } else {
            advisories.warn(format!(
                "`temperature` is not applied by {vendor} and was ignored"
            ));
        }
    }

    let asked_for_words = settings
        .timestamp_granularities
        .as_ref()
        .is_some_and(|g| g.contains(&transcription::TimestampGranularity::Word));
    if settings.timestamp_granularities.is_some()
        && settings.response_format != transcription::TranscriptionResponseFormat::VerboseJson
    {
        advisories.warn(format!(
            "`timestamp_granularities` applies only to response_format=verbose_json and was \
             ignored for {}",
            settings.response_format.as_str()
        ));
    }
    // Segments and subtitle cues are cut from word timings, so ask for them wherever the vendor
    // has a switch. A deployment that turned them off keeps them off unless the caller asked for
    // words by name. Vendors that always return words (Deepgram, Speechmatics, Rev AI) have no
    // switch and need nothing here.
    let needs_timings = settings.response_format.requires_timestamps() || asked_for_words;
    if needs_timings && crate::core::capabilities::stt_honours(vendor, "word_timestamps") {
        match std_config.features.word_timestamps {
            Some(false) if !asked_for_words => {}
            _ => std_config.features.word_timestamps = Some(true),
        }
    }
}

/// Caption-sized segments for `verbose_json`, `srt` and `vtt`, cut from the vendor's words.
///
/// A vendor that returned text but no word timings gets one segment spanning the audio, and the
/// caller is told: before, `srt` answered 200 with an empty body, which a player accepts and shows
/// nothing for.
fn transcript_segments(
    text: &str,
    words: &[crate::core::stt::WordTiming],
    duration: Option<f64>,
    settings: &transcription::TranscriptionSettings,
    vendor: &str,
    advisories: &mut Advisories,
) -> Vec<transcription::Segment> {
    let timed: Vec<transcription::TimedWord<'_>> = words
        .iter()
        .map(|w| transcription::TimedWord {
            text: &w.word,
            start: w.start,
            end: w.end,
            speaker: w.speaker_id.as_deref(),
        })
        .collect();
    let segments = transcription::caption_segments(&timed);
    if !segments.is_empty() || text.trim().is_empty() {
        return segments;
    }
    if settings.response_format.requires_timestamps() {
        advisories.warn(format!(
            "{vendor} returned no word timings, so the transcript is one segment spanning the \
             whole audio"
        ));
    }
    vec![transcription::Segment {
        id: 0,
        start: 0.0,
        end: duration.unwrap_or(0.0),
        text: text.trim().to_string(),
    }]
}

fn render_transcription(
    format: &transcription::TranscriptionResponseFormat,
    // `/v1/audio/translations`. OpenAI's `verbose_json` names the task, and this route said
    // "transcribe" for both until it took the flag.
    translate: bool,
    result: &transcription::TranscriptionResult,
    advisories: &Advisories,
    words: &[crate::core::stt::WordTiming],
    speakers: &[crate::core::stt::SpeakerInfo],
    alternatives: &[String],
) -> Response {
    use transcription::TranscriptionResponseFormat as F;
    let mut headers = HeaderMap::new();
    if let Ok(ct) = format.content_type().parse() {
        headers.insert(header::CONTENT_TYPE, ct);
    }
    advisories.apply(&mut headers);
    match format {
        F::Json => (
            StatusCode::OK,
            headers,
            Json(serde_json::json!({ "text": result.text })),
        )
            .into_response(),
        F::VerboseJson => {
            let mut body = serde_json::json!({
                "task": if translate { "translate" } else { "transcribe" },
                "language": result.language,
                "duration": result.duration,
                "text": result.text,
                "segments": result.segments,
            });
            // `words` is OpenAI's own shape for word-level timings, plus `speaker` where the
            // provider diarized. Present only when there is something to say: an always-present
            // empty array would read as "asked for, and there were none". `words` is already
            // empty here when the caller's `timestamp_granularities` left `word` out.
            //
            // `segments` are the same cues `srt` and `vtt` render — cut from the words by
            // `transcription::caption_segments`, since the vendors return words, not spans.
            if let Some(obj) = body.as_object_mut() {
                if !words.is_empty() {
                    obj.insert(
                        "words".into(),
                        serde_json::Value::Array(
                            words
                                .iter()
                                .map(|w| {
                                    let mut entry = serde_json::json!({
                                        "word": w.word,
                                        "start": w.start,
                                        "end": w.end,
                                    });
                                    if let (Some(e), Some(speaker)) =
                                        (entry.as_object_mut(), w.speaker_id.as_ref())
                                    {
                                        e.insert(
                                            "speaker".into(),
                                            serde_json::Value::String(speaker.clone()),
                                        );
                                    }
                                    entry
                                })
                                .collect(),
                        ),
                    );
                }
                if !speakers.is_empty() {
                    obj.insert(
                        "speakers".into(),
                        serde_json::Value::Array(
                            speakers
                                .iter()
                                .map(|s| serde_json::json!({ "id": s.speaker_id }))
                                .collect(),
                        ),
                    );
                }
                // The runner-up hypotheses, best-first. `text` is still the best one, so a caller
                // that ignores this field sees exactly what it saw before.
                if !alternatives.is_empty() {
                    obj.insert("alternatives".into(), serde_json::json!(alternatives));
                }
            }
            // Omitted entirely when there is nothing to say. An always-present empty array reads
            // as "checked, nothing wrong", which is a claim this cannot honestly make for the
            // knobs no provider declares support for either way.
            if !advisories.is_empty()
                && let Some(obj) = body.as_object_mut()
            {
                obj.insert("warnings".into(), serde_json::json!(advisories.as_slice()));
            }
            (StatusCode::OK, headers, Json(body)).into_response()
        }
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
    advisories: &Advisories,
) -> Response {
    let mut headers = HeaderMap::new();
    if let Ok(ct) = format.content_type().parse() {
        headers.insert(header::CONTENT_TYPE, ct);
    }
    // Headers only: the body is the backend's own and is deliberately not re-rendered, so
    // injecting a `warnings` key would mean parsing and rebuilding exactly what this function
    // exists to pass through untouched.
    advisories.apply(&mut headers);
    (StatusCode::OK, headers, body).into_response()
}

#[cfg(test)]
mod voice_descriptor_tests {
    use super::describing_voice;
    use crate::core::voice::{Age, Gender};
    use bud_auth::endpoint_config::VoiceDescriptor as Described;

    fn described() -> Described {
        Described {
            gender: Some("female".into()),
            locale: Some("en-GB".into()),
            accent: None,
            age: Some("young".into()),
            name_hint: Some("warm".into()),
        }
    }

    #[test]
    fn a_described_voice_reaches_the_resolver() {
        // The defect this closes: budadmin wrote these five fields, budapp validated and
        // published them, bud-auth parsed them — and nothing on the synthesis path read the
        // result, so a deployment configured only by description answered `400 voice required`.
        let d = describing_voice(Some(&described())).expect("a described voice converts");
        assert_eq!(d.gender, Some(Gender::Female));
        assert_eq!(d.locale.as_deref(), Some("en-GB"));
        assert_eq!(d.age, Some(Age::Young));
    }

    #[test]
    fn the_free_text_field_is_a_timbre_not_a_name() {
        // It is carried on the blob as `name_hint` and the control above it promises timbre
        // words. The resolver's own `name_hint` scores a substring match against the voice NAME,
        // strongest of all — so sending "calm" there would pick a voice called Calmly over every
        // actually-calm voice in the catalog.
        let d = describing_voice(Some(&described())).unwrap();
        assert_eq!(d.style.as_deref(), Some("warm"));
        assert_eq!(d.name_hint, None);
    }

    #[test]
    fn nothing_described_means_no_catalog_fetch() {
        // An all-empty descriptor must not reach the vendor: it would spend a network round trip
        // to decide nothing, on the synthesis path.
        assert!(describing_voice(None).is_none());
        assert!(describing_voice(Some(&Described::default())).is_none());
    }

    #[test]
    fn an_unparseable_gender_or_age_is_dropped_rather_than_failing() {
        // budapp validates these against its own vocabulary, but a blob written by hand or by a
        // future path should degrade to "unspecified" rather than refuse to synthesise.
        let d = describing_voice(Some(&Described {
            gender: Some("wobbly".into()),
            age: Some("ancient".into()),
            locale: Some("en-US".into()),
            ..Default::default()
        }))
        .expect("still describes a locale");
        assert_eq!(d.gender, None);
        assert_eq!(d.age, None);
        assert_eq!(d.locale.as_deref(), Some("en-US"));
    }
}

#[cfg(test)]
mod speech_error_tests {
    use super::{
        PickedVoice, no_voice_error, pick_default_voice, rejection_error, vendor_default_voice,
    };
    use axum::http::StatusCode;
    use waav_openai_audio::speech::AudioFormat;

    async fn body(resp: axum::response::Response) -> (StatusCode, serde_json::Value) {
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    const REFUSAL: &str = "elevenlabs rejected the request (404 Not Found): A voice with voice_id 'alloy' was not found.";

    #[tokio::test]
    async fn a_refused_request_voice_is_a_400_on_the_voice_param() {
        let (status, json) = body(rejection_error(
            REFUSAL.into(),
            "alloy",
            super::VoiceOrigin::Request,
            "elevenlabs",
            "eleven-v3",
        ))
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(json["error"]["type"], "invalid_request_error");
        assert_eq!(json["error"]["param"], "voice");
        let message = json["error"]["message"].as_str().unwrap();
        assert!(
            message.starts_with(REFUSAL),
            "vendor sentence kept first: {message}"
        );
        assert!(message.contains("omit it"), "says how to fix it: {message}");
        assert!(message.contains("voice id from elevenlabs"), "{message}");
    }

    #[tokio::test]
    async fn a_refusal_about_something_else_keeps_no_param() {
        // The deployment's voice was used, so the caller did not choose it and cannot be told to
        // change a field they never sent.
        let (status, json) = body(rejection_error(
            REFUSAL.into(),
            "",
            super::VoiceOrigin::Request,
            "elevenlabs",
            "eleven-v3",
        ))
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"]["param"].is_null());
        assert_eq!(json["error"]["message"], REFUSAL);

        let model = "elevenlabs rejected the request (400 Bad Request): model_id: unknown model";
        let (_, json) = body(rejection_error(
            model.into(),
            "Rachel",
            super::VoiceOrigin::Request,
            "elevenlabs",
            "eleven-v3",
        ))
        .await;
        assert!(json["error"]["param"].is_null(), "{json}");
    }

    fn voice(id: &str, name: &str) -> crate::handlers::voices::Voice {
        crate::handlers::voices::Voice {
            id: id.into(),
            sample: String::new(),
            name: name.into(),
            accent: String::new(),
            gender: String::new(),
            language: String::new(),
            ..Default::default()
        }
    }

    #[test]
    fn an_account_without_the_vendor_default_gets_its_own_first_voice() {
        // The live case: Rachel is not on this free-tier account, and using it answers 402.
        let catalog = [
            voice("EXAVITQu4vr4xnSDxMaL", "Sarah"),
            voice("FGY2WhTYpPnrIDTdsKH5", "Laura"),
        ];
        assert_eq!(
            pick_default_voice(Some("21m00Tcm4TlvDq8ikWAM"), &catalog),
            Some(PickedVoice {
                id: "EXAVITQu4vr4xnSDxMaL".into(),
                from_account: Some("Sarah".into()),
            })
        );
    }

    #[test]
    fn the_vendor_default_wins_when_the_account_has_it() {
        let catalog = [
            voice("EXAVITQu4vr4xnSDxMaL", "Sarah"),
            voice("21m00Tcm4TlvDq8ikWAM", "Rachel"),
        ];
        assert_eq!(
            pick_default_voice(Some("21m00Tcm4TlvDq8ikWAM"), &catalog),
            Some(PickedVoice {
                id: "21m00Tcm4TlvDq8ikWAM".into(),
                from_account: None
            })
        );
    }

    #[test]
    fn no_catalog_keeps_the_fixed_default_and_no_default_means_none() {
        assert_eq!(
            pick_default_voice(Some("alloy"), &[]),
            Some(PickedVoice {
                id: "alloy".into(),
                from_account: None
            })
        );
        assert_eq!(pick_default_voice(None, &[]), None);
        // A vendor with no fixed default still gets a voice when its account lists one.
        assert_eq!(
            pick_default_voice(None, &[voice("", "blank"), voice("v1", "One")]).map(|p| p.id),
            Some("v1".into())
        );
    }

    #[tokio::test]
    async fn a_refused_default_says_it_was_the_default_and_how_to_choose() {
        // The live case: no voice named, Rachel picked, and a free-tier account refused it.
        let refusal = "elevenlabs rejected the request (402 Payment Required): Free users cannot use library voices via the API.";
        let (status, json) = body(rejection_error(
            refusal.into(),
            "21m00Tcm4TlvDq8ikWAM",
            super::VoiceOrigin::Default,
            "elevenlabs",
            "eleven-v3",
        ))
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(json["error"]["param"], "voice");
        let message = json["error"]["message"].as_str().unwrap();
        assert!(message.starts_with(refusal), "{message}");
        assert!(
            message.contains("'21m00Tcm4TlvDq8ikWAM'"),
            "names the voice: {message}"
        );
        assert!(
            message.contains("audio settings"),
            "says how to choose: {message}"
        );
    }

    // --- serve_as_requested: a `wav` response is a WAV file -------------------------------

    #[test]
    fn raw_pcm_asked_for_as_wav_gets_a_riff_header() {
        // The live case: ElevenLabs `pcm_24000` for a `wav` request, 96 000 bytes, no header.
        let pcm = vec![0u8; 96_000];
        let served = super::serve_as_requested(AudioFormat::Wav, pcm, 24_000).unwrap();
        assert_eq!(&served.bytes[0..4], b"RIFF");
        assert_eq!(&served.bytes[8..12], b"WAVE");
        assert_eq!(served.bytes.len(), 96_000 + 44);
        // The sample rate lands in the header (bytes 24..28, little-endian).
        assert_eq!(
            u32::from_le_bytes(served.bytes[24..28].try_into().unwrap()),
            24_000
        );
        assert_eq!(served.content_type, "audio/wav");
        assert_eq!(served.substituted, None);
    }

    #[test]
    fn a_real_wav_is_passed_through_untouched() {
        let wav = crate::core::stt::wav::encode_pcm16_wav(&[1, 0, 2, 0], 16_000, 1).unwrap();
        let served = super::serve_as_requested(AudioFormat::Wav, wav.clone(), 24_000).unwrap();
        assert_eq!(served.bytes, wav, "no second header");
    }

    #[test]
    fn another_container_is_labelled_as_what_it_is() {
        let mp3 = b"ID3\x04\x00\x00\x00\x00\x00\x00rest-of-an-mp3".to_vec();
        let served = super::serve_as_requested(AudioFormat::Wav, mp3.clone(), 24_000).unwrap();
        assert_eq!(served.bytes, mp3, "never wrapped");
        assert_eq!(served.content_type, "audio/mpeg");
        assert_eq!(served.substituted, Some("mp3"));
    }

    #[test]
    fn formats_other_than_wav_are_untouched() {
        let bytes = vec![7u8; 32];
        let served = super::serve_as_requested(AudioFormat::Mp3, bytes.clone(), 24_000).unwrap();
        assert_eq!(served.bytes, bytes);
        assert_eq!(served.content_type, "audio/mpeg");
        let served = super::serve_as_requested(AudioFormat::Pcm, bytes.clone(), 24_000).unwrap();
        assert_eq!(served.bytes, bytes, "pcm stays headerless");
    }

    #[test]
    fn elevenlabs_formats_are_the_ones_its_client_can_request() {
        let formats = super::vendor_output_formats("elevenlabs").unwrap();
        assert_eq!(
            formats,
            vec![
                AudioFormat::Mp3,
                AudioFormat::Opus,
                AudioFormat::Wav,
                AudioFormat::Pcm
            ],
            "aac and flac have no ElevenLabs output format"
        );
        assert_eq!(
            super::vendor_output_formats("deepgram"),
            None,
            "unknown means allow"
        );
    }

    #[test]
    fn streaming_latency_is_dropped_only_where_it_was_measured_to_fail() {
        assert!(super::rejects_streaming_latency("elevenlabs", "eleven_v3"));
        assert!(!super::rejects_streaming_latency(
            "elevenlabs",
            "eleven_flash_v2_5"
        ));
    }

    #[test]
    fn speed_is_flagged_only_where_it_was_measured_to_do_nothing() {
        assert!(super::speed_is_ignored("elevenlabs", "eleven_v3"));
        assert!(!super::speed_is_ignored(
            "elevenlabs",
            "eleven_multilingual_v2"
        ));
        assert!(!super::speed_is_ignored("openai", "eleven_v3"));
    }

    #[tokio::test]
    async fn a_refused_deployment_voice_says_it_is_the_deployments() {
        // Live: `voice: bogus-voice-xyz` configured on the deployment, none in the request.
        let refusal = "elevenlabs rejected the request (400 Bad Request): An invalid ID has been received: 'bogus-voice-xyz'. Make sure to provide a correct one.";
        let (status, json) = body(rejection_error(
            refusal.into(),
            "bogus-voice-xyz",
            super::VoiceOrigin::Deployment,
            "elevenlabs",
            "eleven-v3",
        ))
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(
            json["error"]["param"].is_null(),
            "the caller did not send it: {json}"
        );
        let message = json["error"]["message"].as_str().unwrap();
        assert!(
            message.contains("deployment 'eleven-v3' is configured with"),
            "{message}"
        );
    }

    #[tokio::test]
    async fn a_voice_settings_refusal_is_not_about_the_voice() {
        let refusal = "elevenlabs rejected the request (400 Bad Request): voice_settings.stability must be one of 0.0, 0.5, 1.0";
        let (_, json) = body(rejection_error(
            refusal.into(),
            "JBFqnCBsd6RMkjVDRZzb",
            super::VoiceOrigin::Default,
            "elevenlabs",
            "eleven-v3",
        ))
        .await;
        assert!(json["error"]["param"].is_null(), "{json}");
        assert_eq!(json["error"]["message"], refusal);
    }

    #[tokio::test]
    async fn a_refusal_unrelated_to_the_voice_does_not_blame_the_default() {
        // Live: "👍" with no voice named. ElevenLabs refused the TEXT, and the response said the
        // account could not use George.
        let refusal = "elevenlabs rejected the request (400 Bad Request): Input at position 0 has empty text. All inputs must include non-empty text after removing speaker tags and emojis.";
        let (status, json) = body(rejection_error(
            refusal.into(),
            "JBFqnCBsd6RMkjVDRZzb",
            super::VoiceOrigin::Default,
            "elevenlabs",
            "eleven-v3",
        ))
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"]["param"].is_null(), "{json}");
        assert_eq!(json["error"]["message"], refusal);
    }

    #[test]
    fn vendors_with_a_known_default_fall_back_to_it() {
        assert_eq!(
            vendor_default_voice("elevenlabs"),
            // George, not Rachel: a free-tier key is refused Rachel (402, "library voices").
            Some("JBFqnCBsd6RMkjVDRZzb")
        );
        assert_eq!(vendor_default_voice("openai"), Some("alloy"));
        assert_eq!(vendor_default_voice("deepgram"), Some("aura-2-thalia-en"));
        // No default is known for these, and guessing one would pick a voice nobody chose.
        assert_eq!(vendor_default_voice("aws-polly"), None);
        assert_eq!(vendor_default_voice("hume"), None);
    }

    #[tokio::test]
    async fn no_voice_anywhere_names_the_deployment_and_the_fix() {
        let (status, json) = body(no_voice_error("polly-tts", "aws-polly")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(json["error"]["param"], "voice");
        let message = json["error"]["message"].as_str().unwrap();
        assert!(
            message.contains("'polly-tts'") && message.contains("aws-polly"),
            "{message}"
        );
        assert!(message.contains("audio settings"), "{message}");
    }
}

#[cfg(test)]
mod request_field_tests {
    use super::{
        Advisories, PROMPT_VENDORS, TEMPERATURE_VENDORS, apply_request_stt_fields,
        transcript_segments,
    };
    use crate::core::stt::standard::{StandardSTTConfig, SttFeatures};
    use crate::core::stt::{STTConfig, WordTiming};
    use waav_openai_audio::transcription::{
        TimestampGranularity, TranscriptionResponseFormat as F, TranscriptionSettings,
    };

    fn settings(format: F) -> TranscriptionSettings {
        TranscriptionSettings {
            endpoint: "stt".into(),
            response_format: format,
            language: None,
            prompt: None,
            temperature: None,
            timestamp_granularities: None,
            translate: false,
        }
    }

    fn std_for(vendor: &str) -> StandardSTTConfig {
        StandardSTTConfig::from_base(STTConfig {
            provider: vendor.into(),
            api_key: "k".into(),
            language: "en-US".into(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".into(),
            model: String::new(),
        })
    }

    fn apply(vendor: &str, s: &TranscriptionSettings) -> (StandardSTTConfig, Vec<String>) {
        let mut cfg = std_for(vendor);
        let mut adv = Advisories::new();
        apply_request_stt_fields(&mut cfg, s, vendor, &mut adv);
        (cfg, adv.as_slice().to_vec())
    }

    /// The lists are claims about provider configs. Each entry is checked against the config it
    /// names, so a vendor added to a list without a config that reads the key fails here.
    #[test]
    fn every_listed_vendor_really_sends_the_field() {
        let with = |key: &str, value: serde_json::Value| {
            let mut c = std_for("x");
            c.extras.0.insert(key.into(), value);
            c
        };
        for v in PROMPT_VENDORS {
            let c = with("prompt", serde_json::json!("Bud, WaaV"));
            let got = match *v {
                "openai" => crate::core::stt::openai::OpenAISTTConfig::from_standard(&c).prompt,
                "groq" => crate::core::stt::groq::GroqSTTConfig::from_standard(&c).prompt,
                "sarvam" => crate::core::stt::sarvam::SarvamSTTConfig::from_standard(&c).prompt,
                other => panic!("no check for prompt on {other}; add one"),
            };
            assert_eq!(got.as_deref(), Some("Bud, WaaV"), "{v}");
        }
        for v in TEMPERATURE_VENDORS {
            let c = with("temperature", serde_json::json!(0.3));
            let got = match *v {
                "openai" => crate::core::stt::openai::OpenAISTTConfig::from_standard(&c)
                    .temperature
                    .map(f64::from),
                "groq" => crate::core::stt::groq::GroqSTTConfig::from_standard(&c)
                    .temperature
                    .map(f64::from),
                "elevenlabs" => {
                    crate::core::stt::elevenlabs::ElevenLabsBatchConfig::from_standard(&c)
                        .temperature
                }
                other => panic!("no check for temperature on {other}; add one"),
            };
            let got = got.unwrap_or_else(|| panic!("{v} dropped temperature"));
            assert!((got - 0.3).abs() < 1e-6, "{v}: {got}");
        }
    }

    #[test]
    fn a_vendor_that_cannot_use_prompt_or_temperature_says_so() {
        let s = TranscriptionSettings {
            prompt: Some("Bud".into()),
            temperature: Some(0.2),
            ..settings(F::Json)
        };
        let (cfg, adv) = apply("deepgram", &s);
        assert!(cfg.extras.0.get("prompt").is_none() && cfg.extras.0.get("temperature").is_none());
        assert!(
            adv.iter()
                .any(|w| w.contains("`prompt` is not applied by deepgram")),
            "{adv:?}"
        );
        assert!(
            adv.iter()
                .any(|w| w.contains("`temperature` is not applied by deepgram")),
            "{adv:?}"
        );
    }

    #[test]
    fn a_vendor_that_can_gets_them_without_a_warning() {
        let s = TranscriptionSettings {
            prompt: Some("Bud".into()),
            temperature: Some(0.2),
            ..settings(F::Json)
        };
        let (cfg, adv) = apply("openai", &s);
        assert_eq!(cfg.extras.0.get("prompt"), Some(&serde_json::json!("Bud")));
        // Through the decimal text: 0.2, not the widened f32 0.20000000298023224.
        assert_eq!(
            cfg.extras.0.get("temperature"),
            Some(&serde_json::json!(0.2))
        );
        assert!(adv.is_empty(), "{adv:?}");
    }

    #[test]
    fn the_openai_prompt_keeps_the_deployments_key_terms() {
        let mut c = std_for("openai");
        c.features = SttFeatures {
            keyterms: Some(vec!["Kubernetes".into(), "Dapr".into()]),
            ..Default::default()
        };
        c.extras
            .0
            .insert("prompt".into(), serde_json::json!("A platform talk."));
        let cfg = crate::core::stt::openai::OpenAISTTConfig::from_standard(&c);
        assert_eq!(
            cfg.prompt.as_deref(),
            Some("A platform talk. Kubernetes, Dapr")
        );
    }

    #[test]
    fn subtitle_formats_turn_word_timings_on_where_the_vendor_has_a_switch() {
        for format in [F::Srt, F::Vtt, F::VerboseJson] {
            let (cfg, _) = apply("openai", &settings(format));
            assert_eq!(cfg.features.word_timestamps, Some(true), "{format:?}");
        }
        let (cfg, _) = apply("openai", &settings(F::Json));
        assert_eq!(cfg.features.word_timestamps, None, "json needs no timings");
    }

    #[test]
    fn a_deployment_that_turned_timings_off_keeps_them_off_unless_words_are_asked_for() {
        let mut cfg = std_for("openai");
        cfg.features.word_timestamps = Some(false);
        let mut adv = Advisories::new();
        apply_request_stt_fields(&mut cfg, &settings(F::Srt), "openai", &mut adv);
        assert_eq!(cfg.features.word_timestamps, Some(false));

        let asked = TranscriptionSettings {
            timestamp_granularities: Some(vec![TimestampGranularity::Word]),
            ..settings(F::VerboseJson)
        };
        apply_request_stt_fields(&mut cfg, &asked, "openai", &mut adv);
        assert_eq!(cfg.features.word_timestamps, Some(true));
    }

    #[test]
    fn granularities_on_a_non_verbose_format_are_named_as_ignored() {
        let s = TranscriptionSettings {
            timestamp_granularities: Some(vec![TimestampGranularity::Word]),
            ..settings(F::Srt)
        };
        let (_, adv) = apply("deepgram", &s);
        assert!(
            adv.iter()
                .any(|w| w.contains("`timestamp_granularities` applies only")),
            "{adv:?}"
        );
    }

    fn word(w: &str, start: f64, end: f64) -> WordTiming {
        WordTiming {
            word: w.into(),
            start,
            end,
            confidence: None,
            speaker_id: None,
            logprob: None,
        }
    }

    #[test]
    fn segments_are_cut_from_the_vendors_words() {
        let words = [
            word("The", 0.0, 0.2),
            word("quick", 0.2, 0.5),
            word("fox.", 0.5, 0.9),
            word("It", 1.0, 1.1),
            word("ran.", 1.1, 1.4),
        ];
        let mut adv = Advisories::new();
        let segs = transcript_segments(
            "The quick fox. It ran.",
            &words,
            Some(1.5),
            &settings(F::Srt),
            "deepgram",
            &mut adv,
        );
        assert_eq!(
            segs.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(),
            vec!["The quick fox.", "It ran."]
        );
        assert!(adv.is_empty());
    }

    #[test]
    fn text_without_word_timings_is_one_segment_and_the_caller_is_told() {
        let mut adv = Advisories::new();
        let segs = transcript_segments(
            "Hello there.",
            &[],
            Some(2.5),
            &settings(F::Vtt),
            "yandex",
            &mut adv,
        );
        assert_eq!(segs.len(), 1);
        assert_eq!((segs[0].start, segs[0].end), (0.0, 2.5));
        assert_eq!(segs[0].text, "Hello there.");
        assert!(
            adv.as_slice()
                .iter()
                .any(|w| w.contains("yandex returned no word timings")),
            "{:?}",
            adv.as_slice()
        );
    }

    /// The wiring, not just the cutter: before this, `render_transcription` wrote a literal empty
    /// `segments` and the subtitle renderers read a result nobody filled.
    #[tokio::test]
    async fn srt_vtt_and_verbose_json_carry_the_segments() {
        use waav_openai_audio::transcription::{Segment, TranscriptionResult};
        let result = TranscriptionResult {
            text: "Hello there.".into(),
            language: Some("en".into()),
            duration: Some(1.0),
            segments: vec![Segment {
                id: 0,
                start: 0.0,
                end: 0.9,
                text: "Hello there.".into(),
            }],
        };
        let render = |f: F| {
            super::render_transcription(&f, false, &result, &Advisories::new(), &[], &[], &[])
        };
        let text = |r: axum::response::Response| async {
            let b = axum::body::to_bytes(r.into_body(), usize::MAX)
                .await
                .unwrap();
            String::from_utf8(b.to_vec()).unwrap()
        };
        assert_eq!(
            text(render(F::Srt)).await,
            "1\n00:00:00,000 --> 00:00:00,900\nHello there.\n\n"
        );
        assert!(
            text(render(F::Vtt))
                .await
                .contains("00:00:00.000 --> 00:00:00.900\nHello there.")
        );
        let v: serde_json::Value =
            serde_json::from_str(&text(render(F::VerboseJson)).await).unwrap();
        assert_eq!(v["segments"][0]["text"], "Hello there.");
        assert_eq!(v["segments"][0]["end"], 0.9);
        assert_eq!(v["task"], "transcribe");
    }

    #[tokio::test]
    async fn the_translations_route_labels_its_task() {
        use waav_openai_audio::transcription::TranscriptionResult;
        let result = TranscriptionResult {
            text: "Hello.".into(),
            ..Default::default()
        };
        let r = super::render_transcription(
            &F::VerboseJson,
            true,
            &result,
            &Advisories::new(),
            &[],
            &[],
            &[],
        );
        let b = axum::body::to_bytes(r.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&b).unwrap();
        assert_eq!(v["task"], "translate");
    }

    #[test]
    fn silence_is_no_segments_and_no_warning() {
        let mut adv = Advisories::new();
        let segs = transcript_segments("  ", &[], Some(3.0), &settings(F::Srt), "x", &mut adv);
        assert!(segs.is_empty() && adv.is_empty());
    }
}
