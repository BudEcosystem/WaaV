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
    leg::STT_VENDOR,
    leg::STT_DURATION_MS,
    leg::STT_TTFB_MS,
    leg::TTS_VENDOR,
    leg::TTS_DURATION_MS,
    leg::TTS_TTFB_MS,
    leg::LLM_MODEL,
    leg::LLM_DURATION_MS,
];

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

    #[test]
    fn the_billing_dimensions_are_the_vendors_own_units() {
        // A token count on a voice turn would be all zeroes: TTS bills per character, STT per
        // second of audio.
        assert!(ALL.contains(&turn::CHARACTERS));
        assert!(ALL.contains(&turn::AUDIO_SECONDS));
        assert!(
            !ALL.iter().any(|a| a.contains("token")),
            "token counts are meaningless for a voice turn"
        );
    }
}
