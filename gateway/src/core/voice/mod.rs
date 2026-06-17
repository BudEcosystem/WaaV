//! Unified Voice-Descriptor system (P4 of the SDK standardization plan).
//!
//! A developer asks for a voice by CHARACTERISTICS — `{gender, locale/accent,
//! style, age, name_hint}` — and the gateway resolves it SERVER-SIDE to a concrete
//! provider `voice_id` over the `/voices` catalog, so the same descriptor works
//! across Deepgram / ElevenLabs / Azure / … with no client edit. The raw
//! `voice_id` always wins as an escape hatch.
//!
//! # Architecture (mirrors [`crate::core::emotion`] / [`crate::core::lang`])
//!
//! ```text
//!   client {female, en-US, warm}
//!        └─ resolve_voice(descriptor, catalog_for(provider), provider_default)
//!             ├─ deepgram   → aura-asteria-en
//!             ├─ elevenlabs → 21m00Tcm4TlvDq8ikWAM (Rachel)
//!             └─ azure      → en-US-JennyNeural
//!        (no match → provider default + config_warning, NEVER a 400)
//! ```
//!
//! # Call site
//!
//! Resolution fires at the TTS config→provider boundary (`TTSWebSocketConfig`),
//! AFTER the raw `voice_id` escape hatch, and BEFORE provider construction. The
//! catalog comes from `handlers::voices` (cached to avoid a live provider hit on
//! every turn).

pub mod resolver;
pub mod types;

pub use resolver::{ResolvedVoice, resolve_voice};
pub use types::{Age, Gender, VoiceDescriptor};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handlers::voices::Voice;

    #[test]
    fn public_surface_is_reachable() {
        let d = VoiceDescriptor {
            gender: Some(Gender::Female),
            age: Some(Age::Young),
            ..Default::default()
        };
        let catalog = vec![Voice {
            id: "v1".into(),
            sample: String::new(),
            name: "Young Woman".into(),
            accent: "American".into(),
            gender: "Female".into(),
            language: "English".into(),
        }];
        let r: ResolvedVoice = resolve_voice(&d, &catalog, "default");
        assert_eq!(r.voice_id, "v1");
    }
}
