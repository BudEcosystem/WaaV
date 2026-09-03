//! HTTP endpoint adapter

use async_trait::async_trait;
use std::collections::HashMap;
use std::time::Duration;
use tracing::debug;

use super::EndpointAdapter;
use crate::dag::context::DAGContext;
use crate::dag::definition::HttpMethod;
use crate::dag::error::{DAGError, DAGResult};
use crate::dag::nodes::DAGData;

fn ssrf_protected_http_client() -> DAGResult<reqwest::Client> {
    crate::core::net::ssrf_protected_client(crate::core::net::HTTP_URL_SCHEMES)
        .map_err(|e| DAGError::ConfigError(format!("failed to build DAG HTTP adapter client: {e}")))
}

/// HTTP endpoint adapter
pub struct HttpAdapter {
    id: String,
    url: String,
    method: HttpMethod,
    headers: HashMap<String, String>,
    timeout: Duration,
    client: Option<reqwest::Client>,
    http_client_init_error: Option<String>,
    connected: bool,
}

impl HttpAdapter {
    /// Create a new HTTP adapter
    pub fn new(id: impl Into<String>, url: impl Into<String>) -> Self {
        let (client, http_client_init_error) = match ssrf_protected_http_client() {
            Ok(client) => (Some(client), None),
            Err(error) => (None, Some(error.to_string())),
        };
        let connected = client.is_some();
        Self {
            id: id.into(),
            url: url.into(),
            method: HttpMethod::POST,
            headers: HashMap::new(),
            timeout: Duration::from_secs(30),
            client,
            http_client_init_error,
            connected,
        }
    }

    /// Create a new HTTP adapter with SSRF URL validation.
    pub fn try_new(id: impl Into<String>, url: impl Into<String>) -> DAGResult<Self> {
        let url = url.into();
        crate::core::net::validate_url_for_ssrf(&url, crate::core::net::HTTP_URL_SCHEMES)
            .map_err(DAGError::ConfigError)?;
        Ok(Self {
            id: id.into(),
            url,
            method: HttpMethod::POST,
            headers: HashMap::new(),
            timeout: Duration::from_secs(30),
            client: Some(ssrf_protected_http_client()?),
            http_client_init_error: None,
            connected: true,
        })
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

    /// Set timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Get URL
    pub fn url(&self) -> &str {
        &self.url
    }
}

#[async_trait]
impl EndpointAdapter for HttpAdapter {
    fn endpoint_type(&self) -> &str {
        "http"
    }

    fn endpoint_id(&self) -> &str {
        &self.id
    }

    async fn send(&self, data: DAGData, ctx: &DAGContext) -> DAGResult<DAGData> {
        let Some(client) = &self.client else {
            return Err(DAGError::HttpEndpointError {
                url: self.url.clone(),
                error: self
                    .http_client_init_error
                    .clone()
                    .unwrap_or_else(|| "DAG HTTP adapter client was not initialized".to_string()),
            });
        };
        let payload = data.to_json();

        debug!(
            endpoint_id = %self.id,
            url = %self.url,
            method = ?self.method,
            "HTTP request"
        );

        let mut request = client
            .request(self.method.clone().into(), &self.url)
            .timeout(self.timeout)
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

        let response = request
            .send()
            .await
            .map_err(|e| DAGError::HttpEndpointError {
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

        let json: serde_json::Value =
            response
                .json()
                .await
                .map_err(|e| DAGError::HttpEndpointError {
                    url: self.url.clone(),
                    error: format!("Failed to parse response: {}", e),
                })?;

        Ok(DAGData::Json(json))
    }

    fn is_connected(&self) -> bool {
        self.connected && self.client.is_some()
    }

    async fn connect(&mut self) -> DAGResult<()> {
        if self.client.is_none() {
            return Err(DAGError::HttpEndpointError {
                url: self.url.clone(),
                error: self
                    .http_client_init_error
                    .clone()
                    .unwrap_or_else(|| "DAG HTTP adapter client was not initialized".to_string()),
            });
        }
        self.connected = true;
        Ok(())
    }

    async fn disconnect(&mut self) -> DAGResult<()> {
        self.connected = false;
        Ok(())
    }

    fn timeout(&self) -> Duration {
        self.timeout
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_adapter_builder() {
        let adapter = HttpAdapter::new("test", "https://api.example.com")
            .with_method(HttpMethod::POST)
            .with_header("Authorization", "Bearer token")
            .with_timeout(Duration::from_secs(60));

        assert_eq!(adapter.endpoint_id(), "test");
        assert_eq!(adapter.url(), "https://api.example.com");
        assert_eq!(adapter.timeout(), Duration::from_secs(60));
    }

    #[test]
    fn test_http_adapter_default_connected() {
        let adapter = HttpAdapter::new("test", "https://api.example.com");
        assert!(adapter.is_connected());
    }

    #[test]
    fn test_http_adapter_try_new_blocks_ssrf_and_ws_scheme() {
        let _env = crate::core::net::ssrf_env_lock();
        assert!(HttpAdapter::try_new("test", "https://api.example.com").is_ok());
        assert!(HttpAdapter::try_new("test", "http://127.0.0.1:8080/admin").is_err());
        assert!(HttpAdapter::try_new("test", "wss://api.example.com/socket").is_err());
    }

    #[tokio::test]
    async fn http_adapter_client_init_failure_returns_typed_error() {
        let adapter = HttpAdapter {
            id: "http".to_string(),
            url: "https://api.example.com".to_string(),
            method: HttpMethod::POST,
            headers: HashMap::new(),
            timeout: Duration::from_secs(30),
            client: None,
            http_client_init_error: Some("client build failed".to_string()),
            connected: false,
        };
        let ctx = DAGContext::new("client-init-failure");

        let error = adapter
            .send(DAGData::Text("payload".to_string()), &ctx)
            .await
            .expect_err("missing client must return an HTTP endpoint error");

        match error {
            DAGError::HttpEndpointError { error, .. } => {
                assert!(error.contains("client build failed"), "{error}");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }
}
