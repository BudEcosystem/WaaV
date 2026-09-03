//! Huawei Cloud IAM Token Management
//!
//! This module handles IAM token authentication for Huawei Cloud services.
//! Tokens are obtained using username/password credentials and cached for reuse.

use serde::Serialize;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{debug, info};

use super::config::{HuaweiCloudRegion, HuaweiIamTokenResponse, TOKEN_VALIDITY_SECS};
use crate::core::stt::base::STTError;

// =============================================================================
// Constants
// =============================================================================

/// HTTP request timeout for IAM token requests.
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// Buffer time before token expiration to trigger refresh (1 hour).
const TOKEN_REFRESH_BUFFER_SECS: u64 = 3600;

fn huawei_iam_http_client() -> Result<reqwest::Client, reqwest::Error> {
    crate::core::net::ssrf_protected_client_builder(crate::core::net::HTTP_URL_SCHEMES)
        .timeout(HTTP_TIMEOUT)
        .build()
}

fn huawei_iam_default_http_client() -> Option<reqwest::Client> {
    match huawei_iam_http_client() {
        Ok(client) => Some(client),
        Err(e) => {
            tracing::error!(error = %e, "failed to build SSRF-protected Huawei IAM HTTP client");
            None
        }
    }
}

fn huawei_iam_http_client_unavailable_error() -> STTError {
    STTError::ConfigurationError(
        "Huawei IAM HTTP client unavailable: failed to initialize reqwest client".to_string(),
    )
}

// =============================================================================
// IAM Token Request
// =============================================================================

/// IAM token request body structure.
#[derive(Debug, Clone, Serialize)]
pub struct IamTokenRequest {
    /// Authentication information.
    pub auth: IamAuth,
}

/// IAM authentication information.
#[derive(Debug, Clone, Serialize)]
pub struct IamAuth {
    /// Identity information.
    pub identity: IamIdentity,
    /// Scope information.
    pub scope: IamScope,
}

/// IAM identity information.
#[derive(Debug, Clone, Serialize)]
pub struct IamIdentity {
    /// Authentication methods.
    pub methods: Vec<String>,
    /// Password credentials.
    pub password: IamPassword,
}

/// IAM password credentials.
#[derive(Debug, Clone, Serialize)]
pub struct IamPassword {
    /// User information.
    pub user: IamUser,
}

/// IAM user information.
#[derive(Debug, Clone, Serialize)]
pub struct IamUser {
    /// Username.
    pub name: String,
    /// Password.
    pub password: String,
    /// Domain information.
    pub domain: IamDomain,
}

/// IAM domain information.
#[derive(Debug, Clone, Serialize)]
pub struct IamDomain {
    /// Domain name.
    pub name: String,
}

/// IAM scope information.
#[derive(Debug, Clone, Serialize)]
pub struct IamScope {
    /// Project information.
    pub project: IamProject,
}

/// IAM project information.
#[derive(Debug, Clone, Serialize)]
pub struct IamProject {
    /// Project name (region code).
    pub name: String,
}

impl IamTokenRequest {
    /// Create a new IAM token request.
    pub fn new(
        username: &str,
        password: &str,
        domain_name: &str,
        region: HuaweiCloudRegion,
    ) -> Self {
        Self {
            auth: IamAuth {
                identity: IamIdentity {
                    methods: vec!["password".to_string()],
                    password: IamPassword {
                        user: IamUser {
                            name: username.to_string(),
                            password: password.to_string(),
                            domain: IamDomain {
                                name: domain_name.to_string(),
                            },
                        },
                    },
                },
                scope: IamScope {
                    project: IamProject {
                        name: region.code().to_string(),
                    },
                },
            },
        }
    }

    /// Serialize to JSON string.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

// =============================================================================
// Token Manager
// =============================================================================

/// IAM token manager with automatic refresh capability.
#[derive(Debug)]
pub struct HuaweiTokenManager {
    /// Cached access token.
    token: RwLock<Option<String>>,

    /// Token expiration timestamp (Unix seconds).
    expires_at: RwLock<Option<u64>>,

    /// HTTP client for token requests.
    client: Option<reqwest::Client>,
}

impl HuaweiTokenManager {
    /// Create a new token manager.
    pub fn new() -> Self {
        Self {
            token: RwLock::new(None),
            expires_at: RwLock::new(None),
            client: huawei_iam_default_http_client(),
        }
    }

    /// Create a token manager with a caller-provided protected HTTP client.
    pub fn with_client(client: reqwest::Client) -> Self {
        Self {
            token: RwLock::new(None),
            expires_at: RwLock::new(None),
            client: Some(client),
        }
    }

    /// Get current timestamp in Unix seconds.
    fn now_secs() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    /// Check if the token is valid (with buffer time).
    pub async fn is_token_valid(&self) -> bool {
        let expires_at = *self.expires_at.read().await;
        let token = self.token.read().await;

        match (token.as_ref(), expires_at) {
            (Some(_), Some(exp)) => {
                Self::now_secs() < exp.saturating_sub(TOKEN_REFRESH_BUFFER_SECS)
            }
            _ => false,
        }
    }

    /// Get the cached token if valid.
    pub async fn get_cached_token(&self) -> Option<String> {
        if self.is_token_valid().await {
            self.token.read().await.clone()
        } else {
            None
        }
    }

    /// Get or refresh the access token.
    pub async fn get_token(
        &self,
        username: &str,
        password: &str,
        domain_name: &str,
        region: HuaweiCloudRegion,
    ) -> Result<String, STTError> {
        self.get_token_with_override(username, password, domain_name, region, None)
            .await
    }

    /// Get or refresh the access token, optionally redirecting the IAM token POST to `iam_override`.
    ///
    /// Behaviorally identical to [`Self::get_token`] when `iam_override` is `None`. The override
    /// (scheme+host) lets the credential-free mock harness redirect IAM without touching the cache
    /// or token semantics.
    pub async fn get_token_with_override(
        &self,
        username: &str,
        password: &str,
        domain_name: &str,
        region: HuaweiCloudRegion,
        iam_override: Option<&str>,
    ) -> Result<String, STTError> {
        // Check if we have a valid cached token
        if let Some(token) = self.get_cached_token().await {
            return Ok(token);
        }

        // Fetch new token
        self.fetch_token(username, password, domain_name, region, iam_override)
            .await
    }

    /// Fetch a new token from IAM.
    ///
    /// `iam_override`, when `Some(base)`, redirects the IAM token POST to that base (scheme+host),
    /// keeping the IAM path+query — used by the credential-free mock harness. `None` is production.
    pub async fn fetch_token(
        &self,
        username: &str,
        password: &str,
        domain_name: &str,
        region: HuaweiCloudRegion,
        iam_override: Option<&str>,
    ) -> Result<String, STTError> {
        let url = crate::core::tts::standard::override_rest_endpoint(
            &region.iam_endpoint(),
            iam_override,
        );
        let request = IamTokenRequest::new(username, password, domain_name, region);

        let request_json = request.to_json().map_err(|e| {
            STTError::ConfigurationError(format!("Failed to serialize IAM request: {}", e))
        })?;

        debug!("Fetching new Huawei Cloud IAM token from {}", url);

        let client = self
            .client
            .as_ref()
            .ok_or_else(huawei_iam_http_client_unavailable_error)?;

        let response = client
            .post(&url)
            .header("Content-Type", "application/json")
            .body(request_json)
            .send()
            .await
            .map_err(|e| STTError::AuthenticationFailed(format!("IAM request failed: {}", e)))?;

        // Token is returned in the X-Subject-Token header
        let token = response
            .headers()
            .get("X-Subject-Token")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(STTError::AuthenticationFailed(format!(
                "IAM authentication failed with status {}: {}",
                status, body
            )));
        }

        let token = token.ok_or_else(|| {
            STTError::AuthenticationFailed("No X-Subject-Token header in IAM response".to_string())
        })?;

        // Parse response body to get expiration time
        let body = response.text().await.unwrap_or_default();
        let expires_at =
            if let Ok(token_response) = serde_json::from_str::<HuaweiIamTokenResponse>(&body) {
                // Parse ISO 8601 expiration time
                parse_iso8601_timestamp(&token_response.token.expires_at)
                    .unwrap_or_else(|| Self::now_secs() + TOKEN_VALIDITY_SECS)
            } else {
                // Default to 24 hours if parsing fails
                Self::now_secs() + TOKEN_VALIDITY_SECS
            };

        // Cache the token
        *self.token.write().await = Some(token.clone());
        *self.expires_at.write().await = Some(expires_at);

        let remaining_secs = expires_at.saturating_sub(Self::now_secs());
        info!(
            "Huawei Cloud IAM token obtained, valid for {} hours",
            remaining_secs / 3600
        );

        Ok(token)
    }

    /// Clear the cached token.
    pub async fn clear(&self) {
        *self.token.write().await = None;
        *self.expires_at.write().await = None;
        debug!("Huawei Cloud IAM token cache cleared");
    }

    /// Force token refresh.
    pub async fn refresh(
        &self,
        username: &str,
        password: &str,
        domain_name: &str,
        region: HuaweiCloudRegion,
    ) -> Result<String, STTError> {
        self.clear().await;
        self.fetch_token(username, password, domain_name, region, None)
            .await
    }
}

impl Default for HuaweiTokenManager {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Helper Functions
// =============================================================================

/// Parse ISO 8601 timestamp to Unix seconds.
fn parse_iso8601_timestamp(timestamp: &str) -> Option<u64> {
    // Format: "2024-01-15T12:00:00.000000Z" or "2024-01-15T12:00:00Z"
    // Simple parsing without external crate dependencies

    // Remove timezone indicator
    let cleaned = timestamp
        .trim_end_matches('Z')
        .trim_end_matches("+00:00")
        .replace('T', " ");

    // Split into date and time
    let parts: Vec<&str> = cleaned.split(' ').collect();
    if parts.len() != 2 {
        return None;
    }

    let date_parts: Vec<&str> = parts[0].split('-').collect();
    if date_parts.len() != 3 {
        return None;
    }

    // Remove fractional seconds
    let time_str = parts[1].split('.').next().unwrap_or(parts[1]);
    let time_parts: Vec<&str> = time_str.split(':').collect();
    if time_parts.len() != 3 {
        return None;
    }

    let year: i32 = date_parts[0].parse().ok()?;
    let month: u32 = date_parts[1].parse().ok()?;
    let day: u32 = date_parts[2].parse().ok()?;
    let hour: u32 = time_parts[0].parse().ok()?;
    let minute: u32 = time_parts[1].parse().ok()?;
    let second: u32 = time_parts[2].parse().ok()?;

    // Convert to Unix timestamp (simplified calculation)
    // This is a simple approximation, good enough for token expiration
    let days_since_epoch = days_since_unix_epoch(year, month, day)?;
    let seconds_in_day = hour * 3600 + minute * 60 + second;

    Some(days_since_epoch as u64 * 86400 + seconds_in_day as u64)
}

/// Calculate days since Unix epoch (1970-01-01).
fn days_since_unix_epoch(year: i32, month: u32, day: u32) -> Option<i64> {
    // Simplified calculation
    if year < 1970 || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    let mut days: i64 = 0;

    // Add days for complete years
    for y in 1970..year {
        days += if is_leap_year(y) { 366 } else { 365 };
    }

    // Add days for complete months in current year
    let days_in_month = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    for m in 1..month {
        let mut d = days_in_month[m as usize - 1];
        if m == 2 && is_leap_year(year) {
            d = 29;
        }
        days += d as i64;
    }

    // Add days in current month
    days += day as i64 - 1;

    Some(days)
}

/// Check if a year is a leap year.
fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::ErrorKind;

    #[test]
    fn test_iam_token_request_serialization() {
        let request = IamTokenRequest::new(
            "testuser",
            "testpass",
            "testdomain",
            HuaweiCloudRegion::CnNorth4,
        );

        let json = request.to_json().unwrap();
        assert!(json.contains("\"methods\":[\"password\"]"));
        assert!(json.contains("\"name\":\"testuser\""));
        assert!(json.contains("\"name\":\"testdomain\""));
        assert!(json.contains("\"name\":\"cn-north-4\""));
    }

    #[test]
    fn test_token_manager_creation() {
        let manager = HuaweiTokenManager::new();
        let rt = tokio::runtime::Runtime::new().unwrap();

        // Should start with no valid token
        let is_valid = rt.block_on(manager.is_token_valid());
        assert!(!is_valid);
    }

    #[tokio::test]
    async fn token_manager_without_http_client_returns_typed_error_without_panic() {
        let manager = HuaweiTokenManager {
            token: RwLock::new(None),
            expires_at: RwLock::new(None),
            client: None,
        };

        let err = manager
            .fetch_token(
                "testuser",
                "testpass",
                "testdomain",
                HuaweiCloudRegion::CnNorth4,
                None,
            )
            .await
            .expect_err("missing HTTP client should return a typed error");

        match err {
            STTError::ConfigurationError(msg) => assert!(
                msg.contains("Huawei IAM HTTP client unavailable"),
                "unexpected error: {msg}"
            ),
            other => panic!("expected ConfigurationError, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn huawei_iam_redirect_policy_rejects_private_hop() {
        let _env = crate::core::net::ssrf_env_lock();
        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(listener) => listener,
            Err(err) => {
                if err.kind() == ErrorKind::PermissionDenied {
                    eprintln!("Skipping huawei_iam_redirect_policy_rejects_private_hop: {err}");
                    return;
                }
                panic!("Failed to bind redirect test server listener: {err}");
            }
        };
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut buf = [0u8; 1024];
            let _ = socket.read(&mut buf).await;
            let response = concat!(
                "HTTP/1.1 302 Found\r\n",
                "Location: http://127.0.0.1:9/metadata\r\n",
                "Content-Length: 0\r\n",
                "\r\n"
            );
            let _ = socket.write_all(response.as_bytes()).await;
        });

        let err = huawei_iam_http_client()
            .unwrap()
            .get(format!("http://{addr}/start"))
            .send()
            .await
            .expect_err("private redirect target must be rejected");
        let mut error_chain = err.to_string();
        let mut source = std::error::Error::source(&err);
        while let Some(error) = source {
            error_chain.push_str(": ");
            error_chain.push_str(&error.to_string());
            source = error.source();
        }
        assert!(
            error_chain.contains("redirect URL rejected"),
            "unexpected redirect error: {error_chain}"
        );
    }

    #[tokio::test]
    async fn test_token_manager_no_cached_token() {
        let manager = HuaweiTokenManager::new();
        let token = manager.get_cached_token().await;
        assert!(token.is_none());
    }

    #[tokio::test]
    async fn test_token_manager_clear() {
        let manager = HuaweiTokenManager::new();

        // Manually set a token
        *manager.token.write().await = Some("test_token".to_string());
        *manager.expires_at.write().await = Some(HuaweiTokenManager::now_secs() + 7200);

        assert!(manager.is_token_valid().await);

        // Clear and verify
        manager.clear().await;
        assert!(!manager.is_token_valid().await);
    }

    #[test]
    fn test_parse_iso8601_timestamp() {
        // Test standard format
        let ts = parse_iso8601_timestamp("2024-01-15T12:00:00Z");
        assert!(ts.is_some());

        // Test with fractional seconds
        let ts = parse_iso8601_timestamp("2024-01-15T12:00:00.123456Z");
        assert!(ts.is_some());

        // Test with timezone offset
        let ts = parse_iso8601_timestamp("2024-01-15T12:00:00+00:00");
        assert!(ts.is_some());

        // Test invalid format
        let ts = parse_iso8601_timestamp("invalid");
        assert!(ts.is_none());
    }

    #[test]
    fn test_days_since_unix_epoch() {
        // Unix epoch
        assert_eq!(days_since_unix_epoch(1970, 1, 1), Some(0));

        // One day after epoch
        assert_eq!(days_since_unix_epoch(1970, 1, 2), Some(1));

        // Known date (2024-01-01 is about 19723 days since epoch)
        let days = days_since_unix_epoch(2024, 1, 1);
        assert!(days.is_some());
        // 2024-01-01 00:00:00 UTC = 1704067200 Unix timestamp
        // 1704067200 / 86400 = 19723
        assert_eq!(days.unwrap(), 19723);

        // Invalid dates
        assert!(days_since_unix_epoch(1969, 1, 1).is_none());
        assert!(days_since_unix_epoch(2024, 0, 1).is_none());
        assert!(days_since_unix_epoch(2024, 13, 1).is_none());
    }

    #[test]
    fn test_is_leap_year() {
        assert!(is_leap_year(2000)); // Divisible by 400
        assert!(is_leap_year(2024)); // Divisible by 4, not by 100
        assert!(!is_leap_year(1900)); // Divisible by 100, not by 400
        assert!(!is_leap_year(2023)); // Not divisible by 4
    }

    #[test]
    fn test_token_validity_with_buffer() {
        let manager = HuaweiTokenManager::new();
        let rt = tokio::runtime::Runtime::new().unwrap();

        rt.block_on(async {
            let now = HuaweiTokenManager::now_secs();

            // Token expiring in 30 minutes (less than buffer) should be invalid
            *manager.token.write().await = Some("test_token".to_string());
            *manager.expires_at.write().await = Some(now + 1800); // 30 min
            assert!(!manager.is_token_valid().await);

            // Token expiring in 2 hours (more than buffer) should be valid
            *manager.expires_at.write().await = Some(now + 7200); // 2 hours
            assert!(manager.is_token_valid().await);
        });
    }
}
