//! Tinkoff VoiceKit gRPC Client
//!
//! Implements bidirectional streaming gRPC client for Tinkoff's SpeechToText service.
//! Uses tonic for gRPC transport with TLS and API key authentication.
//!
//! ## Service Definition
//!
//! ```protobuf
//! service SpeechToText {
//!     rpc StreamingRecognize(stream StreamingRecognizeRequest)
//!         returns (stream StreamingRecognizeResponse);
//! }
//! ```

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::{Buf, BufMut, Bytes};
use futures::Stream;
use tokio::sync::mpsc;
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};
use tonic::{Request, Status, Streaming};
use tracing::{debug, info, warn};

use super::config::TinkoffSttConfig;
use super::messages::{
    StreamingRecognitionConfig, StreamingRecognizeRequest, StreamingRecognizeResponse,
};
use super::{GRPC_SERVICE_PATH, TINKOFF_GRPC_ENDPOINT};
use crate::core::stt::base::STTError;

/// Create a gRPC channel to Tinkoff VoiceKit
///
/// This establishes a secure TLS connection to Tinkoff's STT service.
pub async fn create_tinkoff_channel(config: &TinkoffSttConfig) -> Result<Channel, STTError> {
    // Create TLS configuration
    let tls_config = ClientTlsConfig::new().domain_name("api.tinkoff.ai");

    // Build and connect the channel
    let channel = Endpoint::from_static(TINKOFF_GRPC_ENDPOINT)
        .tls_config(tls_config)
        .map_err(|e| STTError::ConfigurationError(format!("TLS config error: {}", e)))?
        .connect_timeout(Duration::from_secs(config.connection_timeout_secs))
        .timeout(Duration::from_secs(config.request_timeout_secs))
        .connect()
        .await
        .map_err(|e| STTError::ConnectionFailed(format!("gRPC connection failed: {}", e)))?;

    info!("Connected to Tinkoff VoiceKit gRPC endpoint");
    Ok(channel)
}

/// Create metadata map with Tinkoff authentication headers
///
/// All requests to Tinkoff's API require these headers for authentication.
pub fn create_tinkoff_metadata(
    config: &TinkoffSttConfig,
) -> Result<tonic::metadata::MetadataMap, STTError> {
    let mut metadata = tonic::metadata::MetadataMap::new();

    // Authentication headers
    metadata.insert(
        "x-api-key",
        parse_header_value(&config.api_key, "x-api-key")?,
    );
    metadata.insert(
        "x-secret-key",
        parse_header_value(&config.secret_key, "x-secret-key")?,
    );

    Ok(metadata)
}

/// Parse a string into an ASCII metadata value
fn parse_header_value(
    value: &str,
    name: &str,
) -> Result<tonic::metadata::AsciiMetadataValue, STTError> {
    value
        .parse()
        .map_err(|_| STTError::ConfigurationError(format!("Invalid {} header value", name)))
}

/// Tinkoff gRPC streaming client
///
/// Implements bidirectional streaming for real-time speech recognition.
pub struct TinkoffGrpcClient {
    channel: Channel,
    config: TinkoffSttConfig,
}

impl TinkoffGrpcClient {
    /// Create a new gRPC client
    pub fn new(channel: Channel, config: TinkoffSttConfig) -> Self {
        Self { channel, config }
    }

    /// Open a fresh bidirectional `StreamingRecognize` stream over the given audio receiver.
    ///
    /// The featured session is re-established intrinsically by opening a new stream: the request
    /// metadata (x-api-key / x-secret-key) is rebuilt from config, and [`AudioChunkStream`] sends
    /// the FIRST request as the featured `StreamingRecognitionConfig` (encoding, language,
    /// VAD/diarization/profanity/speech-contexts, interim-results), then audio. This is what the
    /// supervised transport calls per (re)connect, so a reconnect restores the *featured* session
    /// rather than a bare one. Returns the raw response stream for the caller to drain; a
    /// connection-level failure here is classified reconnectable vs fatal by
    /// [`classify_grpc_outcome`].
    pub(super) async fn open_stream(
        &self,
        audio_rx: mpsc::Receiver<Bytes>,
        streaming_config: StreamingRecognitionConfig,
    ) -> Result<Streaming<Bytes>, Status> {
        // Create the request stream (re-yields the featured config first, then audio).
        let request_stream = AudioChunkStream::new(audio_rx, streaming_config);

        // Create metadata
        let metadata = create_tinkoff_metadata(&self.config)
            .map_err(|e| Status::invalid_argument(e.to_string()))?;

        // Create the gRPC request with metadata
        let mut request = Request::new(request_stream);
        *request.metadata_mut() = metadata;

        // Clone channel for the streaming call
        let channel = self.channel.clone();
        do_streaming_recognize(channel, request).await
    }

    /// Start a bidirectional streaming session
    ///
    /// Returns a sender for audio chunks and a receiver for transcription results.
    #[allow(dead_code)] // Retained for the non-supervised path / tests; the supervised transport
    // drives `open_stream` directly.
    pub async fn start_streaming(
        &self,
        streaming_config: StreamingRecognitionConfig,
    ) -> Result<
        (
            mpsc::Sender<Bytes>,
            mpsc::Receiver<Result<StreamingRecognizeResponse, STTError>>,
        ),
        STTError,
    > {
        // Create channels for communication
        let (audio_tx, audio_rx) = mpsc::channel::<Bytes>(100);
        let (result_tx, result_rx) =
            mpsc::channel::<Result<StreamingRecognizeResponse, STTError>>(100);

        // Create the request stream
        let request_stream = AudioChunkStream::new(audio_rx, streaming_config);

        // Create metadata
        let metadata = create_tinkoff_metadata(&self.config)?;

        // Create the gRPC request with metadata
        let mut request = Request::new(request_stream);
        *request.metadata_mut() = metadata;

        // Clone channel for the streaming task
        let channel = self.channel.clone();

        // Spawn the streaming task
        tokio::spawn(async move {
            match do_streaming_recognize(channel, request).await {
                Ok(response_stream) => {
                    process_response_stream(response_stream, result_tx).await;
                }
                Err(e) => {
                    let _ = result_tx
                        .send(Err(STTError::ConnectionFailed(format!(
                            "gRPC call failed: {}",
                            e
                        ))))
                        .await;
                }
            }
        });

        Ok((audio_tx, result_rx))
    }
}

/// Map a gRPC status to a [`ReconnectOutcome`](crate::core::websocket::reconnectable_stream::ReconnectOutcome):
/// transient transport failures (Unavailable, DeadlineExceeded, Internal, Cancelled, Aborted,
/// ResourceExhausted) are reconnectable; auth/permission/argument errors are fatal (retrying would
/// fail identically). Mirrors Tinkoff's own `is_retriable` taxonomy.
pub(super) fn classify_grpc_outcome(
    status: tonic::Status,
) -> crate::core::websocket::reconnectable_stream::ReconnectOutcome {
    use crate::core::websocket::reconnectable_stream::{ReconnectOutcome, StreamError};
    use tonic::Code;
    match status.code() {
        Code::Unavailable
        | Code::DeadlineExceeded
        | Code::Internal
        | Code::Cancelled
        | Code::Aborted
        | Code::ResourceExhausted => {
            ReconnectOutcome::Reconnectable(StreamError::new(format!("grpc {}", status.code())))
        }
        _ => ReconnectOutcome::Fatal(StreamError::new(format!("grpc {}", status.code()))),
    }
}

/// Perform the StreamingRecognize gRPC call using tonic's low-level Grpc client
async fn do_streaming_recognize<S>(
    channel: Channel,
    request: Request<S>,
) -> Result<Streaming<Bytes>, Status>
where
    S: Stream<Item = Vec<u8>> + Send + 'static,
{
    use tonic::codegen::http::uri::PathAndQuery;

    // Create a Grpc client wrapper around the channel
    let mut grpc = tonic::client::Grpc::new(channel);

    // Ensure we're ready
    grpc.ready()
        .await
        .map_err(|e| Status::unavailable(format!("Service not ready: {}", e)))?;

    // Create the codec
    let codec = TinkoffCodec;

    // Parse the path
    let path = PathAndQuery::from_static(GRPC_SERVICE_PATH);

    // Make the bidirectional streaming call
    let response = grpc.streaming(request, path, codec).await?;

    Ok(response.into_inner())
}

/// Process the response stream from Tinkoff
async fn process_response_stream(
    mut stream: Streaming<Bytes>,
    result_tx: mpsc::Sender<Result<StreamingRecognizeResponse, STTError>>,
) {
    use futures::StreamExt;

    while let Some(result) = stream.next().await {
        match result {
            Ok(data) => {
                match StreamingRecognizeResponse::decode(&data) {
                    Ok(response) => {
                        if !response.results.is_empty() {
                            debug!(
                                transcript = ?response.best_transcript(),
                                is_final = response.has_final_result(),
                                "Received transcript from Tinkoff"
                            );
                            if result_tx.send(Ok(response)).await.is_err() {
                                break; // Receiver dropped
                            }
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "Failed to decode Tinkoff response");
                    }
                }
            }
            Err(status) => {
                let error = grpc_status_to_stt_error(status);
                let _ = result_tx.send(Err(error)).await;
                break;
            }
        }
    }

    debug!("Tinkoff response stream ended");
}

/// Stream adapter for audio chunks
struct AudioChunkStream {
    rx: mpsc::Receiver<Bytes>,
    config: Option<StreamingRecognitionConfig>,
    first_chunk: bool,
}

impl AudioChunkStream {
    fn new(rx: mpsc::Receiver<Bytes>, config: StreamingRecognitionConfig) -> Self {
        Self {
            rx,
            config: Some(config),
            first_chunk: true,
        }
    }
}

impl Stream for AudioChunkStream {
    type Item = Vec<u8>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // First chunk sends configuration
        if self.first_chunk {
            self.first_chunk = false;
            if let Some(config) = self.config.take() {
                let request = StreamingRecognizeRequest::config(config);
                return Poll::Ready(Some(request.encode()));
            }
        }

        // Subsequent chunks send audio
        match Pin::new(&mut self.rx).poll_recv(cx) {
            Poll::Ready(Some(audio_data)) => {
                let request = StreamingRecognizeRequest::audio(audio_data);
                Poll::Ready(Some(request.encode()))
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Codec for Tinkoff gRPC messages (raw bytes)
#[derive(Debug, Clone, Default)]
pub struct TinkoffCodec;

impl tonic::codec::Codec for TinkoffCodec {
    type Encode = Vec<u8>;
    type Decode = Bytes;
    type Encoder = TinkoffEncoder;
    type Decoder = TinkoffDecoder;

    fn encoder(&mut self) -> Self::Encoder {
        TinkoffEncoder
    }

    fn decoder(&mut self) -> Self::Decoder {
        TinkoffDecoder
    }
}

#[derive(Debug, Clone, Default)]
pub struct TinkoffEncoder;

impl tonic::codec::Encoder for TinkoffEncoder {
    type Item = Vec<u8>;
    type Error = Status;

    fn encode(
        &mut self,
        item: Self::Item,
        dst: &mut tonic::codec::EncodeBuf<'_>,
    ) -> Result<(), Self::Error> {
        dst.reserve(item.len());
        dst.put_slice(&item);
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
pub struct TinkoffDecoder;

impl tonic::codec::Decoder for TinkoffDecoder {
    type Item = Bytes;
    type Error = Status;

    fn decode(
        &mut self,
        src: &mut tonic::codec::DecodeBuf<'_>,
    ) -> Result<Option<Self::Item>, Self::Error> {
        let remaining = src.remaining();
        if remaining == 0 {
            Ok(None)
        } else {
            let data = src.copy_to_bytes(remaining);
            Ok(Some(data))
        }
    }
}

/// Convert gRPC status to STT error
pub fn grpc_status_to_stt_error(status: Status) -> STTError {
    let code = status.code();
    let message = status.message().to_string();

    match code {
        tonic::Code::Unauthenticated | tonic::Code::PermissionDenied => {
            STTError::AuthenticationFailed(format!("{:?}: {}", code, message))
        }
        tonic::Code::Unavailable => {
            STTError::ConnectionFailed(format!("Service unavailable: {}", message))
        }
        tonic::Code::InvalidArgument => {
            STTError::ConfigurationError(format!("Invalid argument: {}", message))
        }
        tonic::Code::DeadlineExceeded => {
            STTError::NetworkError(format!("Request timed out: {}", message))
        }
        _ => STTError::ProviderError(format!("gRPC error {:?}: {}", code, message)),
    }
}

/// gRPC error code descriptions for Tinkoff API
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TinkoffGrpcError {
    /// Request completed successfully (code 0)
    Ok,
    /// Request was cancelled by the user (code 1)
    Cancelled,
    /// Invalid argument provided (code 3)
    InvalidArgument,
    /// Permission denied - invalid credentials (code 7)
    PermissionDenied,
    /// Resource exhausted - rate limited (code 8)
    ResourceExhausted,
    /// Internal server error (code 13)
    Internal,
    /// Service temporarily unavailable (code 14)
    Unavailable,
    /// Unauthenticated - missing credentials (code 16)
    Unauthenticated,
    /// Unknown error code
    Unknown(i32),
}

impl TinkoffGrpcError {
    /// Parse from tonic status code
    pub fn from_code(code: i32) -> Self {
        match code {
            0 => Self::Ok,
            1 => Self::Cancelled,
            3 => Self::InvalidArgument,
            7 => Self::PermissionDenied,
            8 => Self::ResourceExhausted,
            13 => Self::Internal,
            14 => Self::Unavailable,
            16 => Self::Unauthenticated,
            _ => Self::Unknown(code),
        }
    }

    /// Get human-readable description
    pub fn description(&self) -> &'static str {
        match self {
            Self::Ok => "Request completed successfully",
            Self::Cancelled => "Request was cancelled by user",
            Self::InvalidArgument => "Invalid argument - check request parameters",
            Self::PermissionDenied => "Permission denied - check credentials",
            Self::ResourceExhausted => "Rate limited - slow down requests",
            Self::Internal => "Internal server error - try again later",
            Self::Unavailable => "Service temporarily unavailable - try again later",
            Self::Unauthenticated => "Unauthenticated - provide valid credentials",
            Self::Unknown(_) => "Unknown error occurred",
        }
    }

    /// Check if this is a retriable error
    pub fn is_retriable(&self) -> bool {
        matches!(
            self,
            Self::Unavailable | Self::Internal | Self::ResourceExhausted
        )
    }
}

impl From<tonic::Status> for TinkoffGrpcError {
    fn from(status: tonic::Status) -> Self {
        Self::from_code(status.code() as i32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tinkoff_grpc_error_from_code() {
        assert_eq!(TinkoffGrpcError::from_code(0), TinkoffGrpcError::Ok);
        assert_eq!(TinkoffGrpcError::from_code(1), TinkoffGrpcError::Cancelled);
        assert_eq!(
            TinkoffGrpcError::from_code(7),
            TinkoffGrpcError::PermissionDenied
        );
        assert_eq!(TinkoffGrpcError::from_code(13), TinkoffGrpcError::Internal);
        assert_eq!(
            TinkoffGrpcError::from_code(14),
            TinkoffGrpcError::Unavailable
        );
        assert_eq!(
            TinkoffGrpcError::from_code(16),
            TinkoffGrpcError::Unauthenticated
        );
        assert_eq!(
            TinkoffGrpcError::from_code(99),
            TinkoffGrpcError::Unknown(99)
        );
    }

    #[test]
    fn test_tinkoff_grpc_error_is_retriable() {
        assert!(!TinkoffGrpcError::Ok.is_retriable());
        assert!(!TinkoffGrpcError::Cancelled.is_retriable());
        assert!(!TinkoffGrpcError::PermissionDenied.is_retriable());
        assert!(TinkoffGrpcError::Internal.is_retriable());
        assert!(TinkoffGrpcError::Unavailable.is_retriable());
        assert!(TinkoffGrpcError::ResourceExhausted.is_retriable());
    }

    #[test]
    fn test_parse_header_value() {
        assert!(parse_header_value("valid-token", "token").is_ok());
        assert!(parse_header_value("api-key-123", "x-api-key").is_ok());
    }

    #[test]
    fn test_grpc_status_to_stt_error() {
        let status = Status::unauthenticated("invalid token");
        let error = grpc_status_to_stt_error(status);
        assert!(matches!(error, STTError::AuthenticationFailed(_)));

        let status = Status::unavailable("service down");
        let error = grpc_status_to_stt_error(status);
        assert!(matches!(error, STTError::ConnectionFailed(_)));

        let status = Status::invalid_argument("bad param");
        let error = grpc_status_to_stt_error(status);
        assert!(matches!(error, STTError::ConfigurationError(_)));
    }
}
