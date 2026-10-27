//! A deployment's data settings (Release 5 of segmented speech-to-text): where the vendor
//! processes caller audio (`stt.data_region: eu`) and whether it may keep it
//! (`stt.data_retention: none`).
//!
//! budapp stores them on the deployment; the gateway carries them to every vendor client as two
//! extras, so a streaming socket, a prerecorded upload and the segmented engine read one source.
//! A vendor client that cannot honour one is never used for that deployment: the session or the
//! request is refused with `stt_data_setting_unavailable` before any audio is sent.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

/// The extras key carrying `stt.data_region`.
pub const DATA_REGION_EXTRA: &str = "data_region";
/// The extras key carrying `stt.data_retention`.
pub const DATA_RETENTION_EXTRA: &str = "data_retention";
/// The refusal code.
pub const REFUSAL_CODE: &str = "stt_data_setting_unavailable";

/// What a deployment asked of its vendor.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DataSettings {
    /// Process the audio in the EU.
    pub eu: bool,
    /// Keep nothing.
    pub no_retention: bool,
}

impl DataSettings {
    fn read(get: impl Fn(&str) -> Option<String>) -> Self {
        let is =
            |key: &str, want: &str| get(key).is_some_and(|v| v.trim().eq_ignore_ascii_case(want));
        Self {
            eu: is(DATA_REGION_EXTRA, "eu"),
            no_retention: is(DATA_RETENTION_EXTRA, "none"),
        }
    }

    /// From a vendor config's extras.
    pub fn from_extras(extras: &Map<String, Value>) -> Self {
        Self::read(|k| extras.get(k).and_then(Value::as_str).map(str::to_string))
    }

    /// From a session request's string extras.
    pub fn from_pairs(extras: &BTreeMap<String, String>) -> Self {
        Self::read(|k| extras.get(k).cloned())
    }

    /// From the deployment's `stt` block.
    pub fn from_settings(stt: &bud_auth::SttSettings) -> Self {
        Self::read(|k| match k {
            DATA_REGION_EXTRA => stt.data_region.clone(),
            DATA_RETENTION_EXTRA => stt.data_retention.clone(),
            _ => None,
        })
    }

    pub fn is_empty(&self) -> bool {
        !self.eu && !self.no_retention
    }

    /// The extras that carry these settings to a vendor client.
    pub fn extras(&self) -> Map<String, Value> {
        let mut m = Map::new();
        if self.eu {
            m.insert(DATA_REGION_EXTRA.into(), json!("eu"));
        }
        if self.no_retention {
            m.insert(DATA_RETENTION_EXTRA.into(), json!("none"));
        }
        m
    }

    pub fn union(self, other: Self) -> Self {
        Self {
            eu: self.eu || other.eu,
            no_retention: self.no_retention || other.no_retention,
        }
    }
}

/// Which of today's vendor clients serves the audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientPath {
    /// The vendor's streaming socket (or today's buffering client) on a live session.
    Streaming,
    /// A prerecorded upload on `/v1/audio/transcriptions`.
    Prerecorded,
}

/// A setting the client cannot carry, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unapplied {
    pub setting: &'static str,
    pub reason: &'static str,
}

/// The settings `vendor`'s client on `path` cannot carry. `own_address`: the deployment (or the
/// operator) named the vendor address, which then decides where audio goes.
pub fn unapplied(
    vendor: &str,
    path: ClientPath,
    settings: DataSettings,
    own_address: bool,
) -> Vec<Unapplied> {
    let v = vendor.trim().to_ascii_lowercase();
    let mut out = Vec::new();
    let eu_host = matches!(
        (v.as_str(), path),
        ("deepgram" | "elevenlabs", _) | ("assemblyai", ClientPath::Prerecorded)
    );
    if settings.eu && !eu_host && !own_address {
        out.push(Unapplied {
            setting: "stt.data_region",
            reason: "no_eu_address",
        });
    }
    // The operator's own server keeps what its operator decides.
    let own_server = own_address && crate::core::tts::self_hosted::is_self_hosted(&v);
    if settings.no_retention && !own_server && !matches!(v.as_str(), "deepgram" | "elevenlabs") {
        out.push(Unapplied {
            setting: "stt.data_retention",
            reason: "no_request_switch",
        });
    }
    out
}

/// The refusal text and its details. `deployment`: a Bud deployment asked, rather than a
/// standalone session's own extras.
pub fn refusal(
    vendor: &str,
    unapplied: &[(&'static str, &'static str)],
    deployment: bool,
) -> (String, Value) {
    let asks: Vec<&str> = unapplied
        .iter()
        .map(|(setting, _)| match *setting {
            "stt.data_region" => "process audio in the EU",
            _ => "keep no audio",
        })
        .collect();
    let who = if deployment {
        "This deployment"
    } else {
        "This session"
    };
    let text = format!(
        "{who} asks its speech-to-text vendor to {}, and {vendor} cannot be asked to on this \
         path; no audio was sent. Choose a vendor that can, or change its data settings.",
        asks.join(" and ")
    );
    let settings: Vec<Value> = unapplied
        .iter()
        .map(|(setting, reason)| json!({ "setting": setting, "reason": reason }))
        .collect();
    (text, json!({ "provider": vendor, "settings": settings }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn both() -> DataSettings {
        DataSettings {
            eu: true,
            no_retention: true,
        }
    }

    #[test]
    fn settings_round_trip_through_extras() {
        let s = both();
        assert_eq!(DataSettings::from_extras(&s.extras()), s);
        assert!(DataSettings::default().extras().is_empty());
        let mut pairs = BTreeMap::new();
        pairs.insert("data_region".to_string(), " EU ".to_string());
        pairs.insert("data_retention".to_string(), "vendor_default".to_string());
        assert_eq!(
            DataSettings::from_pairs(&pairs),
            DataSettings {
                eu: true,
                no_retention: false
            }
        );
    }

    #[test]
    fn the_deployment_block_is_read() {
        let stt: bud_auth::SttSettings = serde_json::from_value(json!({
            "data_region": "eu",
            "data_retention": "none"
        }))
        .unwrap();
        assert_eq!(DataSettings::from_settings(&stt), both());
    }

    #[test]
    fn only_vendors_with_an_eu_host_and_a_switch_carry_both() {
        let names = |v: &str, p: ClientPath, own: bool| -> Vec<&'static str> {
            unapplied(v, p, both(), own)
                .into_iter()
                .map(|u| u.setting)
                .collect()
        };
        for path in [ClientPath::Streaming, ClientPath::Prerecorded] {
            assert!(names("deepgram", path, false).is_empty());
            assert!(names("ElevenLabs", path, false).is_empty());
            assert_eq!(
                names("openai", path, false),
                vec!["stt.data_region", "stt.data_retention"]
            );
        }
        assert_eq!(
            names("assemblyai", ClientPath::Prerecorded, false),
            vec!["stt.data_retention"]
        );
        assert_eq!(
            names("assemblyai", ClientPath::Streaming, false),
            vec!["stt.data_region", "stt.data_retention"]
        );
        // The deployment's own address decides the region, not the retention.
        assert_eq!(
            names("groq", ClientPath::Streaming, true),
            vec!["stt.data_retention"]
        );
        assert!(
            unapplied(
                "groq",
                ClientPath::Streaming,
                DataSettings::default(),
                false
            )
            .is_empty()
        );
        // A self-hosted server at the deployment's own address carries both; Azure OpenAI's own
        // resource address carries the region only (retention is an account setting there).
        assert!(names("self_hosted", ClientPath::Prerecorded, true).is_empty());
        assert_eq!(
            names("azure_openai", ClientPath::Prerecorded, true),
            vec!["stt.data_retention"]
        );
    }

    #[test]
    fn the_refusal_names_each_setting() {
        let (text, details) = refusal(
            "groq",
            &[
                ("stt.data_region", "no_eu_address"),
                ("stt.data_retention", "no_request_switch"),
            ],
            true,
        );
        assert!(text.starts_with("This deployment asks"), "{text}");
        let (standalone, _) = refusal("groq", &[("stt.data_region", "no_eu_address")], false);
        assert!(standalone.starts_with("This session asks"), "{standalone}");
        assert!(
            text.contains("process audio in the EU and keep no audio"),
            "{text}"
        );
        assert!(text.contains("no audio was sent"));
        assert_eq!(details["settings"][1]["reason"], "no_request_switch");
        assert_eq!(details["provider"], "groq");
    }
}
