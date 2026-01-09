//! HTTP endpoint adapter

use std::collections::HashMap;
use std::time::Duration;
use async_trait::async_trait;
use tracing::debug;

use super::EndpointAdapter;
use crate::dag::context::DAGContext;
use crate::dag::definition::HttpMethod;
use crate::dag::nodes::DAGData;
use crate::dag::error::{DAGError, DAGResult};

/// HTTP endpoint adapter
pub struct HttpAdapter {
    id: String,
    url: String,
    method: HttpMethod,
    headers: HashMap<String, String>,
    timeout: Duration,
    client: reqwest::Client,
    connected: bool,
}

impl HttpAdapter {
    /// Create a new HTTP adapter
    pub fn new(id: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            url: url.into(),
            method: HttpMethod::POST,
            headers: HashMap::new(),
            timeout: Duration::from_secs(30),
            client: reqwest::Client::new(),
            connected: true, // HTTP is stateless, always "connected"
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
        let payload = data.to_json();

        debug!(
            endpoint_id = %self.id,
            url = %self.url,
            method = ?self.method,
            "HTTP request"
        );

        let mut request = self.client
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

    fn is_connected(&self) -> bool {
        self.connected
    }

    async fn connect(&mut self) -> DAGResult<()> {
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
}
