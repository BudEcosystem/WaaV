//! Provider metrics for performance monitoring.
//!
//! This module provides:
//! - [`ProviderMetrics`] - Per-provider metrics including TTFB
//! - [`RequestTimer`] - RAII timer for request tracking
//! - [`ProviderMetricsSnapshot`] - Point-in-time metrics snapshot

pub mod provider;

pub use provider::{ProviderMetrics, ProviderMetricsSnapshot, RequestTimer};
