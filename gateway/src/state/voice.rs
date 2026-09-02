//! Voice-endpoint resolution and the audio operations the OpenAI handlers drive.
//!
//! A Bud caller names an *endpoint* ("deepgram-tts"), not a vendor. Resolution turns that name
//! into a `VoiceEndpoint` from `voice_table` — vendor, base URL, decrypted credential, declared
//! capabilities — and the operations below drive WaaV's existing provider layer with it.
//!
//! Self-hosted deployments are not a special case here. `voice_table` carries a deployment URL
//! in the same `api_base` field a vendor uses, so a Whisper on vLLM is resolved and driven by
//! exactly this code (FRD-018 §5.2) — which is what closes the gap where budgateway returns
//! `CapabilityNotSupported` for every self-hosted audio model today.

use std::sync::Arc;

use bud_auth::credentials::VoiceEndpoint;
use waav_openai_audio::speech::SpeechSettings;
use waav_openai_audio::transcription::{Segment, TranscriptionResult, TranscriptionSettings};

use crate::state::AppState;

/// Synthesised audio, plus what the caller needs to play it.
pub struct SynthesisOutput {
    pub bytes: Vec<u8>,
    /// Only meaningful for container-less formats; the handler turns it into `X-Sample-Rate`.
    pub sample_rate: Option<u32>,
}

#[derive(Debug, thiserror::Error)]
pub enum VoiceOpError {
    #[error("the Bud control plane is not configured; voice endpoints are unavailable")]
    NoControlPlane,
    #[error("provider error: {0}")]
    Provider(String),
    #[error("endpoint '{0}' has no credential and its vendor requires one")]
    MissingCredential(String),
}

impl AppState {
    /// Look up a voice endpoint by the name the caller used, checking it serves what was asked.
    ///
    /// The capability check is not decoration: an endpoint registered for transcription will
    /// happily accept a synthesis request otherwise and fail deep inside a vendor call, with an
    /// error that names neither the endpoint nor the mistake.
    pub fn resolve_voice_endpoint(&self, name: &str, capability: &str) -> Option<VoiceEndpoint> {
        let plane = self.bud_mode.as_ref()?.plane();
        let endpoint = plane.voice_endpoint(name)?;
        if endpoint.serves(capability) {
            Some(endpoint)
        } else {
            tracing::warn!(
                endpoint = %name,
                requested = %capability,
                serves = ?endpoint.endpoints,
                "voice endpoint does not serve the requested capability"
            );
            None
        }
    }

    /// Synthesise speech through the endpoint's vendor.
    pub async fn synthesize(
        &self,
        endpoint: &VoiceEndpoint,
        settings: &SpeechSettings,
    ) -> Result<SynthesisOutput, VoiceOpError> {
        let api_key = endpoint.credential.clone().unwrap_or_default();

        // A keyless vendor is a self-hosted deployment; a keyless hosted vendor is a
        // misconfiguration worth naming rather than a 401 from the vendor five seconds later.
        if api_key.is_empty() && endpoint.vendor != "self_hosted" {
            return Err(VoiceOpError::MissingCredential(endpoint.vendor.clone()));
        }

        let tts_config = crate::core::tts::TTSConfig {
            provider: endpoint.vendor.clone(),
            api_key,
            voice_id: Some(settings.voice.clone()),
            model: endpoint.model.clone().unwrap_or_default(),
            speaking_rate: settings.speaking_rate,
            audio_format: Some(settings.format.as_waav_format().to_string()),
            ..Default::default()
        };

        let audio = crate::handlers::speak::synthesize_once(
            Arc::clone(&self.core_state),
            tts_config,
            &settings.text,
        )
        .await
        .map_err(|e| VoiceOpError::Provider(e.to_string()))?;

        Ok(SynthesisOutput {
            bytes: audio.0,
            sample_rate: audio.1,
        })
    }

    /// Transcribe (or translate) an uploaded file through the endpoint's vendor.
    pub async fn transcribe(
        &self,
        endpoint: &VoiceEndpoint,
        settings: &TranscriptionSettings,
        file: Vec<u8>,
    ) -> Result<TranscriptionResult, VoiceOpError> {
        let api_key = endpoint.credential.clone().unwrap_or_default();
        if api_key.is_empty() && endpoint.vendor != "self_hosted" {
            return Err(VoiceOpError::MissingCredential(endpoint.vendor.clone()));
        }

        let transcript = crate::core::stt::transcribe_file(
            &endpoint.vendor,
            &api_key,
            endpoint.api_base.as_deref(),
            endpoint.model.as_deref(),
            settings.language.as_deref(),
            settings.translate,
            file,
        )
        .await
        .map_err(|e| VoiceOpError::Provider(e.to_string()))?;

        Ok(TranscriptionResult {
            text: transcript.text,
            language: transcript.language,
            duration: transcript.duration,
            segments: transcript
                .segments
                .into_iter()
                .enumerate()
                .map(|(i, s)| Segment {
                    id: i as u32,
                    start: s.start,
                    end: s.end,
                    text: s.text,
                })
                .collect(),
        })
    }
}
