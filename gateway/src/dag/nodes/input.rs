//! Input nodes for DAG pipelines
//!
//! These nodes serve as entry points for data into the DAG.
//! They receive data from external sources (WebSocket, LiveKit, etc.)
//! and pass it into the pipeline.

use std::sync::Arc;
use async_trait::async_trait;

use super::{DAGNode, DAGData, NodeCapability};
use crate::dag::context::DAGContext;
use crate::dag::error::DAGResult;

/// Audio input node
///
/// Receives audio data from WebSocket or LiveKit and passes it into the pipeline.
/// This is typically the entry point for real-time voice processing.
#[derive(Debug, Clone)]
pub struct AudioInputNode {
    id: String,
    /// Expected sample rate (for validation)
    expected_sample_rate: Option<u32>,
    /// Expected audio format
    expected_format: Option<String>,
}

impl AudioInputNode {
    /// Create a new audio input node
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            expected_sample_rate: None,
            expected_format: None,
        }
    }

    /// Set expected sample rate for validation
    pub fn with_sample_rate(mut self, rate: u32) -> Self {
        self.expected_sample_rate = Some(rate);
        self
    }

    /// Set expected format for validation
    pub fn with_format(mut self, format: impl Into<String>) -> Self {
        self.expected_format = Some(format.into());
        self
    }
}

#[async_trait]
impl DAGNode for AudioInputNode {
    fn id(&self) -> &str {
        &self.id
    }

    fn node_type(&self) -> &str {
        "audio_input"
    }

    fn capabilities(&self) -> Vec<NodeCapability> {
        vec![
            NodeCapability::AudioInput,
            NodeCapability::AudioOutput,
            NodeCapability::Streaming,
        ]
    }

    async fn execute(&self, input: DAGData, _ctx: &mut DAGContext) -> DAGResult<DAGData> {
        // Audio input node is a passthrough - it just validates and forwards audio
        match &input {
            DAGData::Audio(_) | DAGData::TTSAudio(_) => Ok(input),
            DAGData::Binary(bytes) => {
                // Convert binary to audio
                Ok(DAGData::Audio(bytes.clone()))
            }
            other => {
                tracing::warn!(
                    node_id = %self.id,
                    input_type = %other.type_name(),
                    "Audio input node received non-audio data, passing through"
                );
                Ok(input)
            }
        }
    }

    fn clone_boxed(&self) -> Arc<dyn DAGNode> {
        Arc::new(self.clone())
    }
}

/// Text input node
///
/// Receives text data from WebSocket messages and passes it into the pipeline.
/// Used for text-based interactions (chat messages, prompts, etc.).
#[derive(Debug, Clone)]
pub struct TextInputNode {
    id: String,
    /// Maximum allowed text length
    max_length: Option<usize>,
    /// Trim whitespace
    trim: bool,
}

impl TextInputNode {
    /// Create a new text input node
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            max_length: None,
            trim: true,
        }
    }

    /// Set maximum allowed text length
    pub fn with_max_length(mut self, max: usize) -> Self {
        self.max_length = Some(max);
        self
    }

    /// Disable whitespace trimming
    pub fn without_trim(mut self) -> Self {
        self.trim = false;
        self
    }
}

#[async_trait]
impl DAGNode for TextInputNode {
    fn id(&self) -> &str {
        &self.id
    }

    fn node_type(&self) -> &str {
        "text_input"
    }

    fn capabilities(&self) -> Vec<NodeCapability> {
        vec![
            NodeCapability::TextInput,
            NodeCapability::TextOutput,
            NodeCapability::JsonInput,
        ]
    }

    async fn execute(&self, input: DAGData, _ctx: &mut DAGContext) -> DAGResult<DAGData> {
        match input {
            DAGData::Text(text) => {
                let processed = if self.trim {
                    text.trim().to_string()
                } else {
                    text
                };

                // Check max length
                if let Some(max) = self.max_length {
                    if processed.len() > max {
                        return Ok(DAGData::Text(processed[..max].to_string()));
                    }
                }

                Ok(DAGData::Text(processed))
            }
            DAGData::Json(json) => {
                // Try to extract text from JSON
                if let Some(text) = json.as_str() {
                    Ok(DAGData::Text(text.to_string()))
                } else if let Some(text) = json.get("text").and_then(|v| v.as_str()) {
                    Ok(DAGData::Text(text.to_string()))
                } else if let Some(text) = json.get("content").and_then(|v| v.as_str()) {
                    Ok(DAGData::Text(text.to_string()))
                } else if let Some(text) = json.get("message").and_then(|v| v.as_str()) {
                    Ok(DAGData::Text(text.to_string()))
                } else {
                    Ok(DAGData::Json(json))
                }
            }
            DAGData::STTResult(result) => {
                // Pass through STT result (it contains text)
                Ok(DAGData::STTResult(result))
            }
            other => {
                tracing::warn!(
                    node_id = %self.id,
                    input_type = %other.type_name(),
                    "Text input node received non-text data, passing through"
                );
                Ok(other)
            }
        }
    }

    fn clone_boxed(&self) -> Arc<dyn DAGNode> {
        Arc::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    #[tokio::test]
    async fn test_audio_input_passthrough() {
        let node = AudioInputNode::new("audio_in");
        let mut ctx = DAGContext::new("test");

        let input = DAGData::Audio(Bytes::from("audio data"));
        let output = node.execute(input.clone(), &mut ctx).await.unwrap();

        assert!(matches!(output, DAGData::Audio(_)));
    }

    #[tokio::test]
    async fn test_audio_input_binary_conversion() {
        let node = AudioInputNode::new("audio_in");
        let mut ctx = DAGContext::new("test");

        let input = DAGData::Binary(Bytes::from("binary audio"));
        let output = node.execute(input, &mut ctx).await.unwrap();

        assert!(matches!(output, DAGData::Audio(_)));
    }

    #[tokio::test]
    async fn test_text_input_trim() {
        let node = TextInputNode::new("text_in");
        let mut ctx = DAGContext::new("test");

        let input = DAGData::Text("  hello world  ".to_string());
        let output = node.execute(input, &mut ctx).await.unwrap();

        if let DAGData::Text(text) = output {
            assert_eq!(text, "hello world");
        } else {
            panic!("Expected text output");
        }
    }

    #[tokio::test]
    async fn test_text_input_max_length() {
        let node = TextInputNode::new("text_in").with_max_length(5);
        let mut ctx = DAGContext::new("test");

        let input = DAGData::Text("hello world".to_string());
        let output = node.execute(input, &mut ctx).await.unwrap();

        if let DAGData::Text(text) = output {
            assert_eq!(text, "hello");
        } else {
            panic!("Expected text output");
        }
    }

    #[tokio::test]
    async fn test_text_input_from_json() {
        let node = TextInputNode::new("text_in");
        let mut ctx = DAGContext::new("test");

        let input = DAGData::Json(serde_json::json!({ "text": "from json" }));
        let output = node.execute(input, &mut ctx).await.unwrap();

        if let DAGData::Text(text) = output {
            assert_eq!(text, "from json");
        } else {
            panic!("Expected text output");
        }
    }

    #[test]
    fn test_audio_input_capabilities() {
        let node = AudioInputNode::new("audio_in");
        let caps = node.capabilities();

        assert!(caps.contains(&NodeCapability::AudioInput));
        assert!(caps.contains(&NodeCapability::AudioOutput));
        assert!(caps.contains(&NodeCapability::Streaming));
    }
}
