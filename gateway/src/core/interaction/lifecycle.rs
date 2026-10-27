//! Dated vendor retirements (the plan's Lifecycle track). A session that names a provider or
//! model that is retiring gets one warning and runs; from the shutdown date it is refused.
//! This applies with interaction mode on or off, and only to traffic that goes to the vendor's
//! own host: a session whose provider is pointed elsewhere (`OPENAI_BASE_URL`, a
//! `<PROVIDER>_REALTIME_URL` override, a Bud deployment's `api_base`) is not affected, so a
//! self-hosted server that answers to `tts-1` keeps working.
//!
//! Speech-to-speech providers follow the capability data's lifecycle rules (`regime.rs`); the
//! speech-to-text and text-to-speech legs of the cascade follow [`CASCADE`], the same dates as
//! the data's `cascade.lifecycle.related` items.

use regex::Regex;
use serde_json::{Value, json};

use super::regime;
use crate::plugin::dispatch;

/// The leg of a cascade session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Leg {
    Stt,
    Tts,
}

impl Leg {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stt => "stt",
            Self::Tts => "tts",
        }
    }
}

/// What a session is told: `config_warning` (`warning` on `/realtime`) when not `fatal`, else
/// an `error` with this code and no session.
#[derive(Debug, Clone, PartialEq)]
pub struct Notice {
    /// `provider_sunset`, `model_retiring`, `provider_retired` or `model_retired`.
    pub code: &'static str,
    pub fatal: bool,
    pub message: String,
    /// `{provider, model, shutdown, replacement}`.
    pub detail: Value,
}

struct Rule {
    leg: Leg,
    /// The gateway's provider name (`plugin::dispatch`); its aliases resolve to it.
    provider: &'static str,
    /// The models it covers; `None`: the provider itself retires.
    models: Option<&'static str>,
    warning_from: Option<&'static str>,
    shutdown: &'static str,
    replacement: Option<&'static str>,
}

/// The cascade's dated retirements (plan section 3.1; `PROVIDER_COVERAGE.md` section 5).
const CASCADE: &[Rule] = &[
    // tts-1, tts-1-hd, gpt-4o-mini-tts and their dated snapshots. OpenAI names no
    // speech-synthesis successor (a realtime model is not one: plan Decision 20).
    Rule {
        leg: Leg::Tts,
        provider: "openai",
        models: Some("^(tts-1|gpt-4o-mini-tts)"),
        warning_from: Some("2026-10-08"),
        shutdown: "2027-01-06",
        replacement: None,
    },
    Rule {
        leg: Leg::Stt,
        provider: "openai",
        models: Some("^(whisper-1|gpt-4o-transcribe|gpt-4o-mini-transcribe)"),
        warning_from: Some("2026-11-28"),
        shutdown: "2027-02-26",
        replacement: Some("gpt-transcribe"),
    },
    Rule {
        leg: Leg::Tts,
        provider: "hume",
        models: None,
        warning_from: None,
        shutdown: "2026-11-13",
        replacement: None,
    },
];

/// The host a provider's own traffic goes to.
fn vendor_host(provider: &str) -> Option<&'static str> {
    match provider {
        "openai" => Some("api.openai.com"),
        "hume" => Some("api.hume.ai"),
        "gemini" => Some("generativelanguage.googleapis.com"),
        _ => None,
    }
}

/// Whether traffic to `base` (none: the provider's default) reaches the vendor itself.
fn reaches_vendor(provider: &str, base: Option<&str>) -> bool {
    let Some(base) = base.map(str::trim).filter(|b| !b.is_empty()) else {
        return true;
    };
    let host = url::Url::parse(base)
        .ok()
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase));
    host.is_some() && host.as_deref() == vendor_host(provider)
}

fn notice(
    retired: bool,
    provider_level: bool,
    provider: &str,
    model: &str,
    shutdown: &str,
    replacement: Option<&str>,
) -> Notice {
    let (code, subject) = match (provider_level, retired) {
        (true, false) => ("provider_sunset", provider.to_owned()),
        (true, true) => ("provider_retired", provider.to_owned()),
        (false, false) => ("model_retiring", model.to_owned()),
        (false, true) => ("model_retired", model.to_owned()),
    };
    let tail = replacement.map_or_else(String::new, |r| format!("; use {r}"));
    let message = if retired {
        format!("{subject} shut down on {shutdown}{tail}. The session is refused.")
    } else {
        format!("{subject} shuts down on {shutdown}{tail}.")
    };
    Notice {
        code,
        fatal: retired,
        message,
        detail: json!({"provider": provider, "model": model, "shutdown": shutdown,
                       "replacement": replacement}),
    }
}

/// A cascade leg: `model` is the id the provider sends (its default already applied), `base`
/// the address its traffic goes to when it is not the vendor's own.
pub fn cascade(
    leg: Leg,
    provider: &str,
    model: &str,
    base: Option<&str>,
    today: &str,
) -> Option<Notice> {
    let provider = match leg {
        Leg::Stt => dispatch::resolve_stt_provider(provider.trim())?.canonical_name(),
        Leg::Tts => dispatch::resolve_tts_provider(provider.trim())?.canonical_name(),
    };
    let rule = CASCADE.iter().find(|rule| {
        rule.leg == leg
            && rule.provider == provider
            && rule
                .models
                .is_none_or(|pattern| Regex::new(pattern).is_ok_and(|re| re.is_match(model)))
    })?;
    if !reaches_vendor(provider, base) {
        return None;
    }
    let retired = today >= rule.shutdown;
    if !retired && rule.warning_from.is_some_and(|from| today < from) {
        return None;
    }
    Some(notice(
        retired,
        rule.models.is_none(),
        rule.provider,
        model,
        rule.shutdown,
        rule.replacement,
    ))
}

/// A speech-to-speech provider (`/realtime`): its model (`None`: the provider's default) under
/// the capability data's rules. `override_url`: the server's `<PROVIDER>_REALTIME_URL`.
pub fn realtime(
    provider: &str,
    model: Option<&str>,
    override_url: Option<&str>,
    today: &str,
) -> Option<Notice> {
    let id = regime::provider_id(provider);
    let found = regime::lifecycle(id, model, today)?;
    if !reaches_vendor(id, override_url) {
        return None;
    }
    let shutdown = found["shutdown"].as_str().unwrap_or_default();
    let code = found["code"].as_str().unwrap_or_default();
    Some(notice(
        found["status"] == "retired",
        code.starts_with("provider_"),
        id,
        found["model"].as_str().unwrap_or_default(),
        shutdown,
        found["replacement"].as_str(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regime_refuses_hume_after_2026_11_13_and_warns_before() {
        let before = realtime("hume", None, None, "2026-10-14").expect("a warning");
        assert_eq!((before.code, before.fatal), ("provider_sunset", false));
        assert_eq!(before.detail["shutdown"], "2026-11-13");
        let after = realtime("hume", Some("EVI 3"), None, "2026-11-13").expect("a refusal");
        assert_eq!((after.code, after.fatal), ("provider_retired", true));
        // Hume's text-to-speech goes with it.
        let tts = cascade(Leg::Tts, "Hume", "", None, "2026-10-14").unwrap();
        assert_eq!((tts.code, tts.fatal), ("provider_sunset", false));
        let tts = cascade(Leg::Tts, "hume-ai", "octave-2", None, "2026-11-13").unwrap();
        assert_eq!((tts.code, tts.fatal), ("provider_retired", true));
        // Every name the factories accept for Hume.
        let tts = cascade(Leg::Tts, "hume_ai", "", None, "2026-11-13").unwrap();
        assert_eq!((tts.code, tts.fatal), ("provider_retired", true));
        for evi in ["evi", "hume_evi", "hume-evi"] {
            let after = realtime(evi, None, None, "2026-11-13").expect("a refusal");
            assert_eq!((after.code, after.fatal), ("provider_retired", true));
        }
    }

    #[test]
    fn openai_defaults_warn_from_their_dates_and_are_refused_from_the_shutdown() {
        // Text-to-speech: warned from 2026-10-08, refused from 2027-01-06.
        assert_eq!(
            cascade(Leg::Tts, "openai", "tts-1", None, "2026-10-07"),
            None
        );
        let warned = cascade(Leg::Tts, "openai", "tts-1-hd", None, "2026-10-08").unwrap();
        assert_eq!((warned.code, warned.fatal), ("model_retiring", false));
        assert_eq!(warned.message, "tts-1-hd shuts down on 2027-01-06.");
        let snapshot = cascade(
            Leg::Tts,
            "openai",
            "gpt-4o-mini-tts-2025-12-15",
            None,
            "2027-01-06",
        );
        assert_eq!(
            snapshot.map(|n| (n.code, n.fatal)),
            Some(("model_retired", true))
        );
        // Transcription: warned from 2026-11-28, refused from 2027-02-26.
        assert_eq!(
            cascade(Leg::Stt, "openai", "whisper-1", None, "2026-10-14"),
            None
        );
        let stt = cascade(Leg::Stt, "openai", "gpt-4o-transcribe", None, "2026-11-28").unwrap();
        assert_eq!(stt.detail["replacement"], "gpt-transcribe");
        assert!(
            cascade(Leg::Stt, "openai", "whisper-1", None, "2027-02-26")
                .unwrap()
                .fatal
        );
        // Realtime: the empty model is gpt-realtime, warned from 2026-10-22.
        assert_eq!(realtime("openai", None, None, "2026-10-21"), None);
        let rt = realtime("openai", None, None, "2026-10-22").unwrap();
        assert_eq!(
            (rt.code, rt.detail["model"].as_str()),
            ("model_retiring", Some("gpt-realtime"))
        );
        assert_eq!(rt.detail["replacement"], "gpt-realtime-2.1");
        assert!(
            realtime("openai", Some("gpt-realtime-mini"), None, "2027-01-20")
                .unwrap()
                .fatal
        );
        assert_eq!(
            realtime("openai", Some("gpt-realtime-2.1"), None, "2027-06-01"),
            None
        );
    }

    #[test]
    fn a_session_whose_traffic_goes_elsewhere_or_to_another_model_is_untouched() {
        let local = Some("http://127.0.0.1:8124/v1");
        assert_eq!(
            cascade(Leg::Tts, "openai", "tts-1", local, "2027-02-01"),
            None
        );
        let own = Some("https://api.openai.com/v1");
        assert!(cascade(Leg::Tts, "openai", "tts-1", own, "2027-02-01").is_some());
        assert_eq!(
            cascade(Leg::Tts, "openai", "gpt-tts-2", None, "2027-02-01"),
            None
        );
        assert_eq!(
            cascade(Leg::Stt, "deepgram", "nova-3", None, "2027-03-01"),
            None
        );
        assert_eq!(cascade(Leg::Stt, "hume", "", None, "2027-03-01"), None);
        let mock = Some("ws://127.0.0.1:3402/v1/realtime?model=prompt:vendor");
        assert_eq!(realtime("openai", None, mock, "2027-02-01"), None);
        assert!(
            realtime(
                "openai",
                None,
                Some("wss://api.openai.com/v1/realtime"),
                "2027-02-01"
            )
            .is_some()
        );
        assert_eq!(realtime("deepgram", None, None, "2027-02-01"), None);
        assert_eq!(realtime("grok", None, None, "2027-02-01"), None);
    }

    #[test]
    fn models_the_vendors_already_retired_are_refused() {
        let gemini = realtime(
            "gemini",
            Some("gemini-2.0-flash-live-001"),
            None,
            "2026-10-14",
        )
        .unwrap();
        assert_eq!((gemini.code, gemini.fatal), ("model_retired", true));
        assert_eq!(gemini.detail["replacement"], "gemini-3.8-live");
        let nova = realtime(
            "nova_sonic",
            Some("amazon.nova-sonic-v1:0"),
            None,
            "2026-10-14",
        )
        .unwrap();
        assert_eq!((nova.code, nova.fatal), ("model_retired", true));
        assert_eq!(realtime("gemini", None, None, "2026-10-14"), None);
        assert_eq!(realtime("nova_sonic", None, None, "2026-10-14"), None);
    }
}
