//! Shared text-processing utilities for the voice pipeline.
//!
//! Centralizes text handling that was previously duplicated (and divergent)
//! between the conversation pump and the DAG LLM node — see
//! `PIPECAT_FIX_PLAN.md` cluster C.

pub mod sentence;

pub use sentence::{SentenceAggregator, SentenceConfig};
