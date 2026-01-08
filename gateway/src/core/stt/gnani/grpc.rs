//! Gnani.ai gRPC Client
//!
//! Implements bidirectional streaming gRPC client for Gnani's Listener service.
//! Uses tonic for gRPC transport with mTLS authentication.
//!
//! ## Service Definition
//!
//! ```protobuf
//! service Listener {
//!     rpc DoSpeechToText(stream SpeechChunk) returns (stream TranscriptChunk);
//! }
//! ```

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::{Buf, BufMut, Bytes};
use futures::Stream;
use tokio::sync::mpsc;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint};
use tonic::{Request, Status, Streaming};
use tracing::{debug, info, warn};

use super::config::GnaniSTTConfig;
use super::messages::{SpeechChunk, TranscriptChunk};
use crate::core::stt::base::STTError;

/// Gnani gRPC endpoint
const GNANI_GRPC_ENDPOINT: &str = "https://asr.gnani.ai:443";

/// gRPC service path for Listener.DoSpeechToText
const GRPC_SERVICE_PATH: &str = "/Listener/DoSpeechToText";

/// Create a gRPC channel with Gnani mTLS configuration
///
/// This establishes a secure connection to Gnani's ASR service using the
/// provided SSL certificate for server verification.
pub async fn create_gnani_channel(config: &GnaniSTTConfig) -> Result<Channel, STTError> {
    // Load the certificate
    let cert_pem = config
        .load_certificate()
        .map_err(STTError::ConfigurationError)?;

    // Create TLS configuration with the CA certificate
    let tls_config = ClientTlsConfig::new()
        .ca_certificate(Certificate::from_pem(&cert_pem))
        .domain_name("asr.gnani.ai");

    // Build and connect the channel
    let channel = Endpoint::from_static(GNANI_GRPC_ENDPOINT)
        .tls_config(tls_config)
        .map_err(|e| STTError::ConfigurationError(format!("TLS config error: {}", e)))?
        .connect_timeout(Duration::from_secs(config.connection_timeout_secs))
        .timeout(Duration::from_secs(config.request_timeout_secs))
        .connect()
        .await
        .map_err(|e| STTError::ConnectionFailed(format!("gRPC connection failed: {}", e)))?;

    info!("Connected to Gnani gRPC endpoint");
    Ok(channel)
}

/// Create metadata map with Gnani authentication headers
///
/// All requests to Gnani's API require these headers for authentication
/// and request configuration.
pub fn create_gnani_metadata(
    config: &GnaniSTTConfig,
) -> Result<tonic::metadata::MetadataMap, STTError> {
    let mut metadata = tonic::metadata::MetadataMap::new();

    // Authentication headers
    metadata.insert("token", parse_header_value(&config.token, "token")?);
    metadata.insert(
        "accesskey",
        parse_header_value(&config.access_key, "accesskey")?,
    );

    // Request configuration headers
    metadata.insert("lang", parse_header_value(&config.base.language, "lang")?);
    metadata.insert(
        "audioformat",
        parse_header_value(config.audio_format.as_str(), "audioformat")?,
    );
    metadata.insert(
        "encoding",
        parse_header_value(&config.base.encoding, "encoding")?,
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

/// Gnani gRPC streaming client
///
/// Implements bidirectional streaming for real-time speech recognition.
pub struct GnaniGrpcClient {
    channel: Channel,
    config: GnaniSTTConfig,
}

impl GnaniGrpcClient {
    /// Create a new gRPC client
    pub fn new(channel: Channel, config: GnaniSTTConfig) -> Self {
        Self { channel, config }
    }

    /// Start a bidirectional streaming session
    ///
    /// Returns a sender for audio chunks and a receiver for transcription results.
    pub async fn start_streaming(
        &self,
    ) -> Result<
        (
            mpsc::Sender<Bytes>,
            mpsc::Receiver<Result<TranscriptChunk, STTError>>,
        ),
        STTError,
    > {
        // Create channels for communication
        let (audio_tx, audio_rx) = mpsc::channel::<Bytes>(100);
        let (result_tx, result_rx) = mpsc::channel::<Result<TranscriptChunk, STTError>>(100);

        // Create the request stream
        let request_stream = AudioChunkStream::new(
            audio_rx,
            self.config.token.clone(),
            self.config.base.language.clone(),
        );

        // Create metadata
        let metadata = create_gnani_metadata(&self.config)?;

        // Create the gRPC request with metadata
        let mut request = Request::new(request_stream);
        *request.metadata_mut() = metadata;

        // Clone channel for the streaming task
        let channel = self.channel.clone();

        // Spawn the streaming task
        tokio::spawn(async move {
            match do_speech_to_text(channel, request).await {
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

/// Perform the DoSpeechToText gRPC call using tonic's low-level Grpc client
async fn do_speech_to_text<S>(
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
    grpc.ready().await.map_err(|e| {
        Status::unavailable(format!("Service not ready: {}", e))
    })?;

    // Create the codec
    let codec = GnaniCodec::default();

    // Parse the path
    let path = PathAndQuery::from_static(GRPC_SERVICE_PATH);

    // Make the bidirectional streaming call
    let response = grpc
        .streaming(request, path, codec)
        .await?;

    Ok(response.into_inner())
}

/// Process the response stream from Gnani
async fn process_response_stream(
    mut stream: Streaming<Bytes>,
    result_tx: mpsc::Sender<Result<TranscriptChunk, STTError>>,
) {
    use futures::StreamExt;

    while let Some(result) = stream.next().await {
        match result {
            Ok(data) => {
                match TranscriptChunk::decode(&data) {
                    Ok(chunk) => {
                        if chunk.has_content() {
                            debug!(
                                transcript = %chunk.best_transcript(),
                                is_final = chunk.is_final,
                                confidence = chunk.confidence,
                                "Received transcript chunk"
                            );
                            if result_tx.send(Ok(chunk)).await.is_err() {
                                break; // Receiver dropped
                            }
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "Failed to decode transcript chunk");
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

    debug!("Response stream ended");
}

/// Stream adapter for audio chunks
struct AudioChunkStream {
    rx: mpsc::Receiver<Bytes>,
    token: String,
    lang: String,
    first_chunk: bool,
}

impl AudioChunkStream {
    fn new(rx: mpsc::Receiver<Bytes>, token: String, lang: String) -> Self {
        Self {
            rx,
            token,
            lang,
            first_chunk: true,
        }
    }
}

impl Stream for AudioChunkStream {
    type Item = Vec<u8>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.rx).poll_recv(cx) {
            Poll::Ready(Some(audio_data)) => {
                let chunk = if self.first_chunk {
                    self.first_chunk = false;
                    // First chunk includes auth info
                    SpeechChunk::new(audio_data, &self.token, &self.lang)
                } else {
                    // Subsequent chunks only have audio
                    SpeechChunk::audio_only(audio_data)
                };
                Poll::Ready(Some(chunk.encode()))
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Codec for Gnani gRPC messages (raw bytes)
#[derive(Debug, Clone, Default)]
struct GnaniCodec;

impl tonic::codec::Codec for GnaniCodec {
    type Encode = Vec<u8>;
    type Decode = Bytes;
    type Encoder = GnaniEncoder;
    type Decoder = GnaniDecoder;

    fn encoder(&mut self) -> Self::Encoder {
        GnaniEncoder
    }

    fn decoder(&mut self) -> Self::Decoder {
        GnaniDecoder
    }
}

#[derive(Debug, Clone, Default)]
struct GnaniEncoder;

impl tonic::codec::Encoder for GnaniEncoder {
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
struct GnaniDecoder;

impl tonic::codec::Decoder for GnaniDecoder {
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

/// gRPC error code descriptions for Gnani API
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GnaniGrpcError {
    /// Request completed successfully (code 0)
    Ok,
    /// Request was cancelled by the user (code 1)
    Cancelled,
    /// Permission denied - invalid credentials or unsupported language (code 7)
    PermissionDenied,
    /// Internal server error (code 13)
    Internal,
    /// Service temporarily unavailable (code 14)
    Unavailable,
    /// Unknown error code
    Unknown(i32),
}

impl GnaniGrpcError {
    /// Parse from tonic status code
    pub fn from_code(code: i32) -> Self {
        match code {
            0 => Self::Ok,
            1 => Self::Cancelled,
            7 => Self::PermissionDenied,
            13 => Self::Internal,
            14 => Self::Unavailable,
            _ => Self::Unknown(code),
        }
    }

    /// Get human-readable description
    pub fn description(&self) -> &'static str {
        match self {
            Self::Ok => "Request completed successfully",
            Self::Cancelled => "Request was cancelled by user",
            Self::PermissionDenied => "Permission denied - check credentials or language support",
            Self::Internal => "Internal server error - check grpc_message for details",
            Self::Unavailable => "Service temporarily unavailable - try again later",
            Self::Unknown(_) => "Unknown error occurred",
        }
    }

    /// Check if this is a retriable error
    pub fn is_retriable(&self) -> bool {
        matches!(self, Self::Unavailable | Self::Internal)
    }
}

impl From<tonic::Status> for GnaniGrpcError {
    fn from(status: tonic::Status) -> Self {
        Self::from_code(status.code() as i32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gnani_grpc_error_from_code() {
        assert_eq!(GnaniGrpcError::from_code(0), GnaniGrpcError::Ok);
        assert_eq!(GnaniGrpcError::from_code(1), GnaniGrpcError::Cancelled);
        assert_eq!(
            GnaniGrpcError::from_code(7),
            GnaniGrpcError::PermissionDenied
        );
        assert_eq!(GnaniGrpcError::from_code(13), GnaniGrpcError::Internal);
        assert_eq!(GnaniGrpcError::from_code(14), GnaniGrpcError::Unavailable);
        assert_eq!(GnaniGrpcError::from_code(99), GnaniGrpcError::Unknown(99));
    }

    #[test]
    fn test_gnani_grpc_error_is_retriable() {
        assert!(!GnaniGrpcError::Ok.is_retriable());
        assert!(!GnaniGrpcError::Cancelled.is_retriable());
        assert!(!GnaniGrpcError::PermissionDenied.is_retriable());
        assert!(GnaniGrpcError::Internal.is_retriable());
        assert!(GnaniGrpcError::Unavailable.is_retriable());
    }

    #[test]
    fn test_parse_header_value() {
        assert!(parse_header_value("valid-token", "token").is_ok());
        assert!(parse_header_value("en-IN", "lang").is_ok());
    }
}
