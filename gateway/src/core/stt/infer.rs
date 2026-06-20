//! WaaV-Infer STT adapter (`provider = "waav-infer"`) — Regime A, Cascade-over-Infer.
//!
//! `INFER_GATEWAY_INTEGRATION.md` §5 (GW-3 breaker half landed here; the native WS v1
//! transport mapping is the sibling GW1 tasks). Infer is "the self-hosted inference tier
//! behind the gateway's existing provider seams" — so it plugs in behind [`BaseSTT`] like any
//! other provider, through the `inventory`→registry path, with **zero** handler/route changes.
//!
//! This T3 slice gives the provider its identity and its resilience seam: it constructs from an
//! [`STTConfig`], reports its provider info, and stores the shared, process-global
//! [`ResilienceHandles`] (the per-provider [`CircuitBreaker`] + the reconnect governor) so the
//! GW-3 breaker classification (`crate::core::resilience::infer_reject`) is wired to the same
//! breaker every `waav-infer` session shares. The streaming connect/transport (native WS v1) is
//! filled in by the cascade-transport task; until then the lifecycle methods return a typed
//! `ConnectionFailed` rather than panicking.

use bytes::Bytes;

use super::base::{BaseSTT, STTConfig, STTError, STTErrorCallback, STTResultCallback};
use crate::core::resilience::ResilienceHandles;

/// The canonical provider id and its accepted aliases (kept in one place so the registry
/// registration and any wire/standardizer test agree).
pub const INFER_STT_PROVIDER_ID: &str = "waav-infer";
pub const INFER_STT_ALIASES: &[&str] = &["infer", "waav_infer", "waavinfer", "self-hosted"];

/// `BaseSTT` adapter for a WaaV-Infer cascade STT model.
pub struct InferSTT {
    config: STTConfig,
    /// Shared process-global resilience handles (W-D2): the per-provider breaker GW-3 records
    /// Infer outcomes against, plus the reconnect governor. `None` until [`set_resilience`].
    resilience: Option<ResilienceHandles>,
    ready: bool,
}

impl InferSTT {
    /// The default Infer endpoint when the config carries none (single-box sidecar over loopback).
    /// Real transport wiring is the sibling task; this keeps the field meaningful today.
    pub const DEFAULT_ENDPOINT: &'static str = "ws://127.0.0.1:8123/v1/realtime";

    /// The shared breaker for this provider, if resilience has been injected.
    pub fn breaker(&self) -> Option<&std::sync::Arc<crate::core::resilience::CircuitBreaker>> {
        self.resilience.as_ref().map(|r| &r.breaker)
    }
}

#[async_trait::async_trait]
impl BaseSTT for InferSTT {
    fn new(config: STTConfig) -> Result<Self, STTError>
    where
        Self: Sized,
    {
        Ok(Self {
            config,
            resilience: None,
            ready: false,
        })
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        // Native WS v1 transport is the sibling GW1 task; surface a typed error (never panic) so
        // the registry's panic-isolation and the breaker classification stay meaningful.
        Err(STTError::ConnectionFailed(
            "waav-infer STT transport (native WS v1) not yet wired in this build".into(),
        ))
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
        self.ready = false;
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.ready
    }

    async fn send_audio(&mut self, _audio_data: Bytes) -> Result<(), STTError> {
        Err(STTError::ConnectionFailed(
            "waav-infer STT not connected".into(),
        ))
    }

    async fn on_result(&mut self, _callback: STTResultCallback) -> Result<(), STTError> {
        Ok(())
    }

    async fn on_error(&mut self, _callback: STTErrorCallback) -> Result<(), STTError> {
        Ok(())
    }

    fn get_config(&self) -> Option<&STTConfig> {
        Some(&self.config)
    }

    async fn update_config(&mut self, config: STTConfig) -> Result<(), STTError> {
        self.config = config;
        Ok(())
    }

    fn set_config_only(&mut self, config: STTConfig) {
        self.config = config;
    }

    fn get_provider_info(&self) -> &'static str {
        "waav-infer (self-hosted cascade STT)"
    }

    /// Store the shared resilience handles so GW-3 classification records Infer outcomes against
    /// the one breaker every `waav-infer` session shares.
    fn set_resilience(&mut self, resilience: ResilienceHandles) {
        self.resilience = Some(resilience);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::resilience::{
        BreakerVerdict, CircuitState, ResilienceRegistry, record_infer_outcome,
    };

    fn infer_config() -> STTConfig {
        STTConfig {
            provider: INFER_STT_PROVIDER_ID.to_string(),
            model: "parakeet-tdt".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn constructs_and_reports_provider_info() {
        let stt = InferSTT::new(infer_config()).expect("infer STT constructs");
        assert_eq!(stt.get_provider_info(), "waav-infer (self-hosted cascade STT)");
        assert_eq!(
            stt.get_config().map(|c| c.model.as_str()),
            Some("parakeet-tdt")
        );
        // No resilience injected yet ⇒ no breaker handle.
        assert!(stt.breaker().is_none());
    }

    #[tokio::test]
    async fn connect_returns_typed_error_not_panic() {
        let mut stt = InferSTT::new(infer_config()).unwrap();
        let err = stt.connect().await.expect_err("transport not wired yet");
        assert!(matches!(err, STTError::ConnectionFailed(_)));
    }

    #[test]
    fn shares_the_provider_breaker_for_gw3_classification() {
        // The adapter takes the SAME breaker the registry hands every `waav-infer` session, so a
        // GW-3 verdict recorded through one adapter is visible tier-wide.
        let reg = ResilienceRegistry::new(8);
        let mut stt = InferSTT::new(infer_config()).unwrap();
        stt.set_resilience(reg.handles_for(INFER_STT_PROVIDER_ID));
        let breaker = stt.breaker().expect("resilience injected").clone();

        // A reject-reason storm does not open the shared breaker (GW-3).
        for _ in 0..50 {
            record_infer_outcome(&breaker, BreakerVerdict::for_error_code("model_not_ready"));
        }
        assert_eq!(breaker.state(), CircuitState::Closed);

        // Genuine failures do.
        for _ in 0..10 {
            record_infer_outcome(&breaker, BreakerVerdict::Failure);
        }
        assert_eq!(breaker.state(), CircuitState::Open);
    }
}
