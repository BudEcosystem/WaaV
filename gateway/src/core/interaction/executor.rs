//! The verb executor of a speech-to-speech session in interaction mode (plan section 9.2): one
//! task per provider session turns the gateway's verbs into the provider's wire, behind a
//! cloneable handle, so a caller's audio frame never waits for a lock on the provider.
//!
//! It keeps an ordered outbox of what could not be sent while the connection was down and
//! sends it, in order, once the provider is back; one speech slot (a gateway line goes out
//! only when the provider is not speaking, and the next waits for its response to end);
//! connection epochs (a tool result for a call made on an earlier connection takes a
//! call-free path: context, then a reply); and an output gate that holds the provider's
//! audio when it must stop and has no wire to cancel (at most [`GATE_MAX`]).

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures::future::BoxFuture;
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;
use tracing::{debug, warn};

use crate::core::realtime::{
    AudioOutputCallback, BaseRealtime, ConnectionState, FunctionCallCallback, RealtimeConfig,
    RealtimeError, RealtimeErrorCallback, RealtimeResponseOverride, RealtimeResult, RealtimeVerb,
    ReconnectionCallback, ReplayConversationItem, ResponseDoneCallback, SpeechEventCallback,
    TranscriptCallback,
};

use super::actor::Msg;

/// The outbox holds at most this many sends; the oldest is dropped beyond it.
pub const OUTBOX_MAX: usize = 32;
/// The longest the output gate holds the provider's audio.
pub const GATE_MAX: Duration = Duration::from_secs(8);
/// A gateway line whose response never ends frees the slot after this long.
const SLOT_MAX: Duration = Duration::from_secs(20);

/// What the gateway asks of the provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verb {
    SayExact(String),
    SayGuided(String),
    AddContext(String),
    StopOutput,
}

impl Verb {
    pub(crate) fn realtime(&self) -> RealtimeVerb<'_> {
        match self {
            Verb::SayExact(text) => RealtimeVerb::SayExact(text),
            Verb::SayGuided(text) => RealtimeVerb::SayGuided(text),
            Verb::AddContext(text) => RealtimeVerb::AddContext(text),
            Verb::StopOutput => RealtimeVerb::StopOutput,
        }
    }
}

/// The provider connection as the executor uses it. `/realtime` implements it over its
/// provider; the `/v1/realtime` relay over GA frames to the vendor.
#[async_trait]
pub trait Wire: Send + Sync {
    /// `InvalidConfiguration`: the provider has no wire for the verb. Any other error: the
    /// connection is down.
    async fn verb(&mut self, verb: &Verb) -> RealtimeResult<()>;
    /// A function call's output, then a response when `reply`.
    async fn tool_result(&mut self, call_id: &str, output: &str, reply: bool)
    -> RealtimeResult<()>;
    /// A response, with no instructions.
    async fn reply(&mut self) -> RealtimeResult<()>;
    /// The provider itself, for the session's own operations (`/realtime` only).
    fn provider(&mut self) -> Option<&mut dyn BaseRealtime> {
        None
    }
}

#[async_trait]
impl Wire for Box<dyn BaseRealtime> {
    async fn verb(&mut self, verb: &Verb) -> RealtimeResult<()> {
        self.apply_verb(verb.realtime()).await
    }

    async fn tool_result(
        &mut self,
        call_id: &str,
        output: &str,
        reply: bool,
    ) -> RealtimeResult<()> {
        self.submit_function_result(call_id, output).await?;
        if reply {
            self.create_response().await?;
        }
        Ok(())
    }

    async fn reply(&mut self) -> RealtimeResult<()> {
        self.create_response().await
    }

    fn provider(&mut self) -> Option<&mut dyn BaseRealtime> {
        Some(self.as_mut())
    }
}

/// What the provider did, as the executor needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// This many bytes of the provider's audio went to the caller.
    Audio(usize),
    /// The provider is in a response of its own (it speaks, or calls a function): no gateway
    /// line goes out until that response ends.
    Responding,
    ResponseDone,
    Reconnected,
    /// The provider says it is still working (`true`) or idle (`false`) apart from its
    /// responses (Gemini `interactionStatus`, provider fix `gemini_interaction_status`): no
    /// gateway line goes out while it works, even after its response ended.
    Working(bool),
    /// The provider ignored the line just sent because it is in a reply of its own (Deepgram
    /// `INJECT_AGENT_MESSAGE_DURING_AGENT_SPEECH`): the line goes out again when that reply
    /// ends.
    Refused,
}

/// Holds the provider's audio while a stop is in force on a provider that cannot cancel.
#[derive(Debug)]
pub struct OutputGate {
    base: std::time::Instant,
    /// Milliseconds after `base` until which audio is held; 0 = open.
    until: AtomicU64,
}

impl Default for OutputGate {
    fn default() -> Self {
        Self {
            base: std::time::Instant::now(),
            until: AtomicU64::new(0),
        }
    }
}

impl OutputGate {
    fn now_ms(&self) -> u64 {
        u64::try_from(self.base.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// Whether the provider's audio may go to the caller now.
    pub fn pass(&self) -> bool {
        self.now_ms() >= self.until.load(Ordering::Relaxed)
    }

    fn close(&self) {
        let until = self.now_ms() + u64::try_from(GATE_MAX.as_millis()).unwrap_or(8_000);
        self.until.store(until, Ordering::Relaxed);
    }

    fn open(&self) {
        self.until.store(0, Ordering::Relaxed);
    }

    /// Hold the provider's audio for `ms` (at most [`GATE_MAX`]): a gateway clip plays.
    pub fn hold(&self, ms: u64) {
        let max = u64::try_from(GATE_MAX.as_millis()).unwrap_or(8_000);
        let until = self.now_ms() + ms.min(max);
        self.until.fetch_max(until, Ordering::Relaxed);
    }
}

/// The provider's audio as the executor counts it: when its last chunk came, and until when
/// it plays at the caller. The session's health watcher reads it (chapter 10). Written for
/// every chunk, so no lock: milliseconds after `base`, plus one (0: no audio yet).
#[derive(Debug)]
pub struct AudioClock {
    base: Instant,
    last: AtomicU64,
    until: AtomicU64,
}

impl Default for AudioClock {
    fn default() -> Self {
        Self {
            base: Instant::now(),
            last: AtomicU64::new(0),
            until: AtomicU64::new(0),
        }
    }
}

impl AudioClock {
    fn ms(&self, at: Instant) -> u64 {
        let ms = at.saturating_duration_since(self.base).as_millis();
        u64::try_from(ms).unwrap_or(u64::MAX - 1) + 1
    }

    /// When the provider's last chunk of audio came.
    pub fn last(&self) -> Option<Instant> {
        let last = self.last.load(Ordering::Relaxed);
        (last > 0).then(|| self.base + Duration::from_millis(last - 1))
    }

    /// Whether the provider's audio still plays at the caller.
    pub fn audible(&self) -> bool {
        self.until.load(Ordering::Relaxed) > self.ms(Instant::now())
    }

    fn note(&self, until: Instant) {
        self.last.store(self.ms(Instant::now()), Ordering::Relaxed);
        self.until.store(self.ms(until), Ordering::Relaxed);
    }
}

type Op = Box<dyn for<'a> FnOnce(&'a mut dyn BaseRealtime) -> BoxFuture<'a, ()> + Send>;

enum Cmd {
    Say {
        line: u64,
        verb: Verb,
    },
    Send(Verb),
    ToolResult {
        call_id: String,
        epoch: u64,
        output: String,
        reply: bool,
    },
    Signal(Signal),
    Op(Op),
    /// The session is over: the executor ends and lets go of the provider.
    Close,
}

/// The executor's handle: cheap to clone, never blocks.
#[derive(Clone)]
pub struct ExecHandle {
    tx: mpsc::UnboundedSender<Cmd>,
    epoch: Arc<AtomicU64>,
    connected: Arc<AtomicBool>,
    gate: Arc<OutputGate>,
    clock: Arc<AudioClock>,
}

impl ExecHandle {
    /// Speak a gateway line through the speech slot. The session hears `LineSent(line)` when
    /// it went out and `LineEnded(line, spoken)` when its response ended (`spoken: false`:
    /// the provider has no wire for the verb).
    pub fn say(&self, line: u64, verb: Verb) {
        let _ = self.tx.send(Cmd::Say { line, verb });
    }

    /// Context or a stop: sent now, in order.
    pub fn send(&self, verb: Verb) {
        let _ = self.tx.send(Cmd::Send(verb));
    }

    /// A function call's output. `epoch` is the connection the call came on.
    pub fn tool_result(&self, call_id: String, epoch: u64, output: String, reply: bool) {
        let _ = self.tx.send(Cmd::ToolResult {
            call_id,
            epoch,
            output,
            reply,
        });
    }

    pub fn signal(&self, signal: Signal) {
        let _ = self.tx.send(Cmd::Signal(signal));
    }

    /// The connection the provider is on now (starts at 0, one more per reconnect).
    pub fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::Relaxed)
    }

    pub fn gate(&self) -> Arc<OutputGate> {
        self.gate.clone()
    }

    /// The provider's audio so far.
    pub fn clock(&self) -> &AudioClock {
        &self.clock
    }

    /// End the executor: the provider (and the callbacks it holds) is dropped.
    pub fn close(&self) {
        let _ = self.tx.send(Cmd::Close);
    }

    /// Run one of the session's own operations on the provider, in order with the verbs.
    async fn op<T: Send + 'static>(
        &self,
        f: impl for<'a> FnOnce(&'a mut dyn BaseRealtime) -> BoxFuture<'a, RealtimeResult<T>>
        + Send
        + 'static,
    ) -> RealtimeResult<T> {
        let (reply, answer) = oneshot::channel();
        let op: Op = Box::new(move |provider| {
            Box::pin(async move {
                let _ = reply.send(f(provider).await);
            })
        });
        self.tx
            .send(Cmd::Op(op))
            .map_err(|_| RealtimeError::NotConnected)?;
        answer.await.unwrap_or(Err(RealtimeError::NotConnected))
    }
}

/// Something sent while the connection was down.
enum Held {
    Send(Verb),
    Tool {
        call_id: String,
        epoch: u64,
        output: String,
        reply: bool,
    },
}

struct Executor {
    wire: Box<dyn Wire>,
    epoch: Arc<AtomicU64>,
    connected: Arc<AtomicBool>,
    gate: Arc<OutputGate>,
    clock: Arc<AudioClock>,
    session: mpsc::Sender<Msg>,
    /// PCM16 bytes per millisecond of the provider's audio.
    bytes_per_ms: usize,
    outbox: VecDeque<Held>,
    waiting: VecDeque<(u64, Verb)>,
    /// The gateway line whose response has not ended, and when it went out.
    slot: Option<(u64, Instant)>,
    /// What the line in the slot said, to send again if the provider refuses it.
    slot_verb: Option<Verb>,
    /// The provider's audio is playing at the caller until then.
    playing_until: Instant,
    /// Since when the provider is in a response of its own.
    busy: Option<Instant>,
    /// Since when the provider says it works on after its response (`Signal::Working`).
    working: Option<Instant>,
}

/// The executor's commands before it starts.
pub struct Pending(mpsc::UnboundedReceiver<Cmd>);

/// A handle whose commands wait until [`start`]: the provider's callbacks hold it before the
/// provider connects.
pub fn handle() -> (ExecHandle, Pending) {
    let (tx, rx) = mpsc::unbounded_channel();
    let handle = ExecHandle {
        tx,
        epoch: Arc::new(AtomicU64::new(0)),
        connected: Arc::new(AtomicBool::new(true)),
        gate: Arc::new(OutputGate::default()),
        clock: Arc::default(),
    };
    (handle, Pending(rx))
}

/// Start the executor of `handle` over a connected provider. `session` gets `LineSent` and
/// `LineEnded`; `output_rate` is the provider's audio sample rate.
pub fn start(
    handle: &ExecHandle,
    pending: Pending,
    wire: Box<dyn Wire>,
    session: mpsc::Sender<Msg>,
    output_rate: u32,
) {
    let executor = Executor {
        wire,
        epoch: handle.epoch.clone(),
        connected: handle.connected.clone(),
        gate: handle.gate.clone(),
        clock: handle.clock.clone(),
        session,
        bytes_per_ms: (output_rate as usize * 2 / 1000).max(1),
        outbox: VecDeque::new(),
        waiting: VecDeque::new(),
        slot: None,
        slot_verb: None,
        playing_until: Instant::now(),
        busy: None,
        working: None,
    };
    tokio::spawn(executor.run(pending.0));
}

/// [`handle`] and [`start`] at once.
#[cfg(test)]
pub fn spawn(wire: Box<dyn Wire>, session: mpsc::Sender<Msg>, output_rate: u32) -> ExecHandle {
    let (handle, pending) = handle();
    start(&handle, pending, wire, session, output_rate);
    handle
}

fn down(error: &RealtimeError) -> bool {
    !matches!(error, RealtimeError::InvalidConfiguration(_))
}

impl Executor {
    async fn run(mut self, mut rx: mpsc::UnboundedReceiver<Cmd>) {
        loop {
            let wake = self.wake();
            let cmd = tokio::select! {
                cmd = rx.recv() => match cmd {
                    Some(Cmd::Close) | None => break,
                    Some(cmd) => Some(cmd),
                },
                () = tokio::time::sleep_until(wake.unwrap_or_else(Instant::now)), if wake.is_some() => None,
            };
            if let Some(cmd) = cmd {
                self.on(cmd).await;
            }
            self.expire().await;
            self.speak().await;
        }
        if let Some(provider) = self.wire.provider() {
            let _ = provider.disconnect().await;
        }
    }

    /// When a waiting line may go out, or the line in the slot is given up.
    fn wake(&self) -> Option<Instant> {
        let held = [self.busy, self.working].into_iter().flatten().min();
        match (self.slot, held) {
            (Some((_, sent)), _) => Some(sent + SLOT_MAX),
            (None, Some(since)) if !self.waiting.is_empty() => Some(since + SLOT_MAX),
            // While the connection is down nothing can go out: `Reconnected` wakes the loop.
            (None, None) if !self.waiting.is_empty() && self.connected.load(Ordering::Relaxed) => {
                Some(self.playing_until)
            }
            _ => None,
        }
    }

    async fn on(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Say { line, verb } => self.waiting.push_back((line, verb)),
            Cmd::Send(verb) => self.send(Held::Send(verb)).await,
            Cmd::ToolResult {
                call_id,
                epoch,
                output,
                reply,
            } => {
                let held = Held::Tool {
                    call_id,
                    epoch,
                    output,
                    reply,
                };
                self.send(held).await
            }
            Cmd::Signal(Signal::Audio(bytes)) => {
                let now = Instant::now();
                let ms = (bytes / self.bytes_per_ms) as u64;
                self.playing_until = self.playing_until.max(now) + Duration::from_millis(ms);
                self.clock.note(self.playing_until);
            }
            // A response during a gateway line is that line's.
            Cmd::Signal(Signal::Responding) => {
                if self.slot.is_none() && self.busy.is_none() {
                    self.busy = Some(Instant::now());
                }
            }
            Cmd::Signal(Signal::ResponseDone) => {
                self.gate.open();
                self.busy = None;
                if let Some((line, _)) = self.slot.take() {
                    self.tell(Msg::LineEnded { line, spoken: true }).await;
                }
            }
            Cmd::Signal(Signal::Working(true)) => {
                self.working.get_or_insert_with(Instant::now);
            }
            Cmd::Signal(Signal::Working(false)) => self.working = None,
            Cmd::Signal(Signal::Refused) => {
                if let (Some((line, _)), Some(verb)) = (self.slot.take(), self.slot_verb.take()) {
                    debug!(
                        line,
                        "interaction line refused by the provider; sent again later"
                    );
                    self.waiting.push_front((line, verb));
                    self.busy.get_or_insert_with(Instant::now);
                }
            }
            Cmd::Signal(Signal::Reconnected) => {
                let epoch = self.epoch.fetch_add(1, Ordering::Relaxed) + 1;
                self.connected.store(true, Ordering::Relaxed);
                // The provider's response in progress went with the connection: its end
                // never comes.
                self.busy = None;
                self.working = None;
                self.gate.open();
                debug!(
                    epoch,
                    held = self.outbox.len(),
                    "interaction provider back; sending what was held"
                );
                // A line whose response was lost with the connection is over.
                if let Some((line, _)) = self.slot.take() {
                    self.tell(Msg::LineEnded {
                        line,
                        spoken: false,
                    })
                    .await;
                }
                for held in std::mem::take(&mut self.outbox) {
                    self.send(held).await;
                }
            }
            Cmd::Op(op) => match self.wire.provider() {
                Some(provider) => op(provider).await,
                None => warn!("a provider operation on a session without a provider"),
            },
            Cmd::Close => {}
        }
    }

    async fn tell(&self, msg: Msg) {
        let _ = self.session.send(msg).await;
    }

    /// Send now, or hold it in the outbox while the connection is down.
    async fn send(&mut self, held: Held) {
        if !self.connected.load(Ordering::Relaxed) {
            return self.hold(held);
        }
        let current = self.epoch.load(Ordering::Relaxed);
        let sent = match &held {
            Held::Send(Verb::StopOutput) => match self.wire.verb(&Verb::StopOutput).await {
                // No wire to cancel with: the gate holds the audio until the response ends.
                Err(RealtimeError::InvalidConfiguration(_)) => {
                    self.gate.close();
                    Ok(())
                }
                other => other,
            },
            Held::Send(verb) => self.wire.verb(verb).await,
            Held::Tool {
                call_id,
                epoch,
                output,
                reply,
            } if *epoch == current => self.wire.tool_result(call_id, output, *reply).await,
            // The call was made on a connection that is gone: the provider no longer knows
            // its id, so the output goes in as context.
            Held::Tool { output, reply, .. } => {
                let context = format!("The result of the caller's earlier request: {output}");
                match self.wire.verb(&Verb::AddContext(context)).await {
                    Ok(()) if *reply => self.wire.reply().await,
                    other => other,
                }
            }
        };
        match sent {
            Ok(()) => {}
            Err(error) if down(&error) => {
                self.connected.store(false, Ordering::Relaxed);
                self.hold(held);
            }
            Err(error) => warn!(%error, "interaction verb refused by the provider"),
        }
    }

    fn hold(&mut self, held: Held) {
        if self.outbox.len() == OUTBOX_MAX {
            warn!("interaction outbox full; the oldest send is dropped");
            self.outbox.pop_front();
        }
        self.outbox.push_back(held);
    }

    async fn expire(&mut self) {
        // A response whose end never came holds the slot no longer.
        if self.busy.is_some_and(|since| since.elapsed() >= SLOT_MAX) {
            self.busy = None;
        }
        if self
            .working
            .is_some_and(|since| since.elapsed() >= SLOT_MAX)
        {
            self.working = None;
        }
        if let Some((line, sent)) = self.slot
            && sent.elapsed() >= SLOT_MAX
        {
            self.slot = None;
            self.tell(Msg::LineEnded { line, spoken: true }).await;
        }
    }

    /// The next waiting line, when the slot is free, the provider's audio has played out and
    /// the connection is up.
    async fn speak(&mut self) {
        while self.slot.is_none()
            && self.busy.is_none()
            && self.working.is_none()
            && Instant::now() >= self.playing_until
            && self.connected.load(Ordering::Relaxed)
        {
            let Some((line, verb)) = self.waiting.pop_front() else {
                return;
            };
            match self.wire.verb(&verb).await {
                Ok(()) => {
                    self.slot = Some((line, Instant::now()));
                    self.slot_verb = Some(verb);
                    self.tell(Msg::LineSent { line }).await;
                }
                Err(error) if down(&error) => {
                    self.connected.store(false, Ordering::Relaxed);
                    self.waiting.push_front((line, verb));
                }
                Err(_) => {
                    self.tell(Msg::LineEnded {
                        line,
                        spoken: false,
                    })
                    .await
                }
            }
        }
    }
}

/// The session's provider behind the executor: the `/realtime` handler keeps using it as its
/// provider, and every call runs on the executor's task, in order with the verbs.
pub struct Steered {
    exec: ExecHandle,
    info: serde_json::Value,
    rates: Option<(u32, u32)>,
}

impl Steered {
    pub fn new(exec: ExecHandle, info: serde_json::Value, rates: Option<(u32, u32)>) -> Self {
        Self { exec, info, rates }
    }
}

fn not_here<T>() -> RealtimeResult<T> {
    Err(RealtimeError::InvalidConfiguration(
        "the provider is driven by interaction mode".to_string(),
    ))
}

#[async_trait]
impl BaseRealtime for Steered {
    fn new(_config: RealtimeConfig) -> RealtimeResult<Self> {
        not_here()
    }

    async fn connect(&mut self) -> RealtimeResult<()> {
        Ok(())
    }

    async fn disconnect(&mut self) -> RealtimeResult<()> {
        let done = self.exec.op(|p| p.disconnect()).await;
        self.exec.close();
        done
    }

    fn is_ready(&self) -> bool {
        self.exec.connected.load(Ordering::Relaxed)
    }

    fn get_connection_state(&self) -> ConnectionState {
        if self.is_ready() {
            ConnectionState::Connected
        } else {
            ConnectionState::Reconnecting
        }
    }

    async fn send_audio(&mut self, audio_data: Bytes) -> RealtimeResult<()> {
        self.exec.op(move |p| p.send_audio(audio_data)).await
    }

    async fn send_text(&mut self, text: &str) -> RealtimeResult<()> {
        let text = text.to_owned();
        self.exec
            .op(move |p| Box::pin(async move { p.send_text(&text).await }))
            .await
    }

    async fn create_response(&mut self) -> RealtimeResult<()> {
        self.exec.op(|p| p.create_response()).await
    }

    async fn create_response_with(
        &mut self,
        overrides: RealtimeResponseOverride,
    ) -> RealtimeResult<()> {
        self.exec
            .op(move |p| p.create_response_with(overrides))
            .await
    }

    async fn cancel_response(&mut self) -> RealtimeResult<()> {
        self.exec.op(|p| p.cancel_response()).await
    }

    async fn commit_audio_buffer(&mut self) -> RealtimeResult<()> {
        self.exec.op(|p| p.commit_audio_buffer()).await
    }

    async fn clear_audio_buffer(&mut self) -> RealtimeResult<()> {
        self.exec.op(|p| p.clear_audio_buffer()).await
    }

    fn on_transcript(&mut self, _callback: TranscriptCallback) -> RealtimeResult<()> {
        not_here()
    }

    fn on_audio(&mut self, _callback: AudioOutputCallback) -> RealtimeResult<()> {
        not_here()
    }

    fn on_error(&mut self, _callback: RealtimeErrorCallback) -> RealtimeResult<()> {
        not_here()
    }

    fn on_function_call(&mut self, _callback: FunctionCallCallback) -> RealtimeResult<()> {
        not_here()
    }

    fn on_speech_event(&mut self, _callback: SpeechEventCallback) -> RealtimeResult<()> {
        not_here()
    }

    fn on_response_done(&mut self, _callback: ResponseDoneCallback) -> RealtimeResult<()> {
        not_here()
    }

    fn on_reconnection(&mut self, _callback: ReconnectionCallback) -> RealtimeResult<()> {
        not_here()
    }

    async fn update_session(&mut self, config: RealtimeConfig) -> RealtimeResult<()> {
        self.exec.op(move |p| p.update_session(config)).await
    }

    async fn submit_function_result(&mut self, call_id: &str, result: &str) -> RealtimeResult<()> {
        let (call_id, result) = (call_id.to_owned(), result.to_owned());
        self.exec
            .op(move |p| Box::pin(async move { p.submit_function_result(&call_id, &result).await }))
            .await
    }

    fn get_provider_info(&self) -> serde_json::Value {
        self.info.clone()
    }

    fn audio_rates(&self) -> Option<(u32, u32)> {
        self.rates
    }

    async fn truncate_response(&mut self, item_id: &str, audio_end_ms: u64) -> RealtimeResult<()> {
        let item_id = item_id.to_owned();
        self.exec
            .op(move |p| Box::pin(async move { p.truncate_response(&item_id, audio_end_ms).await }))
            .await
    }

    async fn truncate_current_response(&mut self) -> RealtimeResult<Option<(String, u64)>> {
        self.exec.op(|p| p.truncate_current_response()).await
    }

    async fn replay_user_audio_preroll(&mut self) -> RealtimeResult<()> {
        self.exec.op(|p| p.replay_user_audio_preroll()).await
    }

    async fn replay_conversation(
        &mut self,
        items: &[ReplayConversationItem],
    ) -> RealtimeResult<()> {
        let items = items.to_vec();
        self.exec
            .op(move |p| Box::pin(async move { p.replay_conversation(&items).await }))
            .await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// The audio clock: no audio yet, then the last chunk's time and audible until it played.
    #[tokio::test(start_paused = true)]
    async fn the_audio_clock_counts_the_providers_audio() {
        let clock = AudioClock::default();
        assert_eq!(clock.last(), None);
        assert!(!clock.audible());
        tokio::time::advance(Duration::from_millis(40)).await;
        let came = Instant::now();
        clock.note(came + Duration::from_millis(500));
        assert_eq!(clock.last(), Some(came));
        assert!(clock.audible());
        tokio::time::advance(Duration::from_millis(499)).await;
        assert!(clock.audible());
        tokio::time::advance(Duration::from_millis(2)).await;
        assert!(!clock.audible());
        assert_eq!(clock.last(), Some(came));
    }

    /// A provider connection that records what went out and can be down.
    #[derive(Clone, Default)]
    struct Recorder {
        sent: Arc<Mutex<Vec<String>>>,
        down: Arc<AtomicBool>,
        /// Verbs it has no wire for.
        lacks: Vec<&'static str>,
    }

    impl Recorder {
        fn sent(&self) -> Vec<String> {
            self.sent.lock().unwrap().clone()
        }

        fn note(&self, what: String) -> RealtimeResult<()> {
            if self.down.load(Ordering::Relaxed) {
                return Err(RealtimeError::NotConnected);
            }
            self.sent.lock().unwrap().push(what);
            Ok(())
        }
    }

    #[async_trait]
    impl Wire for Recorder {
        async fn verb(&mut self, verb: &Verb) -> RealtimeResult<()> {
            let name = match verb {
                Verb::SayExact(t) => format!("exact:{t}"),
                Verb::SayGuided(t) => format!("guided:{t}"),
                Verb::AddContext(t) => format!("context:{t}"),
                Verb::StopOutput => "stop".to_owned(),
            };
            if self.lacks.iter().any(|lacked| name.starts_with(lacked)) {
                return Err(RealtimeError::InvalidConfiguration("no wire".into()));
            }
            self.note(name)
        }

        async fn tool_result(
            &mut self,
            call_id: &str,
            output: &str,
            reply: bool,
        ) -> RealtimeResult<()> {
            self.note(format!("tool:{call_id}:{output}:{reply}"))
        }

        async fn reply(&mut self) -> RealtimeResult<()> {
            self.note("reply".to_owned())
        }
    }

    fn rig(wire: Recorder) -> (ExecHandle, mpsc::Receiver<Msg>) {
        let (tx, rx) = mpsc::channel(64);
        (spawn(Box::new(wire), tx, 24_000), rx)
    }

    async fn settle() {
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
    }

    fn lines(rx: &mut mpsc::Receiver<Msg>) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            match msg {
                Msg::LineSent { line } => out.push(format!("sent {line}")),
                Msg::LineEnded { line, spoken } => out.push(format!("ended {line} {spoken}")),
                _ => {}
            }
        }
        out
    }

    #[tokio::test(start_paused = true)]
    async fn one_line_at_a_time_and_only_after_the_providers_audio_has_played() {
        let wire = Recorder::default();
        let (exec, mut rx) = rig(wire.clone());
        // One second of the provider's own audio is still playing at the caller.
        exec.signal(Signal::Audio(48_000));
        exec.say(1, Verb::SayGuided("first".into()));
        exec.say(2, Verb::SayGuided("second".into()));
        settle().await;
        assert!(wire.sent().is_empty(), "nothing over the provider's audio");
        tokio::time::advance(Duration::from_millis(1_001)).await;
        settle().await;
        assert_eq!(wire.sent(), ["guided:first"]);
        assert_eq!(lines(&mut rx), ["sent 1"]);
        exec.signal(Signal::ResponseDone);
        settle().await;
        assert_eq!(wire.sent(), ["guided:first", "guided:second"]);
        assert_eq!(lines(&mut rx), ["ended 1 true", "sent 2"]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_line_waits_for_the_end_of_the_providers_own_response() {
        let wire = Recorder::default();
        let (exec, mut rx) = rig(wire.clone());
        // The provider calls a function: its response is still open.
        exec.signal(Signal::Responding);
        exec.say(1, Verb::SayGuided("one moment".into()));
        settle().await;
        assert!(wire.sent().is_empty(), "not inside the provider's response");
        exec.signal(Signal::ResponseDone);
        settle().await;
        assert_eq!(wire.sent(), ["guided:one moment"]);
        // Its own response, and then the line's, end in that order: the line ends with the
        // second, and a response it causes is its own.
        exec.signal(Signal::Responding);
        settle().await;
        assert_eq!(lines(&mut rx), ["sent 1"]);
        exec.signal(Signal::ResponseDone);
        settle().await;
        assert_eq!(lines(&mut rx), ["ended 1 true"]);
    }

    /// Gemini's `interactionStatus` (provider fix `gemini_interaction_status`): a response that
    /// ended while the provider still works in the background does not free the session for
    /// a gateway line; the provider's idle does (or the slot's cap).
    #[tokio::test(start_paused = true)]
    async fn a_line_waits_while_the_provider_says_it_is_still_working() {
        let wire = Recorder::default();
        let (exec, mut rx) = rig(wire.clone());
        exec.signal(Signal::Responding);
        exec.signal(Signal::Working(true));
        exec.signal(Signal::ResponseDone);
        exec.say(1, Verb::SayGuided("your table is booked".into()));
        settle().await;
        assert!(wire.sent().is_empty(), "the provider still works");
        exec.signal(Signal::Working(false));
        settle().await;
        assert_eq!(wire.sent(), ["guided:your table is booked"]);
        assert_eq!(lines(&mut rx), ["sent 1"]);
        // An idle that never comes holds a line no longer than the slot's cap.
        exec.signal(Signal::ResponseDone);
        exec.signal(Signal::Working(true));
        exec.say(2, Verb::SayGuided("second".into()));
        settle().await;
        assert_eq!(wire.sent().len(), 1);
        tokio::time::advance(SLOT_MAX).await;
        settle().await;
        assert_eq!(wire.sent().len(), 2);
    }

    /// Deepgram's "already in the middle of a reply" warning: the refused line goes out again
    /// once the provider's own reply ended, ahead of the lines behind it.
    #[tokio::test(start_paused = true)]
    async fn a_line_the_provider_refused_goes_out_again_after_its_reply() {
        let wire = Recorder::default();
        let (exec, mut rx) = rig(wire.clone());
        exec.say(1, Verb::SayExact("Your table is booked.".into()));
        exec.say(2, Verb::SayExact("Anything else?".into()));
        settle().await;
        assert_eq!(wire.sent(), ["exact:Your table is booked."]);
        exec.signal(Signal::Refused);
        exec.signal(Signal::Responding);
        settle().await;
        assert_eq!(wire.sent().len(), 1, "nothing while the provider replies");
        exec.signal(Signal::ResponseDone);
        settle().await;
        assert_eq!(
            wire.sent(),
            ["exact:Your table is booked.", "exact:Your table is booked."]
        );
        assert_eq!(lines(&mut rx), ["sent 1", "sent 1"]);
        exec.signal(Signal::ResponseDone);
        settle().await;
        assert_eq!(wire.sent().last().unwrap(), "exact:Anything else?");
        assert_eq!(lines(&mut rx), ["ended 1 true", "sent 2"]);
        // A refusal with no line in the slot changes nothing.
        exec.signal(Signal::ResponseDone);
        exec.signal(Signal::Refused);
        exec.say(3, Verb::SayExact("Bye.".into()));
        settle().await;
        assert_eq!(wire.sent().last().unwrap(), "exact:Bye.");
    }

    #[tokio::test(start_paused = true)]
    async fn what_could_not_be_sent_goes_out_in_order_on_the_next_connection() {
        let wire = Recorder::default();
        let (exec, mut rx) = rig(wire.clone());
        wire.down.store(true, Ordering::Relaxed);
        exec.send(Verb::AddContext("a".into()));
        exec.tool_result("call_1".into(), 0, "{}".into(), true);
        exec.say(1, Verb::SayGuided("line".into()));
        settle().await;
        assert!(wire.sent().is_empty());
        assert!(!exec.connected.load(Ordering::Relaxed));
        wire.down.store(false, Ordering::Relaxed);
        exec.signal(Signal::Reconnected);
        settle().await;
        assert_eq!(exec.epoch(), 1);
        // The call was made on the lost connection: its output is context, then a reply.
        assert_eq!(
            wire.sent(),
            [
                "context:a",
                "context:The result of the caller's earlier request: {}",
                "reply",
                "guided:line"
            ]
        );
        assert_eq!(lines(&mut rx), ["sent 1"]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_result_for_a_call_of_this_connection_answers_the_call() {
        let wire = Recorder::default();
        let (exec, _rx) = rig(wire.clone());
        exec.tool_result("call_1".into(), 0, "done".into(), false);
        settle().await;
        assert_eq!(wire.sent(), ["tool:call_1:done:false"]);
    }

    #[tokio::test(start_paused = true)]
    async fn the_gate_holds_the_audio_of_a_provider_that_cannot_stop() {
        let wire = Recorder {
            lacks: vec!["stop"],
            ..Recorder::default()
        };
        let (exec, _rx) = rig(wire.clone());
        let gate = exec.gate();
        assert!(gate.pass());
        exec.send(Verb::StopOutput);
        settle().await;
        assert!(!gate.pass(), "held");
        exec.signal(Signal::ResponseDone);
        settle().await;
        assert!(gate.pass(), "the response is over");
        exec.send(Verb::StopOutput);
        settle().await;
        assert!(!gate.pass());
    }

    #[tokio::test(start_paused = true)]
    async fn a_line_the_provider_has_no_wire_for_ends_unspoken() {
        let wire = Recorder {
            lacks: vec!["exact"],
            ..Recorder::default()
        };
        let (exec, mut rx) = rig(wire.clone());
        exec.say(7, Verb::SayExact("x".into()));
        settle().await;
        assert_eq!(lines(&mut rx), ["ended 7 false"]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_reconnect_ends_the_providers_lost_response() {
        let wire = Recorder::default();
        let (exec, mut rx) = rig(wire.clone());
        exec.signal(Signal::Responding);
        exec.signal(Signal::Reconnected);
        exec.say(1, Verb::SayExact("your question".into()));
        settle().await;
        assert_eq!(wire.sent(), ["exact:your question"]);
        assert_eq!(lines(&mut rx), ["sent 1"]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_waiting_line_does_not_wake_the_executor_while_the_connection_is_down() {
        let (session, _rx) = mpsc::channel(4);
        let executor = Executor {
            wire: Box::new(Recorder::default()),
            epoch: Arc::default(),
            connected: Arc::new(AtomicBool::new(false)),
            gate: Arc::default(),
            clock: Arc::default(),
            session,
            bytes_per_ms: 48,
            outbox: VecDeque::new(),
            waiting: VecDeque::from([(1, Verb::SayGuided("line".into()))]),
            slot: None,
            slot_verb: None,
            playing_until: Instant::now(),
            busy: None,
            working: None,
        };
        assert_eq!(
            executor.wake(),
            None,
            "no timed wake: it would fire at once, forever"
        );
        executor.connected.store(true, Ordering::Relaxed);
        assert!(executor.wake().is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn the_outbox_keeps_the_newest_sends() {
        let wire = Recorder::default();
        let (exec, _rx) = rig(wire.clone());
        wire.down.store(true, Ordering::Relaxed);
        for n in 0..(OUTBOX_MAX + 3) {
            exec.send(Verb::AddContext(n.to_string()));
        }
        settle().await;
        wire.down.store(false, Ordering::Relaxed);
        exec.signal(Signal::Reconnected);
        settle().await;
        let sent = wire.sent();
        assert_eq!(sent.len(), OUTBOX_MAX);
        assert_eq!(sent[0], "context:3");
    }
}
