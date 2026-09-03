//! Plugin-based processor nodes
//!
//! These nodes wrap plugin-based audio and text processors for use in DAG pipelines.

use async_trait::async_trait;
use std::sync::Arc;
use tracing::{debug, info};

use super::{DAGData, DAGNode, NodeCapability};
use crate::dag::context::DAGContext;
use crate::dag::error::{DAGError, DAGResult};
use crate::plugin::{capabilities::AudioFormat, global_registry};

/// Plugin-based processor node
///
/// Wraps a plugin processor (e.g., VAD, noise filter, text normalizer)
/// for use in a DAG pipeline.
#[derive(Clone)]
pub struct ProcessorNode {
    id: String,
    plugin_id: String,
    config: serde_json::Value,
    /// Whether this processor handles audio
    is_audio_processor: bool,
    /// Whether this processor handles text
    is_text_processor: bool,
}

impl ProcessorNode {
    /// Create a new processor node
    pub fn new(id: impl Into<String>, plugin_id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            plugin_id: plugin_id.into(),
            config: serde_json::Value::Null,
            is_audio_processor: true,
            is_text_processor: false,
        }
    }

    /// Create an audio processor node
    pub fn audio_processor(id: impl Into<String>, plugin_id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            plugin_id: plugin_id.into(),
            config: serde_json::Value::Null,
            is_audio_processor: true,
            is_text_processor: false,
        }
    }

    /// Create a text processor node
    pub fn text_processor(id: impl Into<String>, plugin_id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            plugin_id: plugin_id.into(),
            config: serde_json::Value::Null,
            is_audio_processor: false,
            is_text_processor: true,
        }
    }

    /// Set processor configuration
    pub fn with_config(mut self, config: serde_json::Value) -> Self {
        self.config = config;
        self
    }

    /// Get plugin ID
    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    /// Get configuration
    pub fn config(&self) -> &serde_json::Value {
        &self.config
    }
}

impl std::fmt::Debug for ProcessorNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessorNode")
            .field("id", &self.id)
            .field("plugin_id", &self.plugin_id)
            .field("is_audio_processor", &self.is_audio_processor)
            .field("is_text_processor", &self.is_text_processor)
            .finish()
    }
}

#[async_trait]
impl DAGNode for ProcessorNode {
    fn id(&self) -> &str {
        &self.id
    }

    fn node_type(&self) -> &str {
        "processor"
    }

    fn capabilities(&self) -> Vec<NodeCapability> {
        let mut caps = Vec::new();
        if self.is_audio_processor {
            caps.push(NodeCapability::AudioInput);
            caps.push(NodeCapability::AudioOutput);
        }
        if self.is_text_processor {
            caps.push(NodeCapability::TextInput);
            caps.push(NodeCapability::TextOutput);
        }
        caps.push(NodeCapability::Streaming);
        caps
    }

    async fn execute(&self, input: DAGData, ctx: &mut DAGContext) -> DAGResult<DAGData> {
        debug!(
            node_id = %self.id,
            plugin_id = %self.plugin_id,
            input_type = %input.type_name(),
            "Executing processor"
        );

        // Only process audio if this is an audio processor
        if self.is_audio_processor {
            return self.process_audio(input, ctx).await;
        }

        // Text processors are not yet implemented through the plugin system
        // They would need a separate TextProcessor capability
        if self.is_text_processor {
            return Err(DAGError::processor_error(
                &self.plugin_id,
                "text processor plugins are not implemented",
            ));
        }

        Err(DAGError::processor_error(
            &self.plugin_id,
            "processor node has no supported processor type",
        ))
    }

    fn clone_boxed(&self) -> Arc<dyn DAGNode> {
        Arc::new(self.clone())
    }
}

impl ProcessorNode {
    /// Process audio data through the plugin registry
    async fn process_audio(&self, input: DAGData, _ctx: &mut DAGContext) -> DAGResult<DAGData> {
        // Extract audio data from input
        let audio_bytes = match &input {
            DAGData::Audio(bytes) => bytes.clone(),
            DAGData::TTSAudio(audio_data) => audio_data.data.clone(),
            other => {
                return Err(DAGError::data_type_error("audio", other.type_name()));
            }
        };

        // Get audio format from config or use default
        let format = self.get_audio_format();

        // Try to create the processor from the registry
        let registry = global_registry();

        if !registry.has_audio_processor(&self.plugin_id) {
            return Err(DAGError::processor_error(
                &self.plugin_id,
                "audio processor plugin is not registered",
            ));
        }

        // Create the processor with configuration
        let processor = registry
            .create_audio_processor(&self.plugin_id, self.config.clone())
            .map_err(|e| DAGError::processor_error(&self.plugin_id, e))?;

        info!(
            node_id = %self.id,
            plugin_id = %self.plugin_id,
            latency_ms = processor.latency_ms(),
            changes_duration = processor.changes_duration(),
            "Created audio processor"
        );

        // Process the audio
        let processed_bytes = processor
            .process(audio_bytes, &format)
            .await
            .map_err(|e| DAGError::processor_error(&self.plugin_id, e))?;

        debug!(
            node_id = %self.id,
            plugin_id = %self.plugin_id,
            input_len = %match &input { DAGData::Audio(b) => b.len(), DAGData::TTSAudio(a) => a.data.len(), _ => 0 },
            output_len = %processed_bytes.len(),
            "Audio processing complete"
        );

        Ok(DAGData::Audio(processed_bytes))
    }

    /// Extract audio format from node configuration or use defaults
    fn get_audio_format(&self) -> AudioFormat {
        // Try to extract format from config
        if let Some(config_obj) = self.config.as_object() {
            let sample_rate = config_obj
                .get("sample_rate")
                .and_then(|v| v.as_u64())
                .map(|v| v as u32)
                .unwrap_or(16000);

            let channels = config_obj
                .get("channels")
                .and_then(|v| v.as_u64())
                .map(|v| v as u16)
                .unwrap_or(1);

            let bits_per_sample = config_obj
                .get("bits_per_sample")
                .and_then(|v| v.as_u64())
                .map(|v| v as u16)
                .unwrap_or(16);

            return AudioFormat {
                sample_rate,
                channels,
                bits_per_sample,
                ..Default::default()
            };
        }

        AudioFormat::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    #[tokio::test]
    async fn test_processor_fails_when_audio_plugin_missing() {
        let node = ProcessorNode::new("proc", "nonexistent_plugin");
        let mut ctx = DAGContext::new("test");

        let input = DAGData::Audio(Bytes::from("test audio"));
        let err = node
            .execute(input, &mut ctx)
            .await
            .expect_err("missing processor plugin must fail closed, not pass audio through");

        assert!(matches!(err, DAGError::AudioProcessorError { .. }));
        assert!(
            err.to_string().contains("not registered"),
            "error should name the missing plugin registry state: {err}"
        );
    }

    #[tokio::test]
    async fn test_text_processor_fails_instead_of_passthrough() {
        let node = ProcessorNode::text_processor("proc", "normalizer");
        let mut ctx = DAGContext::new("test");

        let err = node
            .execute(DAGData::Text("hello".to_string()), &mut ctx)
            .await
            .expect_err("unsupported text processors must fail closed, not pass text through");

        assert!(matches!(err, DAGError::AudioProcessorError { .. }));
        assert!(
            err.to_string().contains("not implemented"),
            "error should make the unsupported processor type explicit: {err}"
        );
    }

    #[tokio::test]
    async fn test_audio_processor_rejects_non_audio_input() {
        let node = ProcessorNode::audio_processor("proc", "vad");
        let mut ctx = DAGContext::new("test");

        let err = node
            .execute(DAGData::Text("not audio".to_string()), &mut ctx)
            .await
            .expect_err("audio processor fed text must fail typed, not pass data through");

        assert!(matches!(err, DAGError::UnsupportedDataType { .. }));
    }

    #[test]
    fn test_processor_capabilities() {
        let audio_proc = ProcessorNode::audio_processor("proc", "vad");
        let caps = audio_proc.capabilities();
        assert!(caps.contains(&NodeCapability::AudioInput));
        assert!(caps.contains(&NodeCapability::AudioOutput));

        let text_proc = ProcessorNode::text_processor("proc", "normalizer");
        let caps = text_proc.capabilities();
        assert!(caps.contains(&NodeCapability::TextInput));
        assert!(caps.contains(&NodeCapability::TextOutput));
    }
}
