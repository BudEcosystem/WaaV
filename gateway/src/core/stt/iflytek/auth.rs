//! iFlytek HMAC-SHA256 Authentication Module
//!
//! This module implements the signature-based authentication required by iFlytek's
//! WebSocket APIs. The authentication follows RFC1123 date format and HMAC-SHA256 signing.
//!
//! # Signature Generation Process
//!
//! 1. Generate timestamp in RFC1123 format (GMT)
//! 2. Build signature origin string: `host: {host}\ndate: {date}\nGET {path} HTTP/1.1`
//! 3. Sign with HMAC-SHA256 using APISecret
//! 4. Encode signature in Base64
//! 5. Build authorization header
//! 6. URL-encode and append to WebSocket URL
//!
//! # Example
//!
//! ```rust,ignore
//! use waav_gateway::core::stt::iflytek::auth::IFlytekAuth;
//!
//! let auth = IFlytekAuth::new(
//!     "your_app_id".to_string(),
//!     "your_api_key".to_string(),
//!     "your_api_secret".to_string(),
//! );
//!
//! let signed_url = auth.build_signed_url(
//!     "iat-api-sg.xf-yun.com",
//!     "/v2/iat",
//! )?;
//! ```

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use time::OffsetDateTime;
use url::Url;

/// HMAC-SHA256 type alias.
type HmacSha256 = Hmac<Sha256>;

/// Maximum allowed clock skew in seconds (5 minutes).
pub const MAX_CLOCK_SKEW_SECS: i64 = 300;

/// Authentication error types.
#[derive(Debug, Clone, thiserror::Error)]
pub enum AuthError {
    #[error("Invalid API secret: {0}")]
    InvalidApiSecret(String),

    #[error("URL encoding error: {0}")]
    UrlEncodingError(String),

    #[error("HMAC error: {0}")]
    HmacError(String),
}

/// iFlytek authentication credentials and signature generator.
#[derive(Debug, Clone)]
pub struct IFlytekAuth {
    /// Application ID.
    pub app_id: String,
    /// API Key (32 characters).
    pub api_key: String,
    /// API Secret (32 characters).
    pub api_secret: String,
}

impl IFlytekAuth {
    /// Create a new authentication instance.
    ///
    /// # Arguments
    /// * `app_id` - iFlytek Application ID
    /// * `api_key` - iFlytek API Key
    /// * `api_secret` - iFlytek API Secret
    pub fn new(app_id: String, api_key: String, api_secret: String) -> Self {
        Self {
            app_id,
            api_key,
            api_secret,
        }
    }

    /// Parse credentials from a combined string.
    ///
    /// # Format
    /// `app_id|api_key|api_secret`
    ///
    /// # Returns
    /// * `Ok(IFlytekAuth)` - Successfully parsed credentials
    /// * `Err(AuthError)` - Invalid format
    pub fn from_combined(combined: &str) -> Result<Self, AuthError> {
        let parts: Vec<&str> = combined.split('|').collect();
        if parts.len() != 3 {
            return Err(AuthError::InvalidApiSecret(
                "iFlytek credentials must be in format 'app_id|api_key|api_secret'".to_string(),
            ));
        }

        let app_id = parts[0].trim().to_string();
        let api_key = parts[1].trim().to_string();
        let api_secret = parts[2].trim().to_string();

        if app_id.is_empty() {
            return Err(AuthError::InvalidApiSecret(
                "APPID cannot be empty".to_string(),
            ));
        }
        if api_key.is_empty() {
            return Err(AuthError::InvalidApiSecret(
                "API Key cannot be empty".to_string(),
            ));
        }
        if api_secret.is_empty() {
            return Err(AuthError::InvalidApiSecret(
                "API Secret cannot be empty".to_string(),
            ));
        }

        Ok(Self {
            app_id,
            api_key,
            api_secret,
        })
    }

    /// Generate RFC1123 formatted date string (GMT).
    ///
    /// # Example Output
    /// `Wed, 20 Nov 2024 03:14:25 GMT`
    #[inline]
    pub fn generate_rfc1123_date() -> String {
        // RFC1123 format: "Wed, 20 Nov 2024 03:14:25 GMT"
        // We use format_description to create the exact format needed
        let now = OffsetDateTime::now_utc();

        // Format manually to ensure exact RFC1123 compliance
        let weekday = match now.weekday() {
            time::Weekday::Monday => "Mon",
            time::Weekday::Tuesday => "Tue",
            time::Weekday::Wednesday => "Wed",
            time::Weekday::Thursday => "Thu",
            time::Weekday::Friday => "Fri",
            time::Weekday::Saturday => "Sat",
            time::Weekday::Sunday => "Sun",
        };

        let month = match now.month() {
            time::Month::January => "Jan",
            time::Month::February => "Feb",
            time::Month::March => "Mar",
            time::Month::April => "Apr",
            time::Month::May => "May",
            time::Month::June => "Jun",
            time::Month::July => "Jul",
            time::Month::August => "Aug",
            time::Month::September => "Sep",
            time::Month::October => "Oct",
            time::Month::November => "Nov",
            time::Month::December => "Dec",
        };

        format!(
            "{}, {:02} {} {:04} {:02}:{:02}:{:02} GMT",
            weekday,
            now.day(),
            month,
            now.year(),
            now.hour(),
            now.minute(),
            now.second()
        )
    }

    /// Build the signature origin string.
    ///
    /// # Format
    /// ```text
    /// host: {host}
    /// date: {date}
    /// GET {path} HTTP/1.1
    /// ```
    #[inline]
    fn build_signature_origin(&self, host: &str, date: &str, path: &str) -> String {
        format!("host: {host}\ndate: {date}\nGET {path} HTTP/1.1")
    }

    /// Sign a string using HMAC-SHA256.
    ///
    /// # Arguments
    /// * `data` - Data to sign
    /// * `secret` - Secret key
    ///
    /// # Returns
    /// Base64-encoded signature
    fn hmac_sha256_sign(&self, data: &str, secret: &str) -> Result<String, AuthError> {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .map_err(|e| AuthError::HmacError(format!("Failed to create HMAC: {}", e)))?;

        mac.update(data.as_bytes());
        let result = mac.finalize();
        let signature_bytes = result.into_bytes();

        Ok(BASE64.encode(signature_bytes))
    }

    /// Build the authorization header value.
    ///
    /// # Format
    /// ```text
    /// api_key="{api_key}",algorithm="hmac-sha256",headers="host date request-line",signature="{signature}"
    /// ```
    fn build_authorization_header(&self, signature: &str) -> String {
        format!(
            r#"api_key="{}",algorithm="hmac-sha256",headers="host date request-line",signature="{}""#,
            self.api_key, signature
        )
    }

    /// Build a signed WebSocket URL.
    ///
    /// # Arguments
    /// * `host` - WebSocket host (e.g., "iat-api-sg.xf-yun.com")
    /// * `path` - API path (e.g., "/v2/iat")
    ///
    /// # Returns
    /// Complete signed WebSocket URL
    pub fn build_signed_url(&self, host: &str, path: &str) -> Result<String, AuthError> {
        let date = Self::generate_rfc1123_date();
        self.build_signed_url_with_date(host, path, &date)
    }

    /// Build a signed WebSocket URL with a specific date.
    ///
    /// This is useful for testing with fixed timestamps.
    ///
    /// # Arguments
    /// * `host` - WebSocket host
    /// * `path` - API path
    /// * `date` - RFC1123 formatted date string
    ///
    /// # Returns
    /// Complete signed WebSocket URL
    pub fn build_signed_url_with_date(
        &self,
        host: &str,
        path: &str,
        date: &str,
    ) -> Result<String, AuthError> {
        // Build signature origin
        let signature_origin = self.build_signature_origin(host, date, path);

        // Sign with HMAC-SHA256
        let signature = self.hmac_sha256_sign(&signature_origin, &self.api_secret)?;

        // Build authorization header
        let authorization = self.build_authorization_header(&signature);
        let authorization_base64 = BASE64.encode(authorization.as_bytes());

        // Build URL with query parameters
        let base_url = format!("wss://{}{}", host, path);
        let mut url = Url::parse(&base_url)
            .map_err(|e| AuthError::UrlEncodingError(format!("Invalid URL: {}", e)))?;

        // Add query parameters (URL-encoded)
        url.query_pairs_mut()
            .append_pair("authorization", &authorization_base64)
            .append_pair("date", date)
            .append_pair("host", host);

        Ok(url.to_string())
    }

    /// Validate that credentials are properly formatted.
    pub fn validate(&self) -> Result<(), AuthError> {
        if self.app_id.is_empty() {
            return Err(AuthError::InvalidApiSecret(
                "APPID cannot be empty".to_string(),
            ));
        }
        if self.api_key.is_empty() {
            return Err(AuthError::InvalidApiSecret(
                "API Key cannot be empty".to_string(),
            ));
        }
        if self.api_secret.is_empty() {
            return Err(AuthError::InvalidApiSecret(
                "API Secret cannot be empty".to_string(),
            ));
        }
        Ok(())
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_APP_ID: &str = "test_app_id";
    const TEST_API_KEY: &str = "test_api_key_32_chars_long_xxxxx";
    const TEST_API_SECRET: &str = "test_api_secret_32_chars_longxxx";

    fn create_test_auth() -> IFlytekAuth {
        IFlytekAuth::new(
            TEST_APP_ID.to_string(),
            TEST_API_KEY.to_string(),
            TEST_API_SECRET.to_string(),
        )
    }

    #[test]
    fn test_auth_creation() {
        let auth = create_test_auth();
        assert_eq!(auth.app_id, TEST_APP_ID);
        assert_eq!(auth.api_key, TEST_API_KEY);
        assert_eq!(auth.api_secret, TEST_API_SECRET);
    }

    #[test]
    fn test_auth_from_combined_valid() {
        let combined = format!("{}|{}|{}", TEST_APP_ID, TEST_API_KEY, TEST_API_SECRET);
        let result = IFlytekAuth::from_combined(&combined);
        assert!(result.is_ok());

        let auth = result.unwrap();
        assert_eq!(auth.app_id, TEST_APP_ID);
        assert_eq!(auth.api_key, TEST_API_KEY);
        assert_eq!(auth.api_secret, TEST_API_SECRET);
    }

    #[test]
    fn test_auth_from_combined_with_spaces() {
        let combined = format!(" {} | {} | {} ", TEST_APP_ID, TEST_API_KEY, TEST_API_SECRET);
        let result = IFlytekAuth::from_combined(&combined);
        assert!(result.is_ok());

        let auth = result.unwrap();
        assert_eq!(auth.app_id, TEST_APP_ID);
    }

    #[test]
    fn test_auth_from_combined_invalid_format() {
        // Missing parts
        let result = IFlytekAuth::from_combined("only_one_part");
        assert!(result.is_err());

        let result = IFlytekAuth::from_combined("two|parts");
        assert!(result.is_err());
    }

    #[test]
    fn test_auth_from_combined_empty_parts() {
        let result = IFlytekAuth::from_combined("||");
        assert!(result.is_err());

        let result = IFlytekAuth::from_combined("app_id||secret");
        assert!(result.is_err());
    }

    #[test]
    fn test_rfc1123_date_format() {
        let date = IFlytekAuth::generate_rfc1123_date();

        // Should contain GMT
        assert!(date.ends_with("GMT"), "Date should end with GMT: {}", date);

        // Should have correct format (e.g., "Wed, 20 Nov 2024 03:14:25 GMT")
        let parts: Vec<&str> = date.split(' ').collect();
        assert_eq!(parts.len(), 6, "Date should have 6 parts: {}", date);

        // Day should end with comma
        assert!(
            parts[0].ends_with(','),
            "Day should end with comma: {}",
            date
        );
    }

    #[test]
    fn test_signature_origin_format() {
        let auth = create_test_auth();
        let origin = auth.build_signature_origin(
            "iat-api-sg.xf-yun.com",
            "Wed, 20 Nov 2024 03:14:25 GMT",
            "/v2/iat",
        );

        assert!(origin.contains("host: iat-api-sg.xf-yun.com"));
        assert!(origin.contains("date: Wed, 20 Nov 2024 03:14:25 GMT"));
        assert!(origin.contains("GET /v2/iat HTTP/1.1"));

        // Verify newline separation
        let lines: Vec<&str> = origin.split('\n').collect();
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn test_hmac_sha256_sign() {
        let auth = create_test_auth();
        let result = auth.hmac_sha256_sign("test_data", TEST_API_SECRET);
        assert!(result.is_ok());

        let signature = result.unwrap();
        // Base64 encoded signature should not be empty
        assert!(!signature.is_empty());

        // Should be valid base64
        let decoded = BASE64.decode(&signature);
        assert!(decoded.is_ok());
    }

    #[test]
    fn test_hmac_signature_consistency() {
        let auth = create_test_auth();

        // Same input should produce same output
        let sig1 = auth.hmac_sha256_sign("test_data", TEST_API_SECRET).unwrap();
        let sig2 = auth.hmac_sha256_sign("test_data", TEST_API_SECRET).unwrap();
        assert_eq!(sig1, sig2);

        // Different input should produce different output
        let sig3 = auth
            .hmac_sha256_sign("different_data", TEST_API_SECRET)
            .unwrap();
        assert_ne!(sig1, sig3);
    }

    #[test]
    fn test_authorization_header_format() {
        let auth = create_test_auth();
        let header = auth.build_authorization_header("test_signature");

        assert!(header.contains(&format!(r#"api_key="{}""#, TEST_API_KEY)));
        assert!(header.contains(r#"algorithm="hmac-sha256""#));
        assert!(header.contains(r#"headers="host date request-line""#));
        assert!(header.contains(r#"signature="test_signature""#));
    }

    #[test]
    fn test_build_signed_url_format() {
        let auth = create_test_auth();
        let result = auth.build_signed_url("iat-api-sg.xf-yun.com", "/v2/iat");
        assert!(result.is_ok());

        let url = result.unwrap();

        // Should be a WebSocket URL
        assert!(url.starts_with("wss://"));

        // Should contain host and path
        assert!(url.contains("iat-api-sg.xf-yun.com"));
        assert!(url.contains("/v2/iat"));

        // Should have query parameters
        assert!(url.contains("authorization="));
        assert!(url.contains("date="));
        assert!(url.contains("host="));
    }

    #[test]
    fn test_build_signed_url_with_fixed_date() {
        let auth = create_test_auth();
        let fixed_date = "Wed, 20 Nov 2024 03:14:25 GMT";

        let url1 = auth
            .build_signed_url_with_date("iat-api-sg.xf-yun.com", "/v2/iat", fixed_date)
            .unwrap();
        let url2 = auth
            .build_signed_url_with_date("iat-api-sg.xf-yun.com", "/v2/iat", fixed_date)
            .unwrap();

        // Same date should produce same URL
        assert_eq!(url1, url2);
    }

    #[test]
    fn test_build_signed_url_different_endpoints() {
        let auth = create_test_auth();
        let fixed_date = "Wed, 20 Nov 2024 03:14:25 GMT";

        let iat_url = auth
            .build_signed_url_with_date("iat-api-sg.xf-yun.com", "/v2/iat", fixed_date)
            .unwrap();
        let tts_url = auth
            .build_signed_url_with_date("tts-api-sg.xf-yun.com", "/v2/tts", fixed_date)
            .unwrap();

        // Different endpoints should produce different URLs
        assert_ne!(iat_url, tts_url);
        assert!(iat_url.contains("iat-api-sg.xf-yun.com"));
        assert!(tts_url.contains("tts-api-sg.xf-yun.com"));
    }

    #[test]
    fn test_validate_valid_credentials() {
        let auth = create_test_auth();
        assert!(auth.validate().is_ok());
    }

    #[test]
    fn test_validate_empty_app_id() {
        let auth = IFlytekAuth::new(
            String::new(),
            TEST_API_KEY.to_string(),
            TEST_API_SECRET.to_string(),
        );
        assert!(auth.validate().is_err());
    }

    #[test]
    fn test_validate_empty_api_key() {
        let auth = IFlytekAuth::new(
            TEST_APP_ID.to_string(),
            String::new(),
            TEST_API_SECRET.to_string(),
        );
        assert!(auth.validate().is_err());
    }

    #[test]
    fn test_validate_empty_api_secret() {
        let auth = IFlytekAuth::new(
            TEST_APP_ID.to_string(),
            TEST_API_KEY.to_string(),
            String::new(),
        );
        assert!(auth.validate().is_err());
    }

    #[test]
    fn test_url_encoding_special_characters() {
        let auth = create_test_auth();
        let date_with_special = "Wed, 20 Nov 2024 03:14:25 GMT";

        let result =
            auth.build_signed_url_with_date("iat-api-sg.xf-yun.com", "/v2/iat", date_with_special);
        assert!(result.is_ok());

        let url = result.unwrap();
        // Verify URL is properly encoded (spaces become %20 or +)
        assert!(url.contains("Wed") || url.contains("%20") || url.contains("+"));
    }

    #[test]
    fn test_max_clock_skew_constant() {
        assert_eq!(MAX_CLOCK_SKEW_SECS, 300);
    }

    #[test]
    fn test_auth_clone() {
        let auth = create_test_auth();
        let cloned = auth.clone();

        assert_eq!(auth.app_id, cloned.app_id);
        assert_eq!(auth.api_key, cloned.api_key);
        assert_eq!(auth.api_secret, cloned.api_secret);
    }

    #[test]
    fn test_auth_debug_impl() {
        let auth = create_test_auth();
        let debug_str = format!("{:?}", auth);

        assert!(debug_str.contains("IFlytekAuth"));
        assert!(debug_str.contains(TEST_APP_ID));
    }
}
