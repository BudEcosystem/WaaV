//! Batch transcription: driving a STREAMING STT provider to completion from one uploaded file.
//!
//! FRD-018 M5. `/v1/audio/transcriptions` hands over a whole file; every WaaV STT provider is
//! a socket that expects audio to arrive over time and reports results through callbacks.
//! There is no provider-side "transcribe this file" call to delegate to, so the batch shape
//! has to be synthesised here: connect, feed the samples as frames, wait for the provider to
//! stop producing, disconnect, join what came back.
//!
//! **The honest hard part is knowing when it is finished.** A streaming provider never says
//! "that was the whole file" — it says "here is another final segment", forever, until the
//! socket closes. So completion is inferred from two bounds, and both are needed:
//!
//! * a QUIET WINDOW after the last result, which is what actually ends a normal request; and
//! * an OVERALL DEADLINE, because a provider that connects and then says nothing at all would
//!   otherwise hold the request open until the client gives up.
//!
//! Returning a partial transcript on the deadline is deliberate. The alternative — erroring
//! after successfully transcribing four of five minutes — throws away work the user already
//! paid the vendor for.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;
use tracing::{debug, warn};
use waav_openai_audio::pcm::PcmAudio;

// Through the module's re-exports rather than `stt::base::*`, which is private.
use crate::core::stt::{STTConfig, STTErrorCallback, STTResult, STTResultCallback};

/// Audio handed to a provider in frames rather than one buffer.
///
/// A socket handed 25 MB in a single write is a socket that gets its connection reset by at
/// least one vendor, and providers with server-side VAD segment on frame boundaries, so
/// realistic framing also produces better segmentation than one giant blob.
const FRAME_MS: usize = 100;

/// How long after the last result to conclude the provider is done.
///
/// Long enough to survive a vendor's inter-segment gap, short enough that a short clip does
/// not feel hung.
const QUIET_WINDOW: Duration = Duration::from_millis(2_500);

/// Ceiling on the whole transcription, independent of the quiet window.
const OVERALL_DEADLINE: Duration = Duration::from_secs(300);

/// How long to wait for a provider that produces nothing at all before giving up.
///
/// Separated from `OVERALL_DEADLINE` on purpose: silence from the start is a broken
/// connection or a rejected credential, and making that caller wait five minutes to be told
/// so is its own bug.
const FIRST_RESULT_TIMEOUT: Duration = Duration::from_secs(45);

/// What a batch run produced.
pub struct Transcript {
    pub text: String,
    /// True when a bound fired before the provider went quiet on its own, so the text may be
    /// short of the audio. Surfaced so the handler can say so rather than implying completeness.
    pub truncated: bool,
}

#[derive(Default)]
struct Collector {
    segments: Vec<String>,
    last_result_at: Option<Instant>,
    error: Option<String>,
}

/// Run one file through a streaming provider and return the joined transcript.
/// Takes no `AppState`: unlike TTS, the STT side has no `ReqManager` connection pool to draw
/// from (`AppState::get_tts_req_manager` has no STT counterpart), so threading state through
/// would only suggest a warm-connection path that does not exist.
pub async fn transcribe_once(
    provider_name: &str,
    stt_config: STTConfig,
    pcm: &PcmAudio,
) -> Result<Transcript, String> {
    let mut provider = crate::core::stt::create_stt_provider(provider_name, stt_config)
        .map_err(|e| format!("{e}"))?;

    let collector = Arc::new(Mutex::new(Collector::default()));

    // Only FINAL results are kept. Interim results are revisions of text that a later final
    // result restates, so accumulating both duplicates every word in the output.
    let sink = Arc::clone(&collector);
    let on_result: STTResultCallback = Arc::new(move |r: STTResult| {
        let sink = Arc::clone(&sink);
        Box::pin(async move {
            let mut c = sink.lock().await;
            c.last_result_at = Some(Instant::now());
            if r.is_final && !r.transcript.trim().is_empty() {
                c.segments.push(r.transcript);
            }
        })
    });

    let sink = Arc::clone(&collector);
    let on_error: STTErrorCallback = Arc::new(move |e| {
        let sink = Arc::clone(&sink);
        let msg = e.to_string();
        Box::pin(async move {
            let mut c = sink.lock().await;
            // First error wins: later ones are usually consequences of the first (a closed
            // socket reporting every subsequent write), and the first names the real cause.
            if c.error.is_none() {
                c.error = Some(msg);
            }
        })
    });

    provider
        .on_result(on_result)
        .await
        .map_err(|e| format!("{e}"))?;
    provider.on_error(on_error).await.map_err(|e| format!("{e}"))?;
    provider.connect().await.map_err(|e| format!("{e}"))?;

    let started = Instant::now();
    let frame_samples = (pcm.sample_rate as usize * FRAME_MS / 1000).max(1);

    for chunk in pcm.samples.chunks(frame_samples) {
        // Bail out mid-send on a provider error rather than pushing the rest of the file at a
        // socket that has already failed.
        if collector.lock().await.error.is_some() {
            break;
        }
        let mut bytes = Vec::with_capacity(chunk.len() * 2);
        for s in chunk {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        if let Err(e) = provider.send_audio(bytes.into()).await {
            let _ = provider.disconnect().await;
            return Err(format!("{e}"));
        }
    }
    debug!(
        samples = pcm.samples.len(),
        secs = pcm.duration_secs(),
        "batch audio sent; waiting for the provider to settle"
    );

    let truncated = wait_for_settle(&collector, started).await;

    // Disconnect BEFORE reading the transcript: several providers flush a trailing final
    // segment on close, and reading first would drop the last few words of every file.
    let _ = provider.disconnect().await;
    tokio::time::sleep(Duration::from_millis(150)).await;

    let c = collector.lock().await;
    if let Some(err) = &c.error
        && c.segments.is_empty()
    {
        return Err(err.clone());
    }

    Ok(Transcript {
        text: c.segments.join(" ").split_whitespace().collect::<Vec<_>>().join(" "),
        truncated,
    })
}

/// Wait until the provider stops producing, or a bound fires. Returns whether a bound fired.
async fn wait_for_settle(collector: &Arc<Mutex<Collector>>, started: Instant) -> bool {
    loop {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let c = collector.lock().await;

        if c.error.is_some() {
            return false;
        }
        match c.last_result_at {
            Some(last) => {
                if last.elapsed() >= QUIET_WINDOW {
                    return false; // settled normally
                }
            }
            None => {
                if started.elapsed() >= FIRST_RESULT_TIMEOUT {
                    warn!("provider produced no transcript within the first-result timeout");
                    return true;
                }
            }
        }
        if started.elapsed() >= OVERALL_DEADLINE {
            warn!("batch transcription hit the overall deadline; returning a partial transcript");
            return true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_result_timeout_is_shorter_than_the_overall_deadline() {
        // If they were the other way round a dead provider would hold the request for the
        // full deadline, and the caller would be told nothing for five minutes.
        assert!(FIRST_RESULT_TIMEOUT < OVERALL_DEADLINE);
    }

    #[test]
    fn the_quiet_window_is_shorter_than_the_first_result_timeout() {
        // Otherwise a normal short clip could never settle before the no-result path fired.
        assert!(QUIET_WINDOW < FIRST_RESULT_TIMEOUT);
    }

    #[test]
    fn a_frame_is_a_sane_size_at_every_rate_a_provider_might_report() {
        for rate in [8_000u32, 16_000, 24_000, 44_100, 48_000] {
            let frame = (rate as usize * FRAME_MS / 1000).max(1);
            assert!(frame > 0, "rate {rate} produced an empty frame");
            // 100ms of 16-bit mono: comfortably under any vendor's per-message ceiling.
            assert!(frame * 2 < 16_384, "rate {rate} frame is too large: {frame}");
        }
    }

    #[test]
    fn a_zero_rate_still_yields_a_nonempty_frame_rather_than_dividing_by_zero() {
        // decode() rejects a zero rate, but this must not be the thing that panics if it slips.
        assert_eq!((0usize * FRAME_MS / 1000).max(1), 1);
    }
}

/// Forward an upload to a self-hosted OpenAI-compatible backend and return its body verbatim.
///
/// `/audio/translations` is a DIFFERENT route on these servers, not a parameter, so the
/// translate flag selects the URL. Sending a translation request to the transcription route
/// returns source-language text with a 200 — correct-looking and wrong.
pub async fn transcribe_self_hosted(
    api_base: &str,
    api_key: &str,
    model: &str,
    file_bytes: Vec<u8>,
    filename: &str,
    settings: &waav_openai_audio::transcription::TranscriptionSettings,
) -> Result<String, String> {
    use crate::core::tts::self_hosted::{transcription_url, translation_url};

    let url = if settings.translate {
        translation_url(api_base)
    } else {
        transcription_url(api_base)
    };

    let part = reqwest::multipart::Part::bytes(file_bytes).file_name(filename.to_string());
    let mut form = reqwest::multipart::Form::new()
        .part("file", part)
        // The backend's own model name, not the Bud endpoint alias -- the alias means nothing
        // to a server that has never heard of Bud.
        .text("model", model.to_string())
        .text("response_format", settings.response_format.as_str().to_string());

    // Only forward what the caller actually set. Sending `language: ""` makes some servers
    // fail validation on a field the caller never mentioned.
    if let Some(lang) = &settings.language
        && !settings.translate
    {
        form = form.text("language", lang.clone());
    }
    if let Some(prompt) = &settings.prompt {
        form = form.text("prompt", prompt.clone());
    }
    if let Some(t) = settings.temperature {
        form = form.text("temperature", t.to_string());
    }

    let client = reqwest::Client::builder()
        .timeout(OVERALL_DEADLINE)
        .build()
        .map_err(|e| format!("could not build the http client: {e}"))?;

    let mut req = client.post(&url).multipart(form);
    // A keyless in-cluster deployment is normal; an empty Bearer is a malformed credential,
    // not an anonymous one, and some servers reject it outright.
    if !api_key.is_empty() {
        req = req.bearer_auth(api_key);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| format!("self-hosted deployment at {url} is unreachable: {e}"))?;

    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| format!("self-hosted deployment returned an unreadable body: {e}"))?;

    if !status.is_success() {
        // The backend's own message is far more useful than anything synthesised here.
        return Err(format!(
            "self-hosted deployment returned {status}: {}",
            body.chars().take(500).collect::<String>()
        ));
    }
    Ok(body)
}
