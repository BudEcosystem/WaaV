//! The rollout switch: which sessions the segmenting engine covers, and the release in force.
//!
//! Read once at start-up. A malformed value is an error naming its variable, so a pod never
//! starts on a guess.

use std::fmt;

/// The highest release this build implements.
pub const BUILT_RELEASE: u8 = 6;
/// `off`, `allowlist` (default) or `on`.
pub const SWITCH_VAR: &str = "WAAV_SEGMENTED_STT";
/// Comma-separated entries: a deployment name or id, `provider:model`, or `provider:*`.
pub const ALLOWLIST_VAR: &str = "WAAV_SEGMENTED_STT_ALLOWLIST";
/// The release in force, 0 to [`BUILT_RELEASE`] (default).
pub const RELEASE_VAR: &str = "WAAV_STT_LIVE_RELEASE";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchMode {
    Off,
    Allowlist,
    On,
}

impl SwitchMode {
    pub fn as_str(self) -> &'static str {
        match self {
            SwitchMode::Off => "off",
            SwitchMode::Allowlist => "allowlist",
            SwitchMode::On => "on",
        }
    }
}

impl fmt::Display for SwitchMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One allow-list entry, stored lowercased; providers are stored by canonical id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AllowEntry {
    /// A Bud deployment name or id.
    Deployment(String),
    /// `provider:model`: that model of that provider, compared without case.
    Model { provider: String, model: String },
    /// `provider:*`: every model of that provider.
    Provider(String),
}

/// The switch, the allow-list and the release in force.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rollout {
    pub mode: SwitchMode,
    pub allowlist: Vec<AllowEntry>,
    pub release: u8,
}

impl Rollout {
    /// Reads [`SWITCH_VAR`], [`ALLOWLIST_VAR`] and [`RELEASE_VAR`]. A variable that is unset or
    /// blank takes its default.
    pub fn from_env() -> Result<Rollout, String> {
        for name in [SWITCH_VAR, ALLOWLIST_VAR, RELEASE_VAR] {
            if let Err(std::env::VarError::NotUnicode(_)) = std::env::var(name) {
                return Err(format!("{name} must be valid UTF-8"));
            }
        }
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// As [`Rollout::from_env`], reading each variable through `get`.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Rollout, String> {
        let value = |name: &str| {
            get(name)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        let mode = match value(SWITCH_VAR) {
            None => SwitchMode::Allowlist,
            Some(v) => match v.to_lowercase().as_str() {
                "off" => SwitchMode::Off,
                "allowlist" => SwitchMode::Allowlist,
                "on" => SwitchMode::On,
                _ => {
                    return Err(format!(
                        "{SWITCH_VAR} must be off, allowlist or on, not '{v}'"
                    ));
                }
            },
        };
        let allowlist = match value(ALLOWLIST_VAR) {
            None => Vec::new(),
            Some(v) => v
                .split(',')
                .map(str::trim)
                .filter(|entry| !entry.is_empty())
                .map(parse_entry)
                .collect::<Result<_, _>>()?,
        };
        let release = match value(RELEASE_VAR) {
            None => BUILT_RELEASE,
            Some(v) => v
                .parse::<u8>()
                .ok()
                .filter(|r| *r <= BUILT_RELEASE)
                .ok_or_else(|| {
                    format!("{RELEASE_VAR} must be a release from 0 to {BUILT_RELEASE}, not '{v}'")
                })?,
        };
        Ok(Rollout {
            mode,
            allowlist,
            release,
        })
    }

    /// Whether the switch lets the engine take this session. Release 0 covers nothing.
    pub fn covers(&self, deployment: Option<&str>, provider: &str, model: &str) -> bool {
        if self.release == 0 {
            return false;
        }
        match self.mode {
            SwitchMode::Off => false,
            SwitchMode::On => true,
            SwitchMode::Allowlist => {
                let deployment = deployment.map(|d| d.trim().to_lowercase());
                let provider = canonical_provider(provider);
                let model = model.trim().to_lowercase();
                self.allowlist.iter().any(|entry| match entry {
                    AllowEntry::Deployment(name) => deployment.as_ref() == Some(name),
                    AllowEntry::Provider(p) => *p == provider,
                    AllowEntry::Model {
                        provider: p,
                        model: m,
                    } => *p == provider && *m == model,
                })
            }
        }
    }
}

fn parse_entry(entry: &str) -> Result<AllowEntry, String> {
    let Some((provider, model)) = entry.split_once(':') else {
        return Ok(AllowEntry::Deployment(entry.to_lowercase()));
    };
    let (provider, model) = (provider.trim(), model.trim());
    if provider.is_empty() || model.is_empty() {
        return Err(format!(
            "{ALLOWLIST_VAR} entry '{entry}' needs a provider and a model around ':'"
        ));
    }
    if model != "*" && model.contains('*') {
        return Err(format!(
            "{ALLOWLIST_VAR} entry '{entry}': '*' stands only for every model, as in 'provider:*'"
        ));
    }
    // An entry for a provider the map does not know could never take effect: the engine serves
    // only providers in the map.
    let Some(canonical) = crate::map::CapabilityMap::embedded().provider_id(provider) else {
        return Err(format!(
            "{ALLOWLIST_VAR} entry '{entry}' names provider '{provider}', which the capability map does not know"
        ));
    };
    Ok(if model == "*" {
        AllowEntry::Provider(canonical.to_owned())
    } else {
        AllowEntry::Model {
            provider: canonical.to_owned(),
            model: model.to_lowercase(),
        }
    })
}

fn canonical_provider(raw: &str) -> String {
    crate::map::CapabilityMap::embedded()
        .provider_id(raw)
        .map_or_else(|| raw.trim().to_lowercase(), str::to_owned)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn rollout(vars: &[(&str, &str)]) -> Result<Rollout, String> {
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Rollout::from_lookup(|name| vars.get(name).cloned())
    }

    fn with_list(mode: SwitchMode, list: &str) -> Rollout {
        rollout(&[(SWITCH_VAR, mode.as_str()), (ALLOWLIST_VAR, list)]).unwrap()
    }

    #[test]
    fn nothing_set_is_the_allowlist_with_nothing_listed_at_the_built_release() {
        let r = rollout(&[]).unwrap();
        assert_eq!(
            r,
            Rollout {
                mode: SwitchMode::Allowlist,
                allowlist: Vec::new(),
                release: BUILT_RELEASE
            }
        );
        assert!(!r.covers(Some("stt-prod"), "openai", "gpt-transcribe"));
        let blank =
            rollout(&[(SWITCH_VAR, "  "), (ALLOWLIST_VAR, ""), (RELEASE_VAR, " ")]).unwrap();
        assert_eq!(blank, r);
    }

    #[test]
    fn the_switch_takes_its_three_values_without_case() {
        for (raw, mode) in [
            ("off", SwitchMode::Off),
            (" Allowlist ", SwitchMode::Allowlist),
            ("ON", SwitchMode::On),
        ] {
            assert_eq!(rollout(&[(SWITCH_VAR, raw)]).unwrap().mode, mode, "{raw:?}");
        }
        for bad in ["enabled", "true", "1", "allow-list"] {
            let err = rollout(&[(SWITCH_VAR, bad)]).unwrap_err();
            assert!(err.starts_with(SWITCH_VAR) && err.contains(bad), "{err}");
        }
    }

    #[test]
    fn the_release_is_0_to_6() {
        for release in 0..=6u8 {
            assert_eq!(
                rollout(&[(RELEASE_VAR, &format!(" {release} "))])
                    .unwrap()
                    .release,
                release
            );
        }
        for bad in ["7", "-1", "x", "1.5", "256"] {
            let err = rollout(&[(RELEASE_VAR, bad)]).unwrap_err();
            assert!(err.starts_with(RELEASE_VAR) && err.contains(bad), "{err}");
        }
    }

    #[test]
    fn allowlist_entries_are_trimmed_folded_and_canonical() {
        let r = with_list(
            SwitchMode::Allowlist,
            " STT-Prod ,OpenAI:GPT-Transcribe , groq:*,, azure : * ,self-hosted:Whisper,",
        );
        assert_eq!(
            r.allowlist,
            [
                AllowEntry::Deployment("stt-prod".into()),
                AllowEntry::Model {
                    provider: "openai".into(),
                    model: "gpt-transcribe".into()
                },
                AllowEntry::Provider("groq".into()),
                AllowEntry::Provider("microsoft-azure".into()),
                AllowEntry::Model {
                    provider: "self_hosted".into(),
                    model: "whisper".into()
                },
            ]
        );
    }

    #[test]
    fn a_malformed_allowlist_entry_is_an_error_naming_the_variable() {
        for bad in [
            "openai:",
            ":whisper-1",
            ":",
            "openai:gpt-*",
            "opnai:*",
            "good, acme-speech:x",
        ] {
            let err = rollout(&[(ALLOWLIST_VAR, bad)]).unwrap_err();
            assert!(err.starts_with(ALLOWLIST_VAR), "{bad}: {err}");
        }
    }

    #[test]
    fn release_0_covers_nothing() {
        let r = rollout(&[(SWITCH_VAR, "on"), (RELEASE_VAR, "0")]).unwrap();
        assert!(!r.covers(Some("stt-prod"), "openai", "gpt-transcribe"));
    }

    #[test]
    fn off_covers_nothing_and_on_covers_everything() {
        let off = with_list(SwitchMode::Off, "stt-prod,openai:*");
        assert!(!off.covers(Some("stt-prod"), "openai", "gpt-transcribe"));
        let on = rollout(&[(SWITCH_VAR, "on")]).unwrap();
        assert!(on.covers(None, "openai", "gpt-transcribe"));
        assert!(on.covers(Some("anything"), "acme-speech", ""));
    }

    #[test]
    fn the_allowlist_covers_a_listed_deployment() {
        let r = with_list(
            SwitchMode::Allowlist,
            "stt-prod,0b6c6a8e-2f1d-4f7e-9a51-3c2b1d0e9f87",
        );
        assert!(r.covers(Some(" STT-PROD "), "openai", "gpt-transcribe"));
        assert!(r.covers(Some("0B6C6A8E-2F1D-4F7E-9A51-3C2B1D0E9F87"), "groq", "x"));
        assert!(!r.covers(Some("stt-staging"), "openai", "gpt-transcribe"));
        assert!(!r.covers(None, "openai", "gpt-transcribe"));
    }

    #[test]
    fn the_allowlist_covers_every_model_of_a_listed_provider() {
        let r = with_list(SwitchMode::Allowlist, "azure_openai:*");
        assert!(r.covers(None, "azure-openai", "my-transcriber"));
        assert!(r.covers(Some("whatever"), "Azure_OpenAI", ""));
        assert!(!r.covers(None, "openai", "gpt-transcribe"));
    }

    #[test]
    fn the_allowlist_covers_a_listed_model_only() {
        let r = with_list(SwitchMode::Allowlist, "elevenlabs:scribe_v2");
        assert!(r.covers(None, "ElevenLabs", " SCRIBE_V2 "));
        assert!(!r.covers(None, "elevenlabs", "scribe_v2_medical"));
        assert!(!r.covers(None, "openai", "scribe_v2"));
        let unknown = with_list(SwitchMode::Allowlist, "stt-prod");
        assert!(!unknown.covers(None, "acme-speech", "x"));
    }
}
