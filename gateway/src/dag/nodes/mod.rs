//! DAG node types and implementations
//!
//! This module defines the various node types that can be used in a DAG pipeline.
//! Each node type represents a different processing unit that can transform,
//! route, or output data.

mod input;
mod output;
mod processor;
mod provider;
mod endpoint;
mod router;
pub mod transform;

pub use input::{AudioInputNode, TextInputNode};
pub use output::{AudioOutputNode, TextOutputNode, WebhookOutputNode};
pub use processor::ProcessorNode;
pub use provider::{STTProviderNode, TTSProviderNode, RealtimeProviderNode};
pub use endpoint::{HttpEndpointNode, GrpcEndpointNode, WebSocketEndpointNode, IpcEndpointNode, LiveKitEndpointNode};
pub use router::{SplitNode, JoinNode, RouterNode};
pub use transform::{TransformNode, PassthroughNode};

use std::sync::Arc;
use async_trait::async_trait;
use bytes::Bytes;

use super::context::DAGContext;
use super::error::DAGResult;

/// Prelude for convenient imports
pub mod prelude {
    pub use super::{
        DAGNode, DAGData, NodeCapability,
        AudioInputNode, TextInputNode,
        AudioOutputNode, TextOutputNode, WebhookOutputNode,
        ProcessorNode,
        STTProviderNode, TTSProviderNode, RealtimeProviderNode,
        HttpEndpointNode, GrpcEndpointNode, WebSocketEndpointNode,
        IpcEndpointNode, LiveKitEndpointNode,
        SplitNode, JoinNode, RouterNode,
        TransformNode, PassthroughNode,
        STTResultData, TTSAudioData, WordTiming,
    };
}

/// Data flowing between DAG nodes
///
/// This enum represents all possible data types that can flow through
/// the DAG pipeline. Nodes should handle conversion between types
/// where necessary.
#[derive(Debug, Clone)]
pub enum DAGData {
    /// Raw audio bytes (PCM16, typically 16kHz mono)
    Audio(Bytes),

    /// Text content (transcriptions, prompts, responses)
    Text(String),

    /// STT result with metadata
    STTResult(STTResultData),

    /// TTS audio chunk with metadata
    TTSAudio(TTSAudioData),

    /// Generic JSON value (for HTTP/gRPC responses)
    Json(serde_json::Value),

    /// Binary blob (for raw binary data)
    Binary(Bytes),

    /// Multiple values (from Split node or parallel execution)
    Multiple(Vec<DAGData>),

    /// No data (signal completion or passthrough)
    Empty,
}

impl DAGData {
    /// Get data type name for logging/errors
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Audio(_) => "audio",
            Self::Text(_) => "text",
            Self::STTResult(_) => "stt_result",
            Self::TTSAudio(_) => "tts_audio",
            Self::Json(_) => "json",
            Self::Binary(_) => "binary",
            Self::Multiple(_) => "multiple",
            Self::Empty => "empty",
        }
    }

    /// Check if this is audio data
    pub fn is_audio(&self) -> bool {
        matches!(self, Self::Audio(_) | Self::TTSAudio(_))
    }

    /// Check if this is text data
    pub fn is_text(&self) -> bool {
        matches!(self, Self::Text(_) | Self::STTResult(_))
    }

    /// Try to extract as audio bytes
    pub fn as_audio(&self) -> Option<&Bytes> {
        match self {
            Self::Audio(b) => Some(b),
            Self::TTSAudio(t) => Some(&t.data),
            _ => None,
        }
    }

    /// Try to extract as text
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(s) => Some(s),
            Self::STTResult(r) => Some(&r.transcript),
            _ => None,
        }
    }

    /// Try to extract as JSON
    pub fn as_json(&self) -> Option<&serde_json::Value> {
        match self {
            Self::Json(j) => Some(j),
            _ => None,
        }
    }

    /// Convert to JSON representation
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Self::Audio(b) => serde_json::json!({
                "type": "audio",
                "size": b.len()
            }),
            Self::Text(s) => serde_json::json!({
                "type": "text",
                "content": s
            }),
            Self::STTResult(r) => serde_json::json!({
                "type": "stt_result",
                "transcript": r.transcript,
                "is_final": r.is_final,
                "is_speech_final": r.is_speech_final,
                "confidence": r.confidence,
                "language": r.language
            }),
            Self::TTSAudio(t) => serde_json::json!({
                "type": "tts_audio",
                "size": t.data.len(),
                "sample_rate": t.sample_rate,
                "duration_ms": t.duration_ms
            }),
            Self::Json(j) => j.clone(),
            Self::Binary(b) => serde_json::json!({
                "type": "binary",
                "size": b.len()
            }),
            Self::Multiple(items) => serde_json::json!({
                "type": "multiple",
                "count": items.len(),
                "items": items.iter().map(|i| i.to_json()).collect::<Vec<_>>()
            }),
            Self::Empty => serde_json::json!({
                "type": "empty"
            }),
        }
    }

    /// Create from JSON value (for transform results)
    pub fn from_json(value: serde_json::Value) -> Self {
        Self::Json(value)
    }
}

/// STT result data structure
#[derive(Debug, Clone)]
pub struct STTResultData {
    /// Transcribed text
    pub transcript: String,
    /// Whether this is a final result
    pub is_final: bool,
    /// Whether speech has ended (end of utterance)
    pub is_speech_final: bool,
    /// Confidence score (0.0 - 1.0)
    pub confidence: f64,
    /// Detected language (if available)
    pub language: Option<String>,
    /// Word-level timing (if available)
    pub words: Option<Vec<WordTiming>>,
    /// Raw metadata from provider
    pub metadata: serde_json::Value,
    /// Whether speech was actually detected in the audio
    /// This helps distinguish between "transcribed to empty" vs "no speech detected"
    pub speech_detected: bool,
}

impl Default for STTResultData {
    fn default() -> Self {
        Self {
            transcript: String::new(),
            is_final: false,
            is_speech_final: false,
            confidence: 0.0,
            language: None,
            words: None,
            metadata: serde_json::Value::Null,
            speech_detected: false,
        }
    }
}

/// Word-level timing information
#[derive(Debug, Clone)]
pub struct WordTiming {
    pub word: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub confidence: f64,
}

/// TTS audio data structure
#[derive(Debug, Clone)]
pub struct TTSAudioData {
    /// Raw audio bytes
    pub data: Bytes,
    /// Sample rate (Hz)
    pub sample_rate: u32,
    /// Audio format (e.g., "pcm16", "mp3")
    pub format: String,
    /// Duration in milliseconds (if known)
    pub duration_ms: Option<u64>,
    /// Whether this is the final chunk
    pub is_final: bool,
}

impl Default for TTSAudioData {
    fn default() -> Self {
        Self {
            data: Bytes::new(),
            sample_rate: 16000,
            format: "pcm16".to_string(),
            duration_ms: None,
            is_final: false,
        }
    }
}

/// Node capability flags
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeCapability {
    /// Can accept audio input
    AudioInput,
    /// Can produce audio output
    AudioOutput,
    /// Can accept text input
    TextInput,
    /// Can produce text output
    TextOutput,
    /// Can accept JSON input
    JsonInput,
    /// Can produce JSON output
    JsonOutput,
    /// Supports streaming (multiple outputs from single input)
    Streaming,
    /// Can be cancelled mid-execution
    Cancellable,
}

/// Audio processor trait for plugin-based audio processing
///
/// This trait defines the interface for audio processing plugins that can be
/// used in a DAG pipeline. Implementations typically provide noise reduction,
/// VAD, gain control, etc.
#[async_trait]
pub trait AudioProcessor: Send + Sync {
    /// Process audio samples
    ///
    /// Takes raw audio bytes and returns processed audio bytes.
    /// The format is defined by the AudioFormat parameter.
    async fn process(
        &self,
        audio: Bytes,
        format: &crate::plugin::capabilities::AudioFormat,
    ) -> Result<Bytes, String>;

    /// Get processing latency in milliseconds
    ///
    /// Returns the typical latency introduced by this processor.
    fn latency_ms(&self) -> u64 {
        0
    }

    /// Check if processor changes audio duration
    ///
    /// Returns true if the processor may change the duration of audio
    /// (e.g., time-stretching, VAD-based trimming).
    fn changes_duration(&self) -> bool {
        false
    }

    /// Get processor name for logging
    fn name(&self) -> &str;
}

/// Core trait for all DAG nodes
///
/// All node types must implement this trait to be usable in a DAG pipeline.
/// The trait defines the basic contract for node execution and metadata.
#[async_trait]
pub trait DAGNode: Send + Sync {
    /// Get the node ID
    fn id(&self) -> &str;

    /// Get the node type name
    fn node_type(&self) -> &str;

    /// Get node capabilities
    fn capabilities(&self) -> Vec<NodeCapability>;

    /// Execute the node with given input data
    async fn execute(&self, input: DAGData, ctx: &mut DAGContext) -> DAGResult<DAGData>;

    /// Check if node accepts the given input type
    fn accepts_input(&self, data: &DAGData) -> bool {
        let caps = self.capabilities();
        match data {
            DAGData::Audio(_) | DAGData::TTSAudio(_) => {
                caps.contains(&NodeCapability::AudioInput)
            }
            DAGData::Text(_) | DAGData::STTResult(_) => {
                caps.contains(&NodeCapability::TextInput)
            }
            DAGData::Json(_) => caps.contains(&NodeCapability::JsonInput),
            DAGData::Binary(_) | DAGData::Multiple(_) | DAGData::Empty => true,
        }
    }

    /// Check if node supports cancellation
    fn is_cancellable(&self) -> bool {
        self.capabilities().contains(&NodeCapability::Cancellable)
    }

    /// Check if node supports streaming output
    fn is_streaming(&self) -> bool {
        self.capabilities().contains(&NodeCapability::Streaming)
    }

    /// Clone as Arc (for use in compiled DAG)
    fn clone_boxed(&self) -> Arc<dyn DAGNode>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dag_data_type_names() {
        assert_eq!(DAGData::Audio(Bytes::new()).type_name(), "audio");
        assert_eq!(DAGData::Text("test".to_string()).type_name(), "text");
        assert_eq!(DAGData::Empty.type_name(), "empty");
    }

    #[test]
    fn test_dag_data_is_methods() {
        let audio = DAGData::Audio(Bytes::from("test"));
        assert!(audio.is_audio());
        assert!(!audio.is_text());

        let text = DAGData::Text("hello".to_string());
        assert!(!text.is_audio());
        assert!(text.is_text());
    }

    #[test]
    fn test_dag_data_to_json() {
        let stt = DAGData::STTResult(STTResultData {
            transcript: "hello".to_string(),
            is_final: true,
            confidence: 0.95,
            ..Default::default()
        });

        let json = stt.to_json();
        assert_eq!(json["transcript"], "hello");
        assert_eq!(json["is_final"], true);
        assert_eq!(json["confidence"], 0.95);
    }

    #[test]
    fn test_stt_result_default() {
        let result = STTResultData::default();
        assert!(result.transcript.is_empty());
        assert!(!result.is_final);
        assert_eq!(result.confidence, 0.0);
    }
}
