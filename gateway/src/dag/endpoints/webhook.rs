//! Webhook endpoint adapter (fire-and-forget)

use std::collections::HashMap;
use std::time::Duration;
use async_trait::async_trait;
use tracing::{debug, warn};

use super::EndpointAdapter;
use crate::dag::context::DAGContext;
use crate::dag::nodes::DAGData;
use crate::dag::error::{DAGError, DAGResult};

/// Webhook endpoint adapter
///
/// Fire-and-forget webhook delivery. Can optionally wait for response.
pub struct WebhookAdapter {
    id: String,
    url: String,
    headers: HashMap<String, String>,
    timeout: Duration,
    fire_and_forget: bool,
    client: reqwest::Client,
}

impl WebhookAdapter {
    /// Create a new webhook adapter (fire-and-forget)
    pub fn new(id: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            url: url.into(),
            headers: HashMap::new(),
            timeout: Duration::from_secs(5),
            fire_and_forget: true,
            client: reqwest::Client::new(),
        }
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

    /// Wait for response instead of fire-and-forget
    pub fn wait_for_response(mut self) -> Self {
        self.fire_and_forget = false;
        self
    }

    /// Get URL
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Check if fire-and-forget mode
    pub fn is_fire_and_forget(&self) -> bool {
        self.fire_and_forget
    }
}

#[async_trait]
impl EndpointAdapter for WebhookAdapter {
    fn endpoint_type(&self) -> &str {
        "webhook"
    }

    fn endpoint_id(&self) -> &str {
        &self.id
    }

    async fn send(&self, data: DAGData, ctx: &DAGContext) -> DAGResult<DAGData> {
        let payload = data.to_json();

        debug!(
            endpoint_id = %self.id,
            url = %self.url,
            fire_and_forget = %self.fire_and_forget,
            "Webhook send"
        );

        let mut request = self.client
            .post(&self.url)
            .timeout(self.timeout)
            .header("Content-Type", "application/json")
            .header("X-Stream-ID", &ctx.stream_id);

        // Add custom headers
        for (key, value) in &self.headers {
            request = request.header(key, value);
        }

        // Add API key ID if available
        if let Some(api_key_id) = &ctx.api_key_id {
            request = request.header("X-API-Key-ID", api_key_id);
        }

        request = request.json(&payload);

        if self.fire_and_forget {
            // Spawn task and don't wait
            let url = self.url.clone();
            let endpoint_id = self.id.clone();
            tokio::spawn(async move {
                match request.send().await {
                    Ok(response) => {
                        if !response.status().is_success() {
                            warn!(
                                endpoint_id = %endpoint_id,
                                url = %url,
                                status = %response.status(),
                                "Webhook returned non-success status"
                            );
                        }
                    }
                    Err(e) => {
                        warn!(
                            endpoint_id = %endpoint_id,
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

    fn is_connected(&self) -> bool {
        true // HTTP is stateless
    }

    async fn connect(&mut self) -> DAGResult<()> {
        Ok(())
    }

    async fn disconnect(&mut self) -> DAGResult<()> {
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
    fn test_webhook_adapter_builder() {
        let adapter = WebhookAdapter::new("test", "https://hooks.example.com")
            .with_header("Authorization", "Bearer token")
            .with_timeout(Duration::from_secs(10));

        assert_eq!(adapter.endpoint_id(), "test");
        assert_eq!(adapter.url(), "https://hooks.example.com");
        assert!(adapter.is_fire_and_forget());
    }

    #[test]
    fn test_webhook_wait_mode() {
        let adapter = WebhookAdapter::new("test", "https://hooks.example.com")
            .wait_for_response();

        assert!(!adapter.is_fire_and_forget());
    }
}
