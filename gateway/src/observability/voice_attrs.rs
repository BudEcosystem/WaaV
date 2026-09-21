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
#[macro_export]
macro_rules! voice_turn_span {
    (capability = $capability:expr, transport = $transport:expr $(, $extra:ident = $value:expr)* $(,)?) => {
        ::tracing::info_span!(
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
            $( $extra = $value, )*
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
            for name in ALL {
                span.record(*name, "x");
            }
        });
        let seen = seen.lock().unwrap();
        let missing: Vec<_> = ALL.iter().filter(|a| !seen.contains(**a)).collect();
        assert!(
            missing.is_empty(),
            "voice_turn_span! does not declare {missing:?}; recording them is a silent no-op, \
             so those columns would arrive empty with nothing reporting a problem"
        );
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
