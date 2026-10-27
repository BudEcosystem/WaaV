//! A-G6: playback pump — meters a [`PlaybackQueue`] to the transport.
//!
//! When uninterruptible playback is enabled, TTS chunks are enqueued instead of
//! handed straight to the transport. This pump pops them in order and delivers
//! each to the egress sink, then **paces** by the chunk's playback duration so
//! the transport buffer never holds more than ~`lead` of audio. That shallow
//! buffer is what makes a selective barge-in possible: the bulk of pending
//! interruptible audio is still in the gateway queue (droppable via
//! [`PlaybackQueue::clear_interruptible`]), and only ≤`lead` of already-delivered
//! audio rides in the transport (the bounded residual, matching E-G2's goal).
//!
//! The pump owns its task and aborts it on drop (mirrors `HeartbeatMonitor`), so
//! holding it in the VoiceManager ties its lifetime to the session.
//!
//! Interaction mode runs the same queue, sink and enqueuer through
//! [`PlaybackPump::spawn_metered`] instead: PCM16 goes out in 20 ms frames on an
//! absolute playout clock, steered by a [`PumpControl`] (which utterance the
//! audio belongs to, duck, pause, cut) that reports what was delivered.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use tokio::sync::Notify;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use super::playback_queue::{PlaybackQueue, PlaybackUnit};
use crate::core::interaction::echo::{Reference, Tap};
use crate::core::interaction::types::{GateCmd, Ms, PumpEvent, UttId};
use crate::core::tts::AudioData;
use crate::core::voice_manager::callbacks::TTSCompleteCallback;

/// The egress sink the pump delivers each chunk to (the existing transport
/// delivery path — LiveKit / WS).
pub type PlaybackSink =
    Arc<dyn Fn(AudioData) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// Default lookahead: deliver the next chunk this far before the current one is
/// estimated to finish, keeping playout gapless while bounding the transport
/// residual cleared on barge-in.
pub const DEFAULT_PLAYBACK_LEAD: Duration = Duration::from_millis(40);

/// A cheap, cloneable handle for feeding the pump from the TTS callback hot path
/// (the `Arc`s are shared with the running pump). Holds no task, so the wrapper
/// closure can capture it without owning the pump.
#[derive(Clone)]
pub struct PlaybackEnqueuer {
    queue: Arc<PlaybackQueue>,
    notify: Arc<Notify>,
}

impl PlaybackEnqueuer {
    /// Append a unit and wake the pump.
    pub fn enqueue(&self, unit: PlaybackUnit) {
        self.queue.enqueue(unit);
        self.notify.notify_one();
    }

    /// The shared queue (for `clear_interruptible` / `clear_all` on barge-in).
    pub fn queue(&self) -> &Arc<PlaybackQueue> {
        &self.queue
    }
}

/// Meters a [`PlaybackQueue`] to a [`PlaybackSink`].
pub struct PlaybackPump {
    enqueuer: PlaybackEnqueuer,
    handle: Option<JoinHandle<()>>,
}

impl PlaybackPump {
    /// Spawn the pump over `queue`, delivering to `sink`. `fallback_rate` feeds
    /// [`AudioData::playback_ms`] when a chunk has no declared duration; `lead`
    /// is the transport lookahead (see [`DEFAULT_PLAYBACK_LEAD`]).
    pub fn spawn(
        queue: Arc<PlaybackQueue>,
        sink: PlaybackSink,
        fallback_rate: u32,
        lead: Duration,
    ) -> Self {
        let notify = Arc::new(Notify::new());
        let q = queue.clone();
        let n = notify.clone();
        let lead_ms = lead.as_millis() as u64;
        let handle = tokio::spawn(async move {
            loop {
                match q.pop_front() {
                    Some(unit) => {
                        let interruptible = unit.interruptible;
                        let dur_ms = unit.audio.playback_ms(fallback_rate) as u64;
                        sink(unit.audio).await;
                        // Pace: hold the next chunk until this one is `lead` from
                        // done, so the transport stays ≤ lead ahead.
                        let pace = dur_ms.saturating_sub(lead_ms);
                        if pace > 0 {
                            tokio::time::sleep(Duration::from_millis(pace)).await;
                        }
                        // Playout complete: an uninterruptible unit is no longer
                        // protected (its protection spanned queue + this playout).
                        q.mark_played(interruptible);
                    }
                    // Queue drained — park until the next enqueue wakes us. The
                    // notify_one stored by a racing enqueue is not lost (tokio
                    // Notify holds one permit).
                    None => n.notified().await,
                }
            }
        });
        Self {
            enqueuer: PlaybackEnqueuer { queue, notify },
            handle: Some(handle),
        }
    }

    /// Spawn the metered pump of interaction mode over `queue`. Every unit must
    /// come from [`PumpControl::route`]. PCM16 goes to `sink` in 20 ms frames,
    /// each at its slot on the playout clock (the playout end of what was
    /// delivered, minus `lead`), with the control's gain ramp, hold and cut
    /// applied per frame; any other format goes out one whole unit per slot.
    pub fn spawn_metered(
        queue: Arc<PlaybackQueue>,
        sink: PlaybackSink,
        fallback_rate: u32,
        lead: Duration,
        ctl: Arc<PumpControl>,
    ) -> Self {
        let notify = Arc::new(Notify::new());
        let handle = tokio::spawn(run_metered(
            queue.clone(),
            notify.clone(),
            sink,
            fallback_rate,
            lead,
            ctl,
        ));
        Self {
            enqueuer: PlaybackEnqueuer { queue, notify },
            handle: Some(handle),
        }
    }

    /// Enqueue a unit and wake the pump.
    pub fn enqueue(&self, unit: PlaybackUnit) {
        self.enqueuer.enqueue(unit);
    }

    /// A cheap cloneable handle for enqueuing from the TTS callback hot path.
    pub fn enqueuer(&self) -> PlaybackEnqueuer {
        self.enqueuer.clone()
    }

    /// The underlying queue (for `clear_interruptible` / `clear_all` on barge-in).
    pub fn queue(&self) -> &Arc<PlaybackQueue> {
        self.enqueuer.queue()
    }
}

impl Drop for PlaybackPump {
    fn drop(&mut self) {
        if let Some(h) = self.handle.take() {
            h.abort();
        }
    }
}

/// Frame length of the metered pump.
const FRAME_MS: u64 = 20;

/// Cost of one repeated step over a session (a pump frame, an analyser chunk).
#[derive(Default)]
pub struct CostMeter {
    count: AtomicU64,
    total_us: AtomicU64,
    max_us: AtomicU64,
}

impl CostMeter {
    pub fn record(&self, cost: Duration) {
        let us = cost.as_micros() as u64;
        self.count.fetch_add(1, Ordering::Relaxed);
        self.total_us.fetch_add(us, Ordering::Relaxed);
        self.max_us.fetch_max(us, Ordering::Relaxed);
    }

    /// `(average, max)` in microseconds.
    pub fn avg_max_us(&self) -> (u64, u64) {
        let count = self.count.load(Ordering::Relaxed).max(1);
        (
            self.total_us.load(Ordering::Relaxed) / count,
            self.max_us.load(Ordering::Relaxed),
        )
    }
}

/// An utterance with routed audio that has not played out yet.
struct Live {
    utt: UttId,
    /// Units routed and not yet finished by the pump.
    queued: usize,
    delivered_us: u64,
    synth_done: bool,
    /// `Started` went out and `Idle` has not.
    playing: bool,
    /// Fading out: the length of the fade and how much of it is left, in microseconds.
    fade: Option<(u64, u64)>,
}

/// Utterances remembered as played from the speech cache.
const CACHED_KEPT: usize = 16;

struct ControlState {
    /// The one utterance in synthesis and whether its audio is discarded.
    expected: Option<(UttId, bool)>,
    /// The utterance in synthesis is a clip for the bank, and its audio so far.
    capturing: Option<(UttId, Vec<AudioData>)>,
    /// Clips whose synthesis finished, until the session takes them to check.
    captured: Vec<(UttId, AudioData)>,
    /// The clip bank: listening sounds, ready to play.
    clips: Vec<AudioData>,
    /// In play order.
    live: Vec<Live>,
    /// When the audio delivered so far finishes playing at the client.
    playout_end: Instant,
    paused: bool,
    ducked: bool,
    /// The gain of the last frame sent, in dB.
    gain_db: f32,
    duck_db: f32,
    duck_ramp_ms: u32,
    /// A duck was asked and no ducked frame has gone out yet.
    duck_pending: bool,
    /// Barge-in is on: the analyser may duck at a caller onset by itself.
    reflex: bool,
    /// The caller's volume (a `volume` control): a gain on every frame of agent audio, dB.
    volume_db: f32,
    /// One utterance's own extra gain on top (a line said again louder), dB.
    line_gain: Option<(UttId, f32)>,
    /// The latest utterances whose audio came from the speech cache, not a synthesis.
    cached: std::collections::VecDeque<UttId>,
}

impl ControlState {
    fn release(&mut self) {
        self.paused = false;
        self.ducked = false;
        self.duck_pending = false;
    }
}

/// What the pump may do with its next frame.
enum Admit {
    /// Booked: send it now, the gain moving between these two levels (dB). The last is its
    /// playout time, in microseconds since the session started.
    Go(f32, f32, u64),
    /// Its slot on the playout clock is not due yet.
    Wait(Instant),
    /// Paused.
    Hold,
    /// The utterance was cut or failed: drop its audio.
    Gone,
}

/// The control side of the metered pump: which utterance the provider's audio
/// belongs to, the gate (duck, pause), the cut, and the account of what was
/// delivered. Every method returns at once (one short lock, no I/O), so the
/// interaction actor and the audio analyser call it inline. It also owns the
/// session clock and the two cost meters of the audio shell.
pub struct PumpControl {
    events: UnboundedSender<PumpEvent>,
    lead_ms: u64,
    /// Session start: every `Ms` of the session counts from here.
    start: Instant,
    state: Mutex<ControlState>,
    /// The session's completion step ([`Self::set_on_played`]).
    on_played: Mutex<Option<TTSCompleteCallback>>,
    /// Wakes the pump out of a hold or a wait for a slot (cut, failure, gate).
    wake: Notify,
    frame_cost: CostMeter,
    chunk_cost: CostMeter,
    /// The PCM16 audio as delivered, for the analyser's echo defence, while one may read
    /// it ([`Self::set_echo_reference`]); written through `tap`, which only the pump locks.
    reference: Mutex<Reference>,
    tap: Mutex<Tap>,
    echo_reference: AtomicBool,
    /// The caller is on a phone line ([`Self::set_phone_leg`]).
    phone_leg: AtomicBool,
    /// The pump's own queue: a clip is played through it.
    enqueuer: Mutex<Option<PlaybackEnqueuer>>,
}

impl PumpControl {
    /// `lead_ms` is how far ahead of playout the pump delivers: the audio a
    /// client may still hold when a cut reaches it.
    pub fn new(events: UnboundedSender<PumpEvent>, lead_ms: u64) -> Self {
        let start = Instant::now();
        Self {
            events,
            lead_ms,
            start,
            state: Mutex::new(ControlState {
                expected: None,
                capturing: None,
                captured: Vec::new(),
                clips: Vec::new(),
                live: Vec::new(),
                playout_end: start,
                paused: false,
                ducked: false,
                gain_db: 0.0,
                duck_db: -15.0,
                duck_ramp_ms: 60,
                duck_pending: false,
                reflex: true,
                volume_db: 0.0,
                line_gain: None,
                cached: std::collections::VecDeque::new(),
            }),
            on_played: Mutex::new(None),
            wake: Notify::new(),
            frame_cost: CostMeter::default(),
            chunk_cost: CostMeter::default(),
            reference: Mutex::new(Reference::default()),
            tap: Mutex::new(Tap::default()),
            echo_reference: AtomicBool::new(false),
            phone_leg: AtomicBool::new(false),
            enqueuer: Mutex::new(None),
        }
    }

    /// The queue of the pump this control meters; set when the pump is spawned.
    pub fn set_enqueuer(&self, enqueuer: PlaybackEnqueuer) {
        *self.enqueuer.lock() = Some(enqueuer);
    }

    /// The provider's audio for `utt` is a clip: it is kept, never played as an utterance,
    /// and [`Self::take_captured`] once its synthesis is done.
    pub fn capture(&self, utt: UttId) {
        let mut s = self.state.lock();
        s.expected = Some((utt, true));
        s.capturing = Some((utt, Vec::new()));
    }

    /// `utt`'s audio comes from the speech cache: nothing asks the voice's vendor for it.
    pub fn note_cached(&self, utt: UttId) {
        let mut s = self.state.lock();
        if s.cached.len() == CACHED_KEPT {
            s.cached.pop_front();
        }
        s.cached.push_back(utt);
    }

    /// Whether `utt`'s audio came from the speech cache ([`Self::note_cached`]).
    pub fn was_cached(&self, utt: UttId) -> bool {
        self.state.lock().cached.contains(&utt)
    }

    /// The whole audio of clip `utt`, once `SynthDone` went out for it; `None` when the
    /// provider gave none.
    pub fn take_captured(&self, utt: UttId) -> Option<AudioData> {
        let mut s = self.state.lock();
        let at = s.captured.iter().position(|(clip, _)| *clip == utt)?;
        Some(s.captured.swap_remove(at).1)
    }

    /// How many captured clips wait to be taken ([`Self::take_captured`]).
    pub fn captured_clips(&self) -> usize {
        self.state.lock().captured.len()
    }

    /// The clip bank from now on (empty: no listening sound). Each clip plays 3 dB down, as a
    /// listening sound is quieter than speech.
    pub fn set_clips(&self, clips: Vec<AudioData>) {
        let mut clips = clips;
        for clip in &mut clips {
            if crate::core::tts::sniff::is_linear_pcm16(&clip.format) {
                apply_gain(&mut clip.data, CLIP_GAIN_DB, CLIP_GAIN_DB);
            }
        }
        let mut s = self.state.lock();
        s.clips = clips;
        s.captured.clear();
    }

    /// How many clips the bank holds.
    pub fn clips(&self) -> usize {
        self.state.lock().clips.len()
    }

    /// Whether any audio of `utt` came from the provider (queued or played).
    pub fn has_audio(&self, utt: UttId) -> bool {
        let s = self.state.lock();
        (s.live.iter()).any(|live| live.utt == utt && (live.queued > 0 || live.delivered_us > 0))
    }

    /// Plays clip `n` (round the bank) as utterance `utt`, after whatever is queued. The
    /// usual pump events follow (`Started`, `Idle`); [`Self::fail`] stops it. `false` when
    /// the bank is empty or the pump is not running.
    pub fn play_clip(&self, n: usize, utt: UttId) -> bool {
        let audio = {
            let s = self.state.lock();
            if s.clips.is_empty() {
                return false;
            }
            s.clips[n % s.clips.len()].clone()
        };
        self.play(audio, utt)
    }

    /// Plays audio made before (a line said when a service fails) as utterance `utt`, after
    /// whatever is queued, as [`Self::play_clip`] plays a clip. `false` when the pump is not
    /// running.
    pub fn play(&self, audio: AudioData, utt: UttId) -> bool {
        let Some(enqueuer) = self.enqueuer.lock().clone() else {
            return false;
        };
        let mut s = self.state.lock();
        s.live.push(Live {
            utt,
            queued: 1,
            delivered_us: 0,
            synth_done: true,
            playing: false,
            fade: None,
        });
        drop(s);
        enqueuer.enqueue(PlaybackUnit {
            audio,
            interruptible: true,
            tag: utt.0,
        });
        true
    }

    /// Milliseconds since the session started: the `at` of every pump and
    /// analyser event, and the clock the actor runs on.
    pub fn now_ms(&self) -> Ms {
        self.start.elapsed().as_millis() as Ms
    }

    /// Microseconds since the session started, on the clock the reference is written on.
    pub fn now_us(&self) -> u64 {
        self.start.elapsed().as_micros() as u64
    }

    pub fn lead_ms(&self) -> u64 {
        self.lead_ms
    }

    /// The agent's audio as it was delivered, each sample at its playout time.
    pub fn reference(&self) -> &Mutex<Reference> {
        &self.reference
    }

    /// Whether the pump records what it delivers in [`Self::reference`]: only when an echo
    /// defence may read it.
    pub fn set_echo_reference(&self, on: bool) {
        self.echo_reference.store(on, Ordering::Relaxed);
    }

    /// One delivered PCM16 frame at `rate` whose first sample plays `at_us` into the session.
    pub fn record_reference(&self, at_us: u64, pcm: &[u8], rate: u32) {
        self.tap.lock().write(at_us, pcm, rate, &self.reference);
    }

    /// The caller turned out to be on a phone line (a SIP participant of the room).
    pub fn set_phone_leg(&self) {
        self.phone_leg.store(true, Ordering::Relaxed);
    }

    pub fn phone_leg(&self) -> bool {
        self.phone_leg.load(Ordering::Relaxed)
    }

    /// Duck depth (dB, negative), the time the ramp takes, and whether the
    /// analyser may duck by reflex (barge-in is on); -15 dB over 60 ms and on
    /// until set.
    pub fn set_duck(&self, db: f32, ramp_ms: u32, reflex: bool) {
        let mut s = self.state.lock();
        s.duck_db = db;
        s.duck_ramp_ms = ramp_ms;
        s.reflex = reflex;
    }

    /// The gain of the agent's audio from the next frame on (a `volume` control), dB.
    pub fn set_volume(&self, db: f32) {
        self.state.lock().volume_db = db;
    }

    /// Utterance `utt` only plays this much louder (dB) than the rest ("say that again,
    /// louder"); the last such line replaces the one before.
    pub fn set_line_gain(&self, utt: UttId, db: f32) {
        self.state.lock().line_gain = Some((utt, db));
    }

    /// The analyser's reflex at a caller onset: duck when barge-in is on and
    /// delivered agent audio is still playing. Returns whether it ducked; the
    /// duplex machine then decides, and releases the duck if it opens no
    /// barge-in.
    pub fn reflex_duck(&self) -> bool {
        let mut s = self.state.lock();
        let duck = s.reflex && Instant::now() < s.playout_end;
        if duck && !s.ducked {
            s.ducked = true;
            s.duck_pending = true;
        }
        duck
    }

    /// What the session does when one utterance's audio has all gone to the
    /// sink (flush the egress re-framer, tell the client). The pump runs it
    /// after the utterance's last frame; a discard never reaches it.
    pub fn set_on_played(&self, step: TTSCompleteCallback) {
        *self.on_played.lock() = Some(step);
    }

    async fn played(&self) {
        let step = self.on_played.lock().clone();
        if let Some(step) = step {
            step().await;
        }
    }

    /// Cost of one pump frame (slicing and gain, not the sink).
    pub fn frame_cost(&self) -> &CostMeter {
        &self.frame_cost
    }

    /// Cost of one analyser chunk; the analyser records it here so the actor
    /// reads both meters from one place.
    pub fn chunk_cost(&self) -> &CostMeter {
        &self.chunk_cost
    }

    /// The provider's audio belongs to `utt` from now on; with `discard` it is
    /// synthesised (and cached) but never played.
    pub fn expect(&self, utt: UttId, discard: bool) {
        self.state.lock().expected = Some((utt, discard));
    }

    /// The provider finished the expected utterance. `SynthDone` goes out, and
    /// the `Idle` that closes the utterance follows it: from the pump when the
    /// audio has played out, or at once when nothing is left to play.
    ///
    /// Returns `true` in the second case: the utterance is over now, so the
    /// caller runs the session's completion step itself (the pump runs it in
    /// the first case). `false` for a discard and when nothing was expected.
    pub fn synth_done(&self) -> bool {
        let mut s = self.state.lock();
        let Some((utt, discard)) = s.expected.take() else {
            return false;
        };
        if let Some((_, chunks)) = s.capturing.take_if(|(capturing, _)| *capturing == utt)
            && let Some(clip) = clip_of(chunks)
        {
            s.captured.push((utt, clip));
        }
        self.emit(PumpEvent::SynthDone { utt });
        if discard {
            return false;
        }
        match s.live.iter().position(|l| l.utt == utt) {
            Some(i) if s.live[i].queued > 0 || s.live[i].playing => {
                s.live[i].synth_done = true;
                // The pump may be waiting for more of it.
                self.wake.notify_one();
                false
            }
            found => {
                let delivered_ms = found.map_or(0, |i| s.live.remove(i).delivered_us / 1000);
                self.emit(PumpEvent::Idle {
                    utt,
                    delivered_ms,
                    at: self.now_ms(),
                });
                true
            }
        }
    }

    /// `utt` failed: its audio is no longer expected and what is queued or in
    /// flight is dropped.
    pub fn fail(&self, utt: UttId) {
        let mut s = self.state.lock();
        if s.expected.is_some_and(|(expected, _)| expected == utt) {
            s.expected = None;
        }
        s.live.retain(|l| l.utt != utt);
        drop(s);
        self.wake.notify_one();
    }

    /// `utt` yields: audio of it that has not started playing is dropped (as [`Self::fail`]),
    /// and audio that plays fades out over `ms` (PCM16; another format stops at once) and
    /// then stops. No `Idle` follows either way.
    pub fn fade_out(&self, utt: UttId, ms: u32) {
        let mut s = self.state.lock();
        match s.live.iter_mut().find(|l| l.utt == utt) {
            Some(live) if live.playing && ms > 0 => {
                let total = u64::from(ms) * 1000;
                live.fade.get_or_insert((total, total));
                drop(s);
                self.wake.notify_one();
            }
            _ => {
                drop(s);
                self.fail(utt);
            }
        }
    }

    /// Tag a chunk from the provider with the one expected utterance. `None`
    /// drops the audio: no utterance is expected (it was cut, or it is over),
    /// or the expected one is a discard.
    pub fn route(&self, audio: AudioData, interruptible: bool) -> Option<PlaybackUnit> {
        let mut s = self.state.lock();
        let (utt, discard) = s.expected?;
        if let Some((_, chunks)) = s
            .capturing
            .as_mut()
            .filter(|(capturing, _)| *capturing == utt)
        {
            chunks.push(audio);
            return None;
        }
        if discard {
            return None;
        }
        match s.live.iter().position(|l| l.utt == utt) {
            Some(i) => s.live[i].queued += 1,
            None => s.live.push(Live {
                utt,
                queued: 1,
                delivered_us: 0,
                synth_done: false,
                playing: false,
                fade: None,
            }),
        }
        Some(PlaybackUnit {
            audio,
            interruptible,
            tag: utt.0,
        })
    }

    /// `Duck` ramps the gain down, `Pause` holds the audio where it is,
    /// `Resume` lets it run again and ramps the gain back, `Open` does the same
    /// at full gain at once.
    pub fn gate(&self, cmd: GateCmd) {
        let mut s = self.state.lock();
        match cmd {
            GateCmd::Duck => {
                if !s.ducked {
                    s.ducked = true;
                    s.duck_pending = true;
                }
            }
            GateCmd::Pause => s.paused = true,
            GateCmd::Resume => s.release(),
            GateCmd::Open => {
                s.release();
                s.gain_db = 0.0;
            }
        }
        drop(s);
        self.wake.notify_one();
    }

    /// Stop all agent audio now: nothing is expected, what is queued or in
    /// flight is dropped, the gate opens. Returns the milliseconds delivered to
    /// the sink for every utterance that still had audio to play (up to
    /// `lead_ms` of it may not have been heard). A second call returns nothing.
    pub fn cut(&self) -> Vec<(UttId, u64)> {
        self.cut_audio(false)
    }

    /// [`Self::cut`], but a discarded utterance in synthesis (a line rendered into the speech
    /// cache ahead of this cut) stays expected: its synthesis goes on and its `SynthDone`
    /// still comes.
    pub fn cut_keeping_warm(&self) -> Vec<(UttId, u64)> {
        self.cut_audio(true)
    }

    fn cut_audio(&self, keep_warm: bool) -> Vec<(UttId, u64)> {
        let mut s = self.state.lock();
        if !(keep_warm && s.expected.is_some_and(|(_, discard)| discard)) {
            s.expected = None;
        }
        let report = s
            .live
            .drain(..)
            .map(|l| (l.utt, l.delivered_us / 1000))
            .collect();
        // The client drops what it buffered, so the playout clock restarts.
        s.playout_end = Instant::now();
        s.release();
        s.gain_db = 0.0;
        drop(s);
        self.wake.notify_one();
        report
    }

    /// Whether delivered agent audio is still playing at the client.
    #[cfg(test)]
    fn is_audible(&self) -> bool {
        Instant::now() < self.state.lock().playout_end
    }

    fn emit(&self, event: PumpEvent) {
        let _ = self.events.send(event);
    }

    /// The slot of the next frame: the playout end of what was delivered,
    /// minus the lead.
    fn next_slot(&self, lead: Duration) -> Instant {
        let end = self.state.lock().playout_end;
        end.checked_sub(lead).unwrap_or(end)
    }

    async fn wait_slot(&self, due: Instant) {
        tokio::select! {
            _ = tokio::time::sleep_until(due) => {}
            _ = self.wake.notified() => {}
        }
    }

    /// Book `dur` of `utt` for delivery now, or say why not.
    fn admit(&self, utt: UttId, dur: Duration, lead: Duration, pcm: bool) -> Admit {
        let mut s = self.state.lock();
        let Some(i) = s.live.iter().position(|l| l.utt == utt) else {
            return Admit::Gone;
        };
        if s.paused {
            return Admit::Hold;
        }
        // A fade that is over, or one on audio that cannot be faded, ends the utterance.
        if s.live[i].fade.is_some_and(|(_, left)| left == 0 || !pcm) {
            s.live.remove(i);
            return Admit::Gone;
        }
        let now = Instant::now();
        // After an underrun (or before the first frame) the clock restarts now.
        let end = s.playout_end.max(now);
        let due = end.checked_sub(lead).unwrap_or(now);
        if due > now {
            return Admit::Wait(due);
        }
        s.playout_end = end + dur;
        let slot = end.saturating_duration_since(self.start).as_micros() as u64;
        let at = self.now_ms();
        s.live[i].delivered_us += dur.as_micros() as u64;
        if !std::mem::replace(&mut s.live[i].playing, true) {
            self.emit(PumpEvent::Started { utt, at });
        }
        if !pcm {
            return Admit::Go(0.0, 0.0, slot);
        }
        let line_gain = s
            .line_gain
            .filter(|(line, _)| *line == utt)
            .map_or(0.0, |(_, db)| db);
        let from = s.gain_db;
        let target = if s.ducked { s.duck_db } else { 0.0 };
        let step = if s.duck_ramp_ms == 0 || s.duck_db == 0.0 {
            f32::INFINITY
        } else {
            s.duck_db.abs() * (dur.as_micros() as f32 / 1000.0) / s.duck_ramp_ms as f32
        };
        let to = if from > target {
            (from - step).max(target)
        } else {
            (from + step).min(target)
        };
        s.gain_db = to;
        if s.duck_pending && to < 0.0 {
            s.duck_pending = false;
            self.emit(PumpEvent::FirstDuckedFrame { at });
        }
        if let Some((total, left)) = &mut s.live[i].fade {
            // Linear in amplitude, from what is left of the fade to what will be.
            let level = |part: u64| match part {
                0 => SILENT_DB,
                part => 20.0 * (part as f32 / *total as f32).log10(),
            };
            let after = left.saturating_sub(dur.as_micros() as u64);
            let (fade_from, fade_to) = (level(*left), level(after));
            *left = after;
            let volume = s.volume_db + line_gain;
            return Admit::Go(from + fade_from + volume, to + fade_to + volume, slot);
        }
        let volume = s.volume_db + line_gain;
        Admit::Go(from + volume, to + volume, slot)
    }

    /// Wait until `dur` of `utt` may go out; `None` when the utterance is gone.
    async fn admitted(
        &self,
        utt: UttId,
        dur: Duration,
        lead: Duration,
        pcm: bool,
    ) -> Option<(f32, f32, u64)> {
        loop {
            match self.admit(utt, dur, lead, pcm) {
                Admit::Go(from_db, to_db, slot) => return Some((from_db, to_db, slot)),
                Admit::Gone => return None,
                Admit::Hold => self.wake.notified().await,
                Admit::Wait(due) => self.wait_slot(due).await,
            }
        }
    }

    fn unit_done(&self, utt: UttId) {
        let mut s = self.state.lock();
        if let Some(live) = s.live.iter_mut().find(|l| l.utt == utt) {
            live.queued = live.queued.saturating_sub(1);
        }
    }

    /// `utt` has no more audio to send ([`Self::idle_at`]); `at` is the slot of
    /// the frame that did not come. When that was the last of the utterance,
    /// the session's completion step runs.
    async fn idle(&self, utt: UttId) {
        let over = {
            let mut s = self.state.lock();
            let Some(i) = s.live.iter().position(|l| l.utt == utt) else {
                return;
            };
            if std::mem::take(&mut s.live[i].playing) {
                let lead = Duration::from_millis(self.lead_ms);
                let slot = s.playout_end.checked_sub(lead).unwrap_or(self.start);
                self.emit(PumpEvent::Idle {
                    utt,
                    delivered_ms: s.live[i].delivered_us / 1000,
                    at: slot.saturating_duration_since(self.start).as_millis() as Ms,
                });
            }
            let over = s.live[i].synth_done && s.live[i].queued == 0;
            if over {
                s.live.remove(i);
            }
            over
        };
        if over {
            self.played().await;
        }
    }

    /// When `utt`, with nothing queued, is idle: now when the provider has
    /// finished it (or it is gone), else when everything delivered has played
    /// out, which is the provider running late (a stream gap). A chunk that
    /// comes before then continues the same stretch of audio.
    fn idle_at(&self, utt: UttId) -> Instant {
        let s = self.state.lock();
        match s.live.iter().find(|l| l.utt == utt) {
            Some(live) if !live.synth_done || live.queued > 0 => s.playout_end,
            _ => Instant::now(),
        }
    }
}

async fn run_metered(
    queue: Arc<PlaybackQueue>,
    notify: Arc<Notify>,
    sink: PlaybackSink,
    fallback_rate: u32,
    lead: Duration,
    ctl: Arc<PumpControl>,
) {
    // The utterance whose audio went out last. A chunk that arrives by the
    // slot of its next frame keeps the playout gapless.
    let mut current: Option<UttId> = None;
    loop {
        while current.is_some() {
            let due = ctl.next_slot(lead);
            if due <= Instant::now() {
                break;
            }
            ctl.wait_slot(due).await;
        }
        let Some(unit) = queue.pop_front() else {
            if let Some(utt) = current {
                // While less than the lead is buffered the slot is due at once, and an
                // utterance that comes in several chunks has gaps of a few ms between
                // them: it is idle only when it is finished or the client has run dry.
                let idle_at = ctl.idle_at(utt);
                if Instant::now() < idle_at {
                    tokio::select! {
                        _ = notify.notified() => {}
                        _ = ctl.wait_slot(idle_at) => {}
                    }
                    continue;
                }
                current = None;
                ctl.idle(utt).await;
            }
            // Parked until the next enqueue (a racing one left its permit).
            notify.notified().await;
            continue;
        };
        let utt = UttId(unit.tag);
        if let Some(previous) = current.replace(utt).filter(|previous| *previous != utt) {
            ctl.idle(previous).await;
        }
        let interruptible = unit.interruptible;
        play_unit(unit.audio, utt, &sink, fallback_rate, lead, &ctl).await;
        ctl.unit_done(utt);
        queue.mark_played(interruptible);
    }
}

async fn play_unit(
    audio: AudioData,
    utt: UttId,
    sink: &PlaybackSink,
    fallback_rate: u32,
    lead: Duration,
    ctl: &PumpControl,
) {
    if !crate::core::tts::sniff::is_linear_pcm16(&audio.format) {
        // Not sliceable: the whole unit goes out at its slot (pause and cut only).
        let dur = Duration::from_millis(audio.playback_ms(fallback_rate) as u64);
        if ctl.admitted(utt, dur, lead, false).await.is_some() {
            sink(audio).await;
        }
        return;
    }
    // The rate rule of `AudioData::playback_ms`.
    let rate = [audio.sample_rate, fallback_rate]
        .into_iter()
        .find(|rate| *rate != 0)
        .unwrap_or(24_000) as u64;
    let frame_bytes = ((rate * FRAME_MS / 1000) as usize * 2).max(2);
    for frame in audio.data.chunks(frame_bytes) {
        let dur = Duration::from_micros(frame.len() as u64 / 2 * 1_000_000 / rate);
        let Some((from_db, to_db, slot)) = ctl.admitted(utt, dur, lead, true).await else {
            return;
        };
        let began = std::time::Instant::now();
        let mut data = frame.to_vec();
        apply_gain(&mut data, from_db, to_db);
        if ctl.echo_reference.load(Ordering::Relaxed) {
            ctl.record_reference(slot, &data, rate as u32);
        }
        let frame = AudioData {
            data,
            sample_rate: audio.sample_rate,
            format: audio.format.clone(),
            duration_ms: Some(dur.as_millis() as u32),
        };
        ctl.frame_cost.record(began.elapsed());
        sink(frame).await;
    }
}

/// A clip's chunks as one unit of audio.
fn clip_of(chunks: Vec<AudioData>) -> Option<AudioData> {
    let mut chunks = chunks.into_iter();
    let mut clip = chunks.next()?;
    for chunk in chunks {
        clip.data.extend_from_slice(&chunk.data);
    }
    clip.duration_ms = None;
    (!clip.data.is_empty()).then_some(clip)
}

/// A listening sound plays this much quieter than speech.
const CLIP_GAIN_DB: f32 = -3.0;
/// The end of a fade: no audible sample is left.
const SILENT_DB: f32 = -120.0;

/// Scale PCM16 samples in place, the gain moving linearly from `from_db` to
/// `to_db` across the frame.
fn apply_gain(pcm: &mut [u8], from_db: f32, to_db: f32) {
    if from_db == 0.0 && to_db == 0.0 {
        return;
    }
    let from = 10f32.powf(from_db / 20.0);
    let to = 10f32.powf(to_db / 20.0);
    let samples = (pcm.len() / 2) as f32;
    for (i, sample) in pcm.as_chunks_mut::<2>().0.iter_mut().enumerate() {
        let gain = from + (to - from) * (i + 1) as f32 / samples;
        *sample = ((i16::from_le_bytes(*sample) as f32 * gain) as i16).to_le_bytes();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn audio(tag: u8, dur_ms: u32) -> AudioData {
        AudioData {
            data: vec![tag; 8],
            sample_rate: 24_000,
            format: "pcm".to_string(),
            duration_ms: Some(dur_ms),
        }
    }

    /// A sink that records delivered chunk ids and signals each delivery on a
    /// channel so a test can synchronize deterministically.
    fn recording_sink() -> (
        PlaybackSink,
        Arc<Mutex<Vec<u8>>>,
        tokio::sync::mpsc::UnboundedReceiver<u8>,
    ) {
        let delivered = Arc::new(Mutex::new(Vec::<u8>::new()));
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let d = delivered.clone();
        let sink: PlaybackSink = Arc::new(move |a: AudioData| {
            let d = d.clone();
            let tx = tx.clone();
            Box::pin(async move {
                let id = a.data[0];
                d.lock().unwrap().push(id);
                let _ = tx.send(id);
            })
        });
        (sink, delivered, rx)
    }

    #[tokio::test(start_paused = true)]
    async fn pump_delivers_in_order() {
        let queue = Arc::new(PlaybackQueue::new());
        let (sink, delivered, mut rx) = recording_sink();
        let pump = PlaybackPump::spawn(queue, sink, 24_000, DEFAULT_PLAYBACK_LEAD);

        pump.enqueue(PlaybackUnit::interruptible(audio(1, 20)));
        pump.enqueue(PlaybackUnit::interruptible(audio(2, 20)));
        pump.enqueue(PlaybackUnit::interruptible(audio(3, 20)));

        for expected in [1u8, 2, 3] {
            assert_eq!(rx.recv().await, Some(expected));
        }
        assert_eq!(*delivered.lock().unwrap(), vec![1, 2, 3]);
    }

    #[tokio::test(start_paused = true)]
    async fn clear_interruptible_mid_stream_drops_only_interruptible() {
        // Enqueue [int1, uninterruptible, int2]. After the pump delivers int1
        // (and is pacing), a barge-in clears interruptible: int2 is dropped from
        // the queue, the uninterruptible unit is kept and still delivered.
        let queue = Arc::new(PlaybackQueue::new());
        let (sink, delivered, mut rx) = recording_sink();
        let pump = PlaybackPump::spawn(queue.clone(), sink, 24_000, DEFAULT_PLAYBACK_LEAD);

        pump.enqueue(PlaybackUnit::interruptible(audio(1, 60)));
        pump.enqueue(PlaybackUnit::uninterruptible(audio(2, 60)));
        pump.enqueue(PlaybackUnit::interruptible(audio(3, 60)));

        // Wait until int1 is delivered (pump is now pacing on its 60ms chunk).
        assert_eq!(rx.recv().await, Some(1));

        // Barge-in: drop interruptible. int1 already popped; int2 dropped; the
        // uninterruptible unit (2) survives.
        let dropped = queue.clear_interruptible();
        assert_eq!(dropped, 1, "only int2 is still queued and droppable");
        assert!(queue.has_uninterruptible_pending());

        // The uninterruptible unit is delivered next; int2 never is.
        assert_eq!(rx.recv().await, Some(2));
        // Give the pump a chance to (not) deliver anything else.
        tokio::time::advance(Duration::from_millis(500)).await;
        assert_eq!(
            *delivered.lock().unwrap(),
            vec![1, 2],
            "int2 was dropped; only int1 + the uninterruptible unit played"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn pump_resumes_after_idle() {
        // The pump parks when the queue drains and resumes on the next enqueue
        // (no lost wakeup).
        let queue = Arc::new(PlaybackQueue::new());
        let (sink, _delivered, mut rx) = recording_sink();
        let pump = PlaybackPump::spawn(queue, sink, 24_000, DEFAULT_PLAYBACK_LEAD);

        pump.enqueue(PlaybackUnit::interruptible(audio(1, 20)));
        assert_eq!(rx.recv().await, Some(1));
        // Let it drain and park.
        tokio::time::advance(Duration::from_millis(100)).await;
        // Enqueue again — must wake and deliver.
        pump.enqueue(PlaybackUnit::interruptible(audio(2, 20)));
        assert_eq!(rx.recv().await, Some(2));
    }
}

#[cfg(test)]
mod metered_tests {
    use super::*;
    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

    /// 16 kHz PCM16, every sample `value`.
    fn pcm(ms: usize, value: i16) -> AudioData {
        AudioData {
            data: value.to_le_bytes().repeat(16 * ms),
            sample_rate: 16_000,
            format: "pcm".to_string(),
            duration_ms: None,
        }
    }

    fn first_sample(audio: &AudioData) -> i16 {
        i16::from_le_bytes([audio.data[0], audio.data[1]])
    }

    fn last_sample(audio: &AudioData) -> i16 {
        let n = audio.data.len();
        i16::from_le_bytes([audio.data[n - 2], audio.data[n - 1]])
    }

    /// A metered pump whose sink records every delivery with its time (ms since
    /// the rig was built) and then takes `sink_ms` to return.
    struct Rig {
        ctl: Arc<PumpControl>,
        pump: PlaybackPump,
        events: UnboundedReceiver<PumpEvent>,
        sent: Arc<Mutex<Vec<(u64, AudioData)>>>,
    }

    impl Rig {
        fn new(lead_ms: u64, sink_ms: u64) -> Self {
            let (tx, events) = unbounded_channel();
            let ctl = Arc::new(PumpControl::new(tx, lead_ms));
            let sent = Arc::new(Mutex::new(Vec::new()));
            let (recorded, start) = (sent.clone(), Instant::now());
            let sink: PlaybackSink = Arc::new(move |audio: AudioData| {
                let recorded = recorded.clone();
                Box::pin(async move {
                    recorded
                        .lock()
                        .push((start.elapsed().as_millis() as u64, audio));
                    tokio::time::sleep(Duration::from_millis(sink_ms)).await;
                })
            });
            let pump = PlaybackPump::spawn_metered(
                Arc::new(PlaybackQueue::new()),
                sink,
                16_000,
                Duration::from_millis(lead_ms),
                ctl.clone(),
            );
            Self {
                ctl,
                pump,
                events,
                sent,
            }
        }

        /// What the VoiceManager does with a provider chunk.
        fn feed(&self, audio: AudioData) {
            if let Some(unit) = self.ctl.route(audio, true) {
                self.pump.enqueue(unit);
            }
        }

        /// One whole utterance: expected, synthesised, complete.
        fn speak(&self, utt: u64, chunks: Vec<AudioData>) {
            self.ctl.expect(UttId(utt), false);
            for chunk in chunks {
                self.feed(chunk);
            }
            self.ctl.synth_done();
        }

        fn times(&self) -> Vec<u64> {
            self.sent.lock().iter().map(|(at, _)| *at).collect()
        }

        fn events(&mut self) -> Vec<PumpEvent> {
            std::iter::from_fn(|| self.events.try_recv().ok()).collect()
        }
    }

    async fn after(ms: u64) {
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_line_gain_is_that_utterances_only() {
        // "Say that again, louder": 6 dB on the line said again, none on the next.
        let rig = Rig::new(40, 0);
        rig.ctl.set_line_gain(UttId(2), 6.0);
        rig.speak(1, vec![pcm(60, 1000)]);
        rig.speak(2, vec![pcm(60, 1000)]);
        rig.speak(3, vec![pcm(60, 1000)]);
        after(400).await;
        let samples: Vec<i16> = rig
            .sent
            .lock()
            .iter()
            .map(|(_, f)| first_sample(f))
            .collect();
        assert_eq!(samples.len(), 9, "{samples:?}");
        assert!(samples[..3].iter().all(|s| *s == 1000), "{samples:?}");
        assert!(
            samples[3..6].iter().all(|s| (1990..=2000).contains(s)),
            "{samples:?}"
        );
        assert!(samples[6..].iter().all(|s| *s == 1000), "{samples:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn frames_follow_the_absolute_playout_clock() {
        // A 40 ms lead and a sink that takes 5 ms per frame. The lead goes out
        // as a burst, then every frame leaves at its own slot (playout start +
        // delivered - lead): the sink's 5 ms never accumulate.
        let mut rig = Rig::new(40, 5);
        rig.speak(1, vec![pcm(200, 1000)]);
        after(300).await;

        assert_eq!(rig.times(), [0, 5, 10, 20, 40, 60, 80, 100, 120, 140]);
        for (_, frame) in rig.sent.lock().iter() {
            assert_eq!(frame.data.len(), 640, "20 ms of 16 kHz PCM16");
            assert_eq!(frame.duration_ms, Some(20));
            assert_eq!((frame.sample_rate, frame.format.as_str()), (16_000, "pcm"));
            assert_eq!(
                first_sample(frame),
                1000,
                "full gain leaves the audio alone"
            );
        }
        assert_eq!(
            rig.events(),
            [
                PumpEvent::SynthDone { utt: UttId(1) },
                PumpEvent::Started {
                    utt: UttId(1),
                    at: 0
                },
                // The slot of the frame that never came: 200 ms delivered - 40 ms lead.
                PumpEvent::Idle {
                    utt: UttId(1),
                    delivered_ms: 200,
                    at: 160
                },
            ]
        );
        assert!(rig.ctl.frame_cost().avg_max_us().1 < 1_000_000);
    }

    #[tokio::test(start_paused = true)]
    async fn the_echo_reference_is_written_only_for_a_defence_that_reads_it() {
        let rig = Rig::new(40, 0);
        rig.speak(1, vec![pcm(100, 1000)]);
        after(200).await;
        assert_eq!(rig.sent.lock().len(), 5);
        assert_eq!(
            rig.ctl.reference().lock().written(),
            0,
            "no defence, no copy"
        );

        let rig = Rig::new(40, 0);
        rig.ctl.set_echo_reference(true);
        rig.speak(1, vec![pcm(100, 1000)]);
        after(200).await;
        // 100 ms at 16 kHz, from the first frame's slot on.
        assert_eq!(rig.ctl.reference().lock().written(), 1600);
    }

    #[tokio::test(start_paused = true)]
    async fn a_chunk_that_arrives_within_the_lead_keeps_the_run_gapless() {
        let mut rig = Rig::new(40, 0);
        rig.ctl.expect(UttId(1), false);
        rig.feed(pcm(100, 1));
        after(50).await; // delivered to 100 ms; the next slot is at 60
        rig.feed(pcm(40, 1));
        after(200).await;
        rig.ctl.synth_done();

        assert_eq!(rig.times(), [0, 0, 0, 20, 40, 60, 80]);
        assert_eq!(
            rig.events(),
            [
                PumpEvent::Started {
                    utt: UttId(1),
                    at: 0
                },
                // The provider is late: idle once the 140 ms have played out, stamped
                // with the slot of the frame that did not come.
                PumpEvent::Idle {
                    utt: UttId(1),
                    delivered_ms: 140,
                    at: 100
                },
                // The provider finished after the playout: the closing Idle follows at once.
                PumpEvent::SynthDone { utt: UttId(1) },
                PumpEvent::Idle {
                    utt: UttId(1),
                    delivered_ms: 140,
                    at: 250
                },
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn duck_ramps_to_minus_15_db_in_60_ms_and_resume_ramps_back() {
        let mut rig = Rig::new(0, 0);
        rig.speak(1, vec![pcm(400, 10_000)]);
        after(50).await; // frames at 0, 20, 40
        rig.ctl.gate(GateCmd::Duck);
        after(100).await; // frames at 60 .. 140
        rig.ctl.gate(GateCmd::Resume);
        after(100).await; // frames at 160 .. 240

        let sent = rig.sent.lock().clone();
        let db = |sample: i16| 20.0 * (sample as f32 / 10_000.0).log10();
        for (at, frame) in &sent[..3] {
            assert_eq!(
                last_sample(frame),
                10_000,
                "frame at {at} precedes the duck"
            );
        }
        // Three 20 ms frames carry the ramp: -5, -10, -15 dB at their last sample.
        for (i, depth) in [(3, -5.0), (4, -10.0), (5, -15.0)] {
            let reached = db(last_sample(&sent[i].1));
            assert!((reached - depth).abs() < 0.05, "frame {i}: {reached} dB");
            assert!(first_sample(&sent[i].1) > last_sample(&sent[i].1));
        }
        assert_eq!(
            sent[5].0, 100,
            "the ramp ends 60 ms after the first ducked frame"
        );
        for (_, frame) in &sent[6..8] {
            assert_eq!((first_sample(frame), last_sample(frame)), (1778, 1778));
        }
        // Resume: the same ramp back up, then full gain.
        for (i, depth) in [(8, -10.0), (9, -5.0), (10, 0.0)] {
            let reached = db(last_sample(&sent[i].1));
            assert!((reached - depth).abs() < 0.05, "frame {i}: {reached} dB");
        }
        assert_eq!(first_sample(&sent[11].1), 10_000);
        drop(sent);

        assert!(
            rig.events()
                .contains(&PumpEvent::FirstDuckedFrame { at: 60 })
        );
    }

    #[tokio::test(start_paused = true)]
    async fn open_restores_full_gain_at_once_and_a_set_depth_is_used() {
        let rig = Rig::new(0, 0);
        rig.ctl.set_duck(-6.0, 0, true);
        rig.speak(1, vec![pcm(200, 10_000)]);
        after(10).await; // frame at 0
        rig.ctl.gate(GateCmd::Duck);
        after(20).await; // frame at 20: no ramp, -6 dB from its first sample on
        rig.ctl.gate(GateCmd::Open);
        after(20).await; // frame at 40

        let sent = rig.sent.lock();
        assert_eq!(last_sample(&sent[1].1), 5011);
        assert_eq!(
            (first_sample(&sent[2].1), last_sample(&sent[2].1)),
            (10_000, 10_000)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn pause_holds_the_audio_and_resume_continues_it() {
        let mut rig = Rig::new(0, 0);
        rig.speak(1, vec![pcm(200, 7)]);
        after(50).await;
        assert!(rig.ctl.is_audible());
        rig.ctl.gate(GateCmd::Pause);
        after(150).await;
        assert_eq!(rig.times(), [0, 20, 40], "nothing leaves while paused");
        assert!(!rig.ctl.is_audible(), "what was delivered has played out");

        rig.ctl.gate(GateCmd::Resume);
        after(200).await;
        assert_eq!(rig.times(), [0, 20, 40, 200, 220, 240, 260, 280, 300, 320]);
        assert_eq!(
            rig.events().last(),
            Some(&PumpEvent::Idle {
                utt: UttId(1),
                delivered_ms: 200,
                at: 340
            })
        );
    }

    #[tokio::test(start_paused = true)]
    async fn cut_reports_what_was_delivered_and_is_idempotent() {
        let mut rig = Rig::new(0, 0);
        rig.speak(1, vec![pcm(100, 1), pcm(100, 1)]);
        after(50).await;

        assert_eq!(rig.ctl.cut(), [(UttId(1), 60)]);
        assert_eq!(rig.ctl.cut(), []);
        assert!(!rig.ctl.is_audible());
        after(300).await;
        assert_eq!(
            rig.times(),
            [0, 20, 40],
            "neither the rest of the unit nor the queued one"
        );
        assert_eq!(
            rig.events(),
            [
                PumpEvent::SynthDone { utt: UttId(1) },
                PumpEvent::Started {
                    utt: UttId(1),
                    at: 0
                },
            ],
            "a cut utterance never goes idle"
        );

        // The next line plays from now, at full gain.
        rig.speak(2, vec![pcm(40, 2)]);
        after(100).await;
        assert_eq!(rig.times()[3..], [350, 370]);
        assert_eq!(first_sample(&rig.sent.lock()[3].1), 2);
        assert_eq!(
            rig.events(),
            [
                PumpEvent::SynthDone { utt: UttId(2) },
                PumpEvent::Started {
                    utt: UttId(2),
                    at: 350
                },
                PumpEvent::Idle {
                    utt: UttId(2),
                    delivered_ms: 40,
                    at: 390
                },
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_cut_while_ducked_and_paused_opens_the_gate() {
        let rig = Rig::new(0, 0);
        rig.speak(1, vec![pcm(200, 10_000)]);
        rig.ctl.gate(GateCmd::Duck);
        after(70).await;
        rig.ctl.gate(GateCmd::Pause);
        rig.ctl.cut();
        rig.speak(2, vec![pcm(20, 10_000)]);
        after(50).await;
        let sent = rig.sent.lock();
        let (at, frame) = sent.last().unwrap();
        assert_eq!((*at, first_sample(frame)), (70, 10_000));
    }

    #[tokio::test(start_paused = true)]
    async fn audio_with_no_expected_utterance_is_dropped() {
        let mut rig = Rig::new(0, 0);
        assert!(
            rig.ctl.route(pcm(20, 1), true).is_none(),
            "nothing expected"
        );

        rig.ctl.expect(UttId(4), false);
        let unit = rig.ctl.route(pcm(20, 1), true).expect("the expected one");
        assert_eq!((unit.tag, unit.interruptible), (4, true));
        rig.ctl.synth_done();
        assert!(rig.ctl.route(pcm(20, 1), true).is_none(), "it is over");

        rig.ctl.expect(UttId(5), false);
        rig.ctl.cut();
        assert!(rig.ctl.route(pcm(20, 1), true).is_none(), "it was cut");

        // The unit routed for 4 was never enqueued here, and 4 was cut with it.
        rig.pump.enqueue(unit);
        after(100).await;
        assert!(rig.sent.lock().is_empty());
        assert_eq!(rig.events(), [PumpEvent::SynthDone { utt: UttId(4) }]);
    }

    /// The session's completion step, counted with the time it ran.
    fn count_played(rig: &Rig) -> Arc<Mutex<Vec<u64>>> {
        let played = Arc::new(Mutex::new(Vec::new()));
        let (seen, start) = (played.clone(), Instant::now());
        rig.ctl.set_on_played(Arc::new(move || {
            seen.lock().push(start.elapsed().as_millis() as u64);
            Box::pin(async {})
        }));
        played
    }

    #[tokio::test(start_paused = true)]
    async fn the_completion_step_runs_after_the_last_frame_and_never_for_a_discard() {
        let rig = Rig::new(0, 0);
        let played = count_played(&rig);
        // Synthesis is done at once; the audio takes 100 ms to go out.
        rig.ctl.expect(UttId(1), false);
        rig.feed(pcm(100, 7));
        assert!(!rig.ctl.synth_done(), "the pump closes it");
        after(50).await;
        assert!(played.lock().is_empty(), "the utterance is still playing");
        after(100).await;
        assert_eq!(*played.lock(), [100]);
        assert_eq!(rig.times().last(), Some(&80));

        // A warm-up line is synthesised and never played: nobody is told.
        rig.ctl.expect(UttId(2), true);
        rig.feed(pcm(100, 7));
        assert!(!rig.ctl.synth_done());
        // An utterance that was over before its synthesis ended: the caller runs the step.
        rig.ctl.expect(UttId(3), false);
        assert!(rig.ctl.synth_done());
        after(200).await;
        assert_eq!(played.lock().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn chunks_that_arrive_before_the_audio_runs_out_are_one_stretch() {
        // With a lead the first frames go out at once, so the slot of the next one is
        // already due while the provider's next chunk is still on its way.
        let mut rig = Rig::new(40, 0);
        rig.ctl.expect(UttId(1), false);
        rig.feed(pcm(40, 7));
        after(5).await;
        rig.feed(pcm(40, 7));
        after(70).await; // 5 ms before the 80 ms delivered so far have played
        rig.feed(pcm(40, 7));
        after(5).await;
        rig.ctl.synth_done();
        after(200).await;
        assert_eq!(
            rig.events(),
            [
                PumpEvent::Started {
                    utt: UttId(1),
                    at: 0
                },
                PumpEvent::SynthDone { utt: UttId(1) },
                // Finished: idle at once, with nothing more to wait for.
                PumpEvent::Idle {
                    utt: UttId(1),
                    delivered_ms: 120,
                    at: 80
                },
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_provider_that_falls_behind_the_playout_is_a_gap() {
        let mut rig = Rig::new(150, 0);
        rig.ctl.expect(UttId(1), false);
        rig.feed(pcm(40, 7));
        after(39).await;
        assert_eq!(rig.events().len(), 1, "started, and still playing");
        after(2).await;
        assert!(matches!(
            rig.events()[..],
            [PumpEvent::Idle {
                delivered_ms: 40,
                ..
            }]
        ));
        // Its next chunk starts it again.
        rig.feed(pcm(40, 7));
        after(1).await;
        assert!(matches!(rig.events()[..], [PumpEvent::Started { .. }]));
    }

    #[tokio::test(start_paused = true)]
    async fn the_reflex_ducks_only_over_audio_and_only_with_barge_in_on() {
        let rig = Rig::new(0, 0);
        assert!(!rig.ctl.reflex_duck(), "nothing is playing");
        rig.speak(1, vec![pcm(200, 10_000)]);
        after(10).await;
        rig.ctl.set_duck(-6.0, 0, false);
        assert!(!rig.ctl.reflex_duck(), "barge-in is off");
        rig.ctl.set_duck(-6.0, 0, true);
        assert!(rig.ctl.reflex_duck());
        after(20).await;
        assert_eq!(last_sample(&rig.sent.lock().last().unwrap().1), 5011);
    }

    #[tokio::test(start_paused = true)]
    async fn discard_audio_never_reaches_the_sink() {
        let mut rig = Rig::new(40, 0);
        rig.ctl.expect(UttId(9), true);
        rig.feed(pcm(200, 3));
        rig.feed(pcm(200, 3));
        rig.ctl.synth_done();
        after(500).await;

        assert!(rig.sent.lock().is_empty());
        assert!(!rig.ctl.is_audible());
        assert_eq!(rig.events(), [PumpEvent::SynthDone { utt: UttId(9) }]);
    }

    #[tokio::test(start_paused = true)]
    async fn an_utterance_with_no_audio_is_closed_at_synth_done() {
        let mut rig = Rig::new(0, 0);
        after(30).await;
        rig.ctl.expect(UttId(3), false);
        rig.ctl.synth_done();
        rig.ctl.synth_done(); // nothing expected any more
        assert_eq!(
            rig.events(),
            [
                PumpEvent::SynthDone { utt: UttId(3) },
                PumpEvent::Idle {
                    utt: UttId(3),
                    delivered_ms: 0,
                    at: 30
                },
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn the_next_utterance_follows_without_a_gap() {
        let mut rig = Rig::new(0, 0);
        rig.speak(1, vec![pcm(40, 1)]);
        rig.speak(2, vec![pcm(40, 2)]);
        after(200).await;

        assert_eq!(rig.times(), [0, 20, 40, 60]);
        assert_eq!(
            rig.events()[2..],
            [
                PumpEvent::Started {
                    utt: UttId(1),
                    at: 0
                },
                PumpEvent::Idle {
                    utt: UttId(1),
                    delivered_ms: 40,
                    at: 40
                },
                PumpEvent::Started {
                    utt: UttId(2),
                    at: 40
                },
                PumpEvent::Idle {
                    utt: UttId(2),
                    delivered_ms: 40,
                    at: 80
                },
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_utterance_stops_and_takes_no_more_audio() {
        let mut rig = Rig::new(0, 0);
        rig.ctl.expect(UttId(1), false);
        rig.feed(pcm(200, 1));
        after(50).await;
        rig.ctl.fail(UttId(1));
        after(200).await;

        assert_eq!(rig.times(), [0, 20, 40]);
        assert!(rig.ctl.route(pcm(20, 1), true).is_none());
        assert_eq!(
            rig.events(),
            [PumpEvent::Started {
                utt: UttId(1),
                at: 0
            }]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn other_formats_pass_whole_with_pause_and_cut() {
        let mp3 = || AudioData {
            data: vec![0xAB; 10],
            sample_rate: 24_000,
            format: "mp3".to_string(),
            duration_ms: Some(100),
        };
        let rig = Rig::new(0, 0);
        rig.speak(1, vec![mp3(), mp3(), mp3()]);
        rig.ctl.gate(GateCmd::Duck); // no gain on a container
        after(50).await;
        rig.ctl.gate(GateCmd::Pause);
        after(200).await;
        assert_eq!(rig.times(), [0]);

        rig.ctl.gate(GateCmd::Resume);
        after(50).await;
        assert_eq!(rig.times(), [0, 250]);
        assert_eq!(rig.ctl.cut(), [(UttId(1), 200)]);
        after(500).await;
        assert_eq!(rig.times(), [0, 250]);
        for (_, unit) in rig.sent.lock().iter() {
            assert_eq!(
                (unit.data.as_slice(), unit.duration_ms),
                (&[0xAB; 10][..], Some(100))
            );
        }
    }

    #[test]
    fn cost_meter_keeps_average_and_max() {
        let meter = CostMeter::default();
        assert_eq!(meter.avg_max_us(), (0, 0));
        meter.record(Duration::from_micros(100));
        meter.record(Duration::from_micros(300));
        assert_eq!(meter.avg_max_us(), (200, 300));
    }
}
