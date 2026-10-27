//! Segmented speech-to-text on live calls.
//!
//! A file-only model accepts one finished audio file per request; a live call has none. The
//! segmented engine (crate `waav-segmented-stt`) listens with the gateway's own detector, cuts the
//! caller's audio at pauses, uploads each utterance and emits the text as a streaming vendor would.
//! This module adapts it to the gateway: [`adapter::SegmentedStt`] is a `BaseSTT`; [`models`] holds
//! the process-wide detector and end-of-turn models; [`live`] resolves a session against the
//! capability map and builds the provider (the third factory).
//!
//! The plan is `docs/segmented-stt/SEGMENTED_STT_PLAN.md`.

pub mod adapter;
pub mod live;
pub mod models;

pub use adapter::{SegmentedPlan, SegmentedStt};
