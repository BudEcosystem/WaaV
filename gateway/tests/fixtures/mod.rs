//! Test Fixtures Module
//!
//! This module provides test fixtures for WaaV Gateway testing:
//! - Audio fixtures (programmatically generated)
//! - Configuration fixtures
//! - Message fixtures

// Allow dead code in test fixtures - these utilities may be used by future tests
#![allow(dead_code)]

pub mod audio_fixtures;

pub use audio_fixtures::*;
