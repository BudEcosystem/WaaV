//! Concrete turn strategies. The legacy set replicates today's hardcoded
//! behavior (the A-G0 wiring gate); MinWords (A-G3) and the TTFS-aware stop
//! (A-G2) land as siblings.

pub mod legacy;
pub mod min_words;

pub use legacy::{AnySpeechStart, EagerSmartTurnSpeculate, LegacySpeechFinalStop};
pub use min_words::MinWordsStart;
