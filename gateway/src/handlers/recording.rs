use axum::{
    Extension, Json,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use object_store::{Error as ObjectStoreError, ObjectStore, path::Path as ObjectPath};
use serde_json::json;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use crate::auth::Auth;
use crate::state::AppState;

const CONTENT_TYPE: &str = "audio/ogg";
const MAX_STREAM_ID_SIZE: usize = 256;

fn is_valid_stream_id(stream_id: &str) -> bool {
    !stream_id.is_empty()
        && stream_id.len() <= MAX_STREAM_ID_SIZE
        && stream_id != "."
        && !stream_id.contains("..")
        && stream_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn recording_content_disposition(
    stream_id: &str,
) -> Result<HeaderValue, axum::http::header::InvalidHeaderValue> {
    HeaderValue::from_str(&format!("attachment; filename=\"{}.ogg\"", stream_id))
}

/// Build recording object key with optional auth_id for tenant isolation.
///
/// Path format:
/// - With auth_id: `{prefix}/{auth_id}/{stream_id}/audio.ogg` (tenant-scoped)
/// - Without auth_id: `{prefix}/{stream_id}/audio.ogg` (legacy format)
///
/// When auth is enabled, recordings are stored with auth_id prefix for isolation.
fn build_recording_object_key(
    prefix: Option<&String>,
    auth_id: Option<&str>,
    stream_id: &str,
) -> String {
    let normalized_prefix = prefix
        .map(|p| p.trim().trim_end_matches('/'))
        .filter(|p| !p.is_empty());

    match (normalized_prefix, auth_id) {
        // No prefix, no auth_id: stream_id/audio.ogg
        (None, None) => format!("{}/audio.ogg", stream_id),
        // No prefix, with auth_id: auth_id/stream_id/audio.ogg
        (None, Some(auth)) => format!("{}/{}/audio.ogg", auth, stream_id),
        // With prefix, no auth_id: prefix/stream_id/audio.ogg
        (Some(prefix), None) => format!("{}/{}/audio.ogg", prefix, stream_id),
        // With prefix, with auth_id: prefix/auth_id/stream_id/audio.ogg
        (Some(prefix), Some(auth)) => format!("{}/{}/{}/audio.ogg", prefix, auth, stream_id),
    }
}

/// Download recording by stream ID from configured object storage
///
/// When authentication is enabled, recordings are scoped to the authenticated
/// client's ID. Users can only download recordings that belong to their tenant.
/// The recording path includes auth_id prefix: `{prefix}/{auth_id}/{stream_id}/audio.ogg`
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/recording/{stream_id}",
        params(
            ("stream_id" = String, Path, description = "Recording stream identifier", example = "550e8400-e29b-41d4-a716-446655440000")
        ),
        responses(
            (status = 200, description = "Recording retrieved successfully", content_type = "audio/ogg",
                headers(
                    ("Content-Disposition" = String, description = "Suggested filename for download"),
                    ("Content-Length" = u64, description = "Size of the recording in bytes")
                )
            ),
            (status = 400, description = "Invalid stream_id format"),
            (status = 404, description = "Recording not found"),
            (status = 503, description = "Recording storage not configured or unavailable")
        ),
        security(
            ("bearer_auth" = [])
        ),
        tag = "recordings"
    )
)]
pub async fn download_recording(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<Auth>,
    Path(stream_id): Path<String>,
) -> Response {
    // Extract auth_id for tenant-scoped recording access
    let auth_id = auth.id.as_deref();

    info!(stream_id = ?stream_id, auth_id = ?auth_id, "Recording download requested");

    // Log warning if downloading without authentication (potentially insecure)
    if auth_id.is_none() && state.config.auth_required {
        warn!(
            stream_id = ?stream_id,
            "Recording download without auth_id. Enable authentication for tenant isolation."
        );
    }

    if !is_valid_stream_id(&stream_id) {
        error!(
            stream_id = ?stream_id,
            "Invalid stream_id format for recording download"
        );
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Invalid stream_id format"})),
        )
            .into_response();
    }

    let store = match &state.object_store {
        Some(store) => store.clone(),
        None => {
            error!("Recording download attempted but storage is not configured");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "Recording storage not configured"})),
            )
                .into_response();
        }
    };

    let bucket = match &state.recording_bucket {
        Some(bucket) => bucket,
        None => {
            error!("Recording bucket not configured");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "Recording storage not configured"})),
            )
                .into_response();
        }
    };

    // Build object key with auth_id for tenant-scoped access
    let object_key = build_recording_object_key(
        state.config.recording_s3_prefix.as_ref(),
        auth_id,
        &stream_id,
    );

    let object_path = match ObjectPath::parse(object_key.clone()) {
        Ok(path) => path,
        Err(e) => {
            error!(stream_id = ?stream_id, error = %e, "Invalid recording path");
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "Invalid recording path"})),
            )
                .into_response();
        }
    };

    debug!(
        bucket = %bucket,
        key = %object_key,
        "Fetching recording from object storage"
    );

    let get_result = match store.get(&object_path).await {
        Ok(result) => result,
        Err(ObjectStoreError::NotFound { path, .. }) => {
            info!(
                stream_id = ?stream_id,
                key = %object_key,
                path = %path,
                "Recording not found"
            );
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("Recording not found: {}", stream_id)})),
            )
                .into_response();
        }
        Err(e) => {
            error!(
                stream_id = ?stream_id,
                error = ?e,
                "Failed to retrieve recording from storage"
            );
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "Failed to retrieve recording from storage"})),
            )
                .into_response();
        }
    };

    let size = get_result.meta.size;

    let body = match get_result.bytes().await {
        Ok(bytes) => bytes,
        Err(e) => {
            error!(
                stream_id = ?stream_id,
                error = ?e,
                "Failed to read recording from storage"
            );
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "Failed to read recording from storage"})),
            )
                .into_response();
        }
    };

    info!(stream_id = ?stream_id, size, "Recording download successful");

    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(CONTENT_TYPE));
    if let Ok(len) = HeaderValue::from_str(&size.to_string()) {
        headers.insert(header::CONTENT_LENGTH, len);
    }
    let disposition = match recording_content_disposition(&stream_id) {
        Ok(disposition) => disposition,
        Err(e) => {
            error!(
                stream_id = ?stream_id,
                error = %e,
                "Failed to build recording Content-Disposition header"
            );
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Failed to build recording response"})),
            )
                .into_response();
        }
    };
    headers.insert(header::CONTENT_DISPOSITION, disposition);

    (StatusCode::OK, headers, body).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_key_with_prefix_no_auth() {
        let prefix = "recordings".to_string();
        let key = build_recording_object_key(Some(&prefix), None, "abc123");
        assert_eq!(key, "recordings/abc123/audio.ogg");
    }

    #[test]
    fn test_build_key_with_prefix_and_auth() {
        let prefix = "recordings".to_string();
        let key = build_recording_object_key(Some(&prefix), Some("project1"), "abc123");
        assert_eq!(key, "recordings/project1/abc123/audio.ogg");
    }

    #[test]
    fn test_build_key_without_prefix_no_auth() {
        let key = build_recording_object_key(None, None, "abc123");
        assert_eq!(key, "abc123/audio.ogg");
    }

    #[test]
    fn test_build_key_without_prefix_with_auth() {
        let key = build_recording_object_key(None, Some("tenant1"), "abc123");
        assert_eq!(key, "tenant1/abc123/audio.ogg");
    }

    #[test]
    fn test_build_key_with_trailing_slash() {
        let prefix = "recordings/".to_string();
        let key = build_recording_object_key(Some(&prefix), None, "abc123");
        assert_eq!(key, "recordings/abc123/audio.ogg");
    }

    #[test]
    fn test_build_key_with_trailing_slash_and_auth() {
        let prefix = "recordings/".to_string();
        let key = build_recording_object_key(Some(&prefix), Some("client1"), "abc123");
        assert_eq!(key, "recordings/client1/abc123/audio.ogg");
    }

    #[test]
    fn test_invalid_stream_id_empty() {
        assert!(!is_valid_stream_id(""));
    }

    #[test]
    fn test_invalid_stream_id_too_large() {
        assert!(!is_valid_stream_id(&"a".repeat(MAX_STREAM_ID_SIZE + 1)));
    }

    #[test]
    fn test_invalid_stream_id_path_traversal() {
        assert!(!is_valid_stream_id("../etc/passwd"));
        assert!(!is_valid_stream_id(".."));
        assert!(!is_valid_stream_id("."));
    }

    #[test]
    fn test_invalid_stream_id_contains_slash() {
        assert!(!is_valid_stream_id("abc/123"));
    }

    #[test]
    fn test_invalid_stream_id_header_unsafe_characters() {
        for stream_id in [
            "abc\n123",
            "abc\r123",
            "abc\"123",
            "abc\\123",
            "abc 123",
            "abc;filename=evil",
            "recording,backup",
            "\u{fc}mlaut",
        ] {
            assert!(!is_valid_stream_id(stream_id), "{stream_id:?}");
        }
    }

    #[test]
    fn test_valid_stream_id_uuid() {
        assert!(is_valid_stream_id("550e8400-e29b-41d4-a716-446655440000"));
    }

    #[test]
    fn test_valid_stream_id_custom() {
        assert!(is_valid_stream_id("call-123"));
        assert!(is_valid_stream_id("call_123.segment"));
    }

    #[test]
    fn test_recording_content_disposition_for_valid_stream_id() {
        let disposition = recording_content_disposition("call_123.segment")
            .expect("valid stream id should produce a header value");

        assert_eq!(
            disposition.to_str().unwrap(),
            "attachment; filename=\"call_123.segment.ogg\""
        );
    }

    #[test]
    fn test_recording_content_disposition_rejects_control_characters() {
        assert!(recording_content_disposition("bad\nid").is_err());
    }
}
