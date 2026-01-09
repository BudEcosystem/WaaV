//! Output nodes for DAG pipelines
//!
//! These nodes serve as exit points for data from the DAG.
//! They route processed data to external destinations (WebSocket, LiveKit, webhooks).

use std::sync::Arc;
use async_trait::async_trait;
use tracing::{debug, warn};

use super::{DAGNode, DAGData, NodeCapability, TTSAudioData};
use crate::dag::context::DAGContext;
use crate::dag::definition::OutputDestination;
use crate::dag::error::{DAGError, DAGResult};

/// Audio output node
///
/// Routes audio data to the configured destination (WebSocket, LiveKit, etc.).
#[derive(Debug, Clone)]
pub struct AudioOutputNode {
    id: String,
    destination: OutputDestination,
    /// Output sample rate (for format conversion if needed)
    output_sample_rate: Option<u32>,
}

impl AudioOutputNode {
    /// Create a new audio output node
    pub fn new(id: impl Into<String>, destination: OutputDestination) -> Self {
        Self {
            id: id.into(),
            destination,
            output_sample_rate: None,
        }
    }

    /// Create with WebSocket destination (default)
    pub fn websocket(id: impl Into<String>) -> Self {
        Self::new(id, OutputDestination::WebSocket)
    }

    /// Create with LiveKit destination
    pub fn livekit(id: impl Into<String>) -> Self {
        Self::new(id, OutputDestination::LiveKit)
    }

    /// Set output sample rate
    pub fn with_sample_rate(mut self, rate: u32) -> Self {
        self.output_sample_rate = Some(rate);
        self
    }

    /// Get the destination
    pub fn destination(&self) -> &OutputDestination {
        &self.destination
    }
}

#[async_trait]
impl DAGNode for AudioOutputNode {
    fn id(&self) -> &str {
        &self.id
    }

    fn node_type(&self) -> &str {
        "audio_output"
    }

    fn capabilities(&self) -> Vec<NodeCapability> {
        vec![
            NodeCapability::AudioInput,
            NodeCapability::Streaming,
            NodeCapability::Cancellable,
        ]
    }

    async fn execute(&self, input: DAGData, ctx: &mut DAGContext) -> DAGResult<DAGData> {
        // Validate input is audio
        let audio_data = match &input {
            DAGData::Audio(bytes) => TTSAudioData {
                data: bytes.clone(),
                sample_rate: self.output_sample_rate.unwrap_or(16000),
                format: "pcm16".to_string(),
                duration_ms: None,
                is_final: false,
            },
            DAGData::TTSAudio(tts) => tts.clone(),
            DAGData::Empty => {
                debug!(node_id = %self.id, "Audio output received empty data, passing through");
                return Ok(input);
            }
            other => {
                return Err(DAGError::UnsupportedDataType {
                    expected: "audio".to_string(),
                    actual: other.type_name().to_string(),
                });
            }
        };

        // The actual routing to WebSocket/LiveKit is handled by the executor
        // based on the destination configuration. Here we just validate and
        // prepare the data.

        debug!(
            node_id = %self.id,
            destination = ?self.destination,
            audio_size = %audio_data.data.len(),
            sample_rate = %audio_data.sample_rate,
            "Routing audio to output"
        );

        // Store destination info in context for executor
        ctx.metadata.insert("output_destination".to_string(), format!("{:?}", self.destination));

        Ok(DAGData::TTSAudio(audio_data))
    }

    fn clone_boxed(&self) -> Arc<dyn DAGNode> {
        Arc::new(self.clone())
    }
}

/// Text output node
///
/// Routes text data to the configured destination (WebSocket messages, etc.).
#[derive(Debug, Clone)]
pub struct TextOutputNode {
    id: String,
    destination: OutputDestination,
    /// Message type for WebSocket output
    message_type: Option<String>,
}

impl TextOutputNode {
    /// Create a new text output node
    pub fn new(id: impl Into<String>, destination: OutputDestination) -> Self {
        Self {
            id: id.into(),
            destination,
            message_type: None,
        }
    }

    /// Create with WebSocket destination
    pub fn websocket(id: impl Into<String>) -> Self {
        Self::new(id, OutputDestination::WebSocket)
    }

    /// Set custom message type for WebSocket output
    pub fn with_message_type(mut self, msg_type: impl Into<String>) -> Self {
        self.message_type = Some(msg_type.into());
        self
    }

    /// Get the destination
    pub fn destination(&self) -> &OutputDestination {
        &self.destination
    }
}

#[async_trait]
impl DAGNode for TextOutputNode {
    fn id(&self) -> &str {
        &self.id
    }

    fn node_type(&self) -> &str {
        "text_output"
    }

    fn capabilities(&self) -> Vec<NodeCapability> {
        vec![
            NodeCapability::TextInput,
            NodeCapability::JsonInput,
            NodeCapability::TextOutput,
            NodeCapability::JsonOutput,
        ]
    }

    async fn execute(&self, input: DAGData, ctx: &mut DAGContext) -> DAGResult<DAGData> {
        // Extract text content
        let text = match &input {
            DAGData::Text(t) => t.clone(),
            DAGData::STTResult(r) => r.transcript.clone(),
            DAGData::Json(j) => {
                // Try to extract text from common fields
                j.get("text")
                    .or_else(|| j.get("content"))
                    .or_else(|| j.get("message"))
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .unwrap_or_else(|| j.to_string())
            }
            DAGData::Empty => {
                debug!(node_id = %self.id, "Text output received empty data");
                return Ok(input);
            }
            other => {
                return Err(DAGError::UnsupportedDataType {
                    expected: "text".to_string(),
                    actual: other.type_name().to_string(),
                });
            }
        };

        debug!(
            node_id = %self.id,
            destination = ?self.destination,
            text_length = %text.len(),
            "Routing text to output"
        );

        // Store destination info in context for executor
        ctx.metadata.insert("output_destination".to_string(), format!("{:?}", self.destination));
        if let Some(ref msg_type) = self.message_type {
            ctx.metadata.insert("message_type".to_string(), msg_type.clone());
        }

        Ok(DAGData::Text(text))
    }

    fn clone_boxed(&self) -> Arc<dyn DAGNode> {
        Arc::new(self.clone())
    }
}

/// Webhook output node
///
/// Sends data to a webhook endpoint (fire-and-forget).
/// This is useful for notifications, logging, and async integrations.
/// The HTTP client is created once and reused for all requests (connection pooling).
#[derive(Clone)]
pub struct WebhookOutputNode {
    id: String,
    url: String,
    headers: std::collections::HashMap<String, String>,
    /// Timeout for webhook request (ms)
    timeout_ms: u64,
    /// Whether to wait for response
    fire_and_forget: bool,
    /// Pooled HTTP client for connection reuse
    client: reqwest::Client,
}

impl std::fmt::Debug for WebhookOutputNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebhookOutputNode")
            .field("id", &self.id)
            .field("url", &self.url)
            .field("headers", &self.headers)
            .field("timeout_ms", &self.timeout_ms)
            .field("fire_and_forget", &self.fire_and_forget)
            .finish()
    }
}

impl WebhookOutputNode {
    /// Create a new webhook output node
    ///
    /// The HTTP client is created once during construction with default settings
    /// for connection pooling and keep-alive.
    pub fn new(id: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            url: url.into(),
            headers: std::collections::HashMap::new(),
            timeout_ms: 5000,
            fire_and_forget: true,
            client: reqwest::Client::new(),
        }
    }

    /// Add a header to the webhook request
    pub fn with_header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(key.into(), value.into());
        self
    }

    /// Set request timeout
    pub fn with_timeout_ms(mut self, timeout: u64) -> Self {
        self.timeout_ms = timeout;
        self
    }

    /// Wait for webhook response (don't fire-and-forget)
    pub fn wait_for_response(mut self) -> Self {
        self.fire_and_forget = false;
        self
    }

    /// Get the URL
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Get headers
    pub fn headers(&self) -> &std::collections::HashMap<String, String> {
        &self.headers
    }
}

#[async_trait]
impl DAGNode for WebhookOutputNode {
    fn id(&self) -> &str {
        &self.id
    }

    fn node_type(&self) -> &str {
        "webhook_output"
    }

    fn capabilities(&self) -> Vec<NodeCapability> {
        vec![
            NodeCapability::TextInput,
            NodeCapability::JsonInput,
            NodeCapability::AudioInput,
        ]
    }

    async fn execute(&self, input: DAGData, ctx: &mut DAGContext) -> DAGResult<DAGData> {
        // Convert input to JSON for webhook payload
        let payload = input.to_json();

        debug!(
            node_id = %self.id,
            url = %self.url,
            fire_and_forget = %self.fire_and_forget,
            "Sending webhook"
        );

        // Use the pre-created pooled client for connection reuse
        let mut request = self.client
            .post(&self.url)
            .timeout(std::time::Duration::from_millis(self.timeout_ms))
            .header("Content-Type", "application/json")
            .header("X-Stream-ID", &ctx.stream_id);

        // Add custom headers
        for (key, value) in &self.headers {
            request = request.header(key, value);
        }

        // Add API key if available
        if let Some(api_key_id) = &ctx.api_key_id {
            request = request.header("X-API-Key-ID", api_key_id);
        }

        request = request.json(&payload);

        if self.fire_and_forget {
            // Spawn task and don't wait
            let url = self.url.clone();
            let node_id = self.id.clone();
            tokio::spawn(async move {
                match request.send().await {
                    Ok(response) => {
                        if !response.status().is_success() {
                            warn!(
                                node_id = %node_id,
                                url = %url,
                                status = %response.status(),
                                "Webhook returned non-success status"
                            );
                        }
                    }
                    Err(e) => {
                        warn!(
                            node_id = %node_id,
                            url = %url,
                            error = %e,
                            "Webhook request failed"
                        );
                    }
                }
            });

            // Return empty immediately
            Ok(DAGData::Empty)
        } else {
            // Wait for response
            let response = request.send().await.map_err(|e| DAGError::WebhookDeliveryError {
                url: self.url.clone(),
                error: e.to_string(),
            })?;

            if !response.status().is_success() {
                return Err(DAGError::WebhookDeliveryError {
                    url: self.url.clone(),
                    error: format!("HTTP {}", response.status()),
                });
            }

            // Try to parse response as JSON
            match response.json::<serde_json::Value>().await {
                Ok(json) => Ok(DAGData::Json(json)),
                Err(_) => Ok(DAGData::Empty),
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
    async fn test_audio_output_passthrough() {
        let node = AudioOutputNode::websocket("audio_out");
        let mut ctx = DAGContext::new("test");

        let input = DAGData::Audio(Bytes::from("audio data"));
        let output = node.execute(input, &mut ctx).await.unwrap();

        assert!(matches!(output, DAGData::TTSAudio(_)));
    }

    #[tokio::test]
    async fn test_text_output_passthrough() {
        let node = TextOutputNode::websocket("text_out");
        let mut ctx = DAGContext::new("test");

        let input = DAGData::Text("hello world".to_string());
        let output = node.execute(input, &mut ctx).await.unwrap();

        if let DAGData::Text(text) = output {
            assert_eq!(text, "hello world");
        } else {
            panic!("Expected text output");
        }
    }

    #[tokio::test]
    async fn test_text_output_from_stt() {
        let node = TextOutputNode::websocket("text_out");
        let mut ctx = DAGContext::new("test");

        let input = DAGData::STTResult(super::super::STTResultData {
            transcript: "transcribed text".to_string(),
            is_final: true,
            ..Default::default()
        });
        let output = node.execute(input, &mut ctx).await.unwrap();

        if let DAGData::Text(text) = output {
            assert_eq!(text, "transcribed text");
        } else {
            panic!("Expected text output");
        }
    }

    #[test]
    fn test_webhook_builder() {
        let node = WebhookOutputNode::new("webhook", "https://example.com/hook")
            .with_header("Authorization", "Bearer token")
            .with_timeout_ms(10000)
            .wait_for_response();

        assert_eq!(node.url(), "https://example.com/hook");
        assert!(node.headers().contains_key("Authorization"));
        assert_eq!(node.timeout_ms, 10000);
        assert!(!node.fire_and_forget);
    }

    #[test]
    fn test_audio_output_capabilities() {
        let node = AudioOutputNode::websocket("audio_out");
        let caps = node.capabilities();

        assert!(caps.contains(&NodeCapability::AudioInput));
        assert!(caps.contains(&NodeCapability::Streaming));
    }
}
