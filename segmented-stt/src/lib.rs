//! Segmented speech-to-text for live WaaV calls.
//!
//! Some speech-to-text models accept only a finished audio file. A live call has no finished file,
//! so this crate lets the gateway serve one anyway: it listens with its own voice detector, cuts the
//! caller's audio into utterances at pauses, uploads each utterance as a small file, puts the
//! returned text back in order and decides when the caller's turn is over. To the gateway the
//! result looks like a streaming vendor.
//!
//! The plan this implements is `docs/segmented-stt/SEGMENTED_STT_PLAN.md`; where documents
//! disagree, `docs/segmented-stt/INTEGRATION_DECISIONS.md` wins.
//!
//! | Module | What it is |
//! | --- | --- |
//! | [`map`] | The capability map: one row per provider and model saying how a live call reaches it |
//! | [`resolve`] | The resolver: reads the map once per session and picks a transport or a named refusal |
//! | [`rollout`] | The rollout switch, the allow-list and the effective release |
//! | [`control`] | The control record: deployments and rows switched off on a running gateway |
//! | [`audio`] | Wire bytes to 16 kHz mono frames, and WAV encoding |
//! | [`profile`] | Every engine threshold, merged from defaults, the capability row and the session |
//! | [`detector`] | The speech detector trait, the energy detector and a scripted detector for tests |
//! | [`segmenter`] | The state machine that confirms speech and cuts segments at pauses |
//! | [`endpointer`] | The end-of-turn ladder: audio model, text model, silence ceiling |
//! | [`sequencer`] | Upload units, holding and joining, in-order release |
//! | [`turn`] | Joining a turn's text and the two result shapes the engine can emit |
//! | [`live`] | Planning one session: the transcriber, the engine profile, the limiter rate |
//! | [`limits`] | Every time limit of one upload, and the latency store |
//! | [`transcriber`] | One interface for "transcribe this utterance", the attempt loop, limiter and breaker |
//! | [`engine`] | The per-session task that owns all of the above |
//! | [`types`] | The values the engine reports: speech activity, outcomes, facts, notices |

pub mod audio;
pub mod control;
pub mod detector;
pub mod endpointer;
pub mod engine;
pub mod fallback;
pub mod limits;
pub mod live;
pub mod map;
pub mod profile;
pub mod resolve;
pub mod rollout;
pub mod segmenter;
pub mod sequencer;
pub mod transcriber;
pub mod turn;
pub mod types;
