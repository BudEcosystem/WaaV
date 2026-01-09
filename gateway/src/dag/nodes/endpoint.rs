//! Endpoint nodes for external integrations
//!
//! These nodes handle communication with external services via HTTP, gRPC,
//! WebSocket, IPC, and LiveKit.

use std::sync::Arc;
use std::collections::HashMap;
use std::time::Duration;
use async_trait::async_trait;
use bytes::{Buf, BufMut, Bytes};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::RwLock;
use tracing::{debug, info, warn, error};

use super::{DAGNode, DAGData, NodeCapability};
use crate::dag::context::{DAGContext, resource_keys};
use crate::dag::definition::HttpMethod;
use crate::dag::error::{DAGError, DAGResult};
use crate::livekit::LiveKitClient;

/// Generic bytes codec for gRPC calls without proto definitions.
///
/// This codec allows calling any gRPC service by sending and receiving
/// raw bytes. The input/output format interpretation is left to the caller.
#[derive(Debug, Clone, Copy, Default)]
struct GenericBytesCodec;

impl tonic::codec::Codec for GenericBytesCodec {
    type Encode = Bytes;
    type Decode = Bytes;
    type Encoder = GenericBytesEncoder;
    type Decoder = GenericBytesDecoder;

    fn encoder(&mut self) -> Self::Encoder {
        GenericBytesEncoder
    }

    fn decoder(&mut self) -> Self::Decoder {
        GenericBytesDecoder
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct GenericBytesEncoder;

impl tonic::codec::Encoder for GenericBytesEncoder {
    type Item = Bytes;
    type Error = tonic::Status;

    fn encode(&mut self, item: Self::Item, dst: &mut tonic::codec::EncodeBuf<'_>) -> Result<(), Self::Error> {
        dst.put(item);
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct GenericBytesDecoder;

impl tonic::codec::Decoder for GenericBytesDecoder {
    type Item = Bytes;
    type Error = tonic::Status;

    fn decode(&mut self, src: &mut tonic::codec::DecodeBuf<'_>) -> Result<Option<Self::Item>, Self::Error> {
        let remaining = src.remaining();
        if remaining == 0 {
            return Ok(None);
        }
        let mut buf = vec![0u8; remaining];
        src.copy_to_slice(&mut buf);
        Ok(Some(Bytes::from(buf)))
    }
}

/// HTTP endpoint node
///
/// Makes HTTP requests to external APIs and returns responses.
/// The HTTP client is created once and reused for all requests (connection pooling).
#[derive(Clone)]
pub struct HttpEndpointNode {
    id: String,
    url: String,
    method: HttpMethod,
    headers: HashMap<String, String>,
    timeout_ms: u64,
    /// Pooled HTTP client for connection reuse
    client: reqwest::Client,
}

impl HttpEndpointNode {
    /// Create a new HTTP endpoint node
    ///
    /// The HTTP client is created once during construction with default settings:
    /// - Connection pooling enabled (default)
    /// - Keep-alive connections (default)
    /// - Automatic redirect following (default)
    pub fn new(id: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            url: url.into(),
            method: HttpMethod::POST,
            headers: HashMap::new(),
            timeout_ms: 30000,
            client: reqwest::Client::new(),
        }
    }

    /// Set HTTP method
    pub fn with_method(mut self, method: HttpMethod) -> Self {
        self.method = method;
        self
    }

    /// Add a header
    pub fn with_header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(key.into(), value.into());
        self
    }

    /// Set timeout in milliseconds
    pub fn with_timeout_ms(mut self, timeout: u64) -> Self {
        self.timeout_ms = timeout;
        self
    }

    /// Get the URL
    pub fn url(&self) -> &str {
        &self.url
    }
}

impl std::fmt::Debug for HttpEndpointNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpEndpointNode")
            .field("id", &self.id)
            .field("url", &self.url)
            .field("method", &self.method)
            .field("timeout_ms", &self.timeout_ms)
            .finish()
    }
}

#[async_trait]
impl DAGNode for HttpEndpointNode {
    fn id(&self) -> &str {
        &self.id
    }

    fn node_type(&self) -> &str {
        "http_endpoint"
    }

    fn capabilities(&self) -> Vec<NodeCapability> {
        vec![
            NodeCapability::TextInput,
            NodeCapability::JsonInput,
            NodeCapability::JsonOutput,
            NodeCapability::Cancellable,
        ]
    }

    async fn execute(&self, input: DAGData, ctx: &mut DAGContext) -> DAGResult<DAGData> {
        let payload = input.to_json();

        debug!(
            node_id = %self.id,
            url = %self.url,
            method = ?self.method,
            "Making HTTP request"
        );

        // Use the pre-created pooled client for connection reuse
        let mut request = self.client
            .request(self.method.clone().into(), &self.url)
            .timeout(Duration::from_millis(self.timeout_ms))
            .header("Content-Type", "application/json")
            .header("X-Stream-ID", &ctx.stream_id);

        // Add custom headers
        for (key, value) in &self.headers {
            request = request.header(key, value);
        }

        // Add API key if available
        if let Some(api_key) = &ctx.api_key {
            request = request.header("Authorization", format!("Bearer {}", api_key));
        }

        request = request.json(&payload);

        let response = request.send().await.map_err(|e| DAGError::HttpEndpointError {
            url: self.url.clone(),
            error: e.to_string(),
        })?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(DAGError::HttpEndpointError {
                url: self.url.clone(),
                error: format!("HTTP {} - {}", status, error_text),
            });
        }

        let json: serde_json::Value = response.json().await.map_err(|e| DAGError::HttpEndpointError {
            url: self.url.clone(),
            error: format!("Failed to parse response: {}", e),
        })?;

        Ok(DAGData::Json(json))
    }

    fn clone_boxed(&self) -> Arc<dyn DAGNode> {
        Arc::new(self.clone())
    }
}

/// gRPC endpoint node
///
/// Makes gRPC requests to external services.
#[derive(Clone)]
pub struct GrpcEndpointNode {
    id: String,
    address: String,
    service: String,
    method: String,
    timeout_ms: u64,
    /// Domain name for TLS hostname verification (extracted from address if not set)
    tls_domain_name: Option<String>,
    /// Whether to verify TLS certificates (default: true)
    verify_certificates: bool,
}

impl GrpcEndpointNode {
    /// Create a new gRPC endpoint node
    pub fn new(
        id: impl Into<String>,
        address: impl Into<String>,
        service: impl Into<String>,
        method: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            address: address.into(),
            service: service.into(),
            method: method.into(),
            timeout_ms: 30000,
            tls_domain_name: None,
            verify_certificates: true,
        }
    }

    /// Set timeout in milliseconds
    pub fn with_timeout_ms(mut self, timeout: u64) -> Self {
        self.timeout_ms = timeout;
        self
    }

    /// Set TLS domain name for hostname verification
    ///
    /// If not set, the domain is extracted from the address.
    pub fn with_tls_domain_name(mut self, domain: impl Into<String>) -> Self {
        self.tls_domain_name = Some(domain.into());
        self
    }

    /// Disable TLS certificate verification (NOT RECOMMENDED for production)
    ///
    /// # Security Warning
    /// Disabling certificate verification makes the connection vulnerable to
    /// man-in-the-middle attacks. Only use for development/testing.
    pub fn with_insecure_skip_verify(mut self, skip: bool) -> Self {
        if skip {
            warn!("Disabling TLS certificate verification - NOT RECOMMENDED for production");
        }
        self.verify_certificates = !skip;
        self
    }

    /// Get the address
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Extract domain name from address for TLS verification
    fn extract_domain_name(&self) -> Option<String> {
        if let Some(domain) = &self.tls_domain_name {
            return Some(domain.clone());
        }

        // Try to extract domain from address
        let addr = self.address
            .trim_start_matches("https://")
            .trim_start_matches("http://");

        // Split by port separator and take host part
        let host = addr.split(':').next()?;

        // Don't return localhost or IP addresses as domain names
        if host == "localhost" || host.parse::<std::net::IpAddr>().is_ok() {
            return None;
        }

        Some(host.to_string())
    }
}

impl std::fmt::Debug for GrpcEndpointNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GrpcEndpointNode")
            .field("id", &self.id)
            .field("address", &self.address)
            .field("service", &self.service)
            .field("method", &self.method)
            .finish()
    }
}

#[async_trait]
impl DAGNode for GrpcEndpointNode {
    fn id(&self) -> &str {
        &self.id
    }

    fn node_type(&self) -> &str {
        "grpc_endpoint"
    }

    fn capabilities(&self) -> Vec<NodeCapability> {
        vec![
            NodeCapability::TextInput,
            NodeCapability::JsonInput,
            NodeCapability::JsonOutput,
            NodeCapability::Cancellable,
        ]
    }

    async fn execute(&self, input: DAGData, ctx: &mut DAGContext) -> DAGResult<DAGData> {
        debug!(
            node_id = %self.id,
            address = %self.address,
            service = %self.service,
            method = %self.method,
            "Making gRPC request"
        );

        // Serialize input to bytes - support JSON and binary
        let request_bytes: Bytes = match &input {
            DAGData::Json(json) => {
                // Serialize JSON to bytes for the gRPC call
                serde_json::to_vec(json)
                    .map_err(|e| DAGError::GrpcEndpointError {
                        service: self.service.clone(),
                        method: self.method.clone(),
                        error: format!("Failed to serialize JSON input: {}", e),
                    })?
                    .into()
            }
            DAGData::Binary(bytes) => bytes.clone(),
            DAGData::Text(text) => Bytes::from(text.clone()),
            DAGData::Audio(bytes) => bytes.clone(),
            other => {
                // Convert other types to JSON first
                let json = other.to_json();
                serde_json::to_vec(&json)
                    .map_err(|e| DAGError::GrpcEndpointError {
                        service: self.service.clone(),
                        method: self.method.clone(),
                        error: format!("Failed to serialize input: {}", e),
                    })?
                    .into()
            }
        };

        // Build the gRPC path: /{service}/{method}
        let path = format!("/{}/{}", self.service, self.method);

        // Parse the path into PathAndQuery using tonic's re-exported http crate
        // to avoid version conflicts (tonic 0.11 uses http 0.2, project uses http 1.x)
        let path_and_query: tonic::codegen::http::uri::PathAndQuery = path.parse().map_err(|e| {
            DAGError::GrpcEndpointError {
                service: self.service.clone(),
                method: self.method.clone(),
                error: format!("Invalid service/method path '{}': {}", path, e),
            }
        })?;

        // Determine if TLS is needed based on address
        let use_tls = self.address.starts_with("https://") ||
                      (!self.address.starts_with("http://") && !self.address.contains("localhost"));

        // Normalize the address for tonic
        let address = if self.address.starts_with("http://") || self.address.starts_with("https://") {
            self.address.clone()
        } else if use_tls {
            format!("https://{}", self.address)
        } else {
            format!("http://{}", self.address)
        };

        // Create gRPC channel
        let channel = if use_tls {
            // Build TLS config with proper hostname verification
            let mut tls_config = tonic::transport::ClientTlsConfig::new();

            // Set domain name for hostname verification if available
            if let Some(domain) = self.extract_domain_name() {
                debug!(
                    node_id = %self.id,
                    domain = %domain,
                    "Setting TLS domain name for hostname verification"
                );
                tls_config = tls_config.domain_name(domain);
            } else if self.verify_certificates {
                warn!(
                    node_id = %self.id,
                    address = %address,
                    "No domain name for TLS hostname verification - connection may fail"
                );
            }

            // Note: tonic doesn't directly support disabling certificate verification
            // If verify_certificates is false, log a warning but continue
            // In production, this should always be true
            if !self.verify_certificates {
                warn!(
                    node_id = %self.id,
                    "TLS certificate verification disabled - INSECURE"
                );
            }

            tonic::transport::Channel::from_shared(address.clone())
                .map_err(|e| DAGError::GrpcEndpointError {
                    service: self.service.clone(),
                    method: self.method.clone(),
                    error: format!("Invalid endpoint URL '{}': {}", address, e),
                })?
                .tls_config(tls_config)
                .map_err(|e| DAGError::GrpcEndpointError {
                    service: self.service.clone(),
                    method: self.method.clone(),
                    error: format!("Failed to configure TLS: {}", e),
                })?
                .timeout(Duration::from_millis(self.timeout_ms))
                .connect()
                .await
                .map_err(|e| DAGError::GrpcEndpointError {
                    service: self.service.clone(),
                    method: self.method.clone(),
                    error: format!("Failed to connect to '{}': {}", address, e),
                })?
        } else {
            tonic::transport::Channel::from_shared(address.clone())
                .map_err(|e| DAGError::GrpcEndpointError {
                    service: self.service.clone(),
                    method: self.method.clone(),
                    error: format!("Invalid endpoint URL '{}': {}", address, e),
                })?
                .timeout(Duration::from_millis(self.timeout_ms))
                .connect()
                .await
                .map_err(|e| DAGError::GrpcEndpointError {
                    service: self.service.clone(),
                    method: self.method.clone(),
                    error: format!("Failed to connect to '{}': {}", address, e),
                })?
        };

        // Create generic gRPC client
        let mut client = tonic::client::Grpc::new(channel);

        // Store request size before moving bytes into request
        let request_size = request_bytes.len();

        // Prepare the request with metadata
        let mut request = tonic::Request::new(request_bytes);

        // Add stream ID to metadata
        request.metadata_mut().insert(
            "x-stream-id",
            ctx.stream_id.parse().unwrap_or_else(|_| {
                tonic::metadata::MetadataValue::from_static("unknown")
            }),
        );

        // Add API key if available
        if let Some(api_key) = &ctx.api_key {
            if let Ok(value) = format!("Bearer {}", api_key).parse() {
                request.metadata_mut().insert("authorization", value);
            }
        }

        // Set timeout on request
        request.set_timeout(Duration::from_millis(self.timeout_ms));

        info!(
            node_id = %self.id,
            address = %address,
            path = %path,
            request_size = request_size,
            "Sending gRPC request"
        );

        // Make the unary gRPC call using generic bytes codec
        client.ready().await.map_err(|e| DAGError::GrpcEndpointError {
            service: self.service.clone(),
            method: self.method.clone(),
            error: format!("gRPC client not ready: {}", e),
        })?;

        let response = client
            .unary(request, path_and_query, GenericBytesCodec)
            .await
            .map_err(|e| DAGError::GrpcEndpointError {
                service: self.service.clone(),
                method: self.method.clone(),
                error: format!("gRPC call failed: {}", e),
            })?;

        let response_bytes = response.into_inner();
        let response_len = response_bytes.len();

        info!(
            node_id = %self.id,
            response_size = response_len,
            "Received gRPC response"
        );

        // Try to parse response as JSON, fallback to binary
        if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&response_bytes) {
            Ok(DAGData::Json(json))
        } else {
            // Return as binary if not valid JSON
            Ok(DAGData::Binary(response_bytes))
        }
    }

    fn clone_boxed(&self) -> Arc<dyn DAGNode> {
        Arc::new(self.clone())
    }
}

/// WebSocket client endpoint node
///
/// Connects to external WebSocket servers for request-response communication.
/// Each execution establishes a connection, sends a message, waits for response, and closes.
#[derive(Clone)]
pub struct WebSocketEndpointNode {
    id: String,
    url: String,
    headers: HashMap<String, String>,
    timeout_ms: u64,
}

impl WebSocketEndpointNode {
    /// Create a new WebSocket endpoint node
    pub fn new(id: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            url: url.into(),
            headers: HashMap::new(),
            timeout_ms: 30000,
        }
    }

    /// Add a header
    pub fn with_header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(key.into(), value.into());
        self
    }

    /// Set timeout in milliseconds
    pub fn with_timeout_ms(mut self, timeout: u64) -> Self {
        self.timeout_ms = timeout;
        self
    }

    /// Get the URL
    pub fn url(&self) -> &str {
        &self.url
    }
}

impl std::fmt::Debug for WebSocketEndpointNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebSocketEndpointNode")
            .field("id", &self.id)
            .field("url", &self.url)
            .field("timeout_ms", &self.timeout_ms)
            .finish()
    }
}

#[async_trait]
impl DAGNode for WebSocketEndpointNode {
    fn id(&self) -> &str {
        &self.id
    }

    fn node_type(&self) -> &str {
        "websocket_endpoint"
    }

    fn capabilities(&self) -> Vec<NodeCapability> {
        vec![
            NodeCapability::TextInput,
            NodeCapability::JsonInput,
            NodeCapability::AudioInput,
            NodeCapability::TextOutput,
            NodeCapability::JsonOutput,
            NodeCapability::AudioOutput,
            NodeCapability::Streaming,
            NodeCapability::Cancellable,
        ]
    }

    async fn execute(&self, input: DAGData, ctx: &mut DAGContext) -> DAGResult<DAGData> {
        debug!(
            node_id = %self.id,
            url = %self.url,
            "WebSocket endpoint called"
        );

        // Prepare the message to send
        // tungstenite 0.28 uses Utf8Bytes for text and Bytes for binary
        let message = match &input {
            DAGData::Json(json) => {
                let text = serde_json::to_string(json).map_err(|e| DAGError::WebSocketEndpointError {
                    url: self.url.clone(),
                    error: format!("Failed to serialize JSON: {}", e),
                })?;
                tokio_tungstenite::tungstenite::Message::Text(text.into())
            }
            DAGData::Text(text) => {
                tokio_tungstenite::tungstenite::Message::Text(text.clone().into())
            }
            DAGData::Binary(bytes) => {
                tokio_tungstenite::tungstenite::Message::Binary(bytes.clone())
            }
            DAGData::Audio(bytes) => {
                tokio_tungstenite::tungstenite::Message::Binary(bytes.clone())
            }
            other => {
                let json = other.to_json();
                let text = serde_json::to_string(&json).map_err(|e| DAGError::WebSocketEndpointError {
                    url: self.url.clone(),
                    error: format!("Failed to serialize input: {}", e),
                })?;
                tokio_tungstenite::tungstenite::Message::Text(text.into())
            }
        };

        // Build the WebSocket request with custom headers
        let mut request = tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(&self.url)
            .map_err(|e| DAGError::WebSocketEndpointError {
                url: self.url.clone(),
                error: format!("Invalid WebSocket URL: {}", e),
            })?;

        // Add custom headers
        for (key, value) in &self.headers {
            if let (Ok(name), Ok(val)) = (
                key.parse::<tokio_tungstenite::tungstenite::http::HeaderName>(),
                value.parse::<tokio_tungstenite::tungstenite::http::HeaderValue>(),
            ) {
                request.headers_mut().insert(name, val);
            }
        }

        // Add stream ID header
        if let Ok(val) = ctx.stream_id.parse::<tokio_tungstenite::tungstenite::http::HeaderValue>() {
            if let Ok(name) = "X-Stream-ID".parse::<tokio_tungstenite::tungstenite::http::HeaderName>() {
                request.headers_mut().insert(name, val);
            }
        }

        // Add authorization header if API key available
        if let Some(api_key) = &ctx.api_key {
            if let Ok(val) = format!("Bearer {}", api_key).parse::<tokio_tungstenite::tungstenite::http::HeaderValue>() {
                if let Ok(name) = "Authorization".parse::<tokio_tungstenite::tungstenite::http::HeaderName>() {
                    request.headers_mut().insert(name, val);
                }
            }
        }

        info!(
            node_id = %self.id,
            url = %self.url,
            "Connecting to WebSocket endpoint"
        );

        // Connect with timeout
        let timeout = Duration::from_millis(self.timeout_ms);
        let connect_result = tokio::time::timeout(
            timeout,
            tokio_tungstenite::connect_async(request),
        )
        .await
        .map_err(|_| DAGError::WebSocketEndpointError {
            url: self.url.clone(),
            error: format!("Connection timed out after {}ms", self.timeout_ms),
        })?
        .map_err(|e| DAGError::WebSocketEndpointError {
            url: self.url.clone(),
            error: format!("Failed to connect: {}", e),
        })?;

        let (ws_stream, _response) = connect_result;
        let (mut write, mut read) = ws_stream.split();

        info!(
            node_id = %self.id,
            "Connected to WebSocket endpoint, sending message"
        );

        // Send the message
        write.send(message).await.map_err(|e| DAGError::WebSocketEndpointError {
            url: self.url.clone(),
            error: format!("Failed to send message: {}", e),
        })?;

        // Wait for response with timeout
        let response = tokio::time::timeout(timeout, read.next())
            .await
            .map_err(|_| DAGError::WebSocketEndpointError {
                url: self.url.clone(),
                error: format!("Response timed out after {}ms", self.timeout_ms),
            })?
            .ok_or_else(|| DAGError::WebSocketEndpointError {
                url: self.url.clone(),
                error: "Connection closed without response".to_string(),
            })?
            .map_err(|e| DAGError::WebSocketEndpointError {
                url: self.url.clone(),
                error: format!("Failed to receive response: {}", e),
            })?;

        // Close the connection gracefully
        let _ = write.send(tokio_tungstenite::tungstenite::Message::Close(None)).await;

        info!(
            node_id = %self.id,
            "Received WebSocket response"
        );

        // Convert response to DAGData
        // tungstenite 0.28 uses Utf8Bytes for text and Bytes for binary
        match response {
            tokio_tungstenite::tungstenite::Message::Text(utf8_bytes) => {
                // Utf8Bytes derefs to str, so we can use it directly
                let text_str: &str = &utf8_bytes;
                // Try to parse as JSON, fallback to text
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(text_str) {
                    Ok(DAGData::Json(json))
                } else {
                    Ok(DAGData::Text(text_str.to_string()))
                }
            }
            tokio_tungstenite::tungstenite::Message::Binary(bytes) => {
                // Try to parse as JSON, fallback to binary
                if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                    Ok(DAGData::Json(json))
                } else {
                    Ok(DAGData::Binary(bytes))
                }
            }
            tokio_tungstenite::tungstenite::Message::Close(_) => {
                Err(DAGError::WebSocketEndpointError {
                    url: self.url.clone(),
                    error: "Server closed connection".to_string(),
                })
            }
            tokio_tungstenite::tungstenite::Message::Ping(_) |
            tokio_tungstenite::tungstenite::Message::Pong(_) |
            tokio_tungstenite::tungstenite::Message::Frame(_) => {
                // Unexpected control frame as response
                Err(DAGError::WebSocketEndpointError {
                    url: self.url.clone(),
                    error: "Received unexpected control frame instead of response".to_string(),
                })
            }
        }
    }

    fn clone_boxed(&self) -> Arc<dyn DAGNode> {
        Arc::new(self.clone())
    }
}

/// IPC endpoint node
///
/// Communicates with local inference processes via Unix domain sockets.
/// Uses a simple framed protocol: 4-byte big-endian length prefix + data.
///
/// The socket path is derived from the shm_name:
/// - `/waav_whisper_shm` -> `/tmp/waav_whisper_shm.sock`
/// - `waav_kokoro` -> `/tmp/waav_kokoro.sock`
///
/// # Security
/// The shm_name is validated to prevent path traversal attacks.
/// Only alphanumeric characters, underscores, and hyphens are allowed.
#[derive(Clone)]
pub struct IpcEndpointNode {
    id: String,
    /// Sanitized socket name (no path components, safe characters only)
    shm_name: String,
    input_format: Option<String>,
    output_format: Option<String>,
    timeout_ms: u64,
}

/// Validate and sanitize IPC socket name to prevent path traversal attacks.
///
/// Only alphanumeric characters, underscores, and hyphens are allowed.
/// Leading slashes and path components are stripped.
/// Returns an error if the name is empty after sanitization or contains
/// invalid characters that can't be sanitized.
fn sanitize_ipc_socket_name(name: &str) -> DAGResult<String> {
    // Strip leading/trailing slashes and whitespace
    let name = name.trim().trim_matches('/').trim();

    // Check for path traversal attempts
    if name.contains("..") || name.contains('/') || name.contains('\\') {
        return Err(DAGError::ConfigError(format!(
            "Invalid IPC socket name '{}': path components not allowed",
            name
        )));
    }

    // Validate characters (only alphanumeric, underscore, hyphen allowed)
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return Err(DAGError::ConfigError(format!(
            "Invalid IPC socket name '{}': only alphanumeric, underscore, and hyphen allowed",
            name
        )));
    }

    // Check not empty
    if name.is_empty() {
        return Err(DAGError::ConfigError(
            "IPC socket name cannot be empty".to_string()
        ));
    }

    // Check reasonable length (Linux socket path limit is 108 chars)
    // /tmp/{name}.sock = 6 + len + 5 = len + 11, so max len = 97
    if name.len() > 97 {
        return Err(DAGError::ConfigError(format!(
            "IPC socket name '{}' too long: max 97 characters",
            name
        )));
    }

    Ok(name.to_string())
}

impl IpcEndpointNode {
    /// Create a new IPC endpoint node
    ///
    /// The shm_name is validated and sanitized:
    /// - Leading/trailing slashes are stripped
    /// - Only alphanumeric, underscore, and hyphen characters are allowed
    /// - Path traversal attempts (../) are rejected
    ///
    /// # Panics
    /// Panics if the shm_name is invalid. Use `try_new()` for fallible construction.
    pub fn new(id: impl Into<String>, shm_name: impl Into<String>) -> Self {
        let shm_name_str = shm_name.into();
        let sanitized = sanitize_ipc_socket_name(&shm_name_str)
            .expect("Invalid IPC socket name - use try_new() for fallible construction");
        Self {
            id: id.into(),
            shm_name: sanitized,
            input_format: None,
            output_format: None,
            timeout_ms: 30000,
        }
    }

    /// Try to create a new IPC endpoint node with validation
    ///
    /// Returns an error if the shm_name is invalid.
    pub fn try_new(id: impl Into<String>, shm_name: impl Into<String>) -> DAGResult<Self> {
        let sanitized = sanitize_ipc_socket_name(&shm_name.into())?;
        Ok(Self {
            id: id.into(),
            shm_name: sanitized,
            input_format: None,
            output_format: None,
            timeout_ms: 30000,
        })
    }

    /// Set input format (e.g., "pcm16", "json")
    pub fn with_input_format(mut self, format: impl Into<String>) -> Self {
        self.input_format = Some(format.into());
        self
    }

    /// Set output format (e.g., "json", "pcm16")
    pub fn with_output_format(mut self, format: impl Into<String>) -> Self {
        self.output_format = Some(format.into());
        self
    }

    /// Set timeout in milliseconds
    pub fn with_timeout_ms(mut self, timeout: u64) -> Self {
        self.timeout_ms = timeout;
        self
    }

    /// Get the shared memory name
    pub fn shm_name(&self) -> &str {
        &self.shm_name
    }

    /// Get the Unix socket path from shm_name
    ///
    /// The shm_name is already sanitized at construction time,
    /// so this is safe from path traversal attacks.
    fn socket_path(&self) -> String {
        // shm_name is already sanitized - no slashes, safe characters only
        format!("/tmp/{}.sock", self.shm_name)
    }
}

impl std::fmt::Debug for IpcEndpointNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IpcEndpointNode")
            .field("id", &self.id)
            .field("shm_name", &self.shm_name)
            .field("input_format", &self.input_format)
            .field("output_format", &self.output_format)
            .field("timeout_ms", &self.timeout_ms)
            .finish()
    }
}

#[async_trait]
impl DAGNode for IpcEndpointNode {
    fn id(&self) -> &str {
        &self.id
    }

    fn node_type(&self) -> &str {
        "ipc_endpoint"
    }

    fn capabilities(&self) -> Vec<NodeCapability> {
        vec![
            NodeCapability::AudioInput,
            NodeCapability::TextInput,
            NodeCapability::JsonInput,
            NodeCapability::AudioOutput,
            NodeCapability::TextOutput,
            NodeCapability::JsonOutput,
        ]
    }

    #[cfg(unix)]
    async fn execute(&self, input: DAGData, ctx: &mut DAGContext) -> DAGResult<DAGData> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::UnixStream;

        let socket_path = self.socket_path();

        debug!(
            node_id = %self.id,
            socket_path = %socket_path,
            "IPC endpoint called"
        );

        // Prepare the data to send based on input format
        let send_data: Bytes = match &input {
            DAGData::Audio(bytes) => {
                // For audio, check if we should wrap in JSON or send raw
                if self.input_format.as_deref() == Some("json") {
                    // Wrap audio as base64 in JSON
                    let json = serde_json::json!({
                        "type": "audio",
                        "stream_id": ctx.stream_id,
                        "data": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes),
                        "format": "pcm16"
                    });
                    serde_json::to_vec(&json)
                        .map_err(|e| DAGError::IpcEndpointError {
                            name: self.shm_name.clone(),
                            error: format!("Failed to serialize audio: {}", e),
                        })?
                        .into()
                } else {
                    // Send raw audio bytes
                    bytes.clone()
                }
            }
            DAGData::Json(json) => {
                serde_json::to_vec(json)
                    .map_err(|e| DAGError::IpcEndpointError {
                        name: self.shm_name.clone(),
                        error: format!("Failed to serialize JSON: {}", e),
                    })?
                    .into()
            }
            DAGData::Text(text) => {
                if self.input_format.as_deref() == Some("json") {
                    let json = serde_json::json!({
                        "type": "text",
                        "stream_id": ctx.stream_id,
                        "text": text
                    });
                    serde_json::to_vec(&json)
                        .map_err(|e| DAGError::IpcEndpointError {
                            name: self.shm_name.clone(),
                            error: format!("Failed to serialize text: {}", e),
                        })?
                        .into()
                } else {
                    Bytes::from(text.clone())
                }
            }
            DAGData::Binary(bytes) => bytes.clone(),
            other => {
                let json = other.to_json();
                serde_json::to_vec(&json)
                    .map_err(|e| DAGError::IpcEndpointError {
                        name: self.shm_name.clone(),
                        error: format!("Failed to serialize input: {}", e),
                    })?
                    .into()
            }
        };

        let timeout = Duration::from_millis(self.timeout_ms);

        // Connect to Unix socket with timeout
        let mut stream = tokio::time::timeout(timeout, UnixStream::connect(&socket_path))
            .await
            .map_err(|_| DAGError::IpcEndpointError {
                name: self.shm_name.clone(),
                error: format!("Connection timed out after {}ms", self.timeout_ms),
            })?
            .map_err(|e| DAGError::IpcEndpointError {
                name: self.shm_name.clone(),
                error: format!("Failed to connect to '{}': {}", socket_path, e),
            })?;

        info!(
            node_id = %self.id,
            socket_path = %socket_path,
            data_size = send_data.len(),
            "Connected to IPC endpoint, sending data"
        );

        // Send length-prefixed message (4-byte big-endian length + data)
        let len_bytes = (send_data.len() as u32).to_be_bytes();
        stream.write_all(&len_bytes).await.map_err(|e| DAGError::IpcEndpointError {
            name: self.shm_name.clone(),
            error: format!("Failed to send length prefix: {}", e),
        })?;
        stream.write_all(&send_data).await.map_err(|e| DAGError::IpcEndpointError {
            name: self.shm_name.clone(),
            error: format!("Failed to send data: {}", e),
        })?;

        // Read length-prefixed response with timeout
        let mut len_buf = [0u8; 4];
        tokio::time::timeout(timeout, stream.read_exact(&mut len_buf))
            .await
            .map_err(|_| DAGError::IpcEndpointError {
                name: self.shm_name.clone(),
                error: format!("Response timed out after {}ms", self.timeout_ms),
            })?
            .map_err(|e| DAGError::IpcEndpointError {
                name: self.shm_name.clone(),
                error: format!("Failed to read response length: {}", e),
            })?;

        let response_len = u32::from_be_bytes(len_buf) as usize;

        // Sanity check on response length (max 100MB)
        if response_len > 100 * 1024 * 1024 {
            return Err(DAGError::IpcEndpointError {
                name: self.shm_name.clone(),
                error: format!("Response too large: {} bytes", response_len),
            });
        }

        let mut response_buf = vec![0u8; response_len];
        tokio::time::timeout(timeout, stream.read_exact(&mut response_buf))
            .await
            .map_err(|_| DAGError::IpcEndpointError {
                name: self.shm_name.clone(),
                error: format!("Response data timed out after {}ms", self.timeout_ms),
            })?
            .map_err(|e| DAGError::IpcEndpointError {
                name: self.shm_name.clone(),
                error: format!("Failed to read response data: {}", e),
            })?;

        info!(
            node_id = %self.id,
            response_size = response_len,
            "Received IPC response"
        );

        // Parse response based on output format
        match self.output_format.as_deref() {
            Some("pcm16") | Some("audio") => {
                // Return raw audio bytes
                Ok(DAGData::Audio(Bytes::from(response_buf)))
            }
            Some("json") | None => {
                // Try to parse as JSON
                if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&response_buf) {
                    Ok(DAGData::Json(json))
                } else {
                    // Fallback to binary if not valid JSON
                    Ok(DAGData::Binary(Bytes::from(response_buf)))
                }
            }
            Some("text") => {
                match String::from_utf8(response_buf) {
                    Ok(text) => Ok(DAGData::Text(text)),
                    Err(e) => Err(DAGError::IpcEndpointError {
                        name: self.shm_name.clone(),
                        error: format!("Invalid UTF-8 response: {}", e),
                    }),
                }
            }
            Some(other) => {
                warn!(
                    node_id = %self.id,
                    output_format = %other,
                    "Unknown output format, returning as binary"
                );
                Ok(DAGData::Binary(Bytes::from(response_buf)))
            }
        }
    }

    #[cfg(not(unix))]
    async fn execute(&self, _input: DAGData, _ctx: &mut DAGContext) -> DAGResult<DAGData> {
        Err(DAGError::IpcEndpointError {
            name: self.shm_name.clone(),
            error: "IPC endpoints are only supported on Unix platforms".to_string(),
        })
    }

    fn clone_boxed(&self) -> Arc<dyn DAGNode> {
        Arc::new(self.clone())
    }
}

/// LiveKit endpoint node
///
/// Sends data to/from LiveKit rooms.
#[derive(Clone)]
pub struct LiveKitEndpointNode {
    id: String,
    room: Option<String>,
    track_type: Option<String>,
}

impl LiveKitEndpointNode {
    /// Create a new LiveKit endpoint node
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            room: None,
            track_type: None,
        }
    }

    /// Set the room name
    pub fn with_room(mut self, room: impl Into<String>) -> Self {
        self.room = Some(room.into());
        self
    }

    /// Set the track type
    pub fn with_track_type(mut self, track_type: impl Into<String>) -> Self {
        self.track_type = Some(track_type.into());
        self
    }
}

impl std::fmt::Debug for LiveKitEndpointNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveKitEndpointNode")
            .field("id", &self.id)
            .field("room", &self.room)
            .field("track_type", &self.track_type)
            .finish()
    }
}

#[async_trait]
impl DAGNode for LiveKitEndpointNode {
    fn id(&self) -> &str {
        &self.id
    }

    fn node_type(&self) -> &str {
        "livekit_endpoint"
    }

    fn capabilities(&self) -> Vec<NodeCapability> {
        vec![
            NodeCapability::AudioInput,
            NodeCapability::AudioOutput,
            NodeCapability::TextInput,
            NodeCapability::TextOutput,
            NodeCapability::Streaming,
            NodeCapability::Cancellable,
        ]
    }

    async fn execute(&self, input: DAGData, ctx: &mut DAGContext) -> DAGResult<DAGData> {
        debug!(
            node_id = %self.id,
            room = ?self.room,
            track_type = ?self.track_type,
            "LiveKit endpoint called"
        );

        // Get LiveKit client from context external resources
        let livekit_client: Arc<RwLock<LiveKitClient>> = ctx
            .get_resource_as::<RwLock<LiveKitClient>>(resource_keys::LIVEKIT_CLIENT)
            .ok_or_else(|| {
                DAGError::LiveKitEndpointError(
                    "LiveKitClient not found in DAG context. Ensure the connection has a LiveKit client configured.".to_string(),
                )
            })?;

        // Handle different input types
        match input {
            DAGData::Audio(audio_bytes) => {
                debug!(
                    node_id = %self.id,
                    audio_len = audio_bytes.len(),
                    "Sending audio to LiveKit"
                );

                // Convert Bytes to Vec<u8> for LiveKit client
                let audio_vec = audio_bytes.to_vec();

                // Get read lock and send audio
                let client = livekit_client.read().await;
                client.send_tts_audio(audio_vec).await.map_err(|e| {
                    error!(node_id = %self.id, error = %e, "Failed to send audio to LiveKit");
                    DAGError::LiveKitEndpointError(format!("Failed to send audio: {}", e))
                })?;

                debug!(node_id = %self.id, "Audio sent to LiveKit successfully");
                Ok(DAGData::Empty)
            }

            DAGData::TTSAudio(tts_audio) => {
                debug!(
                    node_id = %self.id,
                    audio_len = tts_audio.data.len(),
                    "Sending TTS audio to LiveKit"
                );

                let client = livekit_client.read().await;
                // Convert Bytes to Vec<u8>
                client.send_tts_audio(tts_audio.data.to_vec()).await.map_err(|e| {
                    error!(node_id = %self.id, error = %e, "Failed to send TTS audio to LiveKit");
                    DAGError::LiveKitEndpointError(format!("Failed to send TTS audio: {}", e))
                })?;

                debug!(node_id = %self.id, "TTS audio sent to LiveKit successfully");
                Ok(DAGData::Empty)
            }

            DAGData::Text(text) => {
                debug!(
                    node_id = %self.id,
                    text_len = text.len(),
                    "Sending text message to LiveKit"
                );

                let client = livekit_client.read().await;
                // Send as agent message by default
                client.send_message(&text, "agent", None, false).await.map_err(|e| {
                    error!(node_id = %self.id, error = %e, "Failed to send message to LiveKit");
                    DAGError::LiveKitEndpointError(format!("Failed to send message: {}", e))
                })?;

                debug!(node_id = %self.id, "Text message sent to LiveKit successfully");
                Ok(DAGData::Empty)
            }

            DAGData::Json(json) => {
                debug!(
                    node_id = %self.id,
                    "Sending JSON message to LiveKit"
                );

                // Serialize JSON and send as message
                let text = serde_json::to_string(&json).map_err(|e| {
                    DAGError::LiveKitEndpointError(format!("Failed to serialize JSON: {}", e))
                })?;

                let client = livekit_client.read().await;
                client.send_message(&text, "agent", Some("data"), false).await.map_err(|e| {
                    error!(node_id = %self.id, error = %e, "Failed to send JSON to LiveKit");
                    DAGError::LiveKitEndpointError(format!("Failed to send JSON message: {}", e))
                })?;

                debug!(node_id = %self.id, "JSON message sent to LiveKit successfully");
                Ok(DAGData::Empty)
            }

            DAGData::STTResult(stt_result) => {
                debug!(
                    node_id = %self.id,
                    transcript_len = stt_result.transcript.len(),
                    is_final = stt_result.is_final,
                    "Sending STT result to LiveKit"
                );

                // Format STT result as message
                let message_json = serde_json::json!({
                    "type": "stt_result",
                    "transcript": stt_result.transcript,
                    "is_final": stt_result.is_final,
                    "is_speech_final": stt_result.is_speech_final,
                    "confidence": stt_result.confidence,
                });

                let text = serde_json::to_string(&message_json).map_err(|e| {
                    DAGError::LiveKitEndpointError(format!("Failed to serialize STT result: {}", e))
                })?;

                let client = livekit_client.read().await;
                client.send_message(&text, "agent", Some("transcriptions"), false).await.map_err(|e| {
                    error!(node_id = %self.id, error = %e, "Failed to send STT result to LiveKit");
                    DAGError::LiveKitEndpointError(format!("Failed to send STT result: {}", e))
                })?;

                debug!(node_id = %self.id, "STT result sent to LiveKit successfully");
                Ok(DAGData::Empty)
            }

            DAGData::Binary(bytes) => {
                // Treat binary as audio by default based on track_type
                if self.track_type.as_deref() == Some("audio") || self.track_type.is_none() {
                    debug!(
                        node_id = %self.id,
                        bytes_len = bytes.len(),
                        "Sending binary as audio to LiveKit"
                    );

                    let client = livekit_client.read().await;
                    client.send_tts_audio(bytes.to_vec()).await.map_err(|e| {
                        error!(node_id = %self.id, error = %e, "Failed to send binary audio to LiveKit");
                        DAGError::LiveKitEndpointError(format!("Failed to send binary audio: {}", e))
                    })?;

                    Ok(DAGData::Empty)
                } else {
                    // Send as data message
                    let text = String::from_utf8_lossy(&bytes).to_string();
                    let client = livekit_client.read().await;
                    client.send_message(&text, "agent", Some("data"), false).await.map_err(|e| {
                        DAGError::LiveKitEndpointError(format!("Failed to send binary data: {}", e))
                    })?;

                    Ok(DAGData::Empty)
                }
            }

            DAGData::Empty => {
                debug!(node_id = %self.id, "Empty input, nothing to send");
                Ok(DAGData::Empty)
            }

            DAGData::Multiple(items) => {
                debug!(
                    node_id = %self.id,
                    item_count = items.len(),
                    "Processing multiple items for LiveKit"
                );

                // Process each item sequentially
                for item in items {
                    // Recursively call execute for each item
                    // We need to clone self since we can't move it
                    Box::pin(self.execute(item, ctx)).await?;
                }

                Ok(DAGData::Empty)
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

    #[test]
    fn test_http_endpoint_builder() {
        let node = HttpEndpointNode::new("http", "https://api.example.com")
            .with_method(HttpMethod::POST)
            .with_header("Authorization", "Bearer token")
            .with_timeout_ms(5000);

        assert_eq!(node.id(), "http");
        assert_eq!(node.url(), "https://api.example.com");
        assert_eq!(node.timeout_ms, 5000);
    }

    #[test]
    fn test_grpc_endpoint_builder() {
        let node = GrpcEndpointNode::new(
            "grpc",
            "localhost:50051",
            "inference.LLMService",
            "Generate",
        )
        .with_timeout_ms(10000);

        assert_eq!(node.id(), "grpc");
        assert_eq!(node.address(), "localhost:50051");
        assert_eq!(node.service, "inference.LLMService");
        assert_eq!(node.method, "Generate");
    }

    #[test]
    fn test_ipc_endpoint_builder() {
        // Leading slash is stripped during sanitization
        let node = IpcEndpointNode::new("ipc", "/waav_whisper_shm")
            .with_input_format("pcm16")
            .with_output_format("json");

        assert_eq!(node.id(), "ipc");
        // shm_name is sanitized: leading slash stripped, only safe chars
        assert_eq!(node.shm_name(), "waav_whisper_shm");
        assert_eq!(node.input_format, Some("pcm16".to_string()));
    }

    #[test]
    fn test_ipc_endpoint_path_traversal_protection() {
        // Path traversal attempts should fail
        let result = IpcEndpointNode::try_new("ipc", "../../../etc/passwd");
        assert!(result.is_err());

        let result = IpcEndpointNode::try_new("ipc", "foo/../bar");
        assert!(result.is_err());

        let result = IpcEndpointNode::try_new("ipc", "foo/bar");
        assert!(result.is_err());

        let result = IpcEndpointNode::try_new("ipc", "");
        assert!(result.is_err());

        // Special characters should fail
        let result = IpcEndpointNode::try_new("ipc", "foo$bar");
        assert!(result.is_err());

        // Valid names should work
        let result = IpcEndpointNode::try_new("ipc", "waav_whisper_shm");
        assert!(result.is_ok());

        let result = IpcEndpointNode::try_new("ipc", "my-socket-name");
        assert!(result.is_ok());

        let result = IpcEndpointNode::try_new("ipc", "socket123");
        assert!(result.is_ok());
    }

    #[test]
    fn test_http_endpoint_capabilities() {
        let node = HttpEndpointNode::new("http", "https://api.example.com");
        let caps = node.capabilities();

        assert!(caps.contains(&NodeCapability::JsonInput));
        assert!(caps.contains(&NodeCapability::JsonOutput));
        assert!(caps.contains(&NodeCapability::Cancellable));
    }
}
