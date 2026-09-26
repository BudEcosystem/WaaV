//! Apply a deployment's stored configuration to a request's provider config (FRD-018 Part III).
//!
//! The `config` block on a `voice_table` entry is what an operator chose at deploy time. This is
//! where it stops being data and becomes the configuration a provider is built from — milestones
//! C1 (voice and language), C2 (emotion and pronunciations, on the flat config) and C3 (the full
//! canonical feature vocabulary, through `StandardSTTConfig` / `StandardTTSConfig`).
//!
//! Two rules govern everything here, and they pull in opposite directions, which is why they are
//! stated once rather than re-decided per knob:
//!
//! * **Precedence is request > endpoint > provider default.** A caller who names a voice gets
//!   that voice. A caller who does not gets the deployment's. Nobody gets a silent substitution.
//! * **Unsupported degrades, never fails.** A knob the chosen vendor cannot map warns through
//!   [`Advisories`] and the request is served. A deployment-time config choice must not become a
//!   serve-time outage; the whole point of the warning channel is that "degrade" stops meaning
//!   "degrade silently".
//!
//! The advisories are what make the second rule honest. Before the feature matrix existed WaaV
//! could not *tell* whether a vendor honoured a knob — every `from_standard` is infallible and
//! records nothing — so the choice was between failing on everything and warning about nothing.
//! `core::capabilities` is what makes a third answer possible.

use std::borrow::Cow;

use bud_auth::endpoint_config::{SttSettings, TtsSettings};

use crate::core::capabilities::{stt_honours, tts_honours};
use crate::core::emotion::{
    DeliveryStyle, Emotion, EmotionConfig, EmotionIntensity, IntensityLevel,
    provider_supports_emotions,
};
use crate::core::stt::STTConfig;
use crate::core::stt::standard::{StandardSTTConfig, SttFeatures, TranslationConfig};
use crate::core::tts::Pronunciation;
use crate::core::tts::TTSConfig;
use crate::core::tts::standard::{StandardTTSConfig, TtsFeatures};

use super::advisories::Advisories;

/// Resolve the voice a request should synthesise with.
///
/// Returns `None` when neither the caller nor the deployment names one — the handler turns that
/// into a 400 naming `voice`, which is what OpenAI's schema would have done at parse time before
/// the field became optional.
///
/// The deployment's default is returned *unvalidated*, deliberately: the caller-supplied and
/// endpoint-supplied values go through the same `validate_voice` gate in the handler. A default
/// that bypassed it would turn a config typo into a vendor 401 several seconds later, mentioning
/// neither Bud nor the endpoint (TC-CFG-03).
pub fn resolve_voice(requested: Option<&str>, endpoint_default: Option<&str>) -> Option<String> {
    requested
        .filter(|v| !v.trim().is_empty())
        .or(endpoint_default.filter(|v| !v.trim().is_empty()))
        .map(|v| v.to_string())
}

/// Resolve a language token: request, then the deployment's section override (`tts.language` /
/// `stt.language`), then the endpoint default, then nothing (which means "the provider decides").
///
/// The section outranks the endpoint default because it is the more specific of the two: a
/// deployment that speaks English and transcribes German sets both, and the German override lost
/// to the default on every request.
pub fn resolve_language(
    requested: Option<&str>,
    endpoint_default: Option<&str>,
    section_default: Option<&str>,
) -> Option<String> {
    requested
        .filter(|v| !v.trim().is_empty())
        .or(section_default.filter(|v| !v.trim().is_empty()))
        .or(endpoint_default.filter(|v| !v.trim().is_empty()))
        .map(|v| v.to_string())
}

/// Map a canonical language token to the provider's own notation, recording any advisories.
///
/// The same `core::lang` mapper the WebSocket path uses, so `de-DE` reaches ElevenLabs as `de`
/// and Baidu as its numeric `dev_pid` without the caller knowing either exists. Its warnings —
/// an unrecognised token, a region downgrade — become caller-visible advisories here; on the
/// REST plane they previously went to WaaV's log and nowhere else.
pub fn map_language_for(
    canonical: &str,
    provider: &str,
    model: &str,
    advisories: &mut Advisories,
) -> Option<String> {
    let mapped = crate::core::lang::map_language(canonical, provider, model);
    advisories.extend(mapped.warnings.clone());
    if mapped.omit || mapped.native.is_empty() {
        None
    } else {
        Some(mapped.native)
    }
}

/// The language an upload sends the vendor, in the vendor's notation (C1/C3 step 3).
///
/// `language`, `channels`, `punctuation` and `encoding` were hardcoded in the handler. When
/// nothing names a language the answer is now EMPTY — no language is sent — rather than the
/// `en-US` that stood here: that literal switched off every vendor's detection, and went out
/// unmapped, so OpenAI, Groq and Cartesia received a code in a notation they do not take. A
/// vendor that requires a language supplies its own fallback. The language is routed through the
/// P2 mapper, which is what
/// makes an endpoint default work across vendors, and mapped against the model the vendor will be
/// called with: `stt.model` replaces the endpoint's in [`apply_stt_flat`].
///
/// Detection and a pinned language answer the same question in opposite ways, and a vendor handed
/// both lets the pin win or refuses the pair. So:
///
/// * a request asking for both is refused (`Err` carries the message for a 400);
/// * a request naming its language beats the deployment's detection, which is switched off;
/// * with detection on, no language is pinned — an empty string, which every prerecorded builder
///   omits;
/// * a language the vendor cannot take is omitted too, never replaced by `en-US`, which
///   transcribed Thai speech as English.
pub fn upload_language(
    stt: &mut SttSettings,
    canonical: Option<&str>,
    request_named_language: bool,
    request_asked_detection: bool,
    vendor: &str,
    endpoint_model: Option<&str>,
    advisories: &mut Advisories,
) -> Result<String, String> {
    let mut detect = stt.language_detection == Some(true);
    if detect && request_named_language {
        if request_asked_detection {
            return Err(
                "`language_detection` cannot be combined with `language`: send one or the other"
                    .to_string(),
            );
        }
        stt.language_detection = None;
        detect = false;
        advisories.warn(
            "`language` was given, so this deployment's language detection was not used for this \
             request"
                .to_string(),
        );
    }
    if detect {
        if let Some(pinned) = canonical {
            advisories.warn(format!(
                "the default language {pinned} was not sent, because language detection is on"
            ));
        }
        return Ok(String::new());
    }
    let Some(canonical) = canonical else {
        return Ok(String::new());
    };
    let model = stt
        .model
        .as_deref()
        .filter(|m| !m.trim().is_empty())
        .or(endpoint_model)
        .unwrap_or_default();
    Ok(map_language_for(canonical, vendor, model, advisories).unwrap_or_default())
}

/// Build the emotion configuration a deployment asked for, if any (C2).
///
/// `emotion_config` is a field on the **flat** `TTSConfig`, which is why emotion is the cheapest
/// feature in Part III to make reachable: no Standard plumbing is involved, the REST handler
/// simply never had anything to fill it from.
pub fn emotion_config_for(
    settings: &TtsSettings,
    provider: &str,
    advisories: &mut Advisories,
) -> Option<EmotionConfig> {
    let emotion = match settings.emotion.as_deref() {
        None => None,
        Some(raw) => match Emotion::from_str(raw) {
            Some(e) => Some(e),
            None => {
                advisories.warn(format!(
                    "emotion '{raw}' is not one of WaaV's canonical emotions; synthesising without it"
                ));
                None
            }
        },
    };

    let style = match settings.delivery_style.as_deref() {
        None => None,
        Some(raw) => match DeliveryStyle::from_str(raw) {
            Some(s) => Some(s),
            None => {
                advisories.warn(format!(
                    "delivery_style '{raw}' is not one of WaaV's canonical delivery styles; ignoring it"
                ));
                None
            }
        },
    };

    // A canonical emotion or style the vendor's own mapper does not carry. `provider_supports_
    // emotions` below only asks whether a mapper EXISTS; ElevenLabs has one covering 8 of the 44
    // emotions, so `amazed` was accepted, dropped by the mapper, and never mentioned.
    // Only for a vendor that HAS a mapper; one without is named once, below.
    let has_mapper = provider_supports_emotions(provider);
    let emotion = emotion.filter(|e| {
        let carried = !has_mapper
            || crate::core::emotion::matrix::emotions_for_provider(provider)
                .is_none_or(|list| list.contains(e));
        if !carried {
            advisories.warn(format!(
                "{provider} has no mapping for emotion '{}'; synthesising without it",
                e.as_str()
            ));
        }
        carried
    });
    let style = style.filter(|s| {
        let carried = !has_mapper
            || crate::core::emotion::matrix::styles_for_provider(provider)
                .is_none_or(|list| list.contains(s));
        if !carried {
            advisories.warn(format!(
                "{provider} has no mapping for delivery style '{}'; ignoring it",
                s.as_str()
            ));
        }
        carried
    });

    let intensity = parse_intensity(settings.emotion_intensity.as_ref(), advisories);

    if emotion.is_none() && style.is_none() && intensity.is_none() {
        return None;
    }

    // Warn ONCE per request, naming the vendor. Five of ~39 TTS providers have a real mapper;
    // the rest hit a fallback that synthesises normally, which is correct behaviour and utterly
    // invisible to whoever configured the emotion.
    if !provider_supports_emotions(provider) {
        advisories.warn(format!(
            "{provider} has no emotion mapping in this build; the configured emotion is ignored \
             and the text is synthesised normally"
        ));
    }

    Some(EmotionConfig {
        emotion,
        intensity,
        style,
        description: None,
        context: None,
    })
}

/// A float 0.0-1.0, or `low` / `medium` / `high`.
///
/// Out-of-range floats are **clamped, not rejected** — WaaV's own behaviour, and refusing here
/// would make the deployment surface stricter than the request surface that serves it.
fn parse_intensity(
    raw: Option<&serde_json::Value>,
    advisories: &mut Advisories,
) -> Option<EmotionIntensity> {
    match raw {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) => match IntensityLevel::from_str(s) {
            Some(level) => Some(EmotionIntensity::Named(level)),
            None => {
                advisories.warn(format!(
                    "emotion_intensity '{s}' is not low, medium or high; using the vendor default"
                ));
                None
            }
        },
        Some(serde_json::Value::Number(n)) => n.as_f64().map(|v| {
            let clamped = v.clamp(0.0, 1.0) as f32;
            if (v - clamped as f64).abs() > f64::EPSILON {
                advisories.warn(format!(
                    "emotion_intensity {v} is outside 0.0-1.0 and was clamped to {clamped}"
                ));
            }
            EmotionIntensity::Numeric(clamped)
        }),
        Some(other) => {
            advisories.warn(format!(
                "emotion_intensity must be a number or a named level; got {other}"
            ));
            None
        }
    }
}

/// Apply the deployment's synthesis settings to the flat config (C1/C2).
///
/// `sample_rate` is the one field that can be *cleared* rather than set: a compressed container
/// carries its own rate and vendors reject the pair outright — Deepgram answers `sample_rate is
/// not applicable when encoding=mp3` and the whole request 400s. The caller is told, because a
/// configured 48 kHz silently becoming the container's rate is exactly the kind of "worked, but
/// not how you asked" that this channel exists for.
pub fn apply_tts_flat(
    settings: &TtsSettings,
    config: &mut TTSConfig,
    format_accepts_sample_rate: bool,
    advisories: &mut Advisories,
) {
    if let Some(rate) = settings.sample_rate {
        if format_accepts_sample_rate {
            config.sample_rate = Some(rate);
        } else {
            advisories.warn(format!(
                "sample_rate {rate} was cleared: the requested audio format carries its own rate, \
                 and vendors reject the combination"
            ));
        }
    }
    if let Some(timeout) = settings.connection_timeout {
        config.connection_timeout = Some(timeout);
    }
    if let Some(timeout) = settings.request_timeout {
        config.request_timeout = Some(timeout);
    }
    if let Some(list) = &settings.pronunciations {
        config.pronunciations = list
            .iter()
            .map(|p| Pronunciation {
                word: p.word.clone(),
                pronunciation: p.pronunciation.clone(),
            })
            .collect();
    }
    config.emotion_config = emotion_config_for(settings, &config.provider, advisories);
}

/// Build the canonical TTS feature set from the deployment's settings (C3).
///
/// Each field is checked against the provider's row in the feature matrix, so a knob that will
/// not reach the wire says so. Without that check the vocabulary is reachable and still silent:
/// `use_speaker_boost` maps on exactly one of 37 TTS providers.
pub fn tts_features_for(
    settings: &TtsSettings,
    provider: &str,
    language: Option<&str>,
    advisories: &mut Advisories,
) -> TtsFeatures {
    let mut features = TtsFeatures {
        language: language.map(|l| l.to_string()),
        ..Default::default()
    };

    macro_rules! carry {
        ($field:ident) => {
            if let Some(value) = settings.$field {
                if !tts_honours(provider, stringify!($field)) {
                    advisories.warn(format!(
                        "{} does not honour {} in this build; it was not sent",
                        provider,
                        stringify!($field)
                    ));
                } else {
                    features.$field = Some(value);
                }
            }
        };
    }

    carry!(pitch);
    carry!(volume);
    carry!(rate_percentage);
    carry!(pitch_percentage);
    carry!(stability);
    carry!(similarity_boost);
    carry!(style);
    carry!(use_speaker_boost);
    carry!(streaming);
    carry!(optimize_streaming_latency);
    carry!(sample_rate);

    // `emotion` rides BOTH the flat `emotion_config` (C2, richer: intensity and delivery style)
    // and the canonical feature (C3, a bare token some providers read directly). Set here only
    // when the provider declares the canonical one, so a vendor with a real emotion mapper is
    // driven through the mapper rather than through a raw string it would have to re-parse.
    if let Some(emotion) = &settings.emotion
        && tts_honours(provider, "emotion")
        && !provider_supports_emotions(provider)
    {
        features.emotion = Some(emotion.clone());
    }

    features
}

/// Whether the pre-C3 flat constructor set `vad_events: true` for this provider.
///
/// A short allowlist rather than a guess: it names the providers whose flat constructor is known
/// to differ from their `from_standard` default. Adding a provider here requires reading both of
/// its constructors, which is the point — a blanket `true` would push the field onto vendors that
/// never received it.
fn flat_path_enabled_vad_events(provider: &str) -> bool {
    matches!(provider.to_lowercase().as_str(), "deepgram")
}

/// Apply the deployment's transcription settings to the flat config (C1/C3 step 3).
///
/// The four values this replaces — `channels`, `punctuation`, `encoding`, and the `en-US`
/// language fallback — were hardcoded in the handler. They keep their old values as defaults, so
/// a deployment with no `stt` block behaves exactly as it did.
pub fn apply_stt_flat(settings: &SttSettings, config: &mut STTConfig, advisories: &mut Advisories) {
    if let Some(model) = &settings.model {
        if !model.trim().is_empty() {
            config.model = model.clone();
        }
    }
    if let Some(punctuation) = settings.punctuation {
        config.punctuation = punctuation;
    }
    // `encoding` and `channels` are deliberately NOT applied on this path, and a blob carrying
    // them says so out loud rather than being quietly obeyed.
    //
    // Every upload is decoded to PCM before it reaches a vendor — `PcmAudio` has no channels
    // field, `decode` downmixes stereo to mono by averaging — and `transcribe_once` writes raw
    // little-endian i16. The bytes on the wire are therefore ALWAYS mono linear16. Honouring a
    // configured `encoding: mulaw` would tell the vendor to read those bytes as something they
    // are not, and the result is a fluent, confident, wrong transcript with nothing reporting a
    // problem — worse than a failure, because it looks like success.
    //
    // budapp refuses both at publish. This is the second line of defence, for an entry written
    // before it did.
    if settings
        .encoding
        .as_deref()
        .is_some_and(|e| !e.trim().is_empty())
    {
        advisories.warn(
            "encoding is set on this deployment but the gateway decodes every upload to mono \
             16-bit PCM before sending it; the configured value was ignored"
                .to_string(),
        );
    }
    if settings.channels.is_some() || settings.multichannel == Some(true) {
        advisories.warn(
            "channels/multichannel are set on this deployment but the gateway downmixes every \
             upload to mono before sending it; the configured values were ignored"
                .to_string(),
        );
    }
}

/// Build the canonical STT feature set (C3).
///
/// Only the 13 batch-relevant features exist on this struct's source: the five streaming-only
/// ones are refused by budapp at publish time, naming the transport, so they never arrive here.
pub fn stt_features_for(
    settings: &SttSettings,
    provider: &str,
    advisories: &mut Advisories,
) -> SttFeatures {
    let mut features = SttFeatures::default();

    // Preserve the defaults the REST path has ALWAYS sent, where a provider's two constructors
    // disagree about them.
    //
    // `create_stt_provider` -> `DeepgramSTT::new` hardcodes `vad_events: true`, and so does
    // `DeepgramSTTConfig::default()`. `from_standard` reads `f.vad_events.unwrap_or(false)`. So
    // routing this path through the standard dispatch — which is the whole of C3 — would flip
    // `vad_events` from true to false on every Deepgram transcription that configured nothing.
    // It does not change the transcript (`transcribe_once` keeps only `is_final` results), but it
    // changes what we send the vendor, unrequested, on a live path.
    //
    // Fixed HERE rather than in `from_standard`, because that function is shared with the
    // WebSocket plane, where `false` is the behaviour clients have today — repairing one plane by
    // changing the other is not a repair. C3 says to keep the current values as defaults; this is
    // that rule applied to a default the handler never had to name before, because the flat
    // constructor was naming it.
    //
    // KNOWN GAP: this is one instance of a class. Any provider whose `new()` and `from_standard`
    // disagree about an unset feature's default shifts the same way, and 31 STT providers have not
    // been audited for it. Deepgram is the one this platform deploys.
    if flat_path_enabled_vad_events(provider) {
        features.vad_events = Some(true);
    }

    macro_rules! carry {
        ($field:ident) => {
            if let Some(value) = settings.$field {
                if !stt_honours(provider, stringify!($field)) {
                    advisories.warn(format!(
                        "{} does not honour {} in this build; it was not sent",
                        provider,
                        stringify!($field)
                    ));
                } else {
                    features.$field = Some(value);
                }
            }
        };
        ($field:ident, clone) => {
            if let Some(value) = &settings.$field {
                if !stt_honours(provider, stringify!($field)) {
                    advisories.warn(format!(
                        "{} does not honour {} in this build; it was not sent",
                        provider,
                        stringify!($field)
                    ));
                } else {
                    features.$field = Some(value.clone());
                }
            }
        };
    }

    carry!(diarization);
    carry!(word_timestamps);
    carry!(smart_format);
    carry!(profanity_filter);
    carry!(filler_words);
    carry!(language_detection);
    carry!(entity_detection);
    carry!(numerals);
    carry!(alternatives);
    carry!(sentiment);
    carry!(keyterms, clone);
    carry!(redaction, clone);

    features
}

/// Build the translation request, if the deployment configured one (C3 step 4).
///
/// `translate` is the per-request flag: `/v1/audio/translations` is the English fast path, and it
/// wins over a deployment's target list because the caller chose the route.
pub fn translation_for(
    settings: Option<&bud_auth::endpoint_config::TranslationSettings>,
    translate_to_english: bool,
    advisories: &mut Advisories,
) -> Option<TranslationConfig> {
    if translate_to_english {
        return Some(TranslationConfig {
            translate_to_english: Some(true),
            ..Default::default()
        });
    }
    let settings = settings?;
    let targets = settings.target_languages.as_ref()?;
    // A target outside WaaV's canonical set cannot be sent, and dropping it without a word made
    // a configured translation look like one the vendor had simply not done.
    let parsed: Vec<_> = targets
        .iter()
        .filter_map(|t| {
            let resolved = crate::core::lang::resolve(t);
            if resolved.is_none() {
                advisories.warn(format!(
                    "translation target '{t}' is not a language this gateway can send; it was dropped"
                ));
            }
            resolved
        })
        .collect();
    if parsed.is_empty() {
        return None;
    }
    Some(TranslationConfig {
        target_languages: parsed,
        translate_to_english: settings.translate_to_english,
        partials: settings.partials,
    })
}

/// Wrap a flat TTS config with the deployment's canonical features.
pub fn standard_tts(
    base: TTSConfig,
    settings: &TtsSettings,
    language: Option<&str>,
    advisories: &mut Advisories,
) -> StandardTTSConfig {
    let provider = base.provider.clone();
    StandardTTSConfig {
        features: tts_features_for(settings, &provider, language, advisories),
        base,
        extras: Default::default(),
    }
}

/// Wrap a flat STT config with the deployment's canonical features and translation request.
pub fn standard_stt(
    base: STTConfig,
    settings: &SttSettings,
    translation: Option<TranslationConfig>,
    advisories: &mut Advisories,
) -> StandardSTTConfig {
    let provider = base.provider.clone();
    StandardSTTConfig {
        features: stt_features_for(settings, &provider, advisories),
        base,
        extras: Default::default(),
        translation,
    }
}

/// Lay a request's speech overrides over the deployment's saved settings.
///
/// The merged settings then take the same path a saved setting does, so an override the vendor
/// cannot honour gets the same `x-bud-config-warning` and nothing new is special-cased. `Err` names
/// a value outside the canonical vocabulary: a request's own typo is the caller's to fix, where a
/// saved one only degrades (a deployment-time choice must not become a serve-time outage).
pub fn apply_speech_overrides(
    tts: &mut TtsSettings,
    o: &waav_openai_audio::speech::SpeechOverrides,
) -> Result<(), (&'static str, String)> {
    if let Some(e) = &o.emotion {
        if Emotion::from_str(e).is_none() {
            let known: Vec<&str> = Emotion::all().iter().map(|e| e.as_str()).collect();
            return Err((
                "emotion",
                format!("{e:?} is not one of: {}", known.join(", ")),
            ));
        }
        tts.emotion = Some(e.clone());
    }
    if let Some(d) = &o.delivery_style {
        if DeliveryStyle::from_str(d).is_none() {
            let known: Vec<&str> = DeliveryStyle::all().iter().map(|d| d.as_str()).collect();
            return Err((
                "delivery_style",
                format!("{d:?} is not one of: {}", known.join(", ")),
            ));
        }
        tts.delivery_style = Some(d.clone());
    }
    macro_rules! replace {
        ($($field:ident),*) => {$(
            if o.$field.is_some() {
                tts.$field = o.$field.clone();
            }
        )*};
    }
    replace!(
        emotion_intensity,
        pitch,
        volume,
        rate_percentage,
        pitch_percentage,
        stability,
        similarity_boost,
        style,
        use_speaker_boost,
        sample_rate,
        language
    );
    // Added to the deployment's list; the request wins on the same word.
    if let Some(extra) = &o.pronunciations {
        let mut merged: Vec<bud_auth::endpoint_config::Pronunciation> = tts
            .pronunciations
            .take()
            .unwrap_or_default()
            .into_iter()
            .filter(|p| !extra.iter().any(|e| e.word.eq_ignore_ascii_case(&p.word)))
            .collect();
        merged.extend(
            extra
                .iter()
                .map(|e| bud_auth::endpoint_config::Pronunciation {
                    word: e.word.clone(),
                    pronunciation: e.pronunciation.clone(),
                }),
        );
        tts.pronunciations = Some(merged);
    }
    Ok(())
}

/// Lay a request's transcription overrides over the deployment's saved settings.
///
/// Plain switches replace the saved value. The two compliance settings only tighten: a request can
/// switch `profanity_filter` on but not off, and adds `redaction` categories without removing any.
/// Boosted terms are added to the deployment's.
pub fn apply_transcription_overrides(
    stt: &mut SttSettings,
    o: &waav_openai_audio::transcription::TranscriptionOverrides,
    advisories: &mut Advisories,
) {
    macro_rules! replace {
        ($($field:ident),*) => {$(
            if o.$field.is_some() {
                stt.$field = o.$field;
            }
        )*};
    }
    replace!(
        punctuation,
        diarization,
        smart_format,
        numerals,
        filler_words,
        language_detection
    );
    match (o.profanity_filter, stt.profanity_filter) {
        (Some(true), _) => stt.profanity_filter = Some(true),
        (Some(false), Some(true)) => advisories
            .warn("`profanity_filter` is required by this deployment and stays on".to_string()),
        _ => {}
    }
    let union = |saved: &mut Option<Vec<String>>, extra: &[String]| {
        if extra.is_empty() {
            return;
        }
        let list = saved.get_or_insert_with(Vec::new);
        for term in extra {
            if !list.iter().any(|t| t == term) {
                list.push(term.clone());
            }
        }
    };
    union(&mut stt.keyterms, &o.keyterms);
    union(&mut stt.redaction, &o.redaction);
}

/// Borrowed-or-default access, so a handler does not need a `match` per section.
pub fn tts_of(settings: &bud_auth::VoiceEndpointSettings) -> Cow<'_, TtsSettings> {
    settings.tts()
}

pub fn stt_of(settings: &bud_auth::VoiceEndpointSettings) -> Cow<'_, SttSettings> {
    settings.stt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_speech_override_replaces_the_saved_value_and_adds_pronunciations() {
        use waav_openai_audio::speech::{PronunciationOverride, SpeechOverrides};
        let mut tts = TtsSettings {
            stability: Some(0.8),
            emotion: Some("calm".into()),
            pronunciations: Some(vec![
                bud_auth::endpoint_config::Pronunciation {
                    word: "Bud".into(),
                    pronunciation: "budd".into(),
                },
                bud_auth::endpoint_config::Pronunciation {
                    word: "WaaV".into(),
                    pronunciation: "wave".into(),
                },
            ]),
            ..Default::default()
        };
        let o = SpeechOverrides {
            stability: Some(0.2),
            emotion: Some("happy".into()),
            pronunciations: Some(vec![PronunciationOverride {
                word: "bud".into(),
                pronunciation: "bud".into(),
            }]),
            ..Default::default()
        };
        apply_speech_overrides(&mut tts, &o).unwrap();
        assert_eq!(tts.stability, Some(0.2));
        assert_eq!(tts.emotion.as_deref(), Some("happy"));
        let p = tts.pronunciations.unwrap();
        // WaaV kept; Bud replaced by the request's entry (case-insensitive match).
        assert_eq!(p.len(), 2, "{p:?}");
        assert!(p.iter().any(|x| x.word == "WaaV"));
        assert!(
            p.iter()
                .any(|x| x.word == "bud" && x.pronunciation == "bud")
        );
    }

    #[test]
    fn a_speech_override_outside_the_vocabulary_is_refused_by_name() {
        use waav_openai_audio::speech::SpeechOverrides;
        let mut tts = TtsSettings::default();
        let err = apply_speech_overrides(
            &mut tts,
            &SpeechOverrides {
                emotion: Some("ecstatic-ish".into()),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert_eq!(err.0, "emotion");
        let err = apply_speech_overrides(
            &mut tts,
            &SpeechOverrides {
                delivery_style: Some("yodel".into()),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert_eq!(err.0, "delivery_style");
    }

    #[test]
    fn transcription_overrides_replace_switches_and_only_tighten_compliance() {
        use waav_openai_audio::transcription::TranscriptionOverrides;
        let mut stt = SttSettings {
            diarization: Some(false),
            profanity_filter: Some(true),
            redaction: Some(vec!["pii".into()]),
            keyterms: Some(vec!["Dapr".into()]),
            ..Default::default()
        };
        let o = TranscriptionOverrides {
            diarization: Some(true),
            profanity_filter: Some(false),
            redaction: vec!["pci".into(), "pii".into()],
            keyterms: vec!["Kubernetes".into()],
            ..Default::default()
        };
        let mut adv = Advisories::new();
        apply_transcription_overrides(&mut stt, &o, &mut adv);
        assert_eq!(stt.diarization, Some(true));
        assert_eq!(
            stt.profanity_filter,
            Some(true),
            "a request cannot switch it off"
        );
        assert!(
            adv.as_slice()
                .iter()
                .any(|w| w.contains("`profanity_filter` is required")),
            "{:?}",
            adv.as_slice()
        );
        assert_eq!(
            stt.redaction.unwrap(),
            vec!["pii", "pci"],
            "added, never removed"
        );
        assert_eq!(stt.keyterms.unwrap(), vec!["Dapr", "Kubernetes"]);

        // Switching the filter ON from a request is allowed.
        let mut stt = SttSettings::default();
        let mut adv = Advisories::new();
        apply_transcription_overrides(
            &mut stt,
            &TranscriptionOverrides {
                profanity_filter: Some(true),
                ..Default::default()
            },
            &mut adv,
        );
        assert_eq!(stt.profanity_filter, Some(true));
        assert!(adv.is_empty());
    }

    #[test]
    fn an_emotion_the_vendor_does_not_map_is_named_and_dropped() {
        // Live: `emotion: amazed` on ElevenLabs returned 200 with no word; its mapper carries 8.
        let settings = TtsSettings {
            emotion: Some("amazed".into()),
            ..Default::default()
        };
        let mut adv = Advisories::new();
        let cfg = emotion_config_for(&settings, "elevenlabs", &mut adv);
        assert!(cfg.is_none(), "nothing left to send: {cfg:?}");
        assert!(
            adv.as_slice()
                .iter()
                .any(|w| w.contains("no mapping for emotion 'amazed'")),
            "{:?}",
            adv.as_slice()
        );

        // One ElevenLabs does carry passes untouched and unannounced.
        let settings = TtsSettings {
            emotion: Some("happy".into()),
            ..Default::default()
        };
        let mut adv = Advisories::new();
        assert!(emotion_config_for(&settings, "elevenlabs", &mut adv).is_some());
        assert!(adv.as_slice().is_empty(), "{:?}", adv.as_slice());
    }
    use bud_auth::endpoint_config::Pronunciation as BlobPronunciation;

    fn advisories_for(f: impl FnOnce(&mut Advisories)) -> Vec<String> {
        let mut a = Advisories::new();
        f(&mut a);
        a.as_slice().to_vec()
    }

    // --- C1: precedence -----------------------------------------------------

    #[test]
    fn a_request_voice_wins_over_the_deployment_default() {
        // TC-CFG-02. Precedence is request > endpoint > provider, always.
        assert_eq!(
            resolve_voice(Some("aura-asteria-en"), Some("aura-luna-en")).as_deref(),
            Some("aura-asteria-en")
        );
    }

    #[test]
    fn the_deployment_default_is_used_when_the_request_omits_one() {
        // TC-CFG-01. The whole point of C1: an operator's choice becomes the voice it speaks with.
        assert_eq!(
            resolve_voice(None, Some("aura-luna-en")).as_deref(),
            Some("aura-luna-en")
        );
    }

    #[test]
    fn a_blank_request_voice_falls_through_to_the_default() {
        assert_eq!(
            resolve_voice(Some("   "), Some("aura-luna-en")).as_deref(),
            Some("aura-luna-en")
        );
    }

    #[test]
    fn neither_source_means_no_voice() {
        assert_eq!(resolve_voice(None, None), None);
        assert_eq!(resolve_voice(Some(""), Some("  ")), None);
    }

    #[test]
    fn language_precedence_runs_request_then_section_then_endpoint() {
        assert_eq!(
            resolve_language(Some("fr-FR"), Some("de-DE"), Some("es-ES")).as_deref(),
            Some("fr-FR")
        );
        assert_eq!(
            resolve_language(None, Some("de-DE"), Some("es-ES")).as_deref(),
            Some("es-ES"),
            "the section's own language is more specific than the endpoint default"
        );
        assert_eq!(
            resolve_language(None, Some("de-DE"), None).as_deref(),
            Some("de-DE")
        );
        assert_eq!(
            resolve_language(None, None, Some("es-ES")).as_deref(),
            Some("es-ES")
        );
        assert_eq!(resolve_language(None, None, None), None);
    }

    #[test]
    fn a_language_reaches_the_provider_in_its_own_notation() {
        // TC-CFG-04. `de-DE` is not what every vendor calls German, and the difference is the
        // whole reason the endpoint default goes through the mapper rather than straight to the
        // wire: Deepgram takes the region-qualified token, ElevenLabs takes the bare subtag. A
        // default sent raw would work on one of those two and quietly mis-transcribe on the other.
        let mut a = Advisories::new();
        assert_eq!(
            map_language_for("de-DE", "deepgram", "", &mut a).as_deref(),
            Some("de-DE")
        );
        assert_eq!(
            map_language_for("de-DE", "elevenlabs", "eleven_turbo_v2_5", &mut a).as_deref(),
            Some("de"),
            "the downgrade is the case a raw pass-through gets wrong"
        );
        assert!(a.is_empty(), "a clean mapping must not warn");
    }

    #[test]
    fn an_unrecognised_language_warns_and_is_forwarded() {
        let mut a = Advisories::new();
        let mapped = map_language_for("klingon", "deepgram", "", &mut a);
        assert_eq!(mapped.as_deref(), Some("klingon"));
        assert!(
            !a.is_empty(),
            "the caller has to learn it was not understood"
        );
    }

    // --- C2: emotion --------------------------------------------------------

    #[test]
    fn a_canonical_emotion_becomes_an_emotion_config() {
        let settings = TtsSettings {
            emotion: Some("calm".into()),
            ..Default::default()
        };
        let mut a = Advisories::new();
        let config = emotion_config_for(&settings, "elevenlabs", &mut a).unwrap();

        assert_eq!(config.emotion, Some(Emotion::Calm));
        assert!(
            a.is_empty(),
            "elevenlabs has a real mapper; nothing to warn about"
        );
    }

    #[test]
    fn an_unknown_emotion_degrades_with_a_warning_rather_than_failing() {
        // TC-CFG-08. A 400 here would make a deploy-time typo a serve-time outage.
        let settings = TtsSettings {
            emotion: Some("jubilant".into()),
            ..Default::default()
        };
        let warnings = advisories_for(|a| {
            assert!(emotion_config_for(&settings, "elevenlabs", a).is_none());
        });

        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("jubilant"));
    }

    #[test]
    fn a_vendor_without_an_emotion_mapper_is_named_in_the_warning() {
        // "unsupported" alone tells an operator nothing about which of their two endpoints to fix.
        let settings = TtsSettings {
            emotion: Some("calm".into()),
            ..Default::default()
        };
        let warnings = advisories_for(|a| {
            emotion_config_for(&settings, "deepgram", a);
        });

        assert!(warnings.iter().any(|w| w.contains("deepgram")));
    }

    #[test]
    fn a_named_intensity_maps_to_waavs_own_level() {
        let mut a = Advisories::new();
        assert_eq!(
            parse_intensity(Some(&serde_json::json!("high")), &mut a),
            Some(EmotionIntensity::Named(IntensityLevel::High))
        );
    }

    #[test]
    fn an_out_of_range_intensity_is_clamped_not_refused() {
        // WaaV clamps. Refusing would make the deployment surface stricter than the request one.
        let warnings = advisories_for(|a| {
            assert_eq!(
                parse_intensity(Some(&serde_json::json!(4.0)), a),
                Some(EmotionIntensity::Numeric(1.0))
            );
        });
        assert!(warnings[0].contains("clamped"));
    }

    #[test]
    fn an_empty_emotion_block_produces_no_config() {
        let mut a = Advisories::new();
        assert!(emotion_config_for(&TtsSettings::default(), "elevenlabs", &mut a).is_none());
        assert!(a.is_empty());
    }

    // --- C2: the flat config ------------------------------------------------

    #[test]
    fn pronunciations_reach_the_flat_config() {
        // TC-CFG-09. `synthesize_once` applies these to the text on every path, including REST —
        // it simply never had any to apply.
        let settings = TtsSettings {
            pronunciations: Some(vec![BlobPronunciation {
                word: "API".into(),
                pronunciation: "A P I".into(),
            }]),
            ..Default::default()
        };
        let mut config = TTSConfig {
            provider: "elevenlabs".into(),
            ..Default::default()
        };
        let mut a = Advisories::new();
        apply_tts_flat(&settings, &mut config, true, &mut a);

        assert_eq!(config.pronunciations.len(), 1);
        assert_eq!(config.pronunciations[0].word, "API");
    }

    #[test]
    fn a_sample_rate_is_cleared_for_a_compressed_format_and_the_caller_is_told() {
        let settings = TtsSettings {
            sample_rate: Some(48000),
            ..Default::default()
        };
        let mut config = TTSConfig {
            provider: "deepgram".into(),
            sample_rate: None,
            ..Default::default()
        };
        let warnings = advisories_for(|a| apply_tts_flat(&settings, &mut config, false, a));

        assert_eq!(config.sample_rate, None, "the vendor would reject the pair");
        assert!(warnings.iter().any(|w| w.contains("sample_rate")));
    }

    #[test]
    fn a_sample_rate_is_honoured_for_pcm() {
        let settings = TtsSettings {
            sample_rate: Some(48000),
            ..Default::default()
        };
        let mut config = TTSConfig {
            provider: "deepgram".into(),
            ..Default::default()
        };
        let mut a = Advisories::new();
        apply_tts_flat(&settings, &mut config, true, &mut a);

        assert_eq!(config.sample_rate, Some(48000));
    }

    // --- C3: the canonical vocabulary ---------------------------------------

    #[test]
    fn a_supported_feature_reaches_the_standard_config() {
        // TC-CFG-11. Deepgram's mapping reads `diarization`, so it is sent and nothing warns.
        let settings = SttSettings {
            diarization: Some(true),
            ..Default::default()
        };
        let mut a = Advisories::new();
        let features = stt_features_for(&settings, "deepgram", &mut a);

        assert_eq!(features.diarization, Some(true));
        assert!(a.is_empty());
    }

    #[test]
    fn a_feature_the_vendor_does_not_map_warns_and_is_not_sent() {
        // The defect M1 exists to fix: before the matrix, this reached `from_standard`, was
        // ignored, and nothing anywhere said so.
        let settings = SttSettings {
            sentiment: Some(true),
            ..Default::default()
        };
        let warnings = advisories_for(|a| {
            let features = stt_features_for(&settings, "groq", a);
            assert_eq!(features.sentiment, None);
        });

        assert!(
            warnings
                .iter()
                .any(|w| w.contains("sentiment") && w.contains("groq"))
        );
    }

    #[test]
    fn redaction_categories_are_carried_verbatim() {
        // TC-CFG-12's mapping half. The categories are an OPEN set; vendors interpret their own.
        let settings = SttSettings {
            redaction: Some(vec!["pii".into(), "drivers_license".into()]),
            ..Default::default()
        };
        let mut a = Advisories::new();
        let features = stt_features_for(&settings, "deepgram", &mut a);

        assert_eq!(
            features.redaction.unwrap(),
            vec!["pii".to_string(), "drivers_license".to_string()]
        );
    }

    #[test]
    fn an_untouched_settings_block_changes_nothing_a_vendor_would_notice() {
        // The load-bearing no-op: an existing deployment with no config must behave exactly as it
        // did, or Part III is a breaking change wearing an additive hat.
        let mut a = Advisories::new();
        let stt = stt_features_for(&SttSettings::default(), "groq", &mut a);
        let tts = tts_features_for(&TtsSettings::default(), "elevenlabs", None, &mut a);

        assert_eq!(stt, SttFeatures::default());
        assert_eq!(tts, TtsFeatures::default());
        assert!(a.is_empty(), "silence in, silence out");
    }

    #[test]
    fn switching_to_the_standard_dispatch_does_not_flip_vad_events() {
        // Deepgram's flat constructor hardcodes `vad_events: true`; `from_standard` defaults it
        // to false. C3 routes this path through the latter, so without carrying the old default
        // forward, every Deepgram transcription that configured nothing would quietly start
        // sending `vad_events=false` to the vendor.
        let mut a = Advisories::new();
        let features = stt_features_for(&SttSettings::default(), "deepgram", &mut a);

        assert_eq!(
            features.vad_events,
            Some(true),
            "the pre-C3 default must survive C3"
        );
        assert!(
            a.is_empty(),
            "preserving a default is not something to warn about"
        );

        // Everything else stays untouched — this is one named field, not a licence to populate.
        assert_eq!(features.diarization, None);
        assert_eq!(features.smart_format, None);
    }

    #[test]
    fn the_preserved_default_is_not_pushed_onto_other_vendors() {
        // A blanket `true` would send a field to vendors whose flat constructor never set it.
        let mut a = Advisories::new();
        for provider in ["groq", "openai", "assemblyai", "azure"] {
            assert_eq!(
                stt_features_for(&SttSettings::default(), provider, &mut a).vad_events,
                None,
                "{provider} never received vad_events from the flat path"
            );
        }
    }

    #[test]
    fn the_stt_values_a_deployment_owns_are_configurable_and_keep_their_defaults() {
        let mut config = STTConfig {
            provider: "deepgram".into(),
            language: "en-US".into(),
            channels: 1,
            punctuation: true,
            encoding: "linear16".into(),
            ..Default::default()
        };
        let mut a = Advisories::new();
        apply_stt_flat(&SttSettings::default(), &mut config, &mut a);

        assert_eq!(config.channels, 1);
        assert!(config.punctuation);
        assert_eq!(config.encoding, "linear16");

        let settings = SttSettings {
            punctuation: Some(false),
            model: Some("nova-3".into()),
            ..Default::default()
        };
        apply_stt_flat(&settings, &mut config, &mut a);

        // Only the two the deployment actually owns. `encoding` and `channels` belong to the
        // decoder and are covered by their own cases above.
        assert!(!config.punctuation);
        assert_eq!(config.model, "nova-3");
        assert_eq!(config.encoding, "linear16");
        assert_eq!(config.channels, 1);
    }

    #[test]
    fn a_configured_encoding_is_ignored_and_the_caller_is_told() {
        // The audio on the wire is always mono linear16 — the decoder decides that, not the
        // deployment. Obeying a configured `mulaw` here would make the vendor misread the bytes
        // and return a fluent, wrong transcript with nothing reporting a problem.
        let settings = SttSettings {
            encoding: Some("mulaw".into()),
            ..Default::default()
        };
        let mut config = STTConfig {
            provider: "deepgram".into(),
            encoding: "linear16".into(),
            ..Default::default()
        };
        let warnings = advisories_for(|a| apply_stt_flat(&settings, &mut config, a));

        assert_eq!(config.encoding, "linear16", "the decoder's format must win");
        assert!(warnings.iter().any(|w| w.contains("encoding")));
    }

    #[test]
    fn configured_channels_are_ignored_and_the_caller_is_told() {
        let settings = SttSettings {
            channels: Some(2),
            multichannel: Some(true),
            ..Default::default()
        };
        let mut config = STTConfig {
            provider: "deepgram".into(),
            channels: 1,
            ..Default::default()
        };
        let warnings = advisories_for(|a| apply_stt_flat(&settings, &mut config, a));

        assert_eq!(config.channels, 1, "every upload is downmixed to mono");
        assert!(warnings.iter().any(|w| w.contains("mono")));
    }

    #[test]
    fn multichannel_never_reaches_the_vendor_on_this_transport() {
        // It asks the vendor to transcribe each channel separately, and after the downmix there
        // is only ever one. Carrying it would be configuring something that cannot happen.
        let settings = SttSettings {
            multichannel: Some(true),
            ..Default::default()
        };
        let mut a = Advisories::new();

        assert_eq!(
            stt_features_for(&settings, "deepgram", &mut a).multichannel,
            None
        );
    }

    // --- C3: translation ----------------------------------------------------

    #[test]
    fn the_english_fast_path_wins_over_a_configured_target_list() {
        // `/v1/audio/translations` is a ROUTE choice. A deployment's target list must not turn a
        // translation request into a multi-language one the caller did not ask for.
        let settings = bud_auth::endpoint_config::TranslationSettings {
            target_languages: Some(vec!["es-ES".into()]),
            ..Default::default()
        };
        let config = translation_for(Some(&settings), true, &mut Advisories::new()).unwrap();

        assert_eq!(config.translate_to_english, Some(true));
        assert!(config.target_languages.is_empty());
    }

    #[test]
    fn configured_targets_reach_the_translation_config() {
        // TC-CFG-14.
        let settings = bud_auth::endpoint_config::TranslationSettings {
            target_languages: Some(vec!["es-ES".into(), "de-DE".into()]),
            ..Default::default()
        };
        let config = translation_for(Some(&settings), false, &mut Advisories::new()).unwrap();

        assert_eq!(config.target_languages.len(), 2);
    }

    #[test]
    fn no_translation_configured_means_none() {
        assert!(translation_for(None, false, &mut Advisories::new()).is_none());
        assert!(
            translation_for(
                Some(&bud_auth::endpoint_config::TranslationSettings::default()),
                false,
                &mut Advisories::new()
            )
            .is_none()
        );
    }

    #[test]
    fn with_detection_on_no_language_is_pinned() {
        let mut stt = SttSettings {
            language_detection: Some(true),
            ..Default::default()
        };
        let mut adv = Advisories::new();
        let lang =
            upload_language(&mut stt, None, false, false, "deepgram", None, &mut adv).unwrap();
        assert_eq!(lang, "", "not `en-US`, which would override the detection");

        // A deployment default beside detection is not sent either, and the caller is told.
        let mut adv = Advisories::new();
        let lang = upload_language(
            &mut stt,
            Some("de-DE"),
            false,
            false,
            "deepgram",
            None,
            &mut adv,
        )
        .unwrap();
        assert_eq!(lang, "");
        assert!(adv.as_slice().iter().any(|w| w.contains("de-DE")));
    }

    #[test]
    fn a_request_asking_for_both_is_refused_and_a_named_language_beats_saved_detection() {
        let mut stt = SttSettings {
            language_detection: Some(true),
            ..Default::default()
        };
        let mut adv = Advisories::new();
        assert!(
            upload_language(&mut stt, Some("de"), true, true, "deepgram", None, &mut adv).is_err()
        );

        let mut adv = Advisories::new();
        let lang = upload_language(
            &mut stt,
            Some("de-DE"),
            true,
            false,
            "deepgram",
            None,
            &mut adv,
        )
        .unwrap();
        assert_eq!(lang, "de-DE");
        assert_eq!(
            stt.language_detection, None,
            "switched off for this request"
        );
        assert!(!adv.is_empty());
    }

    /// Nothing names a language → none is sent, and the vendor detects it or applies its own
    /// default. The `en-US` that used to be returned here switched detection off everywhere.
    #[test]
    fn nothing_named_sends_no_language() {
        let mut stt = SttSettings::default();
        let mut adv = Advisories::new();
        for vendor in ["deepgram", "openai", "assemblyai", "elevenlabs"] {
            assert_eq!(
                upload_language(&mut stt, None, false, false, vendor, None, &mut adv).unwrap(),
                "",
                "{vendor}"
            );
        }
    }

    #[test]
    fn a_language_the_mapper_omits_is_not_replaced_by_english() {
        // `auto` maps to "omit the field" on ElevenLabs, meaning "let the vendor detect it". The
        // omission used to become `en-US`, which pinned the very language detection was for.
        let mut stt = SttSettings::default();
        let mut adv = Advisories::new();
        let lang = upload_language(
            &mut stt,
            Some("auto"),
            true,
            false,
            "elevenlabs",
            Some("scribe_v2"),
            &mut adv,
        )
        .unwrap();
        assert_eq!(lang, "");
    }

    #[test]
    fn the_language_is_mapped_for_the_model_the_vendor_is_called_with() {
        // `stt.model` replaces the endpoint's model on the wire, so it is the one the mapper must
        // judge. ElevenLabs enforces `language_code` on v2.5 models only.
        let mut adv = Advisories::new();
        upload_language(
            &mut SttSettings::default(),
            Some("de-DE"),
            true,
            false,
            "elevenlabs",
            Some("eleven_flash_v2_5"),
            &mut adv,
        )
        .unwrap();
        assert!(adv.is_empty(), "{:?}", adv.as_slice());

        let mut stt = SttSettings {
            model: Some("eleven_multilingual_v2".into()),
            ..Default::default()
        };
        let mut adv = Advisories::new();
        upload_language(
            &mut stt,
            Some("de-DE"),
            true,
            false,
            "elevenlabs",
            Some("eleven_flash_v2_5"),
            &mut adv,
        )
        .unwrap();
        assert!(
            adv.as_slice()
                .iter()
                .any(|w| w.contains("eleven_multilingual_v2")),
            "{:?}",
            adv.as_slice()
        );
    }

    #[test]
    fn a_translation_target_that_cannot_be_sent_is_named() {
        let settings = bud_auth::endpoint_config::TranslationSettings {
            target_languages: Some(vec!["xx-YY".into(), "es-ES".into()]),
            ..Default::default()
        };
        let mut adv = Advisories::new();
        let config = translation_for(Some(&settings), false, &mut adv).unwrap();
        assert_eq!(config.target_languages.len(), 1);
        assert!(
            adv.as_slice().iter().any(|w| w.contains("'xx-YY'")),
            "{:?}",
            adv.as_slice()
        );
    }
}
