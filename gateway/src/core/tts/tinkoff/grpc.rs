//! Tinkoff VoiceKit TTS gRPC Client
//!
//! Implements gRPC client for Tinkoff's TextToSpeech service with JWT authentication.
//! Supports both streaming and non-streaming synthesis.
//!
//! ## Service Definition
//!
//! ```protobuf
//! service TextToSpeech {
//!     rpc Synthesize(SynthesizeSpeechRequest) returns (SynthesizeSpeechResponse);
//!     rpc StreamingSynthesize(SynthesizeSpeechRequest) returns (stream StreamingSynthesizeSpeechResponse);
//! }
//! ```

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use bytes::{Buf, BufMut, Bytes};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};
use tonic::{Request, Status, Streaming};
use tracing::info;

use super::config::TinkoffTtsConfig;
use super::messages::{SynthesizeSpeechRequest, SynthesizeSpeechResponse};
use super::{
    GRPC_STREAMING_SYNTHESIZE_PATH, GRPC_SYNTHESIZE_PATH, JWT_EXPIRATION_SECS,
    TINKOFF_GRPC_ENDPOINT, TTS_SCOPE,
};
use crate::core::tts::base::TTSError;

type HmacSha256 = Hmac<Sha256>;

/// Create a gRPC channel to Tinkoff VoiceKit TTS
///
/// Establishes a secure TLS connection to Tinkoff's TTS service.
pub async fn create_tinkoff_tts_channel(config: &TinkoffTtsConfig) -> Result<Channel, TTSError> {
    let endpoint = if let Some(ep) = config.endpoint_override.as_deref() {
        Endpoint::from_shared(ep.to_string()).map_err(|e| {
            TTSError::InvalidConfiguration(format!("Invalid endpoint override: {}", e))
        })?
    } else {
        let tls_config = ClientTlsConfig::new().domain_name("api.tinkoff.ai");
        Endpoint::from_static(TINKOFF_GRPC_ENDPOINT)
            .tls_config(tls_config)
            .map_err(|e| TTSError::InvalidConfiguration(format!("TLS config error: {}", e)))?
    };

    let channel = endpoint
        .connect_timeout(Duration::from_secs(config.connection_timeout_secs))
        .timeout(Duration::from_secs(config.request_timeout_secs))
        .connect()
        .await
        .map_err(|e| TTSError::ConnectionFailed(format!("gRPC connection failed: {}", e)))?;

    info!("Connected to Tinkoff VoiceKit TTS gRPC endpoint");
    Ok(channel)
}

/// Generate a JWT token for Tinkoff API authentication
///
/// Tinkoff uses JWT with HMAC-SHA256 signing. The token structure:
/// - Header: {"alg": "HS256", "typ": "JWT", "kid": "<api_key>"}
/// - Payload: {"iss": "test_issuer", "sub": "test_user", "aud": "<scope>", "exp": <timestamp>}
pub fn generate_jwt_token(
    api_key: &str,
    secret_key: &str,
    scope: &str,
) -> Result<String, TTSError> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| TTSError::InternalError(format!("System time error: {}", e)))?;

    let exp = now.as_secs() + JWT_EXPIRATION_SECS;

    // JWT Header
    let header = serde_json::json!({
        "alg": "HS256",
        "typ": "JWT",
        "kid": api_key
    });
    let header_b64 = URL_SAFE_NO_PAD.encode(header.to_string().as_bytes());

    // JWT Payload. iss/sub identify the calling key — the previous hardcoded
    // "test_issuer"/"test_user" placeholders guaranteed auth rejection in production.
    let payload = serde_json::json!({
        "iss": api_key,
        "sub": api_key,
        "aud": scope,
        "exp": exp
    });
    let payload_b64 = URL_SAFE_NO_PAD.encode(payload.to_string().as_bytes());

    // Create signature.
    let signing_input = format!("{}.{}", header_b64, payload_b64);

    // Tinkoff API secrets are base64-encoded; the HMAC key must be the DECODED bytes.
    // Signing with the raw string (the previous behaviour) produced an invalid signature.
    // Fall back to raw bytes if the secret isn't valid base64 (defensive, no regression).
    let key_bytes = STANDARD
        .decode(secret_key)
        .unwrap_or_else(|_| secret_key.as_bytes().to_vec());
    let mut mac = HmacSha256::new_from_slice(&key_bytes)
        .map_err(|e| TTSError::InvalidConfiguration(format!("Invalid secret key: {}", e)))?;
    mac.update(signing_input.as_bytes());
    let signature = mac.finalize().into_bytes();
    let signature_b64 = URL_SAFE_NO_PAD.encode(signature);

    Ok(format!("{}.{}.{}", header_b64, payload_b64, signature_b64))
}

/// Create metadata map with Tinkoff JWT authentication
pub fn create_tinkoff_tts_metadata(
    config: &TinkoffTtsConfig,
) -> Result<tonic::metadata::MetadataMap, TTSError> {
    let mut metadata = tonic::metadata::MetadataMap::new();

    // Generate JWT token for TTS scope
    let token = generate_jwt_token(&config.api_key, &config.secret_key, TTS_SCOPE)?;

    let auth_value = format!("Bearer {}", token);
    metadata.insert(
        "authorization",
        auth_value.parse().map_err(|_| {
            TTSError::InvalidConfiguration("Invalid authorization header".to_string())
        })?,
    );

    Ok(metadata)
}

/// Tinkoff TTS gRPC client
pub struct TinkoffTtsGrpcClient {
    channel: Channel,
    config: TinkoffTtsConfig,
}

impl TinkoffTtsGrpcClient {
    /// Create a new TTS gRPC client
    pub fn new(channel: Channel, config: TinkoffTtsConfig) -> Self {
        Self { channel, config }
    }

    /// Perform non-streaming synthesis (Synthesize RPC)
    ///
    /// Sends text and receives complete audio in a single response.
    pub async fn synthesize(
        &self,
        request: SynthesizeSpeechRequest,
    ) -> Result<SynthesizeSpeechResponse, TTSError> {
        let encoded = request.encode();

        let metadata = create_tinkoff_tts_metadata(&self.config)?;
        let mut grpc_request = Request::new(encoded);
        *grpc_request.metadata_mut() = metadata;

        let response = do_synthesize(self.channel.clone(), grpc_request).await?;

        SynthesizeSpeechResponse::decode(&response)
            .map_err(|e| TTSError::ProviderError(format!("Failed to decode response: {}", e)))
    }

    /// Perform streaming synthesis (StreamingSynthesize RPC)
    ///
    /// Sends text and receives audio in chunks for lower latency.
    pub async fn streaming_synthesize(
        &self,
        request: SynthesizeSpeechRequest,
    ) -> Result<Streaming<Bytes>, TTSError> {
        let encoded = request.encode();

        let metadata = create_tinkoff_tts_metadata(&self.config)?;
        let mut grpc_request = Request::new(encoded);
        *grpc_request.metadata_mut() = metadata;

        do_streaming_synthesize(self.channel.clone(), grpc_request).await
    }
}

/// Perform the Synthesize unary gRPC call
async fn do_synthesize(channel: Channel, request: Request<Vec<u8>>) -> Result<Bytes, TTSError> {
    use tonic::codegen::http::uri::PathAndQuery;

    let mut grpc = tonic::client::Grpc::new(channel);

    grpc.ready()
        .await
        .map_err(|e| TTSError::ConnectionFailed(format!("Service not ready: {}", e)))?;

    let codec = TinkoffTtsCodec;
    let path = PathAndQuery::from_static(GRPC_SYNTHESIZE_PATH);

    let response = grpc
        .unary(request, path, codec)
        .await
        .map_err(grpc_status_to_tts_error)?;

    Ok(response.into_inner())
}

/// Perform the StreamingSynthesize server-streaming gRPC call
async fn do_streaming_synthesize(
    channel: Channel,
    request: Request<Vec<u8>>,
) -> Result<Streaming<Bytes>, TTSError> {
    use tonic::codegen::http::uri::PathAndQuery;

    let mut grpc = tonic::client::Grpc::new(channel);

    grpc.ready()
        .await
        .map_err(|e| TTSError::ConnectionFailed(format!("Service not ready: {}", e)))?;

    let codec = TinkoffTtsCodec;
    let path = PathAndQuery::from_static(GRPC_STREAMING_SYNTHESIZE_PATH);

    let response = grpc
        .server_streaming(request, path, codec)
        .await
        .map_err(grpc_status_to_tts_error)?;

    Ok(response.into_inner())
}

/// Codec for Tinkoff TTS gRPC messages
#[derive(Debug, Clone, Default)]
pub struct TinkoffTtsCodec;

impl tonic::codec::Codec for TinkoffTtsCodec {
    type Encode = Vec<u8>;
    type Decode = Bytes;
    type Encoder = TinkoffTtsEncoder;
    type Decoder = TinkoffTtsDecoder;

    fn encoder(&mut self) -> Self::Encoder {
        TinkoffTtsEncoder
    }

    fn decoder(&mut self) -> Self::Decoder {
        TinkoffTtsDecoder
    }
}

#[derive(Debug, Clone, Default)]
pub struct TinkoffTtsEncoder;

impl tonic::codec::Encoder for TinkoffTtsEncoder {
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
pub struct TinkoffTtsDecoder;

impl tonic::codec::Decoder for TinkoffTtsDecoder {
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

/// Convert gRPC status to TTS error
pub fn grpc_status_to_tts_error(status: Status) -> TTSError {
    let code = status.code();
    let message = status.message().to_string();

    match code {
        tonic::Code::Unauthenticated | tonic::Code::PermissionDenied => {
            TTSError::AuthenticationFailed(format!("{:?}: {}", code, message))
        }
        tonic::Code::Unavailable => {
            TTSError::ConnectionFailed(format!("Service unavailable: {}", message))
        }
        tonic::Code::InvalidArgument => {
            TTSError::InvalidConfiguration(format!("Invalid argument: {}", message))
        }
        tonic::Code::DeadlineExceeded => {
            TTSError::TimeoutError(format!("Request timed out: {}", message))
        }
        tonic::Code::ResourceExhausted => TTSError::RateLimited {
            retry_after_secs: None,
            message: format!("Rate limited: {}", message),
        },
        _ => TTSError::ProviderError(format!("gRPC error {:?}: {}", code, message)),
    }
}

/// gRPC error type for Tinkoff TTS API
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TinkoffTtsGrpcError {
    #[error("Request completed successfully")]
    Ok,
    #[error("Request was cancelled")]
    Cancelled,
    #[error("Invalid argument provided")]
    InvalidArgument,
    #[error("Permission denied - check credentials")]
    PermissionDenied,
    #[error("Resource exhausted - rate limited")]
    ResourceExhausted,
    #[error("Internal server error")]
    Internal,
    #[error("Service temporarily unavailable")]
    Unavailable,
    #[error("Unauthenticated - provide valid credentials")]
    Unauthenticated,
    #[error("Unknown error: {0}")]
    Unknown(i32),
}

impl TinkoffTtsGrpcError {
    /// Parse from gRPC status code
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

    /// Check if this error is retriable
    pub fn is_retriable(&self) -> bool {
        matches!(
            self,
            Self::Unavailable | Self::Internal | Self::ResourceExhausted
        )
    }
}

impl From<tonic::Status> for TinkoffTtsGrpcError {
    fn from(status: tonic::Status) -> Self {
        Self::from_code(status.code() as i32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_jwt_token() {
        let api_key = "test-api-key";
        let secret_key = "test-secret-key";
        let scope = "tinkoff.cloud.tts";

        let token = generate_jwt_token(api_key, secret_key, scope).unwrap();

        // JWT should have 3 parts separated by dots
        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3);

        // Verify header contains api_key
        let header_json = String::from_utf8(URL_SAFE_NO_PAD.decode(parts[0]).unwrap()).unwrap();
        assert!(header_json.contains("test-api-key"));
        assert!(header_json.contains("HS256"));

        // Verify payload contains scope
        let payload_json = String::from_utf8(URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
        assert!(payload_json.contains("tinkoff.cloud.tts"));
    }

    // Regression: Tinkoff secrets are base64; the HMAC key must be the DECODED bytes, and the
    // claims must not be the old "test_issuer"/"test_user" placeholders.
    #[test]
    fn test_jwt_secret_is_base64_decoded_and_claims_real() {
        use hmac::Mac;
        let api_key = "real-api-key";
        let raw_bytes: &[u8] = b"super-secret-key-material-32bytes";
        let secret_b64 = STANDARD.encode(raw_bytes);
        let scope = "tinkoff.cloud.tts";

        let token = generate_jwt_token(api_key, &secret_b64, scope).unwrap();
        let parts: Vec<&str> = token.split('.').collect();
        let signing_input = format!("{}.{}", parts[0], parts[1]);

        // Signature must be HMAC over the DECODED key bytes.
        let mut mac_ok = super::HmacSha256::new_from_slice(raw_bytes).unwrap();
        mac_ok.update(signing_input.as_bytes());
        let expected = URL_SAFE_NO_PAD.encode(mac_ok.finalize().into_bytes());
        assert_eq!(
            parts[2], expected,
            "signature must use the base64-decoded secret"
        );

        // ...and NOT over the raw base64 string bytes (the old bug).
        let mut mac_wrong = super::HmacSha256::new_from_slice(secret_b64.as_bytes()).unwrap();
        mac_wrong.update(signing_input.as_bytes());
        let wrong = URL_SAFE_NO_PAD.encode(mac_wrong.finalize().into_bytes());
        assert_ne!(parts[2], wrong);

        // Claims are no longer placeholders.
        let payload_json = String::from_utf8(URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
        assert!(!payload_json.contains("test_issuer"));
        assert!(!payload_json.contains("test_user"));
        assert!(payload_json.contains(api_key));
    }

    #[test]
    fn test_jwt_token_expiration() {
        let api_key = "test-api-key";
        let secret_key = "test-secret-key";
        let scope = "tinkoff.cloud.tts";

        let token = generate_jwt_token(api_key, secret_key, scope).unwrap();
        let parts: Vec<&str> = token.split('.').collect();

        let payload_json = String::from_utf8(URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
        let payload: serde_json::Value = serde_json::from_str(&payload_json).unwrap();

        let exp = payload["exp"].as_u64().unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Token should expire in roughly JWT_EXPIRATION_SECS
        assert!(exp > now);
        assert!(exp <= now + JWT_EXPIRATION_SECS + 1);
    }

    #[test]
    fn test_tinkoff_tts_grpc_error_from_code() {
        assert_eq!(TinkoffTtsGrpcError::from_code(0), TinkoffTtsGrpcError::Ok);
        assert_eq!(
            TinkoffTtsGrpcError::from_code(1),
            TinkoffTtsGrpcError::Cancelled
        );
        assert_eq!(
            TinkoffTtsGrpcError::from_code(7),
            TinkoffTtsGrpcError::PermissionDenied
        );
        assert_eq!(
            TinkoffTtsGrpcError::from_code(13),
            TinkoffTtsGrpcError::Internal
        );
        assert_eq!(
            TinkoffTtsGrpcError::from_code(14),
            TinkoffTtsGrpcError::Unavailable
        );
        assert_eq!(
            TinkoffTtsGrpcError::from_code(16),
            TinkoffTtsGrpcError::Unauthenticated
        );
        assert_eq!(
            TinkoffTtsGrpcError::from_code(99),
            TinkoffTtsGrpcError::Unknown(99)
        );
    }

    #[test]
    fn test_tinkoff_tts_grpc_error_is_retriable() {
        assert!(!TinkoffTtsGrpcError::Ok.is_retriable());
        assert!(!TinkoffTtsGrpcError::Cancelled.is_retriable());
        assert!(!TinkoffTtsGrpcError::PermissionDenied.is_retriable());
        assert!(TinkoffTtsGrpcError::Internal.is_retriable());
        assert!(TinkoffTtsGrpcError::Unavailable.is_retriable());
        assert!(TinkoffTtsGrpcError::ResourceExhausted.is_retriable());
    }

    #[test]
    fn test_grpc_status_to_tts_error() {
        let status = Status::unauthenticated("invalid token");
        let error = grpc_status_to_tts_error(status);
        assert!(matches!(error, TTSError::AuthenticationFailed(_)));

        let status = Status::unavailable("service down");
        let error = grpc_status_to_tts_error(status);
        assert!(matches!(error, TTSError::ConnectionFailed(_)));

        let status = Status::invalid_argument("bad param");
        let error = grpc_status_to_tts_error(status);
        assert!(matches!(error, TTSError::InvalidConfiguration(_)));

        let status = Status::deadline_exceeded("timeout");
        let error = grpc_status_to_tts_error(status);
        assert!(matches!(error, TTSError::TimeoutError(_)));

        let status = Status::resource_exhausted("rate limited");
        let error = grpc_status_to_tts_error(status);
        assert!(matches!(error, TTSError::RateLimited { .. }));
    }
}
