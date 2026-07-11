//! Batched / async STT REST handlers (P5).
//!
//! `POST /transcribe/batch`        — submit a prerecorded transcription job.
//! `GET  /transcribe/batch/{id}`   — poll a job by id.
//!
//! The handler resolves the provider credential (client-supplied `api_key` wins, else server
//! config), builds the provider-native prerecorded request via the pure builders in
//! [`crate::core::stt::batch`] (which ENABLE the streaming-gap features), and executes it with
//! `reqwest`. For an async provider (Deepgram-with-callback / AssemblyAI) it returns
//! `{job_id, status:"queued"}` immediately and lets the provider POST to `callback_url` (or the
//! caller polls AssemblyAI). For a synchronous provider (OpenAI / Deepgram-without-callback) it runs
//! inline, stores the canonical result keyed by `job_id`, and returns the completed job.
//!
//! Degrade rule: an unsupported provider/feature returns a `config_warning` and proceeds — NEVER a
//! 400 for a recognized-but-unsupported knob. Genuine client errors (no recognized provider, bad
//! base64, missing audio source for OpenAI) are 400s; a missing/placeholder credential is a 400
//! with a clear "API key not configured" message (the credential-free e2e probe asserts this).

use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json, Response},
};
use serde_json::json;
use tracing::{info, warn};
use uuid::Uuid;

use crate::config::ServerConfig;
#[cfg(test)]
use crate::core::stt::batch::decode_inline_batch_audio_with_limit;
use crate::core::stt::batch::{
    BatchHttpBody, BatchJob, BatchStatus, BatchSubmission, BatchTranscribeRequest,
    batch_provider_supported, build_assemblyai_transcript, build_deepgram_prerecorded,
    build_openai_transcription, decode_inline_batch_audio, validate_batch_base_url,
};
use crate::state::AppState;

/// Default per-request timeout for the provider HTTP call (synchronous providers can take a while
/// on long audio; async submits return fast).
const BATCH_HTTP_TIMEOUT_SECS: u64 = 300;

/// `POST /transcribe/batch`
pub async fn submit_batch(
    State(state): State<Arc<AppState>>,
    Json(req): Json<BatchTranscribeRequest>,
) -> Response {
    let provider = req.config.base.provider.trim().to_lowercase();
    if provider.is_empty() {
        return err(StatusCode::BAD_REQUEST, "missing `provider`");
    }
    if !batch_provider_supported(&provider) {
        return err(
            StatusCode::BAD_REQUEST,
            &format!(
                "batch transcription is not supported for provider '{provider}' \
                 (supported: deepgram, assemblyai, openai)"
            ),
        );
    }

    // Resolve the credential: client-supplied wins, else server config. A missing/placeholder key
    // is a clean 400 ("API key not configured") — the credential-free probe asserts reaching here.
    let api_key = match resolve_key(&state, &req) {
        Ok(k) => k,
        Err(e) => return err(StatusCode::BAD_REQUEST, &e),
    };

    let job_id = Uuid::new_v4().to_string();

    // Build the provider-native submission. AssemblyAI bytes need a prior upload (done below).
    let base_url = match endpoint_override(&req, &provider) {
        Ok(url) => url,
        Err(e) => return err(StatusCode::BAD_REQUEST, &e),
    };
    let submission =
        match build_submission(&provider, &req, &api_key, base_url.as_deref(), &state).await {
            Ok(s) => s,
            Err(e) => return err(StatusCode::BAD_REQUEST, &e),
        };

    let warnings = submission.config_warnings.clone();
    if submission.is_async {
        // Fire the submit; the provider will callback (Deepgram) or be polled (AssemblyAI). We
        // register a Queued job immediately and return it. We do NOT block on completion.
        match execute(&submission).await {
            Ok((status, body)) if status.is_success() => {
                // Provider job-id (Deepgram `request_id` / AssemblyAI `id`) is recorded inside the
                // stored job's result so a later poll/callback can be correlated.
                let provider_job = body.get("request_id").or_else(|| body.get("id")).cloned();
                let mut job = BatchJob::queued(&job_id, warnings);
                job.result = provider_job.map(|pj| json!({ "provider_job": pj }));
                state.batch_jobs.insert(job_id.clone(), job.clone());
                info!(%job_id, provider, "batch job submitted (async)");
                ok(&BatchJob {
                    // Return only the handle (don't leak the provider submit body verbatim).
                    result: None,
                    ..job
                })
            }
            Ok((status, body)) => err_job(
                &state,
                &job_id,
                &format!("provider rejected submission ({status}): {body}"),
            ),
            Err(e) => err_job(&state, &job_id, &format!("submission failed: {e}")),
        }
    } else {
        // Synchronous: run inline, store the canonical result, return the completed job.
        match execute(&submission).await {
            Ok((status, body)) if status.is_success() => {
                let job = BatchJob::completed(&job_id, body, warnings);
                state.batch_jobs.insert(job_id.clone(), job.clone());
                info!(%job_id, provider, "batch job completed (sync)");
                ok(&job)
            }
            Ok((status, body)) => err_job(
                &state,
                &job_id,
                &format!("provider error ({status}): {body}"),
            ),
            Err(e) => err_job(&state, &job_id, &format!("transcription failed: {e}")),
        }
    }
}

/// `GET /transcribe/batch/{job_id}`
pub async fn get_batch(State(state): State<Arc<AppState>>, Path(job_id): Path<String>) -> Response {
    match state.batch_jobs.get(&job_id) {
        Some(job) => ok(job.value()),
        None => err(
            StatusCode::NOT_FOUND,
            &format!("no batch job with id '{job_id}'"),
        ),
    }
}

// ---------------------------------------------------------------------------------------------
// internals
// ---------------------------------------------------------------------------------------------

/// The endpoint override (mock/proxy host) from the standardized `extras`, if present.
fn endpoint_override(
    req: &BatchTranscribeRequest,
    provider: &str,
) -> Result<Option<String>, String> {
    let Some(raw) = req.config.endpoint_override() else {
        return Ok(None);
    };
    let endpoint = raw.trim();
    if endpoint.is_empty() {
        return Ok(None);
    }
    validate_batch_base_url(provider, endpoint)?;
    Ok(Some(endpoint.to_string()))
}

/// Resolve the provider credential (client-supplied first, then server config).
fn resolve_key(state: &AppState, req: &BatchTranscribeRequest) -> Result<String, String> {
    resolve_key_from_config(&state.config, req)
}

fn resolve_key_from_config(
    config: &ServerConfig,
    req: &BatchTranscribeRequest,
) -> Result<String, String> {
    let provider = req.config.base.provider.to_lowercase();
    if let Some(client_key) = client_batch_api_key(&provider, &req.config.base.api_key)? {
        return Ok(client_key);
    }
    config
        .get_api_key(&provider)
        .map_err(|e| format!("{e} (set a client `api_key` or configure the server)"))
}

fn client_batch_api_key(provider: &str, api_key: &str) -> Result<Option<String>, String> {
    let key = api_key.trim();
    if key.is_empty() {
        return Ok(None);
    }
    if provider != "google" && crate::config::utils::is_placeholder_credential(key) {
        return Err(format!(
            "{provider} API key is a placeholder/empty - set a real client `api_key` or configure the server"
        ));
    }
    Ok(Some(key.to_string()))
}

/// Build the per-provider submission, performing the AssemblyAI bytes→upload step when needed.
async fn build_submission(
    provider: &str,
    req: &BatchTranscribeRequest,
    api_key: &str,
    base_url: Option<&str>,
    state: &AppState,
) -> Result<BatchSubmission, String> {
    match provider {
        "deepgram" => {
            build_deepgram_prerecorded(req, api_key, base_url.unwrap_or("https://api.deepgram.com"))
        }
        "openai" => {
            build_openai_transcription(req, api_key, base_url.unwrap_or("https://api.openai.com"))
        }
        "assemblyai" => {
            let host = base_url.unwrap_or("https://api.assemblyai.com");
            // URL source → pass through; bytes source → upload first to obtain an audio_url.
            let audio_url = if let Some(u) = req.audio.url() {
                u.to_string()
            } else if let Some((b64, _ct)) = req.audio.bytes() {
                upload_assemblyai(state, host, api_key, b64).await?
            } else {
                return Err("assemblyai batch requires `url` or `audio_base64`".into());
            };
            build_assemblyai_transcript(req, api_key, host, &audio_url)
        }
        other => Err(format!("unsupported batch provider '{other}'")),
    }
}

/// Upload raw bytes to AssemblyAI `POST /v2/upload`, returning the `upload_url` for `audio_url`.
async fn upload_assemblyai(
    _state: &AppState,
    host: &str,
    api_key: &str,
    audio_base64: &str,
) -> Result<String, String> {
    let bytes = decode_assemblyai_upload_audio(audio_base64)?;
    let client = http_client()?;
    let resp = client
        .post(format!("{}/v2/upload", host.trim_end_matches('/')))
        .header("Authorization", api_key)
        .header("Content-Type", "application/octet-stream")
        .body(bytes)
        .send()
        .await
        .map_err(|e| format!("assemblyai upload failed: {e}"))?;
    let status = resp.status();
    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("assemblyai upload response not JSON: {e}"))?;
    if !status.is_success() {
        return Err(format!("assemblyai upload error ({status}): {v}"));
    }
    v.get("upload_url")
        .and_then(|u| u.as_str())
        .map(str::to_string)
        .ok_or_else(|| "assemblyai upload returned no upload_url".to_string())
}

fn decode_assemblyai_upload_audio(audio_base64: &str) -> Result<Vec<u8>, String> {
    decode_inline_batch_audio(audio_base64)
}

#[cfg(test)]
fn decode_assemblyai_upload_audio_with_limit(
    audio_base64: &str,
    max_decoded_bytes: usize,
) -> Result<Vec<u8>, String> {
    decode_inline_batch_audio_with_limit(audio_base64, max_decoded_bytes)
}

/// Execute a built submission with `reqwest`, returning `(status, json_body)`. A non-JSON body is
/// wrapped as `{"raw": "<text>"}` so callers always get JSON.
async fn execute(sub: &BatchSubmission) -> Result<(StatusCode, serde_json::Value), String> {
    let client = http_client()?;
    let r = &sub.request;
    let mut builder = match r.method.as_str() {
        "POST" => client.post(&r.url),
        "PUT" => client.put(&r.url),
        m => return Err(format!("unsupported method {m}")),
    };
    for (k, v) in &r.headers {
        builder = builder.header(k, v);
    }
    builder = match &r.body {
        BatchHttpBody::Empty => builder,
        BatchHttpBody::Json(v) => builder.json(v),
        BatchHttpBody::Raw { bytes, .. } => builder.body(bytes.clone()),
        BatchHttpBody::Multipart { fields, file } => {
            let mut form = reqwest::multipart::Form::new();
            for (name, value) in fields {
                form = form.text(name.clone(), value.clone());
            }
            if let Some((field, filename, ct, bytes)) = file {
                let part = reqwest::multipart::Part::bytes(bytes.clone())
                    .file_name(filename.clone())
                    .mime_str(ct)
                    .map_err(|e| format!("bad multipart mime: {e}"))?;
                form = form.part(field.clone(), part);
            }
            builder.multipart(form)
        }
    };
    let resp = builder
        .send()
        .await
        .map_err(|e| format!("request error: {e}"))?;
    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let text = resp.text().await.unwrap_or_default();
    let body = serde_json::from_str::<serde_json::Value>(&text).unwrap_or(json!({ "raw": text }));
    Ok((status, body))
}

/// A reqwest client with the batch timeout.
fn http_client() -> Result<reqwest::Client, String> {
    crate::core::net::ssrf_protected_client_builder(crate::core::net::HTTP_URL_SCHEMES)
        .timeout(Duration::from_secs(BATCH_HTTP_TIMEOUT_SECS))
        .build()
        .map_err(|e| format!("http client build failed: {e}"))
}

/// Store an errored job and return it (HTTP 200 — the job FAILED, but the request was processed;
/// the failure is carried in the job body, not the HTTP status, so SDK pollers see a uniform shape).
fn err_job(state: &AppState, job_id: &str, msg: &str) -> Response {
    warn!(%job_id, "batch job error: {msg}");
    let job = BatchJob::errored(job_id, msg);
    state.batch_jobs.insert(job_id.to_string(), job.clone());
    (StatusCode::OK, Json(job)).into_response()
}

/// 200 with a serialized job.
fn ok(job: &BatchJob) -> Response {
    debug_assert!(matches!(
        job.status,
        BatchStatus::Queued | BatchStatus::Completed | BatchStatus::Error | BatchStatus::Processing
    ));
    (StatusCode::OK, Json(job)).into_response()
}

/// A plain error response.
fn err(code: StatusCode, msg: &str) -> Response {
    (code, Json(json!({ "error": msg }))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DAGTimeoutsConfig, PluginConfig};
    use crate::core::stt::STTConfig;
    use crate::core::stt::batch::{BatchAudioSource, BatchFeatures};
    use crate::core::stt::standard::StandardSTTConfig;

    fn req_with_endpoint(endpoint: &str) -> BatchTranscribeRequest {
        BatchTranscribeRequest {
            audio: BatchAudioSource::Bytes {
                audio_base64: "AAAA".into(),
                content_type: None,
            },
            config: StandardSTTConfig::from_base(STTConfig {
                provider: "openai".into(),
                ..Default::default()
            })
            .with_endpoint_override(endpoint),
            batch: BatchFeatures::default(),
            callback_url: None,
            callback_method: None,
            translation: None,
        }
    }

    fn req_with_api_key(provider: &str, api_key: &str) -> BatchTranscribeRequest {
        BatchTranscribeRequest {
            audio: BatchAudioSource::Bytes {
                audio_base64: "AAAA".into(),
                content_type: None,
            },
            config: StandardSTTConfig::from_base(STTConfig {
                provider: provider.into(),
                api_key: api_key.into(),
                ..Default::default()
            }),
            batch: BatchFeatures::default(),
            callback_url: None,
            callback_method: None,
            translation: None,
        }
    }

    fn server_config_with_openai_key(api_key: Option<&str>) -> ServerConfig {
        ServerConfig {
            host: "localhost".to_string(),
            port: 3001,
            tls: None,
            livekit_url: "ws://localhost:7880".to_string(),
            livekit_public_url: "http://localhost:7880".to_string(),
            livekit_api_key: None,
            livekit_api_secret: None,
            deepgram_api_key: None,
            elevenlabs_api_key: None,
            google_credentials: None,
            azure_speech_subscription_key: None,
            azure_speech_region: None,
            cartesia_api_key: None,
            openai_api_key: api_key.map(str::to_string),
            azure_openai_api_key: None,
            azure_openai_endpoint: None,
            grok_api_key: None,
            inworld_api_key: None,
            gemini_api_key: None,
            ultravox_api_key: None,
            speechmatics_api_key: None,
            yandex_api_key: None,
            yandex_folder_id: None,
            assemblyai_api_key: None,
            hume_api_key: None,
            lmnt_api_key: None,
            groq_api_key: None,
            playht_api_key: None,
            playht_user_id: None,
            ibm_watson_api_key: None,
            ibm_watson_instance_id: None,
            ibm_watson_region: None,
            aws_access_key_id: None,
            aws_secret_access_key: None,
            aws_region: None,
            gnani_token: None,
            gnani_access_key: None,
            gnani_certificate_path: None,
            recording_s3_bucket: None,
            recording_s3_region: None,
            recording_s3_endpoint: None,
            recording_s3_access_key: None,
            recording_s3_secret_key: None,
            recording_s3_prefix: None,
            cache_path: None,
            cache_ttl_seconds: Some(3600),
            auth_service_url: None,
            auth_signing_key_path: None,
            auth_api_secrets: Vec::new(),
            auth_timeout_seconds: 5,
            auth_required: false,
            sip: None,
            cors_allowed_origins: None,
            rate_limit_requests_per_second: 60,
            rate_limit_burst_size: 10,
            max_websocket_connections: None,
            max_connections_per_ip: 100,
            ws_processing_timeout_secs: 10,
            realtime_processing_timeout_secs: 30,
            sip_max_participants: 3,
            realtime_endpoint_overrides: Default::default(),
            aliases: Default::default(),
            plugins: PluginConfig::default(),
            dag_timeouts: DAGTimeoutsConfig::default(),
        }
    }

    #[test]
    fn batch_client_placeholder_api_key_is_rejected_before_provider_call() {
        let config = server_config_with_openai_key(Some("server-openai-key"));
        let req = req_with_api_key("openai", "  your-openai-api-key  ");

        let err = resolve_key_from_config(&config, &req)
            .expect_err("placeholder BYOK must not be forwarded to providers");

        assert!(
            err.contains("placeholder"),
            "error should explain placeholder rejection: {err}"
        );
    }

    #[test]
    fn batch_blank_client_api_key_falls_back_to_server_config() {
        let config = server_config_with_openai_key(Some("server-openai-key"));
        let req = req_with_api_key("openai", " \n\t ");

        let key = resolve_key_from_config(&config, &req)
            .expect("blank BYOK should be treated as absent and use server config");

        assert_eq!(key, "server-openai-key");
    }

    #[test]
    fn batch_real_client_api_key_is_trimmed_and_used() {
        let config = server_config_with_openai_key(Some("server-openai-key"));
        let req = req_with_api_key("openai", "  sk-client  ");

        let key = resolve_key_from_config(&config, &req)
            .expect("real BYOK should override server config");

        assert_eq!(key, "sk-client");
    }

    #[test]
    fn assemblyai_upload_audio_decode_is_size_bounded_before_allocation() {
        let ok = decode_assemblyai_upload_audio_with_limit("data:audio/wav;base64,QUJD", 3)
            .expect("three decoded bytes should fit limit");
        assert_eq!(ok, b"ABC");

        let err = decode_assemblyai_upload_audio_with_limit("QUJD", 2)
            .expect_err("decoded payload above limit must be rejected");
        assert!(
            err.contains("decoded size limit"),
            "unexpected limit error: {err}"
        );
    }

    #[test]
    fn batch_endpoint_override_is_ssrf_checked_before_submission() {
        let _env = crate::core::net::ssrf_env_lock();
        let ok = endpoint_override(&req_with_endpoint("https://example.com/proxy"), "openai")
            .expect("public endpoint override should pass");
        assert_eq!(ok.as_deref(), Some("https://example.com/proxy"));

        let err = endpoint_override(&req_with_endpoint("http://127.0.0.1:9000"), "openai")
            .expect_err("loopback endpoint_override must be rejected");
        assert!(err.contains("SSRF protection"), "{err}");

        let err = endpoint_override(&req_with_endpoint("file:///tmp/socket"), "openai")
            .expect_err("non-HTTP endpoint_override must be rejected");
        assert!(err.contains("not allowed"), "{err}");

        let none = endpoint_override(&req_with_endpoint("  "), "openai")
            .expect("empty endpoint_override is ignored");
        assert!(none.is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn batch_http_client_redirect_policy_rejects_private_hop() {
        let _env = crate::core::net::ssrf_env_lock();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local redirect test server");
        let addr = listener.local_addr().expect("local addr");

        tokio::spawn(async move {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let mut buf = [0u8; 1024];
            let _ = tokio::io::AsyncReadExt::read(&mut stream, &mut buf).await;
            let response = concat!(
                "HTTP/1.1 302 Found\r\n",
                "Location: http://127.0.0.1:9/metadata\r\n",
                "Content-Length: 0\r\n",
                "\r\n"
            );
            let _ = tokio::io::AsyncWriteExt::write_all(&mut stream, response.as_bytes()).await;
        });

        let client = http_client().expect("batch HTTP client");
        let err = client
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
}
