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

use crate::core::voice_cost::voice_cost;
use crate::core::voice_error::{VoiceErrorType, VoiceFailure};
use crate::observability::vendor_span;
use crate::observability::voice_attrs;
use crate::observability::voice_span::{FormCapture, Root, RootSpan, VoiceSpans};
use crate::state::{AppState, ResolvedVoiceEndpoint};

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

/// Mark the call failed and answer with OpenAI's error envelope, in one step, so no failure path
/// after endpoint resolution can do one without the other (FRD-021 FR-4). Before, three STT
/// paths returned an error and left the turn `Unset` — counted as successes.
fn fail_call(
    spans: &VoiceSpans,
    class: VoiceErrorType,
    vendor_status: Option<u16>,
    status: StatusCode,
    kind: &str,
    message: String,
    param: Option<&str>,
) -> Response {
    spans.fail(class, vendor_status, &message);
    openai_error(status, kind, message, param)
}

/// [`translation_error`] for a request whose turn is already open: a Bud-side validation failure
/// after resolution is still a failed call.
fn fail_translation(spans: &VoiceSpans, class: VoiceErrorType, err: &AudioError) -> Response {
    spans.fail(class, None, &err.to_string());
    translation_error(err)
}

/// Open the call's `voice.turn` and record who it is for (FRD-021 §6.1).
///
/// * `bud.endpoint_id` — the endpoint UUID, never the alias; `bud.voice.endpoint_name` — the
///   alias the caller sent.
/// * `bud.model_id` and `bud.project_id` — from the allowlist entry that reached the endpoint
///   (the key's own, or the published overlay's for a customer key), i.e. the ENDPOINT's model and
///   project, whether it was named by alias or by id. `bud.project_id` falls back to the
///   principal's only for an entry that carries no project.
/// * `bud.api_key_project_id` — the project of the key that made the call: what this span called
///   `bud.project_id` before FRD-021, and what customer scoping keys on.
///
/// Attribution was always resolvable — `resolve_principal` reads the in-memory snapshot — so the
/// ids are recorded before any work that can fail. `recordable` filters `Some("")`: an empty
/// string is not NULL, and would make the column look populated.
async fn open_turn(
    state: &AppState,
    capability: &'static str,
    resolved: &ResolvedVoiceEndpoint,
    endpoint_name: &str,
    bearer: Option<&str>,
    root: &Root,
    (vendor_key, model_key): (&'static str, &'static str),
) -> VoiceSpans {
    use voice_attrs::turn;
    use waav_openai_audio::recordable;

    let spans = VoiceSpans::open(capability, root);
    spans.record_text(turn::ENDPOINT_ID, Some(resolved.endpoint_id.as_str()));
    spans.record_text(turn::ENDPOINT_NAME, Some(endpoint_name));
    let alias = resolved.alias.as_ref();
    spans.record_text(turn::MODEL_ID, alias.and_then(|a| a.model_id.as_deref()));

    let principal = state.resolve_principal(bearer).await;
    let key_project = principal
        .as_ref()
        .and_then(|p| recordable(p.project_id.as_deref()));
    let endpoint_project = alias.and_then(|a| recordable(a.project_id.as_deref()));
    spans.record_text(turn::PROJECT_ID, endpoint_project.or(key_project));
    spans.record_text(turn::API_KEY_PROJECT_ID, key_project);
    spans.record_text(
        turn::USER_ID,
        principal.as_ref().and_then(|p| p.user_id.as_deref()),
    );
    spans.record_text(
        turn::API_KEY_ID,
        principal.as_ref().and_then(|p| p.api_key_id.as_deref()),
    );
    spans.record_vendor(
        vendor_key,
        model_key,
        &resolved.endpoint.vendor,
        resolved.endpoint.model.as_deref(),
    );
    spans
}

/// `POST /v1/audio/speech`
pub async fn speech_handler(
    State(state): State<Arc<AppState>>,
    root: Option<axum::Extension<RootSpan>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let root = Root::from_extension(root);
    // FRD-021 §6.8: the request as the caller sent it. The response is audio and is not
    // captured; an error response is, as the JSON the caller was shown.
    if root.captures() {
        root.record_request_body(&String::from_utf8_lossy(&body));
    }
    let response = speech_inner(state, &root, headers, body).await;
    root.capture_response(response, false).await
}

async fn speech_inner(
    state: Arc<AppState>,
    root: &Root,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    use voice_attrs::{leg, resilience, turn};

    // The bearer resolves the caller's alias map, which is both the alias -> endpoint id
    // mapping and the authorization boundary. Auth has already passed by the time we get here;
    // this is resolution, not a second check.
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.trim().strip_prefix("Bearer ").unwrap_or(v).to_string());

    // Refusals before the endpoint resolves are not calls: no `voice.turn` (DEG-2). They are
    // failed HTTP requests on the root, which `request_id_middleware` records.
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

    let Some(resolved) =
        state.resolve_voice_endpoint(&settings.endpoint, "text_to_speech", bearer.as_deref())
    else {
        return model_not_found(&settings.endpoint, "text_to_speech");
    };

    // FRD-022 §6.2: the deployment's own rate limits and concurrency cap, before any vendor
    // work. A refusal here is not a call either — nothing was attempted — so it opens no turn.
    // The admission (and with it the concurrency slot) lives until the response is built.
    let admission = match state.admit_deployment(&resolved.endpoint_id).await {
        Ok(a) => a,
        Err(rejection) => {
            info!(
                endpoint = %settings.endpoint,
                retry_after_s = rejection.retry_after().as_secs(),
                "speech request refused by the deployment's own limits"
            );
            return rejection.into_response();
        }
    };

    // FRD-021: the turn opens HERE, as soon as there is an endpoint to attribute it to, so every
    // failure after this point — a bad override, a voice the vendor lacks, a missing credential —
    // is a failed call with a class, not a request that never happened. It used to open after
    // all of them, and those failures reached no fact row at all.
    //
    // NOT `turn_span.enter()` anywhere below. A span guard held across an `.await` attaches the
    // span to whatever task the executor resumes next, so the attributes land on someone else's
    // work. `.instrument()` on the future is the async-correct form.
    let spans = open_turn(
        &state,
        "text_to_speech",
        &resolved,
        &settings.endpoint,
        bearer.as_deref(),
        root,
        (leg::TTS_VENDOR, leg::TTS_MODEL),
    )
    .await;
    spans.record_text(
        resilience::RATE_LIMIT_OUTCOME,
        Some(if admission.headers.is_some() {
            "allow"
        } else {
            "unlimited"
        }),
    );
    let ResolvedVoiceEndpoint {
        endpoint_id,
        endpoint,
        ..
    } = resolved;

    let mut advisories = Advisories::new();
    warn_unrecognised(&settings.unrecognised, &mut advisories);

    let plan = match plan_speech(
        &state,
        &settings.endpoint,
        &endpoint,
        &settings,
        VoicePick::Primary,
        &mut advisories,
    )
    .await
    {
        Ok(plan) => plan,
        Err(refusal) => return refusal.fail_on(&spans),
    };
    // The voice the synthesis runs with, whoever chose it (FRD-021 §6.1, Phase 5).
    spans.record_text(leg::TTS_VOICE, plan.voice.as_deref());
    // Recorded only when there is one: `""` is not NULL (FRD-021 GT-12).
    spans.record_text(turn::LANGUAGE, plan.language.as_deref());

    // `characters` is the billing dimension for synthesis and is recorded on SUCCESS only: set
    // at creation it counted every refused request — a voice the account lacks, text the vendor
    // would not speak — as characters synthesised. It is the text the caller sent, BEFORE
    // pronunciation replacement: what the customer is billed for (FRD-021 §6.4).
    let chars = settings.text.chars().count();
    info!(
        endpoint = %settings.endpoint,
        vendor = %endpoint.vendor,
        format = settings.format.as_str(),
        chars,
        "openai audio/speech"
    );

    let started = std::time::Instant::now();

    // FRD-022 §6.3–6.5: retries on the deployment, the circuit breakers, and the fallback chain,
    // all inside one deadline.
    let mut served = match synthesize_resiliently(
        &state,
        &settings,
        &endpoint_id,
        &endpoint,
        plan,
        &mut advisories,
        &spans,
    )
    .await
    {
        Ok(served) => served,
        Err(failure) => return failure,
    };
    let elapsed_ms = started.elapsed().as_millis() as u64;
    let sample_rate = served.sample_rate;

    // Packaged BEFORE anything billable is recorded: a clip that cannot be served is a failed
    // call, and units or cost on a failure would bill it (FRD-021 TC-EMIT-04).
    let audio = std::mem::take(&mut served.audio);
    let packaged = match serve_as_requested(settings.format, audio, sample_rate) {
        Ok(packaged) => packaged,
        Err(e) => {
            warn!(endpoint = %settings.endpoint, error = %e, "could not package audio");
            return fail_call(
                &spans,
                VoiceErrorType::Internal,
                None,
                StatusCode::BAD_GATEWAY,
                "api_error",
                e,
                None,
            );
        }
    };
    if let Some(actual) = packaged.substituted {
        advisories.warn(format!(
            "requested {} audio, but {} returned {actual}; served as {actual}",
            settings.format.as_str(),
            served.vendor
        ));
    }

    // The success-only record: duration, units, and what they cost (FRD-021 §6.1, §6.4) — at
    // the SERVED deployment's price, so a turn a fallback served is billed as the fallback's
    // (FRD-022 §6.4).
    let served_format = packaged.format_label.unwrap_or(settings.format.as_str());
    let (output_secs, output_rate) =
        output_audio_meta(&packaged.bytes, settings.format, sample_rate);
    spans.record(leg::TTS_DURATION_MS, elapsed_ms);
    if let Some(ttfb) = served.ttfb {
        spans.record(leg::TTS_TTFB_MS, ttfb.as_secs_f64() * 1000.0);
    }
    spans.record(turn::CHARACTERS, chars as u64);
    if let Some(secs) = output_secs {
        spans.record(turn::OUTPUT_AUDIO_SECONDS, secs);
    }
    spans.record_text(turn::AUDIO_FORMAT, Some(served_format));
    if let Some(rate) = output_rate {
        spans.record(turn::SAMPLE_RATE, i64::from(rate));
    }
    spans.record_cost(voice_cost(
        served.pricing.as_ref(),
        "text_to_speech",
        Some(chars as u64),
        None,
        output_secs,
    ));

    let audio = packaged.bytes;
    let mut headers = HeaderMap::new();
    if let Ok(ct) = packaged.content_type.parse() {
        headers.insert(header::CONTENT_TYPE, ct);
    }
    if let Ok(v) = packaged
        .format_label
        .unwrap_or(served.format.as_str())
        .parse()
    {
        headers.insert("x-audio-format", v);
    }
    // Raw samples carry no container, so the rate has to travel out of band or the
    // caller cannot play what they were sent.
    if matches!(settings.format, AudioFormat::Pcm)
        && let Ok(v) = sample_rate.to_string().parse()
    {
        headers.insert("x-sample-rate", v);
    }
    served.apply_headers(&mut headers);
    admission.apply(&mut headers);
    // W1. This response is audio bytes, so a header is the ONLY channel there is; a
    // body-carried advisory would be unreachable on this route by construction.
    advisories.apply(&mut headers);
    (StatusCode::OK, headers, audio).into_response()
}

/// Why a deployment cannot take the request as sent, and what the caller is told when that
/// deployment is the one they named. A fallback that cannot take it is skipped instead.
struct PlanRefusal {
    /// The failed call's class and message; `None` for the refusals that record none.
    failure: Option<VoiceFailure>,
    response: Response,
}

impl PlanRefusal {
    fn classified(class: VoiceErrorType, message: impl Into<String>, response: Response) -> Self {
        Self {
            failure: Some(VoiceFailure::new(class, message)),
            response,
        }
    }

    fn unclassified(response: Response) -> Self {
        Self {
            failure: None,
            response,
        }
    }

    /// Mark the call failed (when the refusal is classified) and answer with the refusal.
    fn fail_on(self, spans: &VoiceSpans) -> Response {
        if let Some(f) = &self.failure {
            spans.fail(f.class, f.vendor_status, &f.message);
        }
        self.response
    }
}

/// Everything decided about ONE deployment before its vendor is called.
struct SpeechPlan {
    std_config: crate::core::tts::standard::StandardTTSConfig,
    /// The deployment's `request_timeout`, the deadline of the whole chain when it is the
    /// primary's.
    deadline: Option<std::time::Duration>,
    voice: Option<String>,
    voice_for_errors: String,
    voice_origin: VoiceOrigin,
    /// The canonical language the request resolved to (for the span).
    language: Option<String>,
}

/// Where a hop's voice comes from.
#[derive(Clone, Copy)]
enum VoicePick<'a> {
    /// The deployment the caller named: request, then deployment, then description, then default.
    Primary,
    /// A fallback deployment (FRD-022 §6.4). A voice id is vendor-specific, so the caller's voice
    /// is used only on the same vendor; otherwise the primary's voice DESCRIPTION is matched in
    /// the fallback vendor's catalog; otherwise the fallback's own configured voice.
    Fallback {
        caller_voice: Option<&'a str>,
        primary: &'a bud_auth::credentials::VoiceEndpoint,
    },
}

fn same_vendor(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// Build the synthesis request for one deployment. `Err` is the refusal the CALLER should see
/// for the primary; a fallback that cannot take the request is skipped instead.
async fn plan_speech(
    state: &Arc<AppState>,
    label: &str,
    endpoint: &bud_auth::credentials::VoiceEndpoint,
    settings: &speech::SpeechSettings,
    pick: VoicePick<'_>,
    advisories: &mut Advisories,
) -> Result<SpeechPlan, PlanRefusal> {
    let primary = matches!(pick, VoicePick::Primary);

    // The request's own speech settings, laid over the deployment's. Merged here, before any
    // vendor work, so a value outside the canonical vocabulary is a 400 and not a wasted call;
    // everything downstream reads the merged copy exactly as it read the saved one.
    let mut tts_overridden = endpoint.config.tts().into_owned();
    if let Err((field, reason)) =
        settings_map::apply_speech_overrides(&mut tts_overridden, &settings.overrides)
    {
        let message = format!("`{field}`: {reason}");
        return Err(PlanRefusal::classified(
            VoiceErrorType::InvalidRequest,
            message.clone(),
            openai_error(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                message,
                Some(field),
            ),
        ));
    }
    if primary && settings.overrides.sample_rate.is_some() && !settings.format.accepts_sample_rate()
    {
        advisories.warn(format!(
            "`sample_rate` applies only to response_format=pcm and was ignored for {}",
            settings.format.as_str()
        ));
    }

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
    //
    // A default is picked only for a vendor whose API REQUIRES a voice. Where the voice is
    // optional (Deepgram, Google) nothing is sent and the vendor applies its own default: a voice
    // WaaV chose would be one neither the caller nor the operator asked for.
    let (requested, cross_vendor_description) = match pick {
        VoicePick::Primary => (settings.voice.as_deref(), None),
        VoicePick::Fallback {
            caller_voice,
            primary: p,
        } => {
            let same = same_vendor(&p.vendor, &endpoint.vendor);
            let described = p
                .config
                .tts()
                .voice_descriptor
                .clone()
                .filter(|d| !d.is_empty());
            (
                caller_voice.filter(|_| same),
                if same { None } else { described },
            )
        }
    };
    let chosen = match cross_vendor_description.as_ref() {
        // FRD-022 §6.4 step 2: the primary's DESCRIPTION, matched in this vendor's catalog.
        Some(d) if requested.is_none() => {
            resolve_described_voice_with(state, endpoint, Some(d), advisories)
                .await
                .map(|v| (Some(v), VoiceOrigin::Described))
        }
        _ => None,
    };
    let (voice, voice_origin) = match chosen {
        Some(chosen) => chosen,
        None => match settings_map::resolve_voice(requested, endpoint.voice.as_deref()) {
            Some(v) if requested.is_some() => (Some(v), VoiceOrigin::Request),
            Some(v) => (Some(v), VoiceOrigin::Deployment),
            None => match resolve_described_voice(state, endpoint, advisories).await {
                Some(v) => (Some(v), VoiceOrigin::Described),
                None if !crate::handlers::voices::voice_required(&endpoint.vendor) => {
                    advisories.warn(format!(
                        "deployment '{label}' has no voice configured, so {}'s own default voice \
                         was used. Set a voice in the deployment's audio settings, or pass one in \
                         `voice`, to choose it.",
                        endpoint.vendor
                    ));
                    (None, VoiceOrigin::Vendor)
                }
                None => match default_voice(state, endpoint, label, advisories).await {
                    Some(v) => (Some(v), VoiceOrigin::Default),
                    None => {
                        return Err(PlanRefusal::classified(
                            VoiceErrorType::InvalidRequest,
                            "`voice` is required: no voice was named, configured or defaulted",
                            no_voice_error(label, &endpoint.vendor),
                        ));
                    }
                },
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
    if let Some(voice) = voice.as_deref()
        && let Err(e) = speech::validate_voice(voice, known_voices_for(&endpoint.vendor))
    {
        return Err(PlanRefusal::classified(
            VoiceErrorType::InvalidRequest,
            e.to_string(),
            translation_error(&e),
        ));
    }

    // A Google deployment names a voice FAMILY (`chirp-3-hd`, `wavenet`…), but Google's API has
    // no family parameter: the family is only in the voice name. With no voice, Google picks one
    // by language from any family — usually Standard — so the deployment's choice was replaced
    // and the request billed at the family's rate (Chirp 3 HD is ~7x Standard). A voice from
    // another family does the same. Both are refused, naming the pattern the voice must follow.
    if let Some(refusal) = google_family_refusal(
        &endpoint.vendor,
        endpoint.model.as_deref().unwrap_or_default(),
        voice.as_deref(),
        voice_origin,
        label,
    ) {
        return Err(PlanRefusal::unclassified(refusal));
    }

    // Refuse a format the vendor cannot produce BEFORE spending a vendor call on it. The
    // alternative was raw samples served under the codec's content type, which nothing can play.
    // (On a fallback hop this skips the hop: FRD-022 §6.4 "Format".)
    if let Some(supported) = vendor_output_formats(&endpoint.vendor)
        && !supported.contains(&settings.format)
    {
        let names: Vec<&str> = supported.iter().map(|f| f.as_str()).collect();
        let message = format!(
            "{} cannot produce {} audio; `response_format` must be one of {}",
            endpoint.vendor,
            settings.format.as_str(),
            names.join(", ")
        );
        return Err(PlanRefusal::classified(
            VoiceErrorType::InvalidRequest,
            message.clone(),
            openai_error(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                message,
                Some("response_format"),
            ),
        ));
    }

    // Accepted fields this route does not apply, said out loud. Each returned 200 audio that
    // silently ignored what the caller asked for.
    if primary
        && settings
            .instructions
            .as_deref()
            .is_some_and(|i| !i.trim().is_empty())
    {
        advisories
            .warn("`instructions` is not applied on /v1/audio/speech and was ignored".to_string());
    }
    if primary
        && settings.speaking_rate.is_some()
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
        let message = format!(
            "Endpoint '{label}' has no credential configured for vendor '{}'",
            endpoint.vendor
        );
        return Err(PlanRefusal::classified(
            VoiceErrorType::Config,
            message.clone(),
            openai_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "api_error",
                message,
                None,
            ),
        ));
    }
    if let Some(refusal) = endpoint_misconfiguration(endpoint, label, advisories) {
        return Err(PlanRefusal::unclassified(refusal));
    }

    // The request's own `language` (a Bud per-request override), then the tts block's, then the
    // endpoint default.
    let language = settings_map::resolve_language(
        settings.overrides.language.as_deref(),
        endpoint.language.as_deref(),
        endpoint.config.tts().language.as_deref(),
    );

    let mut tts_settings = tts_overridden;

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
    if primary && tts_settings.connection_timeout.is_some() {
        advisories.warn(
            "connection_timeout is not applied: connections to the vendor are pooled and shared"
                .to_string(),
        );
    }
    let deadline = tts_settings
        .request_timeout
        .map(std::time::Duration::from_secs);
    let voice_for_errors = voice.clone().unwrap_or_default();
    let model = vendor_model(state, endpoint, voice.as_deref()).await;

    let mut tts_config = crate::core::tts::TTSConfig {
        provider: endpoint.vendor.clone(),
        api_key,
        voice_id: voice.clone(),
        model,
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
        advisories,
    );

    // C1/C3: the synthesis language, mapped into the vendor's own notation. OpenAI's schema has
    // no language field; this one is Bud's per-request override, else the deployment's choice.
    //
    // Mapped with the vendor's TTS mapper. A few vendors spell a language differently for
    // synthesis than for recognition, and the bare vendor name selects the STT one: Google TTS
    // was sent `cmn-Hans-CN`, its recogniser's notation. The WebSocket path already did this.
    let mapped_language = language.as_deref().and_then(|canonical| {
        settings_map::map_language_for(
            canonical,
            &super::ws::config::tts_provider_alias(&endpoint.vendor),
            &tts_config.model,
            advisories,
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
    let mut std_config = settings_map::standard_tts(
        tts_config,
        &tts_settings,
        mapped_language.as_deref(),
        advisories,
    );
    std_config.extras.0.extend(deployment_extras(endpoint));

    Ok(SpeechPlan {
        std_config,
        deadline,
        voice,
        voice_for_errors,
        voice_origin,
        language,
    })
}

/// Audio a deployment produced, and which deployment it was.
struct SpeechServed {
    audio: Vec<u8>,
    format: String,
    sample_rate: u32,
    ttfb: Option<std::time::Duration>,
    served_id: String,
    vendor: String,
    /// The SERVED deployment's pricing: a fallback's turn costs what the fallback costs.
    pricing: Option<bud_auth::credentials::VoicePricing>,
    fell_back: bool,
    voice_substituted: bool,
    bud_mode: bool,
    /// The fallback's own admission (its concurrency slot) — held until the response is built.
    _fallback_admission: Option<crate::core::deployment_policy::Admission>,
}

impl SpeechServed {
    /// `x-bud-endpoint-id` (the SERVED deployment), and on a fallback `x-bud-fallback` and
    /// `x-bud-voice-substituted` (FRD-022 §6.4).
    fn apply_headers(&self, headers: &mut HeaderMap) {
        use crate::core::deployment_policy::{
            FALLBACK_HEADER, SERVED_ENDPOINT_HEADER, VOICE_SUBSTITUTED_HEADER,
        };
        if self.bud_mode
            && let Ok(v) = header::HeaderValue::from_str(&self.served_id)
        {
            headers.insert(SERVED_ENDPOINT_HEADER, v);
        }
        if self.fell_back {
            headers.insert(FALLBACK_HEADER, header::HeaderValue::from_static("true"));
            headers.insert(
                VOICE_SUBSTITUTED_HEADER,
                header::HeaderValue::from_static(if self.voice_substituted {
                    "true"
                } else {
                    "false"
                }),
            );
        }
    }
}

/// The message of a hop that ran out of the chain's deadline — WaaV's own bound, told apart from
/// a vendor or provider timeout by exactly this text.
const CHAIN_DEADLINE_ELAPSED: &str = "the request deadline elapsed";

/// The class a refusal records when no attempt produced a failure of its own: only a failing
/// vendor opens a breaker.
const BREAKER_OPEN_CLASS: VoiceErrorType = VoiceErrorType::Vendor5xx;

/// Synthesise on the primary with its retry policy, behind the circuit breakers, falling back
/// through its fallback chain on a failover-eligible failure — all within one deadline
/// (FRD-022 §6.3–6.5). `Err` is the response to send; the turn is marked failed with it.
async fn synthesize_resiliently(
    state: &Arc<AppState>,
    settings: &speech::SpeechSettings,
    primary_id: &str,
    primary: &bud_auth::credentials::VoiceEndpoint,
    primary_plan: SpeechPlan,
    advisories: &mut Advisories,
    spans: &VoiceSpans,
) -> Result<SpeechServed, Response> {
    use crate::core::deployment_policy::{
        DEFAULT_SPEECH_DEADLINE, breaker_open_response, vendor_key,
    };
    use crate::handlers::speak::SynthesisError;
    use resil::retry::{RetryPolicy, retry};
    use voice_attrs::{leg, resilience};

    const CAPABILITY: &str = "text_to_speech";
    let policies = state.policies.clone();
    let chain_limit = primary_plan.deadline.unwrap_or(DEFAULT_SPEECH_DEADLINE);
    let deadline = tokio::time::Instant::now() + chain_limit;
    let candidates: Vec<Arc<str>> = resil::fallback::expand(
        primary_id,
        |id| {
            if id == primary_id {
                primary.policy.fallback_models.clone()
            } else {
                state
                    .fallback_voice_endpoint(id, CAPABILITY)
                    .map(|e| e.policy.fallback_models)
                    .unwrap_or_default()
            }
        },
        1 + resil::policy::MAX_FALLBACKS * 2,
    );

    let primary_voice = primary_plan.voice.clone();
    let primary_voice_for_errors = primary_plan.voice_for_errors.clone();
    let primary_origin = primary_plan.voice_origin;
    let mut primary_plan = Some(primary_plan);
    let mut retries_total = 0u32;
    let mut primary_error: Option<SynthesisError> = None;
    let mut primary_breaker: Option<resil::breaker::Open> = None;
    let mut fallback_limited: Option<std::time::Duration> = None;
    let mut fallback_attempted = false;
    let mut timed_out = false;

    for (hop, id) in candidates.iter().enumerate() {
        if tokio::time::Instant::now() >= deadline {
            timed_out = true;
            break;
        }
        // The hop's deployment, its own admission and its plan.
        let (endpoint, plan, hop_admission) = if hop == 0 {
            let Some(plan) = primary_plan.take() else {
                break;
            };
            (primary.clone(), plan, None)
        } else {
            let Some(endpoint) = state.fallback_voice_endpoint(id, CAPABILITY) else {
                warn!(fallback = %id, "fallback deployment unknown or not a TTS deployment; skipped");
                continue;
            };
            let admission = match state.admit_deployment(id).await {
                Ok(a) => a,
                Err(rejection) => {
                    let wait = rejection.retry_after();
                    fallback_limited = Some(fallback_limited.map_or(wait, |w| w.min(wait)));
                    info!(fallback = %id, "fallback skipped: its own limit said no");
                    continue;
                }
            };
            let mut hop_advisories = Advisories::new();
            let plan = match plan_speech(
                state,
                id,
                &endpoint,
                settings,
                VoicePick::Fallback {
                    caller_voice: settings.voice.as_deref(),
                    primary,
                },
                &mut hop_advisories,
            )
            .await
            {
                Ok(p) => p,
                Err(_) => {
                    info!(fallback = %id, "fallback skipped: it cannot serve this request");
                    continue;
                }
            };
            fallback_attempted = true;
            (endpoint, plan, Some(admission))
        };

        let vkey = vendor_key(&endpoint.vendor, endpoint.api_base.as_deref());
        if let Some(p) = &policies
            && let Err(open) = p.breakers().check(id, &vkey)
        {
            info!(deployment = %id, retry_in_ms = open.retry_in.as_millis() as u64, "circuit open; hop skipped");
            if hop == 0 {
                primary_breaker = Some(open);
            }
            continue;
        }

        let retry_policy = endpoint
            .policy
            .retry_config
            .as_ref()
            .map(RetryPolicy::interactive)
            .unwrap_or_else(RetryPolicy::none);
        let std_config = plan.std_config;
        let text = settings.text.as_str();
        let outcome = retry(
            &retry_policy,
            Some(deadline),
            |_| {
                let cfg = std_config.clone();
                async move {
                    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                    // Each attempt runs in the call's scope, so its CLIENT span is a child of
                    // `voice.turn` carrying the call's ids (CONTRACTS §1.2a): a retry is a second
                    // vendor span, not a longer first one.
                    let synthesis = spans
                        .vendor_scope(vendor_span::operation::TEXT_TO_SPEECH)
                        .run(crate::handlers::speak::synthesize_once_standard(
                            state, cfg, text,
                        ))
                        .instrument(spans.turn().clone());
                    match tokio::time::timeout(remaining, synthesis).await {
                        Ok(result) => result,
                        Err(_) => Err(SynthesisError::Failed(VoiceFailure::new(
                            VoiceErrorType::Deadline,
                            format!("synthesis timed out: {CHAIN_DEADLINE_ELAPSED}"),
                        ))),
                    }
                }
            },
            |e: &SynthesisError| e.verdict(),
        )
        .await;
        retries_total += outcome.retries;

        match outcome.result {
            Ok(synth) => {
                if let Some(p) = &policies {
                    p.breakers().record_success(id, &vkey);
                }
                spans.record(resilience::RETRY_COUNT, u64::from(retries_total));
                spans.record_text(resilience::SERVED_ENDPOINT_ID, Some(&**id));
                if hop > 0 {
                    spans.record_text(resilience::FALLBACK_FROM, Some(primary_id));
                    // The leg is the fallback's: its vendor, model and voice.
                    spans.record_vendor(
                        leg::TTS_VENDOR,
                        leg::TTS_MODEL,
                        &endpoint.vendor,
                        endpoint.model.as_deref(),
                    );
                    spans.record_text(leg::TTS_VOICE, plan.voice.as_deref());
                    advisories.warn(format!(
                        "served by fallback deployment '{id}' ({}) because '{}' failed",
                        endpoint.vendor, settings.endpoint
                    ));
                }
                return Ok(SpeechServed {
                    audio: synth.audio,
                    format: synth.format,
                    sample_rate: synth.sample_rate,
                    ttfb: synth.ttfb,
                    served_id: id.to_string(),
                    vendor: endpoint.vendor.clone(),
                    pricing: endpoint.pricing.clone(),
                    fell_back: hop > 0,
                    voice_substituted: hop > 0 && plan.voice != primary_voice,
                    bud_mode: state.bud_mode.is_some(),
                    _fallback_admission: hop_admission,
                });
            }
            Err(e) => {
                let verdict = e.verdict();
                if let Some(p) = &policies {
                    p.breakers().record_failure(id, &vkey, &verdict);
                }
                if verdict.vendor_concurrency {
                    warn!(
                        deployment = %id,
                        "the vendor refused for concurrency: this deployment's max_concurrent is \
                         above the vendor plan; lower it"
                    );
                }
                warn!(deployment = %id, hop, error = %e, "synthesis failed");
                if hop == 0 {
                    let surface = !verdict.failover;
                    primary_error = Some(e);
                    // A caller error would fail on every fallback too, and a config error such
                    // as an invalid voice must surface rather than be masked (FRD-022 §6.4).
                    if surface {
                        break;
                    }
                }
            }
        }
    }

    spans.record(resilience::RETRY_COUNT, u64::from(retries_total));
    let fail = |failure: &VoiceFailure| {
        spans.fail(failure.class, failure.vendor_status, &failure.message);
    };
    let deadline_exceeded = || {
        let m = format!(
            "synthesis exceeded deployment '{}''s request_timeout of {}s",
            settings.endpoint,
            chain_limit.as_secs()
        );
        warn!(endpoint = %settings.endpoint, "{m}");
        spans.fail(VoiceErrorType::Deadline, None, &m);
        openai_error(StatusCode::GATEWAY_TIMEOUT, "api_error", m, None)
    };
    match primary_error {
        Some(SynthesisError::Rejected(failure)) => {
            fail(&failure);
            Err(rejection_error(
                failure.message,
                &primary_voice_for_errors,
                primary_origin,
                &primary.vendor,
                &settings.endpoint,
            ))
        }
        // The primary could not serve (it failed over, or its breaker is open) and every
        // fallback was held back by its own limit: say when a fallback can take it (TC-WR-12).
        _ if fallback_limited.is_some()
            && !fallback_attempted
            && (primary_breaker.is_some()
                || primary_error.as_ref().is_some_and(|e| e.verdict().failover)) =>
        {
            match primary_error.as_ref() {
                Some(e) => fail(e.failure()),
                None => spans.fail(
                    BREAKER_OPEN_CLASS,
                    None,
                    "circuit breaker open, and every fallback was held back by its own limit",
                ),
            }
            Err(
                crate::core::deployment_policy::Rejection::Rate(resil::RateHeaders {
                    limit: 0,
                    remaining: 0,
                    reset: 0,
                    retry_after: fallback_limited.map(|d| d.as_secs().max(1)),
                })
                .into_response(),
            )
        }
        Some(SynthesisError::Vendor {
            failure,
            status: 429,
            retry_after,
        }) => {
            // A vendor 429 that survived retries and fallback is a 429 with Retry-After, not a
            // 502 (FRD-022 §6.6, TC-WR-07).
            fail(&failure);
            let mut resp = openai_error(
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limit_error",
                failure.message,
                None,
            );
            let secs = retry_after
                .or(fallback_limited)
                .map(|d| d.as_secs().max(1))
                .unwrap_or(1);
            resp.headers_mut()
                .insert(header::RETRY_AFTER, header::HeaderValue::from(secs));
            Err(resp)
        }
        Some(SynthesisError::Saturated(failure)) => {
            fail(&failure);
            let mut resp = openai_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "api_error",
                failure.message,
                None,
            );
            resp.headers_mut()
                .insert(header::RETRY_AFTER, header::HeaderValue::from(1u64));
            Err(resp)
        }
        Some(e) if timed_out || e.failure().message.contains(CHAIN_DEADLINE_ELAPSED) => {
            Err(deadline_exceeded())
        }
        Some(e) => {
            fail(e.failure());
            // 502, not 500: the failure is upstream of WaaV, and the distinction is what tells
            // an operator whether to look at the vendor or at us.
            Err(openai_error(
                StatusCode::BAD_GATEWAY,
                "api_error",
                e.failure().message.clone(),
                None,
            ))
        }
        None => match primary_breaker {
            Some(open) => {
                spans.fail(BREAKER_OPEN_CLASS, None, "circuit breaker open");
                Err(breaker_open_response(open))
            }
            None => Err(deadline_exceeded()),
        },
    }
}

/// How long the audio a synthesis produced lasts, and at what rate — where that is known WITHOUT
/// decoding (FRD-021 §6.1, WP-5.2).
///
/// * a WAV container → both from its header;
/// * raw PCM (what `response_format=pcm` serves) → `bytes / (rate × 2)`: WaaV's canonical PCM is
///   16-bit little-endian mono (`linear16`), the shape `serve_as_requested` also assumes;
/// * a compressed container → neither. Its duration needs a demuxer (Q-2 is undecided), and a
///   guessed number in a billing input is worse than none (DEG-4).
fn output_audio_meta(
    bytes: &[u8],
    requested: AudioFormat,
    sample_rate: u32,
) -> (Option<f64>, Option<u32>) {
    use crate::core::tts::sniff::{SniffedContainer, sniff_container};
    match sniff_container(bytes) {
        Some(SniffedContainer::Wav) => (
            waav_openai_audio::pcm::wav_duration_secs(bytes),
            wav_sample_rate(bytes),
        ),
        None if matches!(requested, AudioFormat::Pcm) && sample_rate > 0 => (
            Some(bytes.len() as f64 / (f64::from(sample_rate) * 2.0)),
            Some(sample_rate),
        ),
        _ => (None, None),
    }
}

/// A WAV file's sample rate, from its `fmt ` chunk — the header only, like
/// `waav_openai_audio::pcm::wav_duration_secs`. `None` for anything it cannot read with certainty.
fn wav_sample_rate(bytes: &[u8]) -> Option<u32> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let mut pos = 12usize;
    while pos + 8 <= bytes.len() {
        let size = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
        let body = pos + 8;
        if &bytes[pos..pos + 4] == b"fmt " && size >= 16 && body + 8 <= bytes.len() {
            let rate = u32::from_le_bytes([
                bytes[body + 4],
                bytes[body + 5],
                bytes[body + 6],
                bytes[body + 7],
            ]);
            return (rate > 0).then_some(rate);
        }
        let next = body.saturating_add(size).min(bytes.len());
        if next <= pos {
            break;
        }
        pos = next;
    }
    None
}

/// The container of an uploaded file (FRD-021 §6.1 `audio_format`): the bytes' own magic first,
/// then the filename's extension, then the part's content type.
fn upload_format(bytes: &[u8], filename: &str, content_type: Option<&str>) -> Option<String> {
    use crate::core::tts::sniff::sniff_container;
    if let Some(container) = sniff_container(bytes) {
        return Some(container.as_format_str().to_string());
    }
    // Caller-supplied text, bounded before it becomes an attribute.
    let clean = |s: &str| {
        let s = s.trim().to_ascii_lowercase();
        (!s.is_empty() && s.len() <= 16 && s.chars().all(|c| c.is_ascii_alphanumeric()))
            .then_some(s)
    };
    if let Some((_, ext)) = filename.rsplit_once('.')
        && let Some(ext) = clean(ext)
    {
        return Some(ext);
    }
    let subtype = content_type?.split(';').next()?.trim().to_ascii_lowercase();
    let subtype = subtype.strip_prefix("audio/")?;
    let subtype = match subtype {
        "x-wav" | "wave" | "vnd.wave" => "wav",
        "mpeg" | "mpeg3" | "x-mpeg-3" => "mp3",
        other => other,
    };
    clean(subtype)
}

/// The OpenAI output formats a vendor can produce, when WaaV knows; `None` = unknown, allow.
///
/// Derived from the vendor client's own mapping rather than declared beside it: for ElevenLabs,
/// a format is producible when `output_format_for` sends that codec, and `wav`/`pcm` are
/// producible because the vendor returns PCM and this route supplies the RIFF header. A format
/// the mapping sends as PCM — `aac`, `flac` — is one ElevenLabs does not have.
///
/// The same holds for Google, Azure, Cartesia and Speechmatics: each maps a format it has no
/// codec for to its PCM (or WAV) default, and the route then served those bytes under the
/// requested type — raw samples labelled `audio/aac`, a 16 kHz WAV labelled `audio/mpeg`. A
/// format is producible when the vendor's own mapping lands on that codec. Deepgram and OpenAI
/// produce all six and stay `None`.
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
        "google" | "google-tts" => Some(producible(&ALL, |f| {
            use crate::core::tts::google::GoogleAudioEncoding as E;
            matches!(
                (f, E::from_format_string(f.as_waav_format())),
                (AudioFormat::Mp3, E::Mp3)
                    | (AudioFormat::Opus, E::OggOpus)
                    | (AudioFormat::Wav | AudioFormat::Pcm, E::Linear16)
            )
        })),
        "azure" | "microsoft-azure" | "microsoft_azure" => Some(producible(&ALL, |f| {
            let sent = crate::core::tts::azure::AzureAudioEncoding::from_format_string(
                f.as_waav_format(),
                24000,
            );
            match f {
                AudioFormat::Mp3 => sent.content_type() == "audio/mpeg",
                AudioFormat::Opus => sent.content_type() == "audio/ogg",
                AudioFormat::Wav | AudioFormat::Pcm => sent.content_type() == "audio/pcm",
                AudioFormat::Aac => sent.content_type() == "audio/aac",
                AudioFormat::Flac => sent.content_type() == "audio/flac",
            }
        })),
        "cartesia" => Some(producible(&ALL, |f| {
            use crate::core::tts::cartesia::{CartesiaAudioContainer as C, CartesiaOutputFormat};
            matches!(
                (
                    f,
                    CartesiaOutputFormat::from_format_string(f.as_waav_format(), 24000).container
                ),
                (AudioFormat::Mp3, C::Mp3)
                    | (AudioFormat::Wav, C::Wav)
                    | (AudioFormat::Pcm, C::Raw)
            )
        })),
        // Polly: its own mapping decides mp3/opus/wav, but `pcm` is excluded by hand. Polly's PCM
        // is 8 or 16 kHz only, and OpenAI's `pcm` is 24 kHz by definition — the samples would be
        // played at the wrong speed. WAV carries its rate in the header, so it is fine at 16 kHz.
        "aws-polly" | "aws_polly" | "amazon-polly" | "polly" => Some(producible(&ALL, |f| {
            f != AudioFormat::Pcm
                && crate::core::tts::aws_polly::PollyOutputFormat::from_requested(
                    f.as_waav_format(),
                )
                .is_ok()
        })),
        // WAV only. Its one PCM output is 16 kHz, and OpenAI's `pcm` is 24 kHz by definition,
        // which is why the mapping has no name for the route's `linear16`.
        "speechmatics" => Some(producible(&ALL, |f| {
            f.as_waav_format()
                .parse::<crate::core::tts::speechmatics::SpeechmaticsOutputFormat>()
                .is_ok()
        })),
        _ => None,
    }
}

/// The formats in `all` a vendor's mapping says it produces.
fn producible(all: &[AudioFormat], produces: impl Fn(AudioFormat) -> bool) -> Vec<AudioFormat> {
    all.iter().copied().filter(|f| produces(*f)).collect()
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

/// Vendor strings that mean Azure AI Speech (not Azure OpenAI).
fn is_azure_speech(vendor: &str) -> bool {
    matches!(vendor, "azure" | "microsoft-azure" | "microsoft_azure")
}

/// Vendor strings that mean an AWS speech service (SigV4: key pair + region).
fn is_aws_vendor(vendor: &str) -> bool {
    matches!(
        vendor,
        "aws-polly"
            | "aws_polly"
            | "amazon-polly"
            | "polly"
            | "aws-transcribe"
            | "aws_transcribe"
            | "amazon-transcribe"
    )
}

/// A deployment's own vendor parameters, as the providers read them from `extras`.
///
/// Before, `extras` was always empty on this route, so everything a vendor takes only from there
/// was unreachable for a Bud deployment: AWS keys and region (Polly and Transcribe fell back to the
/// GATEWAY's identity in us-east-1), a Google project and location, the Azure Speech host, the
/// Azure OpenAI api-version. Only the keys named here are copied — never `endpoint_override`,
/// which is a destination for the vendor's credential.
fn deployment_extras(
    endpoint: &bud_auth::credentials::VoiceEndpoint,
) -> serde_json::Map<String, serde_json::Value> {
    let mut extras = serde_json::Map::new();
    let mut put = |key: &str, value: Option<&str>| {
        if let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) {
            extras.insert(
                key.to_string(),
                serde_json::Value::String(value.to_string()),
            );
        }
    };
    let vendor = endpoint.vendor.as_str();
    if is_aws_vendor(vendor) {
        // budapp's packed key pair uses AWS's own names without the `aws_` prefix.
        let part = |name: &str| {
            endpoint
                .credential_parts
                .as_ref()
                .and_then(|parts| parts.get(name))
                .map(String::as_str)
        };
        put("aws_access_key_id", part("access_key_id"));
        put("aws_secret_access_key", part("secret_access_key"));
        put("aws_session_token", part("session_token"));
        put("region", endpoint.provider_param("region"));
    } else if matches!(vendor, "google" | "google-tts") {
        put("project_id", endpoint.provider_param("project_id"));
        put("location", endpoint.provider_param("location"));
    } else if crate::core::tts::self_hosted::is_azure_openai(vendor) {
        put(
            crate::core::tts::self_hosted::AZURE_OPENAI_API_VERSION_EXTRA,
            endpoint.provider_param("api_version"),
        );
    } else if is_azure_speech(vendor) {
        // The host (a region, or the resource's custom domain), validated before this point. The
        // STT client reads it from extras; the TTS side reads `TTSConfig.api_base`.
        put(
            crate::core::stt::azure::AZURE_STT_API_BASE_EXTRA,
            endpoint.api_base.as_deref(),
        );
    }
    extras
}

/// A deployment whose configuration cannot reach its vendor, refused before any vendor call.
///
/// The operator's to fix, not the caller's, so it is a 500 naming the endpoint — the same shape as
/// a missing credential. Neither message repeats the configured value: an `api_base` or a
/// credential can carry a secret.
///
/// * **Azure AI Speech** — `api_base` decides the host. Before, it was ignored and every request
///   went to `eastus`, so a key from any other region was refused with a 401 naming neither Bud
///   nor the endpoint. A value that is not an Azure Speech host is refused rather than falling
///   back to `eastus`, which would send the key to the wrong resource.
/// * **Google** — a Bud deployment's credential must be a service-account key. Anything else was
///   read as a FILE PATH on the WaaV pod, or as "use the gateway's own identity" when empty.
fn endpoint_misconfiguration(
    endpoint: &bud_auth::credentials::VoiceEndpoint,
    name: &str,
    advisories: &mut Advisories,
) -> Option<Response> {
    let refuse = |why: String| {
        Some(openai_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "api_error",
            format!(
                "Endpoint '{name}' is misconfigured for vendor '{}': {why}",
                endpoint.vendor
            ),
            None,
        ))
    };
    if is_azure_speech(&endpoint.vendor)
        && let Some(api_base) = endpoint
            .api_base
            .as_deref()
            .filter(|b| !b.trim().is_empty())
    {
        match crate::core::providers::azure::AzureSpeechEndpoint::from_api_base_with_note(api_base)
        {
            Ok((_, Some(note))) => advisories.warn(note),
            Ok((_, None)) => {}
            Err(why) => return refuse(why),
        }
    }
    if is_aws_vendor(&endpoint.vendor) {
        let has = |name: &str| {
            endpoint
                .credential_parts
                .as_ref()
                .and_then(|parts| parts.get(name))
                .is_some_and(|v| !v.trim().is_empty())
        };
        if !(has("access_key_id") && has("secret_access_key")) {
            return refuse(
                "the credential must be an AWS access key pair; without it the request would \
                 authenticate as the gateway's own AWS identity"
                    .to_string(),
            );
        }
        if endpoint.provider_param("region").is_none() {
            return refuse("no AWS region is configured for the deployment".to_string());
        }
    }
    if crate::core::tts::self_hosted::is_azure_openai(&endpoint.vendor)
        && endpoint
            .api_base
            .as_deref()
            .is_none_or(|b| b.trim().is_empty())
    {
        return refuse(
            "no api_base is configured (the Azure OpenAI resource endpoint, \
             https://<resource>.openai.azure.com)"
                .to_string(),
        );
    }
    if matches!(endpoint.vendor.as_str(), "google" | "google-tts")
        && !crate::core::providers::google::is_service_account_json(
            endpoint.credential.as_deref().unwrap_or_default(),
        )
    {
        return refuse(
            "the credential must be a Google service-account key (the JSON file downloaded for \
             the service account); API keys and file paths are not accepted"
                .to_string(),
        );
    }
    None
}

/// The voice-name segment of a Google Text-to-Speech family, for a catalog family name.
///
/// Google names voices `<locale>-<Family>-<Name>` (`en-US-Chirp3-HD-Charon`, `en-US-Wavenet-A`).
/// Source: https://docs.cloud.google.com/text-to-speech/docs/voices
fn google_voice_family(model: &str) -> Option<&'static str> {
    match model.trim().to_ascii_lowercase().as_str() {
        "chirp-3-hd" | "chirp3-hd" => Some("Chirp3-HD"),
        "wavenet" => Some("Wavenet"),
        "neural2" => Some("Neural2"),
        "studio" => Some("Studio"),
        "polyglot" => Some("Polyglot"),
        "standard" => Some("Standard"),
        _ => None,
    }
}

/// A 400 when a Google deployment's family cannot be honoured by the voice it would synthesise
/// with; `None` otherwise (another vendor, no known family, or a voice of that family).
fn google_family_refusal(
    vendor: &str,
    model: &str,
    voice: Option<&str>,
    origin: VoiceOrigin,
    endpoint: &str,
) -> Option<Response> {
    if !matches!(vendor, "google" | "google-tts") {
        return None;
    }
    let family = google_voice_family(model)?;
    let pattern = format!("<locale>-{family}-<Name>, e.g. en-US-{family}-…");
    let message = match voice {
        None => format!(
            "`voice` is required: deployment '{endpoint}' is Google's {model} family, which Google \
             selects only through the voice name. Pass a voice named {pattern}, or set one in the \
             deployment's audio settings."
        ),
        Some(v)
            if v.to_ascii_lowercase()
                .contains(&format!("-{}-", family.to_ascii_lowercase())) =>
        {
            return None;
        }
        Some(v) => {
            let fix = match origin {
                VoiceOrigin::Request => "pass a voice of that family in `voice`",
                _ => "change the voice in the deployment's audio settings, or pass one in `voice`",
            };
            format!(
                "voice '{v}' is not a {model} voice, and deployment '{endpoint}' is Google's \
                 {model} family: Google would synthesise and bill it as a different family. \
                 Voices of this family are named {pattern}; {fix}."
            )
        }
    };
    Some(openai_error(
        StatusCode::BAD_REQUEST,
        "invalid_request_error",
        message,
        Some("voice"),
    ))
}

/// The deployment's model, as the vendor should receive it.
///
/// Verbatim for every vendor but Deepgram, which has no voice parameter: its voice id IS the
/// `model` (`aura-2-thalia-en`), and the provider sends the voice there when one is named. When
/// none is, the deployment's model would be sent instead — and a deployment published under its
/// catalog FAMILY (`aura-2`) names no voice, so Deepgram refused every such request with
/// `400 Invalid 'model' value of 'aura-2'`.
///
/// So with no voice the model is kept only when it is itself a voice on the account (a deployment
/// published under a full voice id), and otherwise left empty: no `model` is sent and Deepgram
/// applies its own default. The account's catalog decides, not a naming rule; it is the list
/// `default_voice` already fetches and caches. An unreadable catalog leaves the model as it was.
async fn vendor_model(
    state: &Arc<AppState>,
    endpoint: &bud_auth::credentials::VoiceEndpoint,
    voice: Option<&str>,
) -> String {
    let model = endpoint.model.clone().unwrap_or_default();
    if voice.is_some() || model.trim().is_empty() || endpoint.vendor != "deepgram" {
        return model;
    }
    let catalog = crate::handlers::voices::fetch_provider_catalog_with_key(
        state,
        &endpoint.vendor,
        endpoint.credential.as_deref(),
    )
    .await;
    if model_names_a_voice(&model, &catalog) {
        model
    } else {
        String::new()
    }
}

/// Whether `model` is one of the catalog's voices. An empty catalog cannot say, so it answers yes
/// and the model is sent as before.
fn model_names_a_voice(model: &str, catalog: &[crate::handlers::voices::Voice]) -> bool {
    catalog.is_empty() || catalog.iter().any(|v| v.id == model.trim())
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
    /// Nothing named or described one, and the vendor takes none: no voice was sent.
    Vendor,
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
        // No voice was sent, so a refusal cannot be about one WaaV should explain.
        (false, _) | (true, VoiceOrigin::Vendor) => (message, None),
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
/// A member of `voice_attrs::ALL` since FRD-021 (WP-5.4), when `VoiceTurnFact` gained the
/// `noise_suppression` column that reads it. Before, it was a trace-only attribute, deliberately
/// kept out of the contract until a column existed for it.
const NOISE_SUPPRESSION_ATTR: &str = voice_attrs::leg::STT_NOISE_SUPPRESSION;

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
            "noise_suppression is configured on this deployment but this gateway build does not \
             include the noise-filter feature; the recording was transcribed unprocessed"
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
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| i16::from_le_bytes(*c))
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
    resolve_described_voice_with(state, endpoint, tts.voice_descriptor.as_ref(), advisories).await
}

/// Resolve a voice DESCRIPTOR — the deployment's own, or on a cross-vendor fallback hop the
/// PRIMARY's (FRD-022 §6.4 step 2) — against `endpoint`'s vendor catalog.
async fn resolve_described_voice_with(
    state: &Arc<AppState>,
    endpoint: &bud_auth::credentials::VoiceEndpoint,
    described: Option<&bud_auth::endpoint_config::VoiceDescriptor>,
    advisories: &mut Advisories,
) -> Option<String> {
    let descriptor = describing_voice(described)?;

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
    root: Option<axum::Extension<RootSpan>>,
    headers: HeaderMap,
    multipart: axum::extract::Multipart,
) -> Response {
    let root = Root::from_extension(root);
    let response = transcription_inner(state, &root, headers, multipart, false).await;
    // FRD-021 §6.8: the transcript returned, or the error JSON — whichever the caller got.
    root.capture_response(response, true).await
}

/// `POST /v1/audio/translations` — same path, but the target language is always English.
pub async fn translation_handler(
    State(state): State<Arc<AppState>>,
    root: Option<axum::Extension<RootSpan>>,
    headers: HeaderMap,
    multipart: axum::extract::Multipart,
) -> Response {
    let root = Root::from_extension(root);
    let response = transcription_inner(state, &root, headers, multipart, true).await;
    root.capture_response(response, true).await
}

async fn transcription_inner(
    state: Arc<AppState>,
    root: &Root,
    headers: HeaderMap,
    mut multipart: axum::extract::Multipart,
    translate: bool,
) -> Response {
    use voice_attrs::{leg, turn};

    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.trim().strip_prefix("Bearer ").unwrap_or(v).to_string());

    // FRD-021 §6.8: the form as sent, and the file as metadata — never the audio. Recorded on the
    // root when this function returns, whichever return that is.
    let mut form = FormCapture::new(root);
    let mut file: Option<Vec<u8>> = None;
    let mut file_content_type: Option<String> = None;
    let mut filename = String::new();
    let mut model = String::new();
    let mut response_format: Option<String> = None;
    let mut language: Option<String> = None;
    let mut prompt: Option<String> = None;
    let mut temperature: Option<f32> = None;
    // OpenAI's SDKs send the array as repeated `timestamp_granularities[]` fields.
    let mut timestamp_granularities: Vec<String> = Vec::new();
    // Bud's per-request overrides of the deployment's transcription settings, and every other
    // field, so an ignored one is named instead of dropped.
    let mut overrides = transcription::TranscriptionOverrides::default();
    let mut unrecognised: Vec<String> = Vec::new();

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
            file_content_type = field.content_type().map(str::to_string);
            match field.bytes().await {
                Ok(b) => {
                    form.file(&filename, file_content_type.as_deref(), b.len());
                    file = Some(b.to_vec());
                }
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
        form.field(&name, &value);
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
            other => match overrides.set(other, &value) {
                Ok(true) => {}
                Ok(false) => {
                    if !other.is_empty() && !unrecognised.iter().any(|n| n == other) {
                        unrecognised.push(other.to_string());
                    }
                }
                Err(e) => return translation_error(&e),
            },
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
        overrides,
        unrecognised,
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
    warn_unrecognised(&settings.unrecognised, &mut advisories);
    // Resolved BEFORE the span: the turn opens only once there is an endpoint to attribute it
    // to (a refusal before this is not a call, DEG-2), and the recorded language is then the
    // one that will be used.
    let Some(resolved) =
        state.resolve_voice_endpoint(&settings.endpoint, capability, bearer.as_deref())
    else {
        return model_not_found(&settings.endpoint, capability);
    };

    // FRD-022 §6.2: the deployment's own rate limits and concurrency cap, before any vendor
    // work. A refusal here is not a call either, so it opens no turn. The slot is held until the
    // response is built.
    let admission = match state.admit_deployment(&resolved.endpoint_id).await {
        Ok(a) => a,
        Err(rejection) => {
            info!(
                endpoint = %settings.endpoint,
                retry_after_s = rejection.retry_after().as_secs(),
                "transcription refused by the deployment's own limits"
            );
            return rejection.into_response();
        }
    };

    // The STT leg's span, mirroring the TTS one: opened as soon as the endpoint resolves, so
    // every failure after this is a failed call with a class (FRD-021 FR-4). `audio_seconds` is
    // the billing dimension for transcription exactly as `characters` is for synthesis, and is
    // recorded on success only.
    let spans = open_turn(
        &state,
        capability,
        &resolved,
        &settings.endpoint,
        bearer.as_deref(),
        root,
        (leg::STT_VENDOR, leg::STT_MODEL),
    )
    .await;
    spans.record_text(
        voice_attrs::resilience::RATE_LIMIT_OUTCOME,
        Some(if admission.headers.is_some() {
            "allow"
        } else {
            "unlimited"
        }),
    );
    let ResolvedVoiceEndpoint {
        endpoint_id,
        endpoint,
        ..
    } = resolved;

    // The language that will actually be used, not just the one the request named: the request,
    // then the stt block's `language`, then the endpoint default (C1 — the precedence is
    // `settings_map::resolve_language`'s; the section outranks the endpoint default). Recorded
    // only when there is one: `""` is not NULL (FRD-021 GT-12).
    let canonical_language = stt_language(&settings, &endpoint);
    spans.record_text(turn::LANGUAGE, canonical_language.as_deref());
    // What was uploaded, read from the bytes in hand — no decoding (FRD-021 §6.1, Phase 5).
    spans.record(turn::INPUT_AUDIO_BYTES, file_bytes.len() as i64);
    spans.record_text(
        turn::AUDIO_FORMAT,
        upload_format(&file_bytes, &filename, file_content_type.as_deref()).as_deref(),
    );
    if let Some(rate) = wav_sample_rate(&file_bytes) {
        spans.record(turn::SAMPLE_RATE, i64::from(rate));
    }

    let upload = SttUpload {
        file: Bytes::from(file_bytes),
        filename,
        decoded: tokio::sync::OnceCell::new(),
    };
    let request = SttRequest {
        settings: &settings,
        capability,
        translate,
        // What the vendor request does, as its CLIENT span names it (CONTRACTS §1.2a).
        vendor_operation: if translate {
            vendor_span::operation::TRANSLATION
        } else {
            vendor_span::operation::TRANSCRIPTION
        },
        upload: &upload,
        spans: &spans,
        started: std::time::Instant::now(),
    };
    let mut response =
        transcribe_resiliently(&state, &request, &endpoint_id, &endpoint, &advisories).await;
    admission.apply(response.headers_mut());
    response
}

/// The language a transcription on `endpoint` runs in: the request, then the deployment's stt
/// block, then its default.
fn stt_language(
    settings: &transcription::TranscriptionSettings,
    endpoint: &bud_auth::credentials::VoiceEndpoint,
) -> Option<String> {
    settings_map::resolve_language(
        settings.language.as_deref(),
        endpoint.language.as_deref(),
        endpoint.config.stt().language.as_deref(),
    )
}

/// The upload, shared by every hop: forwarded whole to an OpenAI-compatible deployment, decoded
/// (once, on first need) for every other vendor.
struct SttUpload {
    file: Bytes,
    filename: String,
    decoded: tokio::sync::OnceCell<Result<waav_openai_audio::pcm::PcmAudio, DecodeFailure>>,
}

/// Why the upload could not be decoded.
enum DecodeFailure {
    /// A container WaaV cannot read, a truncated file, an Opus upload: the caller's audio.
    Unreadable(AudioError),
    /// The decode task itself failed: ours.
    Task,
}

impl SttUpload {
    /// Decode BEFORE touching a provider: a container we cannot read is the caller's problem and
    /// must not cost a vendor connection to discover.
    ///
    /// On a blocking thread: an MP3 is decoded and resampled in full here — seconds of CPU for a
    /// long recording — and on the async runtime that would stall every other request scheduled
    /// on the same worker for as long as it took.
    async fn decoded(&self) -> Result<&waav_openai_audio::pcm::PcmAudio, SttFailure> {
        let decoded = self
            .decoded
            .get_or_init(|| async {
                let (file, name) = (self.file.clone(), self.filename.clone());
                match tokio::task::spawn_blocking(move || {
                    waav_openai_audio::pcm::decode(&file, &name)
                })
                .await
                {
                    Ok(result) => result.map_err(DecodeFailure::Unreadable),
                    Err(e) => {
                        warn!(error = %e, "audio decode task failed");
                        Err(DecodeFailure::Task)
                    }
                }
            })
            .await;
        decoded.as_ref().map_err(|failure| match failure {
            DecodeFailure::Unreadable(e) => SttFailure::Refused(PlanRefusal::classified(
                VoiceErrorType::InputDecode,
                e.to_string(),
                translation_error(e),
            )),
            DecodeFailure::Task => SttFailure::Refused(PlanRefusal::classified(
                VoiceErrorType::Internal,
                "audio decode task failed",
                openai_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "api_error",
                    "the uploaded audio could not be decoded".to_string(),
                    None,
                ),
            )),
        })
    }
}

/// What does not change between hops.
struct SttRequest<'a> {
    settings: &'a transcription::TranscriptionSettings,
    capability: &'static str,
    translate: bool,
    vendor_operation: &'static str,
    upload: &'a SttUpload,
    spans: &'a VoiceSpans,
    started: std::time::Instant,
}

/// One transcription attempt's failure.
enum SttFailure {
    /// The request as sent cannot be served by this deployment (bad upload, a setting it cannot
    /// take): the caller's to fix on the primary, a reason to skip on a fallback.
    Refused(PlanRefusal),
    /// The vendor or the connection failed.
    Vendor(VoiceFailure),
}

impl SttFailure {
    fn verdict(&self) -> resil::classify::Verdict {
        match self {
            // Never retried, never failed over from: it would fail anywhere (FRD-022 §6.4).
            SttFailure::Refused(_) => resil::classify::Verdict {
                retryable: false,
                failover: false,
                breaker: resil::classify::BreakerSignal::Ignore,
                caller_error: true,
                retry_after: None,
                vendor_concurrency: false,
            },
            SttFailure::Vendor(f) => {
                match f
                    .vendor_status
                    .or_else(|| crate::core::deployment_policy::status_in(&f.message))
                {
                    Some(code) => resil::classify::classify(&resil::classify::Failure::Status {
                        code,
                        headers: None,
                        body: Some(f.message.as_bytes()),
                    }),
                    None => crate::core::deployment_policy::classify_message(&f.message, None),
                }
            }
        }
    }
}

/// Transcribe on the primary with its retry policy, behind the circuit breakers, falling back
/// through its fallback chain on a failover-eligible failure — all within one deadline
/// (FRD-022 §6.3–6.5). The upload is already buffered, so a resend is free. A failure marks the
/// turn failed with the response it returns.
async fn transcribe_resiliently(
    state: &Arc<AppState>,
    req: &SttRequest<'_>,
    primary_id: &str,
    primary: &bud_auth::credentials::VoiceEndpoint,
    advisories: &Advisories,
) -> Response {
    use crate::core::deployment_policy::{
        DEFAULT_TRANSCRIPTION_DEADLINE, breaker_open_response, vendor_key,
    };
    use resil::retry::{RetryPolicy, retry};
    use voice_attrs::{leg, resilience};

    let policies = state.policies.clone();
    // STT settings carry no request_timeout of their own; uploads can be long, so the route
    // default bounds the chain.
    let chain_limit = DEFAULT_TRANSCRIPTION_DEADLINE;
    let deadline = tokio::time::Instant::now() + chain_limit;
    let candidates: Vec<Arc<str>> = resil::fallback::expand(
        primary_id,
        |id| {
            if id == primary_id {
                primary.policy.fallback_models.clone()
            } else {
                state
                    .fallback_voice_endpoint(id, req.capability)
                    .map(|e| e.policy.fallback_models)
                    .unwrap_or_default()
            }
        },
        1 + resil::policy::MAX_FALLBACKS * 2,
    );

    let mut retries_total = 0u32;
    let mut primary_failure: Option<SttFailure> = None;
    let mut primary_breaker: Option<resil::breaker::Open> = None;
    let mut fallback_limited: Option<std::time::Duration> = None;
    let mut fallback_attempted = false;

    for (hop, id) in candidates.iter().enumerate() {
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        let (endpoint, _hop_admission) = if hop == 0 {
            (primary.clone(), None)
        } else {
            let Some(endpoint) = state.fallback_voice_endpoint(id, req.capability) else {
                warn!(fallback = %id, "fallback deployment unknown or not serving {}; skipped", req.capability);
                continue;
            };
            match state.admit_deployment(id).await {
                Ok(a) => {
                    fallback_attempted = true;
                    (endpoint, Some(a))
                }
                Err(rejection) => {
                    let wait = rejection.retry_after();
                    fallback_limited = Some(fallback_limited.map_or(wait, |w| w.min(wait)));
                    info!(fallback = %id, "fallback skipped: its own limit said no");
                    continue;
                }
            }
        };

        let vkey = vendor_key(&endpoint.vendor, endpoint.api_base.as_deref());
        if let Some(p) = &policies
            && let Err(open) = p.breakers().check(id, &vkey)
        {
            info!(deployment = %id, retry_in_ms = open.retry_in.as_millis() as u64, "circuit open; hop skipped");
            if hop == 0 {
                primary_breaker = Some(open);
            }
            continue;
        }

        let retry_policy = endpoint
            .policy
            .retry_config
            .as_ref()
            .map(RetryPolicy::interactive)
            .unwrap_or_else(RetryPolicy::none);
        let label: &str = if hop == 0 { &req.settings.endpoint } else { id };
        let endpoint_ref = &endpoint;
        let outcome = retry(
            &retry_policy,
            Some(deadline),
            |_| async move {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                match tokio::time::timeout(
                    remaining,
                    transcribe_on(state, req, label, endpoint_ref, advisories),
                )
                .await
                {
                    Ok(result) => result,
                    Err(_) => Err(SttFailure::Vendor(VoiceFailure::new(
                        VoiceErrorType::Deadline,
                        format!("transcription timed out: {CHAIN_DEADLINE_ELAPSED}"),
                    ))),
                }
            },
            |f: &SttFailure| f.verdict(),
        )
        .await;
        retries_total += outcome.retries;

        match outcome.result {
            Ok(mut response) => {
                if let Some(p) = &policies {
                    p.breakers().record_success(id, &vkey);
                }
                req.spans
                    .record(resilience::RETRY_COUNT, u64::from(retries_total));
                req.spans
                    .record_text(resilience::SERVED_ENDPOINT_ID, Some(&**id));
                use crate::core::deployment_policy::{FALLBACK_HEADER, SERVED_ENDPOINT_HEADER};
                if state.bud_mode.is_some()
                    && let Ok(v) = header::HeaderValue::from_str(id)
                {
                    response.headers_mut().insert(SERVED_ENDPOINT_HEADER, v);
                }
                if hop > 0 {
                    req.spans
                        .record_text(resilience::FALLBACK_FROM, Some(primary_id));
                    // The leg is the fallback's: its vendor and model.
                    req.spans.record_vendor(
                        leg::STT_VENDOR,
                        leg::STT_MODEL,
                        &endpoint.vendor,
                        endpoint.model.as_deref(),
                    );
                    response
                        .headers_mut()
                        .insert(FALLBACK_HEADER, header::HeaderValue::from_static("true"));
                }
                return response;
            }
            Err(failure) => {
                let verdict = failure.verdict();
                if let Some(p) = &policies {
                    p.breakers().record_failure(id, &vkey, &verdict);
                }
                if let SttFailure::Vendor(f) = &failure {
                    warn!(deployment = %id, hop, error = %f, "transcription failed");
                }
                if hop == 0 {
                    let surface = !verdict.failover;
                    primary_failure = Some(failure);
                    if surface {
                        break;
                    }
                }
            }
        }
    }

    let spans = req.spans;
    spans.record(resilience::RETRY_COUNT, u64::from(retries_total));
    let deadline_exceeded = || {
        let m = format!(
            "transcription exceeded deployment '{}''s request_timeout of {}s",
            req.settings.endpoint,
            chain_limit.as_secs()
        );
        spans.fail(VoiceErrorType::Deadline, None, &m);
        openai_error(StatusCode::GATEWAY_TIMEOUT, "api_error", m, None)
    };
    match primary_failure {
        Some(SttFailure::Refused(refusal)) => refusal.fail_on(spans),
        _ if fallback_limited.is_some()
            && !fallback_attempted
            && (primary_breaker.is_some()
                || primary_failure
                    .as_ref()
                    .is_some_and(|f| f.verdict().failover)) =>
        {
            match primary_failure.as_ref() {
                Some(SttFailure::Vendor(f)) => spans.fail(f.class, f.vendor_status, &f.message),
                _ => spans.fail(
                    BREAKER_OPEN_CLASS,
                    None,
                    "circuit breaker open, and every fallback was held back by its own limit",
                ),
            }
            crate::core::deployment_policy::Rejection::Rate(resil::RateHeaders {
                limit: 0,
                remaining: 0,
                reset: 0,
                retry_after: fallback_limited.map(|d| d.as_secs().max(1)),
            })
            .into_response()
        }
        Some(SttFailure::Vendor(f)) if f.message.contains(CHAIN_DEADLINE_ELAPSED) => {
            deadline_exceeded()
        }
        Some(SttFailure::Vendor(f)) => {
            spans.fail(f.class, f.vendor_status, &f.message);
            // A vendor 429 that survived retries and fallback is a 429, not a 502 (FRD-022 §6.6).
            let status = f
                .vendor_status
                .or_else(|| crate::core::deployment_policy::status_in(&f.message));
            if status == Some(429) {
                let mut resp = openai_error(
                    StatusCode::TOO_MANY_REQUESTS,
                    "rate_limit_error",
                    f.message,
                    None,
                );
                let secs = fallback_limited.map(|d| d.as_secs().max(1)).unwrap_or(1);
                resp.headers_mut()
                    .insert(header::RETRY_AFTER, header::HeaderValue::from(secs));
                return resp;
            }
            // 502, not 500: the failure is upstream of WaaV, and the distinction is what tells
            // an operator whether to look at the vendor or at us. The class says which upstream
            // failure.
            openai_error(StatusCode::BAD_GATEWAY, "api_error", f.message, None)
        }
        None => match primary_breaker {
            Some(open) => {
                spans.fail(BREAKER_OPEN_CLASS, None, "circuit breaker open");
                breaker_open_response(open)
            }
            None => deadline_exceeded(),
        },
    }
}

/// One transcription on one deployment. `Ok` is the finished response; its billing and signal
/// attributes are recorded on the turn only then.
async fn transcribe_on(
    state: &Arc<AppState>,
    req: &SttRequest<'_>,
    label: &str,
    endpoint: &bud_auth::credentials::VoiceEndpoint,
    base_advisories: &Advisories,
) -> Result<Response, SttFailure> {
    use voice_attrs::{leg, turn};

    let _ = state;
    let settings = req.settings;
    let spans = req.spans;
    let capability = req.capability;
    let mut advisories = base_advisories.clone();
    let api_key = endpoint.credential.clone().unwrap_or_default();

    // A self-hosted deployment already speaks this exact API, so the file is FORWARDED whole
    // rather than decoded and replayed through a streaming provider. That is not a shortcut:
    // decoding would impose WaaV's WAV-only limit on a backend that may well accept mp3, and
    // the settle heuristic exists only because streaming providers never say "done" -- an
    // HTTP backend answers once and is finished.
    //
    // Azure OpenAI takes the same OpenAI multipart request at its own deployment URL, so it rides
    // the same passthrough with Azure's URL shape and `api-key` header.
    let azure_openai = crate::core::tts::self_hosted::is_azure_openai(&endpoint.vendor);
    if azure_openai || crate::core::tts::self_hosted::is_self_hosted(&endpoint.vendor) {
        let Some(api_base) = endpoint.api_base.clone() else {
            let message = format!(
                "Endpoint '{label}' is {} but has no deployment URL configured",
                if azure_openai {
                    "an Azure OpenAI deployment"
                } else {
                    "self-hosted"
                }
            );
            return Err(SttFailure::Refused(PlanRefusal::classified(
                VoiceErrorType::Config,
                message.clone(),
                openai_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "api_error",
                    message,
                    None,
                ),
            )));
        };
        let file_bytes = req.upload.file.to_vec();
        info!(
            endpoint = %label,
            capability,
            bytes = file_bytes.len(),
            "openai audio/transcriptions -> self-hosted passthrough"
        );
        // The backend receives OpenAI's own fields; Bud's per-request settings have nothing on
        // that wire to ride, so say so rather than drop them.
        if settings.overrides != transcription::TranscriptionOverrides::default() {
            advisories.warn(
                "per-request audio settings are not applied by self-hosted deployments".to_string(),
            );
        }
        // Measured before `file_bytes` is moved into the call below. Header read only: no
        // samples are allocated, so a long upload costs nothing to measure.
        let measured_secs = waav_openai_audio::pcm::wav_duration_secs(&file_bytes);
        let deployment = endpoint.model.clone().unwrap_or_default();
        let forwarded = if azure_openai {
            spans
                .vendor_scope(req.vendor_operation)
                .run(crate::handlers::transcribe::transcribe_azure_openai(
                    &endpoint.vendor,
                    &api_base,
                    &deployment,
                    endpoint.provider_param("api_version"),
                    &api_key,
                    file_bytes,
                    &req.upload.filename,
                    settings,
                ))
                .instrument(spans.turn().clone())
                .await
        } else {
            spans
                .vendor_scope(req.vendor_operation)
                .run(crate::handlers::transcribe::transcribe_self_hosted(
                    &endpoint.vendor,
                    &api_base,
                    &api_key,
                    &deployment,
                    file_bytes,
                    &req.upload.filename,
                    settings,
                ))
                .instrument(spans.turn().clone())
                .await
        };
        return match forwarded {
            Ok(answer) => {
                spans.record(
                    leg::STT_DURATION_MS,
                    req.started.elapsed().as_millis() as u64,
                );
                // `audio_seconds` is the billing dimension for transcription. This branch
                // forwards the upload verbatim and never decodes it, so the field the span
                // reserves was never recorded and the column was permanently NULL for every
                // self-hosted transcription. Read it from the WAV header instead — no samples
                // are allocated. A container the header read cannot parse stays NULL, which is
                // the honest answer: a guessed number in a billing column is worse than none.
                if let Some(secs) = measured_secs {
                    spans.record(turn::AUDIO_SECONDS, secs);
                }
                // At the SERVED deployment's price (FRD-022 §6.4).
                spans.record_cost(voice_cost(
                    endpoint.pricing.as_ref(),
                    capability,
                    None,
                    measured_secs,
                    None,
                ));
                // The backend's own answer to "what language was this", where its body says —
                // `verbose_json` does; the other formats carry none.
                spans.record_text(
                    turn::DETECTED_LANGUAGE,
                    passthrough_language(&answer.body).as_deref(),
                );
                spans.record_text(turn::VENDOR_REQUEST_ID, answer.vendor_request_id.as_deref());
                Ok(passthrough_response(
                    &settings.response_format,
                    answer.body,
                    &advisories,
                ))
            }
            Err(failure) => {
                warn!(endpoint = %label, error = %failure, "self-hosted transcription failed");
                Err(SttFailure::Vendor(failure))
            }
        };
    }

    let audio = req.upload.decoded().await?.clone();

    if api_key.is_empty() && !crate::core::tts::self_hosted::is_self_hosted(&endpoint.vendor) {
        let message = format!(
            "Endpoint '{label}' has no credential configured for vendor '{}'",
            endpoint.vendor
        );
        return Err(SttFailure::Refused(PlanRefusal::classified(
            VoiceErrorType::Config,
            message.clone(),
            openai_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "api_error",
                message,
                None,
            ),
        )));
    }
    if let Some(refusal) = endpoint_misconfiguration(endpoint, label, &mut advisories) {
        return Err(SttFailure::Refused(PlanRefusal::unclassified(refusal)));
    }

    info!(
        endpoint = %label,
        vendor = %endpoint.vendor,
        capability,
        secs = audio.duration_secs(),
        rate = audio.sample_rate,
        "openai audio/transcriptions"
    );

    let mut stt_settings = endpoint.config.stt().into_owned();
    settings_map::apply_transcription_overrides(
        &mut stt_settings,
        &settings.overrides,
        &mut advisories,
    );

    // FRD-018 Part III C1/C3 step 3, and language detection reconciled with it: see
    // `settings_map::upload_language` for the rules. The language is this deployment's: a
    // fallback's own defaults apply on a fallback hop.
    let canonical_language = stt_language(settings, endpoint);
    let mapped_language = match settings_map::upload_language(
        &mut stt_settings,
        canonical_language.as_deref(),
        settings
            .language
            .as_deref()
            .is_some_and(|l| !l.trim().is_empty()),
        settings.overrides.language_detection == Some(true),
        &endpoint.vendor,
        endpoint.model.as_deref(),
        &mut advisories,
    ) {
        Ok(language) => language,
        Err(message) => {
            return Err(SttFailure::Refused(PlanRefusal::classified(
                VoiceErrorType::InvalidRequest,
                message.clone(),
                openai_error(
                    StatusCode::BAD_REQUEST,
                    "invalid_request_error",
                    message,
                    Some("language_detection"),
                ),
            )));
        }
    };

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
        spans.turn(),
        &mut advisories,
    )
    .await;

    // C3: the canonical vocabulary and the translation request reach the provider only through
    // the STANDARD config. `translate` is the ROUTE the caller chose, so it wins over a
    // deployment's configured target list.
    let mut std_config = settings_map::standard_stt(
        stt_config,
        &stt_settings,
        settings_map::translation_for(
            endpoint.config.translation.as_ref(),
            req.translate,
            &mut advisories,
        ),
        &mut advisories,
    );
    apply_request_stt_fields(&mut std_config, settings, &endpoint.vendor, &mut advisories);
    std_config.extras.0.extend(deployment_extras(endpoint));

    match spans
        .vendor_scope(req.vendor_operation)
        .run(crate::handlers::transcribe::transcribe_once_standard(
            &endpoint.vendor,
            std_config,
            &audio,
        ))
        .instrument(spans.turn().clone())
        .await
    {
        Ok(t) => {
            spans.record(
                leg::STT_DURATION_MS,
                req.started.elapsed().as_millis() as u64,
            );
            // The billing dimension, recorded on success only: set before the call, it counted
            // every refused transcription's audio as transcribed.
            spans.record(turn::AUDIO_SECONDS, audio.duration_secs());
            // At the SERVED deployment's price (FRD-022 §6.4).
            spans.record_cost(voice_cost(
                endpoint.pricing.as_ref(),
                capability,
                None,
                Some(audio.duration_secs()),
                None,
            ));
            // Phase 5 signals, each only where the vendor reported it: a confidence it did not
            // send is absent, never 1.0 (DEG-5).
            if let Some(confidence) = t.confidence.filter(|c| c.is_finite()) {
                spans.record(leg::STT_CONFIDENCE, f64::from(confidence));
            }
            spans.record_text(turn::DETECTED_LANGUAGE, t.detected_language.as_deref());
            spans.record_text(turn::VENDOR_REQUEST_ID, t.vendor_request_id.as_deref());
            // What the provider could not honour — a translation target list on a vendor that
            // cannot translate, a batch knob with no equivalent. Produced since the prerecorded
            // driver was written and, until now, logged and nothing else: the response was
            // byte-identical to one where the setting had worked.
            advisories.extend(t.config_warnings.clone());
            if t.truncated {
                warn!(endpoint = %label, "returning a partial transcript");
            }
            // The vendor's own measurement where it reports one; the decoder's otherwise.
            let duration = t.audio_duration.or_else(|| Some(audio.duration_secs()));
            let segments = transcript_segments(
                &t.text,
                &t.words,
                duration,
                settings,
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
            Ok(render_transcription(
                &settings.response_format,
                settings.translate,
                &result,
                &advisories,
                words,
                &t.speakers,
                &t.alternatives,
            ))
        }
        Err(e) => {
            warn!(endpoint = %label, error = %e, "transcription failed");
            match e {
                // The deployment cannot be served as configured: the caller's problem and
                // fixable, so it is a 400 carrying the reason. Returning 502 here — as this
                // path used to for everything — sends an operator to look at the vendor's
                // status page for a model id they typed themselves.
                crate::handlers::transcribe::TranscribeFailure::Configuration(failure) => {
                    Err(SttFailure::Refused(PlanRefusal {
                        failure: Some(failure.clone()),
                        response: openai_error(
                            StatusCode::BAD_REQUEST,
                            "invalid_request_error",
                            failure.message,
                            Some("model"),
                        ),
                    }))
                }
                crate::handlers::transcribe::TranscribeFailure::Upstream(failure) => {
                    Err(SttFailure::Vendor(failure))
                }
            }
        }
    }
}

/// The language a self-hosted backend reported in its body, when it is JSON that carries one
/// (`verbose_json` does). Read, never re-rendered: the body is passed through untouched.
fn passthrough_language(body: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(body).ok()?;
    parsed
        .get("language")?
        .as_str()
        .map(str::trim)
        .filter(|l| !l.is_empty() && l.len() <= 64)
        .map(str::to_string)
}

/// Name each request field nothing reads. serde and the multipart loop both dropped unknown fields
/// silently, so a misspelled setting answered 200 with the deployment's value.
///
/// The names are the caller's, so they are bounded before they become headers and log lines: at
/// most [`MAX_UNRECOGNISED_WARNINGS`] are named, each cut to [`MAX_UNRECOGNISED_NAME_CHARS`] and
/// escaped (a newline in a JSON key would otherwise forge a log line), and the rest are counted.
fn warn_unrecognised(names: &[String], advisories: &mut Advisories) {
    for name in names.iter().take(MAX_UNRECOGNISED_WARNINGS) {
        let shown: String = name
            .chars()
            .take(MAX_UNRECOGNISED_NAME_CHARS)
            .collect::<String>()
            .escape_debug()
            .to_string();
        let cut = if name.chars().count() > MAX_UNRECOGNISED_NAME_CHARS {
            "…"
        } else {
            ""
        };
        advisories.warn(format!(
            "`{shown}{cut}` is not a recognised field and was ignored"
        ));
    }
    if names.len() > MAX_UNRECOGNISED_WARNINGS {
        advisories.warn(format!(
            "{} more unrecognised fields were ignored",
            names.len() - MAX_UNRECOGNISED_WARNINGS
        ));
    }
}

/// How many unrecognised fields are named one by one before the rest are only counted.
const MAX_UNRECOGNISED_WARNINGS: usize = 10;
/// How much of an unrecognised field's name is repeated back.
const MAX_UNRECOGNISED_NAME_CHARS: usize = 64;

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

/// Render a result in the format the caller asked for.
///
/// `text`, `srt` and `vtt` are PLAIN BODIES, not JSON — a client that asked for an SRT file
/// and got `{"text": "1\n00:00:00,000 ..."}` cannot feed it to a player.
///
/// Advisories (W1) ride the HEADERS on every format, because three of the five have no JSON body
/// to put an array in. `verbose_json` additionally carries them in the body, where an SDK finds
/// them without reaching for response headers — which is the natural home, but only covers two
/// of the five cases, so it cannot be the only carrier.
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

    /// Each vendor's own mapping decides, so a format it has no codec for is refused up front
    /// instead of being served as PCM (or a 16 kHz WAV) under the requested content type.
    #[test]
    fn formats_a_vendor_cannot_produce_are_refused_up_front() {
        use AudioFormat::*;
        let formats = |v| super::vendor_output_formats(v).expect(v);
        assert_eq!(
            formats("google"),
            vec![Mp3, Opus, Wav, Pcm],
            "no AAC/FLAC encoding"
        );
        assert_eq!(
            formats("azure"),
            vec![Mp3, Opus, Wav, Pcm],
            "no AAC/FLAC output"
        );
        assert_eq!(
            formats("cartesia"),
            vec![Mp3, Wav, Pcm],
            "raw, wav and mp3 only"
        );
        assert_eq!(
            formats("speechmatics"),
            vec![Wav],
            "16 kHz WAV is its only fit"
        );
        assert_eq!(
            formats("aws-polly"),
            vec![Mp3, Opus, Wav],
            "no AAC/FLAC, and its PCM is 16 kHz, not OpenAI's 24 kHz"
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

    /// A Google family is honoured only by a voice of that family: none, or one from another
    /// family, is a 400 — Google would otherwise choose (and bill) a different family.
    #[test]
    fn a_google_family_needs_a_voice_of_that_family() {
        use super::{VoiceOrigin, google_family_refusal};
        let refused = |model, voice, origin| {
            google_family_refusal("google", model, voice, origin, "ep").map(|r| r.status())
        };
        let bad = Some(StatusCode::BAD_REQUEST);
        assert_eq!(refused("chirp-3-hd", None, VoiceOrigin::Vendor), bad);
        assert_eq!(
            refused("chirp-3-hd", Some("en-US-Standard-C"), VoiceOrigin::Request),
            bad
        );
        assert_eq!(
            refused("wavenet", Some("en-US-Neural2-A"), VoiceOrigin::Deployment),
            bad
        );
        assert_eq!(
            refused(
                "chirp-3-hd",
                Some("en-US-Chirp3-HD-Charon"),
                VoiceOrigin::Request
            ),
            None
        );
        assert_eq!(
            refused("wavenet", Some("de-DE-Wavenet-B"), VoiceOrigin::Request),
            None
        );
        // No known family (empty or other): Google's own default applies.
        assert_eq!(refused("", None, VoiceOrigin::Vendor), None);
        // Other vendors are untouched.
        assert!(
            google_family_refusal("azure", "chirp-3-hd", None, VoiceOrigin::Vendor, "ep").is_none()
        );
    }

    /// WaaV picks a voice only where the vendor cannot synthesise without one. Deepgram and
    /// Google take none, and then choose their own.
    #[test]
    fn a_voice_is_defaulted_only_where_the_vendor_requires_one() {
        use crate::handlers::voices::voice_required;
        assert!(!voice_required("deepgram"));
        assert!(!voice_required("google"));
        for vendor in [
            "elevenlabs",
            "cartesia",
            "openai",
            "azure",
            "aws-polly",
            "speechmatics",
        ] {
            assert!(voice_required(vendor), "{vendor}");
        }
    }

    /// The live failure's no-voice half: a Deepgram deployment published as `aura-2` names no
    /// voice, so with none chosen it must not be sent as the model. A deployment published under
    /// a full voice id still is, and an unreadable catalog changes nothing.
    #[test]
    fn a_deepgram_family_model_is_not_sent_as_a_voice() {
        let catalog = [
            voice("aura-2-thalia-en", "Thalia"),
            voice("aura-asteria-en", "Asteria"),
        ];
        assert!(!super::model_names_a_voice("aura-2", &catalog));
        assert!(!super::model_names_a_voice("aura", &catalog));
        assert!(super::model_names_a_voice("aura-asteria-en", &catalog));
        assert!(super::model_names_a_voice("aura-2", &[]));
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
            overrides: Default::default(),
            unrecognised: Vec::new(),
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
    fn unrecognised_field_names_are_capped_and_escaped() {
        let mut names: Vec<String> = (0..25).map(|i| format!("junk{i}")).collect();
        names[0] = format!("bad\nx-injected: yes{}", "x".repeat(500));
        let mut adv = Advisories::new();
        super::warn_unrecognised(&names, &mut adv);
        assert_eq!(adv.as_slice().len(), super::MAX_UNRECOGNISED_WARNINGS + 1);
        assert!(!adv.as_slice()[0].contains('\n'), "{}", adv.as_slice()[0]);
        assert!(adv.as_slice()[0].chars().count() < 120);
        assert!(adv.as_slice().last().unwrap().starts_with("15 more"));
    }

    #[test]
    fn silence_is_no_segments_and_no_warning() {
        let mut adv = Advisories::new();
        let segs = transcript_segments("  ", &[], Some(3.0), &settings(F::Srt), "x", &mut adv);
        assert!(segs.is_empty() && adv.is_empty());
    }
}

#[cfg(test)]
mod voice_signal_tests {
    //! FRD-021 Phase 5: what the handlers can say about the audio without decoding it.
    use super::{output_audio_meta, passthrough_language, upload_format, wav_sample_rate};
    use waav_openai_audio::speech::AudioFormat;

    fn wav(secs: f64, rate: u32) -> Vec<u8> {
        let samples = (secs * f64::from(rate)).round() as u32;
        let data = samples * 2;
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&(36 + data).to_le_bytes());
        v.extend_from_slice(b"WAVEfmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&rate.to_le_bytes());
        v.extend_from_slice(&(rate * 2).to_le_bytes());
        v.extend_from_slice(&2u16.to_le_bytes());
        v.extend_from_slice(&16u16.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&data.to_le_bytes());
        v.resize(v.len() + data as usize, 0);
        v
    }

    /// TC-EMIT-09: PCM 24 kHz 48,000 bytes is 1.0 s; a 1.5 s WAV is 1.5 s; mp3 is unknown.
    #[test]
    fn output_duration_is_exact_for_pcm_and_wav_and_absent_for_compressed() {
        assert_eq!(
            output_audio_meta(&vec![0u8; 48_000], AudioFormat::Pcm, 24_000),
            (Some(1.0), Some(24_000))
        );
        assert_eq!(
            output_audio_meta(&wav(1.5, 24_000), AudioFormat::Wav, 24_000),
            (Some(1.5), Some(24_000))
        );
        let mut mp3 = b"ID3\x04\x00\x00\x00\x00\x00\x00".to_vec();
        mp3.resize(4_000, 0x55);
        assert_eq!(
            output_audio_meta(&mp3, AudioFormat::Mp3, 24_000),
            (None, None)
        );
        // Raw bytes served for a compressed request are not assumed to be PCM.
        assert_eq!(
            output_audio_meta(&vec![0u8; 4_800], AudioFormat::Opus, 24_000),
            (None, None)
        );
    }

    #[test]
    fn the_wav_rate_is_read_from_the_header_only() {
        assert_eq!(wav_sample_rate(&wav(0.1, 16_000)), Some(16_000));
        assert_eq!(wav_sample_rate(b"RIFF\0\0\0\0WAVE"), None);
        assert_eq!(wav_sample_rate(b"not a wav"), None);
    }

    #[test]
    fn the_upload_format_prefers_the_bytes_then_the_name_then_the_content_type() {
        // An MP3 named `speech.wav` — a browser recorder's default — is an MP3.
        assert_eq!(
            upload_format(b"ID3\x04\x00\x00\x00", "speech.wav", Some("audio/wav")).as_deref(),
            Some("mp3")
        );
        assert_eq!(
            upload_format(&wav(0.1, 16_000), "x.bin", None).as_deref(),
            Some("wav")
        );
        assert_eq!(
            upload_format(b"\x1a\x45\xdf\xa3", "clip.WEBM", None).as_deref(),
            Some("webm")
        );
        assert_eq!(
            upload_format(b"????", "blob", Some("audio/x-wav; codecs=1")).as_deref(),
            Some("wav")
        );
        // Nothing usable, and nothing caller-supplied that is not a plain token.
        assert_eq!(upload_format(b"????", "blob", None), None);
        assert_eq!(
            upload_format(b"????", "x.../../etc", Some("text/plain")),
            None
        );
    }

    #[test]
    fn a_self_hosted_language_is_read_only_from_json_that_carries_one() {
        assert_eq!(
            passthrough_language(r#"{"text":"hola","language":"es"}"#).as_deref(),
            Some("es")
        );
        assert_eq!(passthrough_language(r#"{"text":"hola"}"#), None);
        assert_eq!(passthrough_language(r#"{"language":""}"#), None);
        assert_eq!(
            passthrough_language("1\n00:00:00,000 --> 00:00:01,000\nhola\n"),
            None
        );
    }
}
