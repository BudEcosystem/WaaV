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

use crate::core::stt::batch::{
    BatchHttpBody, BatchJob, BatchStatus, BatchSubmission, BatchTranscribeRequest,
    batch_provider_supported, build_assemblyai_transcript, build_deepgram_prerecorded,
    build_openai_transcription,
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
    let base_url = endpoint_override(&req).map(|s| s.to_string());
    let submission = match build_submission(&provider, &req, &api_key, base_url.as_deref(), &state)
        .await
    {
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
                let provider_job =
                    body.get("request_id").or_else(|| body.get("id")).cloned();
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
pub async fn get_batch(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> Response {
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
fn endpoint_override(req: &BatchTranscribeRequest) -> Option<&str> {
    req.config.endpoint_override()
}

/// Resolve the provider credential (client-supplied first, then server config).
fn resolve_key(state: &AppState, req: &BatchTranscribeRequest) -> Result<String, String> {
    let provider = req.config.base.provider.to_lowercase();
    if !req.config.base.api_key.trim().is_empty() {
        return Ok(req.config.base.api_key.trim().to_string());
    }
    state
        .config
        .get_api_key(&provider)
        .map_err(|e| format!("{e} (set a client `api_key` or configure the server)"))
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
    use base64::Engine;
    let payload = audio_base64
        .rsplit_once("base64,")
        .map(|(_, b)| b)
        .unwrap_or(audio_base64);
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload.trim())
        .map_err(|e| format!("invalid base64 audio: {e}"))?;
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
    reqwest::Client::builder()
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
