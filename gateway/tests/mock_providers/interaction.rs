//! Test doubles for interaction mode, shared by `interaction_actor.rs` and `interaction_ws.rs`
//! (each includes this file with `#[path]`).
//!
//! - [`TtsDouble`]: a TTS provider that synthesises each speak after a delay and, like a real
//!   provider's dispatcher, reports completion only when its queue is empty.
//! - [`SttDouble`]: an STT provider whose results and errors the test pushes; it can also act
//!   as a request-response provider with scripted uploads.
//! - [`LlmDouble`]: an OpenAI-compatible streaming chat endpoint with scripted rounds.
//! - [`FrontDouble`]: the front model's llama-server.
//! - [`ToolDouble`]: a tool webhook that records its calls.
//! - [`BackendDouble`]: any HTTP backend (a webhook, a JSON-RPC agent, a Responses server)
//!   that records its requests and answers from the test's closure.
//!
//! Every delay is a `tokio::time::sleep`, so the doubles follow a paused clock (see
//! [`pace_paused_clock`]). Each double is its own instance (a provider under its own name, a
//! server on its own port).
#![allow(dead_code)]

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::serve::ListenerExt;
use axum::{Json, Router};
use futures::stream;
use hmac::{Hmac, Mac};
use parking_lot::Mutex;
use serde_json::{Value, json};
use sha2::Sha256;
use tokio::net::TcpListener;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;

use waav_gateway::core::stt::{
    BaseSTT, STTConfig, STTError, STTErrorCallback, STTResult, STTResultCallback,
};
use waav_gateway::core::tts::{AudioCallback, AudioData, BaseTTS, TTSConfig, TTSError, TTSResult};
use waav_gateway::global_registry;
use waav_gateway::plugin::metadata::ProviderMetadata;

fn next_id() -> usize {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    NEXT.fetch_add(1, Ordering::SeqCst)
}

/// Takes one from a counter of "the next N"; `false` when it is at zero.
fn take_one(counter: &AtomicUsize) -> bool {
    counter
        .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
        .is_ok()
}

async fn after(ms: u64) {
    if ms > 0 {
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }
}

/// For a test on tokio's paused clock that talks to these doubles over sockets. The paused
/// clock jumps to the next timer whenever every task waits, also while bytes are still on
/// their way through the kernel. This task is a timer every millisecond that then holds the
/// thread for a moment of real time, so a socket hop normally costs a millisecond or two of
/// virtual time instead of the whole wait of the next timer. It ends with the test's runtime.
pub fn pace_paused_clock() -> JoinHandle<()> {
    tokio::spawn(async {
        loop {
            after(1).await;
            std::thread::sleep(Duration::from_micros(20));
        }
    })
}

/// Serves `app` on a loopback port of its own; the address and the server task.
async fn serve(app: Router) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = format!("http://{}", listener.local_addr().expect("address"));
    // No Nagle delay: a small write must not wait in real time for the peer's delayed ACK
    // while the paused clock runs on.
    let listener = listener.tap_io(|stream| {
        let _ = stream.set_nodelay(true);
    });
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (address, server)
}

// ---- text to speech

/// Every sample of the double's audio (PCM16, 16 kHz, mono).
pub const TTS_AMPLITUDE: i16 = 8_000;
const TTS_SAMPLE_RATE: u32 = 16_000;

static TTS_DOUBLES: once_cell::sync::Lazy<Mutex<HashMap<String, Arc<TtsDouble>>>> =
    once_cell::sync::Lazy::new(Mutex::default);

/// The test's side of one mock TTS provider.
pub struct TtsDouble {
    /// The provider name to put in `TTSConfig`.
    pub provider: String,
    /// How long one synthesis takes, and how much audio one character gives.
    pub synth_ms: AtomicU64,
    pub ms_per_char: AtomicU64,
    spoken: Mutex<Vec<String>>,
    fail: AtomicUsize,
    refuse: AtomicUsize,
    clears: AtomicUsize,
    /// Every speaking rate the session set, in order; the audio of a text is shorter at a
    /// higher rate.
    rates: Mutex<Vec<f32>>,
    voices: Mutex<Vec<String>>,
    /// Texts whose audio is silence.
    silent: Mutex<Vec<String>>,
    /// Texts in the speech cache: their audio comes even while syntheses fail.
    cached: Mutex<Vec<String>>,
}

impl TtsDouble {
    /// Registers a provider under a name of its own.
    pub fn register() -> Arc<TtsDouble> {
        let double = Arc::new(TtsDouble {
            provider: format!("ix-tts-{}", next_id()),
            synth_ms: AtomicU64::new(40),
            ms_per_char: AtomicU64::new(50),
            spoken: Mutex::default(),
            fail: AtomicUsize::new(0),
            refuse: AtomicUsize::new(0),
            clears: AtomicUsize::new(0),
            rates: Mutex::default(),
            voices: Mutex::default(),
            silent: Mutex::default(),
            cached: Mutex::default(),
        });
        TTS_DOUBLES
            .lock()
            .insert(double.provider.clone(), double.clone());
        global_registry().register_tts(
            &double.provider,
            Arc::new(|config: TTSConfig| {
                MockTts::new(config).map(|tts| Box::new(tts) as Box<dyn BaseTTS>)
            }),
            ProviderMetadata::tts(&double.provider, "Mock TTS (interaction)"),
        );
        double
    }

    /// Every text the provider was asked to speak, in order.
    pub fn spoken(&self) -> Vec<String> {
        self.spoken.lock().clone()
    }

    /// `text`'s audio is in the speech cache: it plays even while syntheses fail.
    pub fn cache(&self, text: &str) {
        self.cached.lock().push(text.to_owned());
    }

    /// The next `n` speaks fail as a vendor refusal does: the error callback, then completion.
    pub fn fail_next(&self, n: usize) {
        self.fail.store(n, Ordering::SeqCst);
    }

    /// The next `n` speaks are refused outright: `speak` returns an error.
    pub fn refuse_next(&self, n: usize) {
        self.refuse.store(n, Ordering::SeqCst);
    }

    pub fn clears(&self) -> usize {
        self.clears.load(Ordering::SeqCst)
    }

    /// The speaking rates the session set, in order.
    pub fn rates(&self) -> Vec<f32> {
        self.rates.lock().clone()
    }

    /// The voices the session switched to, in order.
    pub fn voices(&self) -> Vec<String> {
        self.voices.lock().clone()
    }

    /// The audio of `text` is silence from now on.
    pub fn silence(&self, text: &str) {
        self.silent.lock().push(text.to_owned());
    }

    /// The length of the audio the double gives for `text`.
    pub fn audio_ms(&self, text: &str) -> u64 {
        text.chars().count() as u64 * self.ms_per_char.load(Ordering::SeqCst)
    }
}

struct Job {
    text: String,
    /// The speaking rate when it was queued.
    rate: f32,
    fail: bool,
    /// The provider's clear count when the job was queued: a clear drops the job.
    epoch: usize,
    callback: Arc<dyn AudioCallback>,
}

struct MockTts {
    double: Arc<TtsDouble>,
    callback: Option<Arc<dyn AudioCallback>>,
    jobs: Option<UnboundedSender<Job>>,
    queued: Arc<AtomicUsize>,
    epoch: Arc<AtomicUsize>,
}

/// The provider's dispatcher: one job at a time, completion only when the queue is empty
/// (`core/tts/provider.rs`), an error reported before the completion.
async fn synthesise(
    mut jobs: UnboundedReceiver<Job>,
    double: Arc<TtsDouble>,
    queued: Arc<AtomicUsize>,
    epoch: Arc<AtomicUsize>,
) {
    while let Some(job) = jobs.recv().await {
        let live = || epoch.load(Ordering::SeqCst) == job.epoch;
        if live() {
            after(double.synth_ms.load(Ordering::SeqCst)).await;
        }
        if live() && job.fail {
            let error = TTSError::AudioGenerationFailed("failed by the test".to_owned());
            job.callback.on_error(error).await;
        } else if live() {
            let ms = (double.audio_ms(&job.text) as f32 / job.rate) as usize;
            let samples = ms * (TTS_SAMPLE_RATE as usize / 1000);
            let silent = double.silent.lock().contains(&job.text);
            let amplitude = if silent { 0 } else { TTS_AMPLITUDE };
            let audio = AudioData {
                data: amplitude.to_le_bytes().repeat(samples),
                sample_rate: TTS_SAMPLE_RATE,
                format: "pcm".to_owned(),
                duration_ms: None,
            };
            job.callback.on_audio(audio).await;
        }
        if queued.fetch_sub(1, Ordering::SeqCst) == 1 && live() {
            job.callback.on_complete().await;
        }
    }
}

#[async_trait::async_trait]
impl BaseTTS for MockTts {
    fn new(config: TTSConfig) -> TTSResult<Self> {
        let double = TTS_DOUBLES.lock().get(&config.provider).cloned();
        let double = double.ok_or_else(|| {
            TTSError::InvalidConfiguration(format!("no TTS double named {}", config.provider))
        })?;
        Ok(MockTts {
            double,
            callback: None,
            jobs: None,
            queued: Arc::default(),
            epoch: Arc::default(),
        })
    }

    async fn connect(&mut self) -> TTSResult<()> {
        Ok(())
    }

    async fn disconnect(&mut self) -> TTSResult<()> {
        Ok(())
    }

    fn is_ready(&self) -> bool {
        true
    }

    async fn speak(&mut self, text: &str, _flush: bool) -> TTSResult<()> {
        if take_one(&self.double.refuse) {
            return Err(TTSError::ProviderError("refused by the test".to_owned()));
        }
        self.double.spoken.lock().push(text.to_owned());
        let Some(callback) = self.callback.clone() else {
            return Ok(());
        };
        let job = Job {
            text: text.to_owned(),
            rate: self.double.rates.lock().last().copied().unwrap_or(1.0),
            fail: !self
                .double
                .cached
                .lock()
                .iter()
                .any(|cached| cached == text)
                && take_one(&self.double.fail),
            epoch: self.epoch.load(Ordering::SeqCst),
            callback,
        };
        self.queued.fetch_add(1, Ordering::SeqCst);
        let (double, queued, epoch) = (&self.double, &self.queued, &self.epoch);
        let jobs = self.jobs.get_or_insert_with(|| {
            let (tx, rx) = mpsc::unbounded_channel();
            tokio::spawn(synthesise(
                rx,
                double.clone(),
                queued.clone(),
                epoch.clone(),
            ));
            tx
        });
        let _ = jobs.send(job);
        Ok(())
    }

    async fn flush(&self) -> TTSResult<()> {
        Ok(())
    }

    async fn clear(&mut self) -> TTSResult<()> {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        self.double.clears.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn on_audio(&mut self, callback: Arc<dyn AudioCallback>) -> TTSResult<()> {
        self.callback = Some(callback);
        Ok(())
    }

    fn get_provider_info(&self) -> Value {
        json!({ "provider": self.double.provider })
    }

    fn takes_rate(&self) -> bool {
        true
    }

    async fn has_cached(&mut self, text: &str) -> bool {
        self.double
            .cached
            .lock()
            .iter()
            .any(|cached| cached == text)
    }

    fn set_voice(&mut self, voice: &str) -> bool {
        self.double.voices.lock().push(voice.to_owned());
        true
    }

    fn set_speaking_rate(&mut self, rate: f32) -> bool {
        self.double.rates.lock().push(rate);
        true
    }
}

// ---- speech to text

static STT_DOUBLES: once_cell::sync::Lazy<Mutex<HashMap<String, Arc<SttDouble>>>> =
    once_cell::sync::Lazy::new(Mutex::default);

/// The test's side of one mock STT provider: the test is the vendor.
pub struct SttDouble {
    /// The provider name to put in `STTConfig`.
    pub provider: String,
    request_response: bool,
    result: Mutex<Option<STTResultCallback>>,
    error: Mutex<Option<STTErrorCallback>>,
    /// Outcomes of the next uploads: the transcript, or the error.
    uploads: Mutex<VecDeque<Result<String, String>>>,
    pub upload_ms: AtomicU64,
    taken: AtomicUsize,
    uploaded: AtomicUsize,
    /// Every byte of audio the provider was sent.
    audio: Mutex<Vec<u8>>,
    /// Instances made: the first is the session's; on a request-response double a later one
    /// checks a listening sound (`clips::heard`) and hears `clip_words` in every clip (nothing
    /// when `None`).
    instances: AtomicUsize,
    clip_words: Mutex<Option<String>>,
    clips_heard: AtomicUsize,
    /// The language of each streaming instance made (a language switch opens a second one).
    languages: Mutex<Vec<String>>,
    /// How many streaming instances were disconnected.
    disconnects: AtomicUsize,
}

impl SttDouble {
    /// Registers a provider under a name of its own. `request_response`: it answers once per
    /// upload (`BaseSTT::take_utterance`) instead of streaming.
    pub fn register(request_response: bool) -> Arc<SttDouble> {
        let double = Arc::new(SttDouble {
            provider: format!("ix-stt-{}", next_id()),
            request_response,
            result: Mutex::default(),
            error: Mutex::default(),
            uploads: Mutex::default(),
            upload_ms: AtomicU64::new(100),
            taken: AtomicUsize::new(0),
            uploaded: AtomicUsize::new(0),
            audio: Mutex::default(),
            instances: AtomicUsize::new(0),
            clip_words: Mutex::default(),
            clips_heard: AtomicUsize::new(0),
            languages: Mutex::default(),
            disconnects: AtomicUsize::new(0),
        });
        STT_DOUBLES
            .lock()
            .insert(double.provider.clone(), double.clone());
        global_registry().register_stt(
            &double.provider,
            Arc::new(|config: STTConfig| {
                let double = STT_DOUBLES.lock().get(&config.provider).cloned();
                // Only a request-response session checks its clips with its recogniser; a
                // streaming double's later instances are later sessions.
                let checker = |double: Arc<SttDouble>| {
                    double.request_response && double.instances.load(Ordering::SeqCst) > 0
                };
                if double.is_some_and(checker) {
                    return ClipStt::new(config).map(|stt| Box::new(stt) as Box<dyn BaseSTT>);
                }
                MockStt::new(config).map(|stt| Box::new(stt) as Box<dyn BaseSTT>)
            }),
            ProviderMetadata::stt(&double.provider, "Mock STT (interaction)"),
        );
        double
    }

    async fn push(&self, text: &str, is_final: bool, is_speech_final: bool) {
        let callback = self.result.lock().clone();
        let callback = callback.expect("the session registered an STT result callback");
        callback(STTResult::new(
            text.to_owned(),
            is_final,
            is_speech_final,
            0.9,
        ))
        .await;
    }

    /// An interim result: the words so far.
    pub async fn interim(&self, text: &str) {
        self.push(text, false, false).await;
    }

    /// A final piece of the segment; more may follow.
    pub async fn final_piece(&self, text: &str) {
        self.push(text, true, false).await;
    }

    /// The last piece of the segment: the vendor's own end of speech.
    pub async fn speech_final(&self, text: &str) {
        self.push(text, true, true).await;
    }

    /// A streaming error.
    pub async fn fail(&self, message: &str) {
        let callback = self.error.lock().clone();
        let callback = callback.expect("the session registered an STT error callback");
        callback(STTError::NetworkError(message.to_owned())).await;
    }

    /// The outcome of the next upload: `Ok(transcript)` or `Err(message)`.
    pub fn script_upload(&self, outcome: Result<&str, &str>) {
        let outcome = outcome.map(str::to_owned).map_err(str::to_owned);
        self.uploads.lock().push_back(outcome);
    }

    /// How often the buffered utterance was taken, and how many of those were uploaded.
    pub fn taken(&self) -> usize {
        self.taken.load(Ordering::SeqCst)
    }

    pub fn uploaded(&self) -> usize {
        self.uploaded.load(Ordering::SeqCst)
    }

    /// Every byte of audio the provider was sent, in order.
    pub fn audio(&self) -> Vec<u8> {
        self.audio.lock().clone()
    }

    /// What the recogniser hears in every listening sound the session checks.
    pub fn hear_in_clips(&self, words: &str) {
        *self.clip_words.lock() = Some(words.to_owned());
    }

    /// How many listening sounds the session had the recogniser check.
    pub fn clips_heard(&self) -> usize {
        self.clips_heard.load(Ordering::SeqCst)
    }

    /// The language of each streaming recogniser the session opened, in order.
    pub fn languages(&self) -> Vec<String> {
        self.languages.lock().clone()
    }

    /// How many streaming recognisers the session disconnected.
    pub fn disconnects(&self) -> usize {
        self.disconnects.load(Ordering::SeqCst)
    }
}

/// An instance the session made to check a listening sound: apart from the session's own.
struct ClipStt {
    double: Arc<SttDouble>,
    result: Option<STTResultCallback>,
}

struct MockStt {
    double: Arc<SttDouble>,
}

#[async_trait::async_trait]
impl BaseSTT for MockStt {
    fn new(config: STTConfig) -> Result<Self, STTError> {
        let double = STT_DOUBLES.lock().get(&config.provider).cloned();
        let double = double.ok_or_else(|| {
            STTError::ConfigurationError(format!("no STT double named {}", config.provider))
        })?;
        double.instances.fetch_add(1, Ordering::SeqCst);
        double.languages.lock().push(config.language.clone());
        Ok(MockStt { double })
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
        self.double.disconnects.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn is_ready(&self) -> bool {
        true
    }

    async fn send_audio(&mut self, audio: bytes::Bytes) -> Result<(), STTError> {
        self.double.audio.lock().extend_from_slice(&audio);
        Ok(())
    }

    async fn on_result(&mut self, callback: STTResultCallback) -> Result<(), STTError> {
        *self.double.result.lock() = Some(callback);
        Ok(())
    }

    async fn on_error(&mut self, callback: STTErrorCallback) -> Result<(), STTError> {
        *self.double.error.lock() = Some(callback);
        Ok(())
    }

    fn get_config(&self) -> Option<&STTConfig> {
        None
    }

    async fn update_config(&mut self, _config: STTConfig) -> Result<(), STTError> {
        Ok(())
    }

    fn get_provider_info(&self) -> &'static str {
        "ix-stt"
    }

    fn is_request_response(&self) -> bool {
        self.double.request_response
    }

    fn has_utterance_flush(&self) -> bool {
        self.double.request_response
    }

    /// As `OpenAISTT::take_utterance`: the upload delivers its final on the result callback; a
    /// failed one reports on the error callback and returns the error too.
    fn take_utterance(
        &mut self,
    ) -> Option<Pin<Box<dyn Future<Output = Result<(), STTError>> + Send>>> {
        if !self.double.request_response {
            return None;
        }
        self.double.taken.fetch_add(1, Ordering::SeqCst);
        let outcome = self.double.uploads.lock().pop_front()?;
        let double = self.double.clone();
        Some(Box::pin(async move {
            double.uploaded.fetch_add(1, Ordering::SeqCst);
            after(double.upload_ms.load(Ordering::SeqCst)).await;
            match outcome {
                Ok(text) => {
                    double.push(&text, true, true).await;
                    Ok(())
                }
                Err(message) => {
                    double.fail(&message).await;
                    Err(STTError::NetworkError(message))
                }
            }
        }))
    }
}

#[async_trait::async_trait]
impl BaseSTT for ClipStt {
    fn new(config: STTConfig) -> Result<Self, STTError> {
        let double = STT_DOUBLES.lock().get(&config.provider).cloned();
        let double = double.ok_or_else(|| {
            STTError::ConfigurationError(format!("no STT double named {}", config.provider))
        })?;
        Ok(ClipStt {
            double,
            result: None,
        })
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
        Ok(())
    }

    fn is_ready(&self) -> bool {
        true
    }

    async fn send_audio(&mut self, _audio: bytes::Bytes) -> Result<(), STTError> {
        Ok(())
    }

    async fn on_result(&mut self, callback: STTResultCallback) -> Result<(), STTError> {
        self.result = Some(callback);
        Ok(())
    }

    async fn on_error(&mut self, _callback: STTErrorCallback) -> Result<(), STTError> {
        Ok(())
    }

    fn get_config(&self) -> Option<&STTConfig> {
        None
    }

    async fn update_config(&mut self, _config: STTConfig) -> Result<(), STTError> {
        Ok(())
    }

    fn get_provider_info(&self) -> &'static str {
        "ix-stt-clip"
    }

    fn is_request_response(&self) -> bool {
        self.double.request_response
    }

    fn take_utterance(
        &mut self,
    ) -> Option<Pin<Box<dyn Future<Output = Result<(), STTError>> + Send>>> {
        if !self.double.request_response {
            return None;
        }
        self.double.clips_heard.fetch_add(1, Ordering::SeqCst);
        let words = self.double.clip_words.lock().clone()?;
        let callback = self.result.clone()?;
        Some(Box::pin(async move {
            callback(STTResult::new(words, true, true, 0.9)).await;
            Ok(())
        }))
    }
}

// ---- the background LLM

/// One streamed reply of the LLM double.
#[derive(Debug, Clone, Default)]
pub struct Round {
    /// The delay before each token, and the token.
    pub tokens: Vec<(u64, String)>,
    /// The tool calls the round ends with: name and arguments.
    pub tool_calls: Vec<(String, Value)>,
    /// The delay before the round ends.
    pub end_ms: u64,
}

impl Round {
    /// Text only: `first_ms` before the first token, `token_ms` before each later one.
    pub fn text(first_ms: u64, token_ms: u64, tokens: &[&str]) -> Round {
        let delays = std::iter::once(first_ms).chain(std::iter::repeat(token_ms));
        Round {
            tokens: delays
                .zip(tokens)
                .map(|(ms, token)| (ms, (*token).to_owned()))
                .collect(),
            ..Round::default()
        }
    }

    /// One tool call after `first_ms`, with no text.
    pub fn tool(first_ms: u64, name: &str, arguments: Value) -> Round {
        Round {
            tool_calls: vec![(name.to_owned(), arguments)],
            end_ms: first_ms,
            ..Round::default()
        }
    }
}

/// Chooses the round for a request body; `None` answers HTTP 500.
pub type Responder = Arc<dyn Fn(&Value) -> Option<Round> + Send + Sync>;

#[derive(Clone, Default)]
struct LlmState {
    requests: Arc<Mutex<Vec<Value>>>,
    rounds: Arc<Mutex<VecDeque<Round>>>,
    responder: Arc<Mutex<Option<Responder>>>,
}

/// An OpenAI-compatible `POST /chat/completions` that streams scripted rounds.
pub struct LlmDouble {
    pub base_url: String,
    state: LlmState,
    server: JoinHandle<()>,
}

impl LlmDouble {
    pub async fn start() -> LlmDouble {
        let state = LlmState::default();
        let app = Router::new()
            .route("/chat/completions", post(llm_chat))
            .with_state(state.clone());
        let (base_url, server) = serve(app).await;
        LlmDouble {
            base_url,
            state,
            server,
        }
    }

    /// The round for the next request (in order), unless a responder is set.
    pub fn push(&self, round: Round) {
        self.state.rounds.lock().push_back(round);
    }

    /// Chooses every round from its request instead of the queue.
    pub fn respond_with(
        &self,
        responder: impl Fn(&Value) -> Option<Round> + Send + Sync + 'static,
    ) {
        *self.state.responder.lock() = Some(Arc::new(responder));
    }

    /// The body of every request so far.
    pub fn requests(&self) -> Vec<Value> {
        self.state.requests.lock().clone()
    }
}

impl Drop for LlmDouble {
    fn drop(&mut self) {
        self.server.abort();
    }
}

/// The messages with this role that the run itself added, as text: those after its request
/// (the last user message that is not a child's `Task update`). The history before it holds
/// the earlier runs' tool rounds.
pub fn run_messages(request: &Value, role: &str) -> Vec<String> {
    let messages = request["messages"].as_array().cloned().unwrap_or_default();
    let update = |message: &Value| {
        (message["content"].as_str()).is_some_and(|text| text.starts_with("Task update"))
    };
    let start = (messages.iter())
        .rposition(|message| message["role"] == "user" && !update(message))
        .unwrap_or(0);
    let run = Value::Array(messages[start..].to_vec());
    messages_of(&run, role)
}

/// The messages of a chat request with this role, as text.
pub fn messages(request: &Value, role: &str) -> Vec<String> {
    messages_of(&request["messages"], role)
}

fn messages_of(messages: &Value, role: &str) -> Vec<String> {
    let messages = messages.as_array().cloned().unwrap_or_default();
    messages
        .iter()
        .filter(|message| message["role"] == role)
        .map(|message| message["content"].as_str().unwrap_or_default().to_owned())
        .collect()
}

fn chunk(delta: Value, finish_reason: Value) -> String {
    json!({
        "id": "chatcmpl-ix", "object": "chat.completion.chunk", "created": 1, "model": "mock",
        "choices": [{ "index": 0, "delta": delta, "finish_reason": finish_reason }],
    })
    .to_string()
}

/// A server-sent event stream of `(delay before, data)` steps.
fn sse(steps: Vec<(u64, String)>) -> Response {
    let events = stream::unfold(VecDeque::from(steps), |mut steps| async move {
        let (ms, data) = steps.pop_front()?;
        after(ms).await;
        let event = Ok::<Event, std::convert::Infallible>(Event::default().data(data));
        Some((event, steps))
    });
    Sse::new(events).into_response()
}

async fn llm_chat(State(state): State<LlmState>, Json(request): Json<Value>) -> Response {
    state.requests.lock().push(request.clone());
    let responder = state.responder.lock().clone();
    let round = match responder {
        Some(responder) => responder(&request),
        None => state.rounds.lock().pop_front(),
    };
    let Some(round) = round else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "no round is scripted").into_response();
    };
    let mut steps: Vec<(u64, String)> = round
        .tokens
        .into_iter()
        .map(|(ms, token)| (ms, chunk(json!({ "content": token }), Value::Null)))
        .collect();
    let last = if round.tool_calls.is_empty() {
        chunk(json!({ "content": "" }), json!("stop"))
    } else {
        let calls: Vec<Value> = round
            .tool_calls
            .iter()
            .enumerate()
            .map(|(index, (name, arguments))| {
                json!({
                    "index": index, "id": format!("call_{index}"), "type": "function",
                    "function": { "name": name, "arguments": arguments.to_string() },
                })
            })
            .collect();
        chunk(json!({ "tool_calls": calls }), json!("tool_calls"))
    };
    steps.push((round.end_ms, last));
    steps.push((0, "[DONE]".to_owned()));
    sse(steps)
}

// ---- the front model

/// What the front double answers a decision request with.
#[derive(Debug, Clone, Default)]
pub struct FrontScript {
    /// What the model generates after the prefill `{"act":"`, for example
    /// `delegate","say":"Let me check that."}`.
    pub reply: String,
    /// The delay before the first character, and between it and the rest.
    pub act_ms: u64,
    pub line_ms: u64,
    /// The narrator's line for a task's update (chapter 8), and how long it takes.
    pub narration: Option<String>,
    pub narration_ms: u64,
    /// The narrator's answer to a follow-up question from a task's result (chapter 8), and
    /// how long it takes.
    pub answer: Option<String>,
    pub answer_ms: u64,
    /// The slots `GET /props` reports (`total_slots`); 0: no `/props`, as a server that does
    /// not say (one slot).
    pub slots: usize,
    /// How long a warm-up (the persona's prompt into a slot's cache) takes.
    pub warm_ms: u64,
    /// With no `reply`: the reply worded from each decision's observation (a model double
    /// that reads the turn in its context, [`reads_the_turn`]).
    pub decide: Option<fn(&str) -> String>,
    /// How long a reading of the words' language takes.
    pub language_ms: u64,
}

/// A front model double for the tests that do not script the front: it reads the caller's
/// words with the events and the allowed acts of the observation, as a model would, in the
/// few shapes the tests use. It is test data, never a gateway path.
pub fn reads_the_turn(observation: &str) -> String {
    let line = |prefix: &str| {
        observation
            .lines()
            .find(|line| line.starts_with(prefix))
            .unwrap_or_default()
            .to_owned()
    };
    let allowed = line("ALLOWED:");
    let may = |act: &str| allowed.split([' ', ',']).any(|word| word == act);
    let user = line("USER");
    let words = user
        .split_once('\u{ab}')
        .map_or("", |(_, rest)| rest.trim_end_matches('\u{bb}'))
        .to_lowercase();
    let says = |phrases: &[&str]| phrases.iter().any(|phrase| words.contains(phrase));
    let task = observation
        .lines()
        .filter(|line| line.starts_with("EVENT: task ") && !line.contains("result ready"))
        .filter_map(|line| line.split_whitespace().nth(2))
        .next_back()
        .map(str::to_owned);
    let control = |kind: &str, value: Option<&str>| match value {
        Some(value) => format!(r#"control","kind":"{kind}","value":"{value}","say":""}}"#),
        None => format!(r#"control","kind":"{kind}","say":""}}"#),
    };
    let bare = words.trim_matches(|c: char| !c.is_alphanumeric());
    if user.starts_with("USER (unclear)") && may("question") {
        return r#"question","say":""}"#.to_owned();
    }
    if user.starts_with("USER (to someone else)") {
        return r#"wait","why":"side_talk","say":""}"#.to_owned();
    }
    if ["okay", "ok", "mm-hm", "mhm", "uh-huh", "yeah", "right"].contains(&bare) {
        return r#"wait","why":"continuer","say":""}"#.to_owned();
    }
    if may("control") {
        let stop = says(&["stop talking", "top talking", "enough"]) || bare == "stop";
        let language = [("spanish", "es"), ("chinese", "zh"), ("german", "de")]
            .into_iter()
            .find(|(name, _)| says(&[&format!("in {name}")]));
        let found = if stop && !says(&["stop by"]) {
            Some(control("stop", None))
        } else if says(&[
            "hold on",
            "give me a second",
            "wait a second",
            "wait a minute",
        ]) {
            Some(control("hold", None))
        } else if says(&["go on", "carry on", "continue"]) {
            Some(control("resume", None))
        } else if says(&["the number"]) {
            Some(control("repeat", Some("number")))
        } else if says(&["again, slower", "again more slowly"]) {
            Some(control("repeat", Some("slower")))
        } else if says(&["again, louder"]) {
            Some(control("repeat", Some("louder")))
        } else if says(&["a person", "a human"]) {
            Some(control("human", None))
        } else if says(&["male voice"]) && !says(&["female voice"]) {
            Some(control("voice", Some("male")))
        } else if says(&["british voice"]) {
            Some(control("voice", Some("british")))
        } else if says(&["say that again", "repeat that"]) {
            Some(control("repeat", None))
        } else if says(&["slower", "slow down"]) {
            Some(control("rate", Some("slower")))
        } else if says(&["faster"]) {
            Some(control("rate", Some("faster")))
        } else if says(&["louder"]) {
            Some(control("volume", Some("louder")))
        } else if says(&["keep it short"]) {
            Some(control("verbosity", Some("brief")))
        } else if says(&["what do you mean"]) {
            Some(control("clarify", None))
        } else {
            language.map(|(_, code)| control("language", Some(code)))
        };
        if let Some(found) = found {
            return found;
        }
    }
    // About a finished task: a question about it, or more work on it.
    let finished = observation
        .lines()
        .filter(|line| line.starts_with("EVENT: task ") && line.contains("result ready"))
        .filter_map(|line| line.split_whitespace().nth(2))
        .next_back();
    if let Some(done) = finished.filter(|_| may("follow_up")) {
        let about = [" it", "the flight", "seat", "leave", "near it"];
        if about.iter().any(|word| words.contains(word)) {
            return format!(r#"follow_up","task":"{done}","say":""}}"#);
        }
    }
    if let Some(task) = task.as_deref() {
        // Words that go on with the request just made.
        if may("update") && words.starts_with("and ") {
            return format!(r#"update","task":"{task}","say":""}}"#);
        }
        if may("abort") && says(&["cancel", "forget it", "never mind", "stop it"]) {
            return format!(r#"abort","task":"{task}","say":""}}"#);
        }
        if may("progress")
            && says(&[
                "how is it going",
                "how's it going",
                "is it done",
                "any news",
            ])
        {
            return format!(r#"progress","task":"{task}","say":"Still working on it."}}"#);
        }
        if may("update") && says(&["actually", "make it", "make that", "instead"]) {
            return format!(r#"update","task":"{task}","say":""}}"#);
        }
    }
    r#"delegate","say":""}"#.to_owned()
}

/// How the gateway's reading of the words' language begins (`front_model::LANGUAGE_SYSTEM`).
const LANGUAGE_READING: &str = "You read one turn of a caller on a phone call held in";
/// How the reading of a request beside a task just started begins (`STARTED_SYSTEM`).
const STARTED_READING: &str = "A caller on a phone call asked for something a moment ago";
/// How the reading of a repeat of a line's number begins (`front_model::NUMBER_SYSTEM`).
const NUMBER_READING: &str = "On a phone call the agent just said a line with a number";

/// Whether a reading shows examples before the caller's words (they are in its last message).
fn with_examples(system: &str) -> bool {
    [LANGUAGE_READING, STARTED_READING, NUMBER_READING]
        .iter()
        .any(|start| system.starts_with(start))
}

/// A front model double's reading of a caller's reply or repeated request (the gateway's
/// `PolicyAction::Read`): the label it means, read with a few phrases, as a model would in
/// the shapes the tests use. Test data, never a gateway path.
pub fn reads_the_reply(system: &str, user: &str) -> String {
    let quoted = |prefix: &str| {
        user.lines()
            .find(|line| line.starts_with(prefix))
            .and_then(|line| line.split_once('\u{ab}'))
            .map_or(String::new(), |(_, rest)| {
                rest.trim_end_matches('\u{bb}').to_lowercase()
            })
    };
    // The language of the words (the gateway's language reading, owner 24 Oct), in the one
    // shape the tests use and as the reading asks, the caller's first word and then the
    // language's name: Spanish request words are Spanish, anything else the pack's English.
    if system.starts_with(LANGUAGE_READING) {
        let said = quoted("Caller:");
        let spanish = ["quiero ", "necesito "]
            .iter()
            .any(|word| said.contains(word));
        let first = said.split_whitespace().next().unwrap_or("hm");
        let name = if spanish { "Spanish" } else { "English" };
        return format!("\u{ab}{first}\u{bb} {name}");
    }
    // A request beside a task just started: about it when it says "its".
    if system.starts_with(STARTED_READING) {
        let said = quoted("New request:");
        let about = said
            .split(|c: char| !c.is_alphanumeric())
            .any(|w| w == "its");
        return if about { "1" } else { "new" }.to_owned();
    }
    // A repeat of the number in the agent's line: the number when the caller names it.
    if system.starts_with(NUMBER_READING) {
        let said = quoted("Caller:");
        let named = said.contains("number") || said.contains("code");
        return if named { "number" } else { "line" }.to_owned();
    }
    let repeat = system.starts_with("A caller on a phone call already asked");
    let said = quoted(if repeat { "New request:" } else { "Caller:" });
    let labels: Vec<&str> = user
        .split("Labels:\n")
        .nth(1)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.split_once(": ").map(|(label, _)| label))
        .collect();
    let has = |label: &str| labels.contains(&label);
    let words: Vec<&str> = said
        .split(|c: char| !(c.is_alphanumeric() || c == '\''))
        .filter(|word| !word.is_empty())
        .collect();
    let says = |phrases: &[&str]| {
        phrases.iter().any(|phrase| {
            let phrase: Vec<&str> = phrase.split(' ').collect();
            words
                .windows(phrase.len())
                .any(|window| window == phrase.as_slice())
        })
    };
    if repeat {
        // The same request again: two words of four letters or more of a task's title.
        let tasks = user.lines().filter_map(|line| {
            let (number, title) = line.split_once(": \u{ab}")?;
            Some((
                number.to_owned(),
                title.trim_end_matches('\u{bb}').to_lowercase(),
            ))
        });
        for (number, title) in tasks.filter(|(number, _)| number.parse::<usize>().is_ok()) {
            let shared = words
                .iter()
                .filter(|word| word.len() >= 4 && title.split(' ').any(|t| t == **word))
                .count();
            if shared >= 2 {
                return number;
            }
        }
        return "new".to_owned();
    }
    let cancel = says(&["cancel", "never mind", "forget it"]);
    let no = says(&["no", "nope", "don't", "not now", "later"]);
    let yes = says(&[
        "yes",
        "yeah",
        "go ahead",
        "please do",
        "do it",
        "book it",
        "sure",
    ]);
    let okay = says(&["okay", "ok", "fine"]);
    let label = if has("answer") {
        if cancel { "cancel" } else { "answer" }
    } else if has("1") {
        // A choice: the option the caller names, or its number.
        let options = user.lines().filter_map(|line| {
            let (number, option) = line.split_once(": ")?;
            number.parse::<usize>().ok()?;
            Some((number, option.to_lowercase()))
        });
        let ordinals = ["first", "second", "third", "fourth"];
        for (number, option) in options {
            let at: usize = number.parse().unwrap_or(0);
            if said.contains(&option) || says(&[ordinals.get(at - 1).copied().unwrap_or("-")]) {
                return number.to_owned();
            }
        }
        if cancel {
            "cancel"
        } else if says(&["no preference", "either", "any of them"]) {
            "any"
        } else if says(&["neither", "none"]) {
            "none"
        } else {
            "other"
        }
    } else if has("second") {
        if says(&["second", "another", "both", "as well", "one more"]) {
            "second"
        } else if says(&["change", "instead", "same one", "replace"]) {
            "change"
        } else if cancel || no {
            "neither"
        } else {
            "other"
        }
    } else if has("stop") {
        if says(&["stop", "cancel", "forget"]) {
            "stop"
        } else if says(&["wait", "keep", "fine", "okay", "yes", "sure", "no rush"]) {
            "keep"
        } else {
            "other"
        }
    } else if cancel && has("cancel") {
        "cancel"
    } else if has("change") && says(&["make it", "instead", "actually", "change"]) {
        "change"
    } else if no && !yes {
        "no"
    } else if yes {
        "yes"
    } else if okay && has("okay") {
        "okay"
    } else if okay {
        "yes"
    } else {
        "other"
    };
    label.to_owned()
}

#[derive(Clone, Default)]
struct FrontState {
    requests: Arc<Mutex<Vec<Value>>>,
    script: Arc<Mutex<FrontScript>>,
    /// Whether an answer from a stored result is being worded, and the requests that came in
    /// meanwhile.
    answering: Arc<Mutex<bool>>,
    during_answer: Arc<Mutex<Vec<Value>>>,
    /// Warm-ups the gateway waited for to the end.
    warmed: Arc<AtomicUsize>,
    /// Language readings the gateway waited for to the end.
    languages_read: Arc<AtomicUsize>,
}

/// The front model's llama-server: `POST /v1/chat/completions`. A streamed request is a
/// decision; the warm-up (not streamed) is answered at once.
pub struct FrontDouble {
    pub url: String,
    state: FrontState,
    server: Option<JoinHandle<()>>,
}

impl FrontDouble {
    pub async fn start(script: FrontScript) -> FrontDouble {
        let state = FrontState::default();
        *state.script.lock() = script;
        let app = Router::new()
            .route("/v1/chat/completions", post(front_chat))
            .route(
                "/health",
                axum::routing::get(|| async { Json(json!({ "status": "ok" })) }),
            )
            .route("/props", axum::routing::get(front_props))
            .with_state(state.clone());
        let (url, server) = serve(app).await;
        FrontDouble {
            url,
            state,
            server: Some(server),
        }
    }

    /// A front model that is down: its port refuses connections.
    pub async fn refusing() -> FrontDouble {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let url = format!("http://{}", listener.local_addr().expect("address"));
        drop(listener);
        FrontDouble {
            url,
            state: FrontState::default(),
            server: None,
        }
    }

    pub fn script(&self, script: FrontScript) {
        *self.state.script.lock() = script;
    }

    /// The bodies of the decision requests so far (the warm-up is not one).
    pub fn decisions(&self) -> Vec<Value> {
        let requests = self.state.requests.lock();
        let decision = |request: &&Value| request["stream"] == true;
        requests.iter().filter(decision).cloned().collect()
    }

    /// The bodies of the narrator's requests so far.
    pub fn narrations(&self) -> Vec<Value> {
        let requests = self.state.requests.lock();
        let narrator = |request: &&Value| {
            let system = request["messages"][0]["content"]
                .as_str()
                .unwrap_or_default();
            request["stream"] != true && system.starts_with("You tell a caller")
        };
        requests.iter().filter(narrator).cloned().collect()
    }

    /// Every request so far, the warm-up included.
    pub fn requests(&self) -> usize {
        self.state.requests.lock().len()
    }

    /// Every request's body so far, in the order they came.
    pub fn bodies(&self) -> Vec<Value> {
        self.state.requests.lock().clone()
    }

    /// The warm-ups that ran to their end (a warm-up the gateway dropped did not).
    pub fn warmed(&self) -> usize {
        self.state.warmed.load(Ordering::SeqCst)
    }

    /// The language readings asked for, and those that ran to their end (one the gateway
    /// dropped did not).
    pub fn languages_read(&self) -> (usize, usize) {
        let requests = self.state.requests.lock();
        let asked = requests
            .iter()
            .filter(|request| {
                let system = request["messages"][0]["content"].as_str();
                system.is_some_and(|system| system.starts_with(LANGUAGE_READING))
            })
            .count();
        (asked, self.state.languages_read.load(Ordering::SeqCst))
    }

    /// The requests that came in while an answer from a stored result was being worded.
    pub fn during_answer(&self) -> Vec<Value> {
        self.state.during_answer.lock().clone()
    }
}

async fn front_props(State(state): State<FrontState>) -> Response {
    match state.script.lock().slots {
        0 => StatusCode::NOT_FOUND.into_response(),
        slots => Json(json!({ "total_slots": slots })).into_response(),
    }
}

impl Drop for FrontDouble {
    fn drop(&mut self) {
        if let Some(server) = &self.server {
            server.abort();
        }
    }
}

async fn front_chat(State(state): State<FrontState>, Json(request): Json<Value>) -> Response {
    state.requests.lock().push(request.clone());
    if *state.answering.lock() {
        state.during_answer.lock().push(request.clone());
    }
    let script = state.script.lock().clone();
    if request["stream"] != true {
        // The narrator's request, or the warm-up.
        let system = request["messages"][0]["content"]
            .as_str()
            .unwrap_or_default();
        let narrator = system.starts_with("You tell a caller");
        let answers = system.starts_with("You answer a caller's question");
        let reads = system.starts_with("You read one reply on a phone call")
            || system.starts_with("A caller on a phone call already asked")
            || with_examples(system);
        // The caller's words are in the last message (a reading may show examples before it).
        let messages = request["messages"].as_array().cloned().unwrap_or_default();
        let user = messages
            .get(1)
            .filter(|_| !with_examples(system))
            .or(messages.last())
            .and_then(|message| message["content"].as_str())
            .unwrap_or_default()
            .to_owned();
        let user = user.as_str();
        // The warm-up: the persona's prompt and the call header, one token.
        let warm_up =
            request["max_tokens"] == 1 && request["messages"].as_array().map(Vec::len) == Some(2);
        let content = match (&script.narration, &script.answer) {
            _ if warm_up => {
                tokio::time::sleep(Duration::from_millis(script.warm_ms)).await;
                state.warmed.fetch_add(1, Ordering::SeqCst);
                String::new()
            }
            _ if system.starts_with(LANGUAGE_READING) => {
                tokio::time::sleep(Duration::from_millis(script.language_ms)).await;
                state.languages_read.fetch_add(1, Ordering::SeqCst);
                reads_the_reply(system, user)
            }
            _ if reads => reads_the_reply(system, user),
            (Some(line), _) if narrator => {
                tokio::time::sleep(Duration::from_millis(script.narration_ms)).await;
                line.clone()
            }
            (_, Some(line)) if answers => {
                *state.answering.lock() = true;
                tokio::time::sleep(Duration::from_millis(script.answer_ms)).await;
                *state.answering.lock() = false;
                line.clone()
            }
            // The context-reading double words an update as the result's first sentence.
            (None, _) if narrator && script.decide.is_some() => {
                let user = request["messages"][1]["content"]
                    .as_str()
                    .unwrap_or_default();
                let result = user.split("Result: \u{ab}").nth(1).unwrap_or_default();
                let result = result.split('\u{bb}').next().unwrap_or_default();
                let end = result.find(". ").map_or(result.len(), |at| at + 1);
                result[..end].trim().to_owned()
            }
            _ => String::new(),
        };
        let message = json!({ "role": "assistant", "content": content });
        return Json(json!({ "choices": [{ "index": 0, "message": message }] })).into_response();
    }
    // The observation: over the agent its lines before the caller's words are a message of
    // their own (`build_messages`), so every user message after the call header is read.
    let observation: Vec<&str> = request["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .skip(3)
        .filter(|message| message["role"] == "user")
        .filter_map(|message| message["content"].as_str())
        .collect();
    let observation = observation.join("\n");
    let reply = match script.decide {
        Some(decide) if script.reply.is_empty() => decide(&observation),
        _ => script.reply.clone(),
    };
    let mut reply = reply.chars();
    let first: String = reply.by_ref().take(1).collect();
    let rest: String = reply.collect();
    sse(vec![
        (
            script.act_ms,
            chunk(json!({ "content": first }), Value::Null),
        ),
        (
            script.line_ms,
            chunk(json!({ "content": rest }), json!("stop")),
        ),
        (0, "[DONE]".to_owned()),
    ])
}

// ---- a tool webhook

/// One call the tool double received.
#[derive(Debug, Clone)]
pub struct ToolCall {
    /// The body as it was sent, and parsed.
    pub raw: String,
    pub body: Value,
    /// Header names in lower case.
    pub headers: HashMap<String, String>,
}

impl ToolCall {
    /// Whether `X-WaaV-Signature` is the HMAC-SHA256 of `"<timestamp>.<body>"` under `secret`.
    pub fn signed_with(&self, secret: &str) -> bool {
        let header = |name: &str| self.headers.get(name).cloned().unwrap_or_default();
        header("x-waav-signature") == sign(secret, &header("x-waav-timestamp"), &self.raw)
    }
}

/// The `X-WaaV-Signature` of `body` sent at `timestamp` under `secret`.
pub fn sign(secret: &str, timestamp: &str, body: &str) -> String {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(secret.as_bytes()).expect("any key");
    mac.update(format!("{timestamp}.{body}").as_bytes());
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

#[derive(Clone)]
struct ToolState {
    calls: Arc<Mutex<Vec<ToolCall>>>,
    reply: Arc<Mutex<Value>>,
    delay_ms: Arc<AtomicU64>,
    fail: Arc<AtomicBool>,
}

/// A tool webhook: `POST <url>` records the call and answers with the scripted JSON.
pub struct ToolDouble {
    pub url: String,
    state: ToolState,
    server: JoinHandle<()>,
}

impl ToolDouble {
    /// `reply` is the 2xx body; its string field `effect` labels what the call committed.
    pub async fn start(reply: Value) -> ToolDouble {
        let state = ToolState {
            calls: Arc::default(),
            reply: Arc::new(Mutex::new(reply)),
            delay_ms: Arc::default(),
            fail: Arc::default(),
        };
        let app = Router::new()
            .route("/tool", post(tool_call))
            .with_state(state.clone());
        let (address, server) = serve(app).await;
        ToolDouble {
            url: format!("{address}/tool"),
            state,
            server,
        }
    }

    pub fn calls(&self) -> Vec<ToolCall> {
        self.state.calls.lock().clone()
    }

    /// How long the tool takes to answer.
    pub fn delay(&self, ms: u64) {
        self.state.delay_ms.store(ms, Ordering::SeqCst);
    }

    /// Answer HTTP 500 from now on.
    pub fn fail(&self, on: bool) {
        self.state.fail.store(on, Ordering::SeqCst);
    }
}

impl Drop for ToolDouble {
    fn drop(&mut self) {
        self.server.abort();
    }
}

async fn tool_call(State(state): State<ToolState>, headers: HeaderMap, raw: String) -> Response {
    let headers = headers
        .iter()
        .map(|(name, value)| {
            let value = value.to_str().unwrap_or_default().to_owned();
            (name.as_str().to_lowercase(), value)
        })
        .collect();
    state.calls.lock().push(ToolCall {
        body: serde_json::from_str(&raw).unwrap_or_default(),
        raw,
        headers,
    });
    after(state.delay_ms.load(Ordering::SeqCst)).await;
    if state.fail.load(Ordering::SeqCst) {
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed by the test").into_response();
    }
    Json(state.reply.lock().clone()).into_response()
}

// ---- any HTTP backend

/// One request a backend double received.
#[derive(Debug, Clone)]
pub struct Hit {
    pub method: String,
    /// The path with its query.
    pub path: String,
    pub call: ToolCall,
}

type HitResponder = Arc<dyn Fn(&Hit) -> (u16, String) + Send + Sync>;

/// An HTTP backend: every request under `url` is recorded and answered by the test's
/// closure with a status and a body (JSON, or server-sent events when it starts with
/// `data:` or `event:`).
pub struct BackendDouble {
    pub url: String,
    hits: Arc<Mutex<Vec<Hit>>>,
    server: JoinHandle<()>,
}

impl BackendDouble {
    pub async fn start(
        responder: impl Fn(&Hit) -> (u16, String) + Send + Sync + 'static,
    ) -> BackendDouble {
        let hits: Arc<Mutex<Vec<Hit>>> = Arc::default();
        let state: (Arc<Mutex<Vec<Hit>>>, HitResponder) = (hits.clone(), Arc::new(responder));
        let app = Router::new().fallback(backend_hit).with_state(state);
        let (url, server) = serve(app).await;
        BackendDouble { url, hits, server }
    }

    pub fn hits(&self) -> Vec<Hit> {
        self.hits.lock().clone()
    }

    /// The parsed bodies of the requests, in order.
    pub fn bodies(&self) -> Vec<Value> {
        self.hits().into_iter().map(|hit| hit.call.body).collect()
    }
}

impl Drop for BackendDouble {
    fn drop(&mut self) {
        self.server.abort();
    }
}

async fn backend_hit(
    State((hits, responder)): State<(Arc<Mutex<Vec<Hit>>>, HitResponder)>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    raw: String,
) -> Response {
    let headers = headers
        .iter()
        .map(|(name, value)| {
            let value = value.to_str().unwrap_or_default().to_owned();
            (name.as_str().to_lowercase(), value)
        })
        .collect();
    let hit = Hit {
        method: method.to_string(),
        path: uri
            .path_and_query()
            .map(|p| p.to_string())
            .unwrap_or_default(),
        call: ToolCall {
            body: serde_json::from_str(&raw).unwrap_or_default(),
            raw,
            headers,
        },
    };
    hits.lock().push(hit.clone());
    let (status, body) = responder(&hit);
    let kind = if body.starts_with("data:") || body.starts_with("event:") {
        "text/event-stream"
    } else {
        "application/json"
    };
    let status = StatusCode::from_u16(status).expect("an HTTP status");
    (status, [("content-type", kind)], body).into_response()
}

// ---- a speech-to-speech vendor

/// The test's choice of what the vendor says for a gateway line: from the words it was asked
/// to say, the words it says.
pub type Speaker = Arc<dyn Fn(&str) -> String + Send + Sync>;

#[derive(Clone, Default)]
struct VendorState {
    received: Arc<Mutex<Vec<Value>>>,
    out: Arc<Mutex<Option<UnboundedSender<String>>>>,
    speaker: Arc<Mutex<Option<Speaker>>>,
    /// What it says for a `response.create` with no instructions, in order.
    replies: Arc<Mutex<VecDeque<String>>>,
    responses: Arc<AtomicU64>,
}

/// A speech-to-speech vendor speaking the OpenAI Realtime GA wire on a WebSocket: it records
/// every client event, sends what the test scripts, and answers a `response.create` itself:
/// a gateway line ("... word for word, and nothing else: <words>") with the speaker's words
/// (the words themselves by default), a bare one with the next scripted reply.
pub struct VendorDouble {
    /// `ws://127.0.0.1:<port>`.
    pub url: String,
    state: VendorState,
    server: JoinHandle<()>,
}

/// The marker of a gateway line in `response.create` instructions.
pub const LINE_MARKER: &str = "word for word, and nothing else: ";

impl VendorDouble {
    pub async fn start() -> VendorDouble {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let url = format!("ws://{}", listener.local_addr().expect("address"));
        let state = VendorState::default();
        let shared = state.clone();
        let server = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let _ = stream.set_nodelay(true);
                // The OpenAI client asks for the `realtime` subprotocol and needs it back.
                let echo = |request: &tokio_tungstenite::tungstenite::handshake::server::Request,
                            mut response: tokio_tungstenite::tungstenite::handshake::server::Response| {
                    let asked = request.headers().get("sec-websocket-protocol").cloned();
                    if let Some(asked) = asked {
                        response.headers_mut().insert("sec-websocket-protocol", asked);
                    }
                    Ok(response)
                };
                let Ok(socket) = tokio_tungstenite::accept_hdr_async(stream, echo).await else {
                    continue;
                };
                tokio::spawn(vendor_connection(socket, shared.clone()));
            }
        });
        VendorDouble { url, state, server }
    }

    pub fn received(&self) -> Vec<Value> {
        self.state.received.lock().clone()
    }

    /// The received events of one type.
    pub fn of(&self, kind: &str) -> Vec<Value> {
        self.received()
            .into_iter()
            .filter(|event| event["type"] == kind)
            .collect()
    }

    /// Send events on the open connection.
    pub fn send(&self, events: Vec<Value>) {
        if let Some(out) = self.state.out.lock().as_ref() {
            for event in events {
                let _ = out.send(event.to_string());
            }
        }
    }

    pub fn speak_with(&self, speaker: impl Fn(&str) -> String + Send + Sync + 'static) {
        *self.state.speaker.lock() = Some(Arc::new(speaker));
    }

    pub fn reply(&self, text: &str) {
        self.state.replies.lock().push_back(text.to_owned());
    }

    /// Waits for the `count`-th received event that matches; fails after ten seconds.
    pub async fn until(&self, what: &str, count: usize, matches: impl Fn(&Value) -> bool) -> Value {
        let arrived = async {
            loop {
                if let Some(event) = self.received().into_iter().filter(&matches).nth(count - 1) {
                    return event;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        };
        match tokio::time::timeout(Duration::from_secs(10), arrived).await {
            Ok(event) => event,
            Err(_) => panic!("the vendor got no {what} #{count}: {:#?}", self.received()),
        }
    }
}

impl Drop for VendorDouble {
    fn drop(&mut self) {
        self.server.abort();
    }
}

async fn vendor_connection(
    socket: tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    state: VendorState,
) {
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let (mut write, mut read) = socket.split();
    let (out, mut queued) = mpsc::unbounded_channel::<String>();
    *state.out.lock() = Some(out.clone());
    let writer = tokio::spawn(async move {
        while let Some(text) = queued.recv().await {
            if write.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });
    while let Some(Ok(message)) = read.next().await {
        let Message::Text(text) = message else {
            continue;
        };
        let Ok(event) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        state.received.lock().push(event.clone());
        if event["type"] != "response.create" {
            continue;
        }
        let instructions = event["response"]["instructions"]
            .as_str()
            .unwrap_or_default();
        let words = match instructions.split_once(LINE_MARKER) {
            Some((_, words)) => {
                let speaker = state.speaker.lock().clone();
                speaker.map_or_else(|| words.to_owned(), |speak| speak(words))
            }
            None => state.replies.lock().pop_front().unwrap_or_default(),
        };
        let n = state.responses.fetch_add(1, Ordering::Relaxed);
        for reply in ga::says(&format!("resp_{n}"), &words) {
            let _ = out.send(reply.to_string());
        }
    }
    writer.abort();
}

/// Server events of the OpenAI Realtime GA wire.
pub mod ga {
    use base64::Engine;
    use serde_json::{Value, json};

    fn response(id: &str, status: &str) -> Value {
        json!({"id": id, "object": "realtime.response", "status": status, "output": []})
    }

    /// One response: a function call.
    pub fn call(id: &str, call_id: &str, name: &str, arguments: Value) -> Vec<Value> {
        let item = format!("item_{id}");
        vec![
            json!({"type": "response.created", "response": response(id, "in_progress")}),
            json!({"type": "response.output_item.added", "response_id": id, "output_index": 0,
                   "item": {"id": item, "type": "function_call", "call_id": call_id, "name": name}}),
            json!({"type": "response.function_call_arguments.done", "response_id": id,
                   "item_id": item, "output_index": 0, "call_id": call_id,
                   "arguments": arguments.to_string()}),
            json!({"type": "response.done", "response": response(id, "completed")}),
        ]
    }

    /// One response that says these words: 100 ms of audio and the transcript. No words: an
    /// empty response.
    pub fn says(id: &str, words: &str) -> Vec<Value> {
        let item = format!("item_{id}");
        let mut out =
            vec![json!({"type": "response.created", "response": response(id, "in_progress")})];
        if !words.is_empty() {
            let audio = base64::engine::general_purpose::STANDARD.encode(vec![0u8; 4_800]);
            out.push(json!({"type": "response.output_audio.delta", "response_id": id,
                            "item_id": item, "output_index": 0, "content_index": 0, "delta": audio}));
            out.push(
                json!({"type": "response.output_audio_transcript.done", "response_id": id,
                            "item_id": item, "output_index": 0, "content_index": 0,
                            "transcript": words}),
            );
        }
        out.push(json!({"type": "response.done", "response": response(id, "completed")}));
        out
    }

    /// The caller's words, transcribed.
    pub fn heard(words: &str) -> Value {
        json!({"type": "conversation.item.input_audio_transcription.completed",
               "item_id": "item_user", "content_index": 0, "transcript": words})
    }
}
