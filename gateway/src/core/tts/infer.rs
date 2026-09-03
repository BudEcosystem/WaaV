//! WaaV-Infer TTS adapter (`provider = "waav-infer"`) — Regime A, Cascade-over-Infer.
//!
//! `INFER_GATEWAY_INTEGRATION.md` §5. Mirror of the STT adapter: Infer's self-hosted cascade
//! TTS plugs in behind [`BaseTTS`] through the `inventory`→registry path with zero handler/route
//! changes. This T3 slice gives the provider its identity and a typed lifecycle; the native WS v1
//! `Speak/ChunkMeta→AudioData` transport mapping (and the GW-5 resilience extension to TTS) are
//! the sibling GW1 tasks. Lifecycle methods return typed errors rather than panicking.

use std::sync::Arc;

use super::base::{AudioCallback, BaseTTS, TTSConfig, TTSError, TTSResult};

/// The canonical provider id and its accepted aliases (shared shape with the STT adapter).
pub const INFER_TTS_PROVIDER_ID: &str = "waav-infer";
pub const INFER_TTS_ALIASES: &[&str] = &["infer", "waav_infer", "waavinfer", "self-hosted"];

/// `BaseTTS` adapter for a WaaV-Infer cascade TTS model.
pub struct InferTTS {
    config: TTSConfig,
    audio_cb: Option<Arc<dyn AudioCallback>>,
    ready: bool,
}

impl InferTTS {
    /// The default Infer endpoint when the config carries none (single-box sidecar over loopback).
    pub const DEFAULT_ENDPOINT: &'static str = "ws://127.0.0.1:8123/v1/realtime";
}

#[async_trait::async_trait]
impl BaseTTS for InferTTS {
    fn new(config: TTSConfig) -> TTSResult<Self>
    where
        Self: Sized,
    {
        Ok(Self {
            config,
            audio_cb: None,
            ready: false,
        })
    }

    async fn connect(&mut self) -> TTSResult<()> {
        // Native WS v1 transport is the sibling GW1 task; surface a typed error (never panic).
        Err(TTSError::ConnectionFailed(
            "waav-infer TTS transport (native WS v1) not yet wired in this build".into(),
        ))
    }

    async fn disconnect(&mut self) -> TTSResult<()> {
        self.ready = false;
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.ready
    }

    async fn speak(&mut self, _text: &str, _flush: bool) -> TTSResult<()> {
        Err(TTSError::ProviderNotReady(
            "waav-infer TTS not connected".into(),
        ))
    }

    async fn flush(&self) -> TTSResult<()> {
        Ok(())
    }

    fn on_audio(&mut self, callback: Arc<dyn AudioCallback>) -> TTSResult<()> {
        self.audio_cb = Some(callback);
        Ok(())
    }

    fn remove_audio_callback(&mut self) -> TTSResult<()> {
        self.audio_cb = None;
        Ok(())
    }

    fn get_provider_info(&self) -> serde_json::Value {
        serde_json::json!({
            "provider": INFER_TTS_PROVIDER_ID,
            "description": "self-hosted cascade TTS",
            "model": self.config.model,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn infer_config() -> TTSConfig {
        TTSConfig {
            provider: INFER_TTS_PROVIDER_ID.to_string(),
            model: "kokoro".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn constructs_and_reports_provider_info() {
        let tts = InferTTS::new(infer_config()).expect("infer TTS constructs");
        let info = tts.get_provider_info();
        assert_eq!(info["provider"], "waav-infer");
        assert_eq!(info["model"], "kokoro");
    }

    #[tokio::test]
    async fn speak_returns_typed_error_not_panic() {
        let mut tts = InferTTS::new(infer_config()).unwrap();
        let err = tts
            .speak("hello", true)
            .await
            .expect_err("transport not wired yet");
        assert!(matches!(err, TTSError::ProviderNotReady(_)));
    }

    #[test]
    fn audio_callback_registers_and_clears() {
        struct Noop;
        impl AudioCallback for Noop {
            fn on_audio(
                &self,
                _audio_data: super::super::base::AudioData,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
                Box::pin(async {})
            }
            fn on_error(
                &self,
                _error: TTSError,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
                Box::pin(async {})
            }
            fn on_complete(
                &self,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
                Box::pin(async {})
            }
        }
        let mut tts = InferTTS::new(infer_config()).unwrap();
        assert!(tts.audio_cb.is_none());
        tts.on_audio(Arc::new(Noop)).unwrap();
        assert!(tts.audio_cb.is_some());
        tts.remove_audio_callback().unwrap();
        assert!(tts.audio_cb.is_none());
    }
}
