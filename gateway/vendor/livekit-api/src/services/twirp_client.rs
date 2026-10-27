// Copyright 2025 LiveKit, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::fmt::Display;

use http::{
    header::{HeaderMap, HeaderValue, CONTENT_TYPE},
    StatusCode,
};
use serde::Deserialize;
use thiserror::Error;

use crate::http_client;

pub const DEFAULT_PREFIX: &str = "/twirp";

#[derive(Debug, Error)]
pub enum TwirpError {
    #[cfg(feature = "services-tokio")]
    #[error("failed to execute the request: {0}")]
    Request(#[from] reqwest::Error),
    #[cfg(feature = "services-async")]
    #[error("failed to execute the request: {0}")]
    Request(#[from] std::io::Error),
    #[error("twirp error: {0}")]
    Twirp(TwirpErrorCode),
    #[error("url error: {0}")]
    Url(#[from] url::ParseError),
    #[error("prost error: {0}")]
    Prost(#[from] prost::DecodeError),
}

#[derive(Debug, Deserialize)]
pub struct TwirpErrorCode {
    pub code: String,
    pub msg: String,
}

impl TwirpErrorCode {
    pub const CANCELED: &'static str = "canceled";
    pub const UNKNOWN: &'static str = "unknown";
    pub const INVALID_ARGUMENT: &'static str = "invalid_argument";
    pub const MALFORMED: &'static str = "malformed";
    pub const DEADLINE_EXCEEDED: &'static str = "deadline_exceeded";
    pub const NOT_FOUND: &'static str = "not_found";
    pub const BAD_ROUTE: &'static str = "bad_route";
    pub const ALREADY_EXISTS: &'static str = "already_exists";
    pub const PERMISSION_DENIED: &'static str = "permission_denied";
    pub const UNAUTHENTICATED: &'static str = "unauthenticated";
    pub const RESOURCE_EXHAUSTED: &'static str = "resource_exhausted";
    pub const FAILED_PRECONDITION: &'static str = "failed_precondition";
    pub const ABORTED: &'static str = "aborted";
    pub const OUT_OF_RANGE: &'static str = "out_of_range";
    pub const UNIMPLEMENTED: &'static str = "unimplemented";
    pub const INTERNAL: &'static str = "internal";
    pub const UNAVAILABLE: &'static str = "unavailable";
    pub const DATA_LOSS: &'static str = "dataloss";
}

impl TwirpErrorCode {
    /// Decode a non-200 Twirp response; fall back to the HTTP status when the body is not a
    /// Twirp JSON error (router 404, proxy error page, empty body).
    pub(crate) fn from_response(status: StatusCode, body: &[u8]) -> Self {
        if let Ok(err) = serde_json::from_slice::<TwirpErrorCode>(body) {
            return err;
        }
        let code = match status {
            StatusCode::NOT_FOUND => Self::BAD_ROUTE,
            StatusCode::UNAUTHORIZED => Self::UNAUTHENTICATED,
            StatusCode::FORBIDDEN => Self::PERMISSION_DENIED,
            StatusCode::BAD_REQUEST => Self::MALFORMED,
            StatusCode::TOO_MANY_REQUESTS => Self::RESOURCE_EXHAUSTED,
            StatusCode::BAD_GATEWAY
            | StatusCode::SERVICE_UNAVAILABLE
            | StatusCode::GATEWAY_TIMEOUT => Self::UNAVAILABLE,
            _ => Self::UNKNOWN,
        };
        let text = String::from_utf8_lossy(&body[..body.len().min(160)]);
        Self { code: code.to_owned(), msg: format!("HTTP {}: {}", status, text.trim()) }
    }
}

impl Display for TwirpErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.msg)
    }
}

pub type TwirpResult<T> = Result<T, TwirpError>;

#[derive(Debug)]
pub struct TwirpClient {
    host: String,
    pkg: String,
    prefix: String,
    client: http_client::Client,
}

impl TwirpClient {
    pub fn new(host: &str, pkg: &str, prefix: Option<&str>) -> Self {
        Self {
            host: host.to_owned(),
            pkg: pkg.to_owned(),
            prefix: prefix.unwrap_or(DEFAULT_PREFIX).to_owned(),
            client: http_client::Client::new(),
        }
    }

    pub async fn request<D: prost::Message, R: prost::Message + Default>(
        &self,
        service: &str,
        method: &str,
        data: D,
        mut headers: HeaderMap,
    ) -> TwirpResult<R> {
        let url = self.method_url(service, method)?;

        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/protobuf"));

        let resp = self.client.post(url).headers(headers).body(data.encode_to_vec()).send().await?;

        let status = resp.status();
        if status == StatusCode::OK {
            Ok(R::decode(resp.bytes().await?)?)
        } else {
            // A Twirp server answers errors with a JSON `{code,msg}` body. Anything else (a
            // router's "404 page not found", a proxy's 502 page) used to surface as the opaque
            // "error decoding response body"; keep the HTTP status and body instead.
            let body = resp.bytes().await?;
            Err(TwirpError::Twirp(TwirpErrorCode::from_response(status, &body)))
        }
    }

    /// `<host><host path>/<prefix>/<pkg>.<service>/<method>`, as one literal path.
    ///
    /// The previous code pushed `"/twirp/livekit.RoomService/CreateRoom"` as a SINGLE path
    /// segment through `path_segments_mut().push()`, which (by design, in every url 2.x)
    /// percent-encodes `/` to `%2F`: the request went to `/%2Ftwirp%2Flivekit.RoomService%2F...`.
    /// LiveKit servers before 1.13 happened to route that; 1.13 answers 404.
    ///
    /// LiveKit URLs are also commonly given with the WebSocket scheme (the signal URL); the
    /// Twirp API is plain HTTP on the same port, so `ws`/`wss` map to `http`/`https`.
    fn method_url(&self, service: &str, method: &str) -> TwirpResult<url::Url> {
        let mut url = url::Url::parse(&self.host)?;
        match url.scheme() {
            "ws" => {
                let _ = url.set_scheme("http");
            }
            "wss" => {
                let _ = url.set_scheme("https");
            }
            _ => {}
        }
        let base = url.path().trim_end_matches('/').to_owned();
        let prefix = self.prefix.trim_matches('/');
        url.set_path(&format!("{}/{}/{}.{}/{}", base, prefix, self.pkg, service, method));
        url.set_query(None);
        url.set_fragment(None);
        Ok(url)
    }
}

#[cfg(test)]
mod url_tests {
    use super::*;

    fn url_for(host: &str) -> String {
        TwirpClient::new(host, "livekit", None)
            .method_url("RoomService", "CreateRoom")
            .unwrap()
            .to_string()
    }

    #[test]
    fn method_path_is_not_percent_encoded() {
        assert_eq!(url_for("http://h:7880"), "http://h:7880/twirp/livekit.RoomService/CreateRoom");
        assert_eq!(url_for("http://h:7880/"), "http://h:7880/twirp/livekit.RoomService/CreateRoom");
        assert!(!url_for("https://h").contains("%2F"));
    }

    #[test]
    fn host_path_prefix_is_kept() {
        assert_eq!(url_for("https://h/lk/"), "https://h/lk/twirp/livekit.RoomService/CreateRoom");
    }

    #[test]
    fn websocket_scheme_maps_to_http() {
        assert_eq!(url_for("ws://h:7880"), "http://h:7880/twirp/livekit.RoomService/CreateRoom");
        assert_eq!(url_for("wss://h"), "https://h/twirp/livekit.RoomService/CreateRoom");
    }

    #[test]
    fn non_twirp_error_body_keeps_status() {
        let e = TwirpErrorCode::from_response(StatusCode::NOT_FOUND, b"404 page not found\n");
        assert_eq!(e.code, TwirpErrorCode::BAD_ROUTE);
        assert!(e.msg.contains("404") && e.msg.contains("page not found"), "{}", e.msg);
        let e = TwirpErrorCode::from_response(
            StatusCode::UNAUTHORIZED,
            br#"{"code":"unauthenticated","msg":"permissions denied"}"#,
        );
        assert_eq!((e.code.as_str(), e.msg.as_str()), ("unauthenticated", "permissions denied"));
    }
}
