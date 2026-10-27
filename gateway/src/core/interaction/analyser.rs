//! The per-session audio analyser: the echo defence, Silero voice activity on 32 ms chunks,
//! onset and the reflex duck, Smart Turn on demand (on a task of its own, so the audio is read
//! while it runs). It turns caller audio into `DuplexEvent`s for the actor.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use bytes::Bytes;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::core::audio::{PumpControl, StreamResampler};
use crate::core::silero_vad::{SileroVAD, SileroVADConfig};
use crate::core::smart_turn::{
    MelStream, SMART_TURN_MAX_FRAMES, SmartTurnDetector, SmartTurnDetectorConfig,
};
use crate::utils::simd_ops::pcm_to_float_simd;

use super::actor::Msg;
use super::config::{DuplexConfig, Tri};
use super::echo::{Defence, to_pcm16};
use super::types::{DuplexEvent, Ms};

/// Both models take 16 kHz audio; Silero takes it in 512-sample chunks (32 ms).
const MODEL_RATE: u32 = 16_000;
const CHUNK: usize = 512;
const CHUNK_MS: u32 = 32;
/// Voiced chunks in a row that make an onset (64 ms).
const ONSET_CHUNKS: u32 = 2;
/// Voiced chunks between two reports of a stretch's level (128 ms).
const LEVEL_CHUNKS: u32 = 4;
/// Silence after the last voiced chunk at which a `Silence` event goes out. The first also ends
/// the stretch of speech (`Offset`); the last repeats for as long as the silence lasts, so a
/// request-response speech-to-text buffer that holds no speech is discarded and never grows.
const SILENCE_MARKS: [u32; 3] = [200, 700, 1500];
/// How long after the last voiced chunk an `Offset` is stamped: the first silence mark in
/// whole chunks.
pub const OFFSET_LAG_MS: u32 = SILENCE_MARKS[0].div_ceil(CHUNK_MS) * CHUNK_MS;
/// How long before now the caller's voice began, at an onset: the stretch's voice is the
/// onset's chunks (`ONSET_CHUNKS`, 64 ms, the stretch's own `voiced_ms`), and the audio after
/// the last of them (`after_ms` read, `queued_ms` waiting) was heard before it was read. The
/// plan's verdict "by 900 ms at the latest" counts from there (live S1 on Deepgram: counted
/// from the end of the first voiced chunk, every deadline cut came 32 ms late).
fn onset_behind_ms(after_ms: u64, queued_ms: u64) -> u64 {
    u64::from(CHUNK_MS * ONSET_CHUNKS) + after_ms + queued_ms
}

/// A caller who speaks over the agent's echo dips under it between syllables: within a
/// stretch of the caller's speech, voiced audio that the echo explains still counts as the
/// caller's for this many chunks (192 ms) after a chunk that was the caller's.
const BRIDGE_CHUNKS: u32 = 6;

/// An echo report this long after the last one starts another stretch of echo.
const ECHO_EPISODE_GAP_MS: Ms = 1_000;
/// `duplex.echo.auto_enable_after_echo_episodes` counts the stretches of echo in this long.
const ECHO_EPISODE_WINDOW_MS: Ms = 30_000;

/// A frame of the caller's audio, and whether the speech-to-text gets it from the analyser
/// (the echo canceller runs: [`SttFeed`]) rather than as it came.
pub type TapFrame = (Bytes, bool);

/// The speech-to-text side of the echo canceller. While the canceller runs the analyser sets
/// `on` (the VoiceManager's `stt_from_tap`), and the cancelled audio of each frame taken for
/// the speech-to-text goes to `tx` as PCM16 at the input rate. `on` is cleared when the
/// analyser stops. Never awaited: a speech-to-text that holds its lock (an upload) must never
/// stall the analyser, whose echo gate loses the echo's lag when it falls behind the clock;
/// audio beyond [`STT_FEED_FRAMES`] waiting for a recogniser that does not take it is dropped.
pub struct SttFeed {
    pub on: Arc<AtomicBool>,
    pub tx: mpsc::Sender<Bytes>,
}

/// The audio the speech-to-text feed holds for a recogniser that waits: 30 s of 20 ms frames.
pub const STT_FEED_FRAMES: usize = 1_500;

/// Clears the switch however the analyser ends: the speech-to-text gets its audio as it
/// comes again.
struct FeedOff(Option<Arc<AtomicBool>>);

impl Drop for FeedOff {
    fn drop(&mut self) {
        if let Some(on) = &self.0 {
            on.store(false, Ordering::Release);
        }
    }
}

/// Spawn the analyser over the caller's audio (PCM16 at `sample_rate`, from the VoiceManager's
/// audio tap). It ends when `rx` closes or the actor is gone. If a model cannot be loaded it
/// sends `Degraded("interaction_missing_build_features")` once and stops.
pub fn spawn(
    rx: mpsc::Receiver<TapFrame>,
    sample_rate: u32,
    ctl: Arc<PumpControl>,
    tx: mpsc::Sender<Msg>,
    cfg: DuplexConfig,
    stt: Option<SttFeed>,
) -> JoinHandle<()> {
    spawn_loading(rx, sample_rate, ctl, tx, cfg, stt).0
}

/// [`spawn`], and what settles once its models are loaded (or failed to load): `ready` waits
/// for it, at most [`MODELS_WAIT`].
pub fn spawn_loading(
    rx: mpsc::Receiver<TapFrame>,
    sample_rate: u32,
    ctl: Arc<PumpControl>,
    tx: mpsc::Sender<Msg>,
    cfg: DuplexConfig,
    stt: Option<SttFeed>,
) -> (JoinHandle<()>, oneshot::Receiver<()>) {
    // The pump keeps a copy of what it delivers only for a defence that may read it.
    ctl.set_echo_reference(cfg.echo.correlation_gate || cfg.echo.aec != Tri::Off);
    let (loaded, models) = oneshot::channel();
    (
        tokio::spawn(run(rx, sample_rate, ctl, tx, cfg, stt, loaded)),
        models,
    )
}

/// The longest `ready` waits for a session's analyser models (Silero and Smart Turn load in
/// 0.3 to 1 s, a cold gateway's first session up to 4.5 s on a loaded machine): past it the
/// session is ready all the same, and the caller's audio is kept until they are there.
pub const MODELS_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// Waits for `models` (from [`spawn_loading`]), at most [`MODELS_WAIT`]. `true`: they loaded
/// (or failed) in time.
pub async fn models_loaded(models: oneshot::Receiver<()>) -> bool {
    tokio::time::timeout(MODELS_WAIT, models).await.is_ok()
}

/// Cancelled 16 kHz audio as PCM16 at the input rate.
fn to_input_rate(clean: &[f32], rate: u32, resampler: &mut StreamResampler) -> Bytes {
    let resampled = resampler.resample(clean, MODEL_RATE, rate);
    let samples = resampled.as_deref().unwrap_or(clean);
    let mut pcm = Vec::with_capacity(samples.len() * 2);
    for sample in samples {
        pcm.extend_from_slice(&to_pcm16(*sample).to_le_bytes());
    }
    Bytes::from(pcm)
}

struct Models {
    vad: SileroVAD,
    turn: TurnModel,
}

impl Models {
    /// Default configurations only: the stubs of a build without the features have no others.
    async fn load() -> anyhow::Result<Self> {
        Ok(Self {
            vad: SileroVAD::new(SileroVADConfig::default()).await?,
            turn: TurnModel {
                // The last 8 s.
                mel: MelStream::new(SMART_TURN_MAX_FRAMES),
                detector: SmartTurnDetector::new(SmartTurnDetectorConfig::default()).await?,
            },
        })
    }
}

/// Smart Turn and its spectrogram.
struct TurnModel {
    mel: MelStream,
    detector: SmartTurnDetector,
}

/// What the audio loop tells the Smart Turn task.
enum ToTurn {
    /// The next chunk of the caller's audio (16 kHz).
    Heard(Vec<f32>),
    /// The probability that the caller's turn is complete, from the audio so far; the tag (the
    /// stretch of speech) comes back with it.
    Ask(u32),
}

impl TurnModel {
    /// Run the model on a task of its own, which keeps the spectrogram of the last 8 s as the
    /// audio comes: a question costs one inference (about 100 ms in a debug build, where the
    /// spectrogram of 8 s at once took 270 ms more), and the audio loop hears an onset
    /// meanwhile. The task reads in order; of the questions it finds waiting together only the
    /// last is answered (the others are for stretches the caller has spoken again since). With
    /// no answer the turn ends at the silence ceiling.
    fn spawn(self) -> (mpsc::UnboundedSender<ToTurn>, mpsc::Receiver<(u32, f32)>) {
        let (tell, mut told) = mpsc::unbounded_channel();
        let (answer, answers) = mpsc::channel(1);
        tokio::spawn(async move {
            let Self {
                mut mel,
                mut detector,
            } = self;
            while let Some(first) = told.recv().await {
                let mut asked = None;
                let mut next = Some(first);
                while let Some(message) = next {
                    match message {
                        ToTurn::Heard(chunk) => {
                            if let Err(error) = mel.push(&chunk) {
                                debug!(%error, "interaction analyser: spectrogram failed");
                            }
                        }
                        ToTurn::Ask(tag) => asked = Some((tag, mel.frames())),
                    }
                    next = told.try_recv().ok();
                }
                let Some((tag, frames)) = asked else {
                    continue;
                };
                // The inference is synchronous: on the blocking pool it holds no runtime
                // worker, so the audio loop (and other sessions) run meanwhile.
                let runtime = tokio::runtime::Handle::current();
                let worked = tokio::task::spawn_blocking(move || {
                    let p = fast_cores::run(|| runtime.block_on(detector.predict(&frames)));
                    (detector, p)
                });
                let Ok((back, p)) = worked.await else {
                    return;
                };
                detector = back;
                match p {
                    Ok(result) => {
                        let p = model_probability(result.probability);
                        if answer.send((tag, p)).await.is_err() {
                            return;
                        }
                    }
                    Err(error) => debug!(%error, "interaction analyser: smart turn failed"),
                }
            }
        });
        (tell, answers)
    }
}

async fn run(
    mut rx: mpsc::Receiver<TapFrame>,
    sample_rate: u32,
    ctl: Arc<PumpControl>,
    tx: mpsc::Sender<Msg>,
    cfg: DuplexConfig,
    stt: Option<SttFeed>,
    loaded: oneshot::Sender<()>,
) {
    let _feed_off = FeedOff(stt.as_ref().map(|feed| feed.on.clone()));
    ctl.set_duck(cfg.duck_db, cfg.duck_ramp_ms, cfg.barge_in);
    let degraded = Msg::Duplex(DuplexEvent::Degraded("interaction_missing_build_features"));
    // The caller's audio while the models load is kept and heard once they are there: the tap
    // holds only a few frames, and the first session of a gateway loads them for seconds (live
    // MA3, load 20: 4.5 s, and the caller's first request was never heard). The echo gate
    // takes these frames as they are read, before the agent has said anything.
    let mut early: VecDeque<TapFrame> = VecDeque::new();
    let load = Models::load();
    tokio::pin!(load);
    let loaded_models = loop {
        tokio::select! {
            models = &mut load => break models,
            frame = rx.recv() => {
                let Some(frame) = frame else {
                    return;
                };
                if early.len() == STT_FEED_FRAMES {
                    early.pop_front();
                }
                early.push_back(frame);
            }
        }
    };
    let _ = loaded.send(());
    let Models { mut vad, turn } = match loaded_models {
        Ok(models) => models,
        Err(error) => {
            warn!(%error, "interaction analyser: a model did not load");
            let _ = tx.send(degraded).await;
            return;
        }
    };

    let mut resampler = StreamResampler::new();
    let mut pending: Vec<f32> = Vec::with_capacity(4 * CHUNK);
    let mut tracker = Tracker::new();
    let threshold = vad.config().threshold;
    let (turn, mut answers) = turn.spawn();
    // Stretches of speech so far: a probability asked for in one is stale in the next.
    let mut stretch = 0u32;
    let mut defence = Defence::new(cfg.echo.correlation_gate);
    defence.set_max_delay(cfg.echo.max_bulk_delay_ms);
    // Stretches of echo lately, and the canceller turned on by them (`aec: auto`).
    let mut episodes: VecDeque<Ms> = VecDeque::new();
    let mut last_echo: Option<Ms> = None;
    let mut auto_aec = false;
    let mut back = StreamResampler::new();
    // Chunks since echo was last reported.
    let mut echo_quiet = LEVEL_CHUNKS;
    let mut words_first = false;
    // Explained chunks that may still count as the caller's (`BRIDGE_CHUNKS`).
    let mut bridge = 0u32;
    loop {
        // The audio kept while the models loaded is worked through at once, and its silence
        // before the first speech is not reported: the speech-to-text already holds what came
        // after it, and a silence ceiling would flush that (live MA3: the request was heard as
        // "[BLANK AUDIO]").
        let quiet_backlog = !early.is_empty() && stretch == 0;
        let frame = match early.pop_front() {
            Some(frame) => Some(frame),
            None => tokio::select! {
                frame = rx.recv() => frame,
                Some((asked_in, p)) = answers.recv() => {
                    // Not once the caller has spoken again: the duplex machine would read it
                    // as the verdict on the new speech.
                    let turn = DuplexEvent::SmartTurn { at: ctl.now_ms(), p };
                    if asked_in == stretch && tx.send(Msg::Duplex(turn)).await.is_err() {
                        return;
                    }
                    continue;
                }
            },
        };
        let Some((frame, for_stt)) = frame else {
            return;
        };
        // The line is known once the caller's track is there. A phone line gets the echo
        // canceller; one without it opens a barge-in on the caller's words, not on a sound
        // that may be the agent's own.
        let phone_leg = ctl.phone_leg();
        defence.set_aec(cfg.echo.aec_on(phone_leg) || auto_aec);
        // From now on the speech-to-text hears what the canceller leaves.
        if let Some(feed) = &stt {
            feed.on.store(defence.aec(), Ordering::Release);
        }
        if words_first != (phone_leg && !defence.aec()) {
            words_first = !words_first;
            let event = DuplexEvent::WordsFirst(words_first);
            if tx.send(Msg::Duplex(event)).await.is_err() {
                return;
            }
        }
        // Audio that came after this frame and waits to be read, in ms (frames are alike).
        let frame_ms = frame.len() as u64 * 500 / u64::from(sample_rate.max(1));
        let queued_ms = (rx.len() + early.len()) as u64 * frame_ms;
        let samples = pcm_to_float_simd(&frame);
        let resampled = resampler.resample(&samples, sample_rate, MODEL_RATE);
        let samples = resampled.as_deref().unwrap_or(&samples);
        if defence.active() {
            let before = pending.len();
            defence.feed(samples, ctl.now_us(), ctl.reference(), &mut pending);
            if let Some(feed) = stt.as_ref().filter(|_| for_stt && defence.aec()) {
                let clean = to_input_rate(&pending[before..], sample_rate, &mut back);
                if !clean.is_empty()
                    && let Err(mpsc::error::TrySendError::Closed(_)) = feed.tx.try_send(clean)
                {
                    return;
                }
            }
        } else {
            pending.extend_from_slice(samples);
        }

        let mut done = 0;
        while pending.len() - done >= CHUNK {
            let chunk = &pending[done..done + CHUNK];
            done += CHUNK;
            let began = std::time::Instant::now();
            let end = defence
                .position()
                .saturating_sub((pending.len() - done) as u64);
            // With the canceller on, the detector hears the caller's channel as it came: the
            // canceller thins a caller who speaks over the echo, and the echo it removed is
            // told from the caller below.
            let came = defence.aec().then(|| defence.came(end)).flatten();
            // The detector reads each chunk behind the previous one's tail (its context).
            let voiced = match vad.probability(came.as_deref().unwrap_or(chunk)) {
                Ok(probability) => probability > threshold,
                Err(error) => {
                    warn!(%error, "interaction analyser: voice activity failed");
                    let _ = tx.send(degraded).await;
                    return;
                }
            };
            // Gone only with the analyser.
            let _ = turn.send(ToTurn::Heard(chunk.to_vec()));
            let power = chunk.iter().map(|s| s * s).sum::<f32>() / CHUNK as f32;
            // Voiced audio that the agent's own delivered audio explains is not the caller.
            let explained = voiced && defence.explains(end, chunk, ctl.reference());
            let bridged = explained && tracker.in_speech && bridge > 0;
            if voiced && !explained {
                bridge = BRIDGE_CHUNKS;
            } else if bridged {
                bridge -= 1;
            }
            let caller = voiced && (!explained || bridged);

            for edge in tracker.chunk(caller, power) {
                if quiet_backlog && matches!(edge, Edge::Silence(_)) {
                    continue;
                }
                let at = ctl.now_ms();
                let event = match edge {
                    Edge::Onset { level_db } => {
                        stretch += 1;
                        // The duplex machine's deadlines count from the caller's voice (live
                        // K1, room path: 900 ms from the onset was 900 ms from when the
                        // analyser got to it).
                        let after = (pending.len() - done) as u64 * 1000 / u64::from(MODEL_RATE);
                        let behind = onset_behind_ms(after, queued_ms);
                        // The reflex: duck the agent now; the duplex machine decides the rest
                        // (the control knows whether barge-in is still on).
                        DuplexEvent::Onset {
                            at: at.saturating_sub(behind),
                            level_db,
                            reflex_ducked: !words_first && ctl.reflex_duck(),
                        }
                    }
                    Edge::Level { level_db } => DuplexEvent::Level { level_db },
                    Edge::Offset { voiced_ms } => DuplexEvent::Offset { at, voiced_ms },
                    Edge::Silence(ms) => DuplexEvent::Silence { at, ms },
                };
                if tx.send(Msg::Duplex(event)).await.is_err() {
                    return;
                }
                if edge == Edge::Silence(SILENCE_MARKS[0]) {
                    let _ = turn.send(ToTurn::Ask(stretch));
                }
            }
            echo_quiet += 1;
            if echo_quiet >= LEVEL_CHUNKS && defence.heard() {
                echo_quiet = 0;
                let now = ctl.now_ms();
                // `aec: auto`: enough stretches of echo lately turn the canceller on for the
                // rest of the session.
                if last_echo.is_none_or(|at| now >= at + ECHO_EPISODE_GAP_MS) {
                    episodes.push_back(now);
                }
                last_echo = Some(now);
                while episodes
                    .front()
                    .is_some_and(|at| now >= at + ECHO_EPISODE_WINDOW_MS)
                {
                    episodes.pop_front();
                }
                let after = cfg.echo.auto_enable_after_echo_episodes as usize;
                if cfg.echo.aec == Tri::Auto
                    && !auto_aec
                    && !defence.aec()
                    && after > 0
                    && episodes.len() >= after
                {
                    auto_aec = true;
                    info!(
                        episodes = episodes.len(),
                        "interaction echo canceller turned on"
                    );
                }
                let echo = DuplexEvent::Echo { at: now };
                if tx.send(Msg::Duplex(echo)).await.is_err() {
                    return;
                }
            }
            ctl.chunk_cost().record(began.elapsed());
            // The actor must get to what was just sent before the next chunk is worked on:
            // with audio waiting, this loop would otherwise not give its thread up.
            tokio::task::yield_now().await;
        }
        pending.drain(..done);
    }
}

/// A short burst of work on the machine's fast cores, where its cores differ in speed (a
/// big.LITTLE CPU). Smart Turn took 95 ms on this build machine's Cortex-X925 cores and 590 ms
/// on its Cortex-A725 ones, and a busy machine puts a short burst on whichever core is free:
/// about one inference in four in the live runs, each then too late for the pause it was
/// asked in. Elsewhere, or when the thread may not run there, the work runs where it is.
mod fast_cores {
    /// Cores of at least this share of the fastest core's capacity count as fast.
    const FAST_SHARE: f32 = 0.9;

    /// The fast cores from each core's capacity: `None` when they are all alike.
    pub(super) fn of(capacities: &[u32]) -> Option<Vec<usize>> {
        let top = *capacities.iter().max()? as f32;
        let fast: Vec<usize> = (0..capacities.len())
            .filter(|&cpu| capacities[cpu] as f32 >= top * FAST_SHARE)
            .collect();
        (fast.len() < capacities.len()).then_some(fast)
    }

    #[cfg(target_os = "linux")]
    pub(super) fn run<T>(work: impl FnOnce() -> T) -> T {
        use std::sync::OnceLock;

        /// `cpu_set_t`: 1,024 cores.
        type CpuSet = [u64; 16];
        // glibc's, which std links already.
        unsafe extern "C" {
            fn sched_getaffinity(pid: i32, size: usize, set: *mut CpuSet) -> i32;
            fn sched_setaffinity(pid: i32, size: usize, set: *const CpuSet) -> i32;
        }
        static FAST: OnceLock<Option<CpuSet>> = OnceLock::new();
        let fast = FAST.get_or_init(|| {
            // The kernel's relative capacity of each core (1,024 for the fastest).
            let capacity = |cpu: usize| {
                let path = format!("/sys/devices/system/cpu/cpu{cpu}/cpu_capacity");
                std::fs::read_to_string(path).ok()?.trim().parse().ok()
            };
            let capacities: Vec<u32> = (0..1_024).map_while(capacity).collect();
            let mut set = [0u64; 16];
            for cpu in of(&capacities)? {
                set[cpu / 64] |= 1 << (cpu % 64);
            }
            Some(set)
        });
        let Some(fast) = fast else {
            return work();
        };
        let size = std::mem::size_of::<CpuSet>();
        let mut was = [0u64; 16];
        // SAFETY: both calls read or write one `CpuSet` of `size` bytes; pid 0 is this thread.
        if unsafe { sched_getaffinity(0, size, &mut was) } != 0 {
            return work();
        }
        let mut on = was;
        on.iter_mut().zip(fast).for_each(|(on, fast)| *on &= fast);
        // SAFETY: as above.
        let moved = on != [0; 16] && unsafe { sched_setaffinity(0, size, &on) } == 0;
        let result = work();
        if moved {
            // SAFETY: as above. A blocking-pool thread goes back to every core it had.
            unsafe { sched_setaffinity(0, size, &was) };
        }
        result
    }

    #[cfg(not(target_os = "linux"))]
    pub(super) fn run<T>(work: impl FnOnce() -> T) -> T {
        work()
    }
}

/// The model's own probability, from what `SmartTurnDetector` reports. The model file ends in
/// a sigmoid and the detector applies a second one, so it reports between 0.5 and 0.731: every
/// finished sentence came out as 0.72 to 0.73, and no turn could reach the eager threshold.
/// The detector is left as it is: sessions without interaction mode read it too.
fn model_probability(reported: f32) -> f32 {
    (reported / (1.0 - reported)).ln().clamp(0.0, 1.0)
}

/// What one chunk changed, before it gets a time.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Edge {
    Onset {
        level_db: f32,
    },
    /// The level of the stretch's voiced chunks so far.
    Level {
        level_db: f32,
    },
    Offset {
        voiced_ms: u32,
    },
    Silence(u32),
}

/// Voiced or not, chunk by chunk, to onsets, offsets and silence marks. No clock, no model.
struct Tracker {
    /// Voiced chunks in a row.
    voiced_run: u32,
    in_speech: bool,
    /// Voiced time of the current stretch of speech.
    voiced_ms: u32,
    /// Time since the last voiced chunk.
    silence_ms: u32,
    /// The next entry of `SILENCE_MARKS`; the last one repeats.
    next_mark: usize,
    /// Mean square of the previous chunk: the onset level covers both onset chunks.
    last_power: f32,
    /// Sum of the mean squares of the stretch's voiced chunks.
    power: f32,
}

impl Tracker {
    fn new() -> Self {
        Self {
            voiced_run: 0,
            in_speech: false,
            voiced_ms: 0,
            silence_ms: 0,
            // Before any speech only the last mark is due.
            next_mark: SILENCE_MARKS.len() - 1,
            last_power: 0.0,
            power: 0.0,
        }
    }

    /// The level of the stretch's voiced chunks so far.
    fn level_db(&self) -> f32 {
        let chunks = (self.voiced_ms / CHUNK_MS) as f32;
        10.0 * (self.power / chunks).max(1e-10).log10()
    }

    /// `power` is the mean square of the chunk's samples.
    fn chunk(&mut self, voiced: bool, power: f32) -> Vec<Edge> {
        let mut edges = Vec::new();
        if voiced {
            self.voiced_run += 1;
            self.silence_ms = 0;
            if self.in_speech {
                self.voiced_ms += CHUNK_MS;
                self.power += power;
                if (self.voiced_ms / CHUNK_MS).is_multiple_of(LEVEL_CHUNKS) {
                    edges.push(Edge::Level {
                        level_db: self.level_db(),
                    });
                }
            } else if self.voiced_run == ONSET_CHUNKS {
                self.in_speech = true;
                self.voiced_ms = ONSET_CHUNKS * CHUNK_MS;
                self.power = self.last_power + power;
                self.next_mark = 0;
                edges.push(Edge::Onset {
                    level_db: self.level_db(),
                });
            }
        } else {
            self.voiced_run = 0;
            self.silence_ms += CHUNK_MS;
            if self.in_speech && self.silence_ms >= SILENCE_MARKS[0] {
                self.in_speech = false;
                edges.push(Edge::Offset {
                    voiced_ms: self.voiced_ms,
                });
            }
            if !self.in_speech && self.silence_ms >= SILENCE_MARKS[self.next_mark] {
                edges.push(Edge::Silence(SILENCE_MARKS[self.next_mark]));
                if self.next_mark + 1 < SILENCE_MARKS.len() {
                    self.next_mark += 1;
                } else {
                    self.silence_ms = 0;
                }
            }
        }
        self.last_power = power;
        edges
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `chunks` chunks in a row, voiced or not, at -20 dBFS: the edges without the level
    /// reports.
    fn feed(tracker: &mut Tracker, voiced: bool, chunks: u32) -> Vec<Edge> {
        (0..chunks)
            .flat_map(|_| tracker.chunk(voiced, 0.01))
            .filter(|edge| !matches!(edge, Edge::Level { .. }))
            .collect()
    }

    /// The onset is the start of its first voiced chunk: both onset chunks (64 ms, the
    /// stretch's `voiced_ms` at the onset) and whatever audio came after them.
    #[test]
    fn the_onset_counts_from_the_start_of_the_first_voiced_chunk() {
        let mut tracker = Tracker::new();
        feed(&mut tracker, true, ONSET_CHUNKS);
        assert_eq!(onset_behind_ms(0, 0), u64::from(tracker.voiced_ms));
        assert_eq!(onset_behind_ms(0, 0), 64);
        // 10 ms read after the second chunk, two 20 ms frames waiting.
        assert_eq!(onset_behind_ms(10, 40), 114);
    }

    #[test]
    fn an_onset_is_two_voiced_chunks_in_a_row() {
        let mut tracker = Tracker::new();
        assert_eq!(feed(&mut tracker, true, 1), []);
        assert_eq!(feed(&mut tracker, false, 1), []);
        assert_eq!(feed(&mut tracker, true, 1), [], "one voiced chunk again");
        assert_eq!(
            feed(&mut tracker, true, 1),
            [Edge::Onset { level_db: -20.0 }]
        );
        assert_eq!(feed(&mut tracker, true, 20), [], "one onset per stretch");
    }

    #[test]
    fn speech_ends_with_an_offset_and_the_three_silence_marks() {
        let mut tracker = Tracker::new();
        feed(&mut tracker, true, 10);
        assert_eq!(feed(&mut tracker, false, 6), [], "192 ms is still a pause");
        assert_eq!(
            feed(&mut tracker, false, 1),
            [Edge::Offset { voiced_ms: 320 }, Edge::Silence(200)]
        );
        assert_eq!(feed(&mut tracker, false, 14), []);
        assert_eq!(feed(&mut tracker, false, 1), [Edge::Silence(700)]);
        assert_eq!(feed(&mut tracker, false, 24), []);
        assert_eq!(feed(&mut tracker, false, 1), [Edge::Silence(1500)]);
        // Only the last mark repeats, every 1,500 ms of further silence.
        assert_eq!(feed(&mut tracker, false, 46), []);
        assert_eq!(feed(&mut tracker, false, 1), [Edge::Silence(1500)]);
    }

    #[test]
    fn a_pause_under_200_ms_keeps_the_speech_going() {
        let mut tracker = Tracker::new();
        feed(&mut tracker, true, 2);
        assert_eq!(feed(&mut tracker, false, 6), []);
        assert_eq!(feed(&mut tracker, true, 3), [], "no second onset");
        assert_eq!(
            feed(&mut tracker, false, 7),
            [Edge::Offset { voiced_ms: 160 }, Edge::Silence(200)]
        );
        // The next stretch starts over.
        assert_eq!(
            feed(&mut tracker, true, 2),
            [Edge::Onset { level_db: -20.0 }]
        );
        assert_eq!(
            feed(&mut tracker, false, 7),
            [Edge::Offset { voiced_ms: 64 }, Edge::Silence(200)]
        );
    }

    #[test]
    fn silence_without_speech_gives_only_the_repeating_last_mark() {
        let mut tracker = Tracker::new();
        assert_eq!(feed(&mut tracker, false, 46), []);
        assert_eq!(feed(&mut tracker, false, 1), [Edge::Silence(1500)]);
        // A lone voiced chunk is no onset, and the silence counts from it again.
        assert_eq!(feed(&mut tracker, false, 40), []);
        assert_eq!(feed(&mut tracker, true, 1), []);
        assert_eq!(feed(&mut tracker, false, 46), []);
        assert_eq!(feed(&mut tracker, false, 1), [Edge::Silence(1500)]);
    }

    #[test]
    fn the_onset_level_is_the_power_of_both_onset_chunks() {
        let mut tracker = Tracker::new();
        tracker.chunk(true, 0.03);
        let edges = tracker.chunk(true, 0.01);
        // Mean square 0.02 is -17 dBFS.
        assert!(
            matches!(&edges[..], [Edge::Onset { level_db }] if (level_db + 16.99).abs() < 0.01)
        );
        assert_eq!(
            Tracker::new().chunk(true, 0.0),
            [],
            "digital silence has a finite level"
        );
    }

    #[test]
    fn the_fast_cores_are_those_near_the_fastest_and_only_when_cores_differ() {
        // This build machine: five slow cores, five fast, five slow, five fast.
        let gb10 = [
            718, 718, 718, 718, 718, 997, 997, 997, 997, 997, 731, 731, 731, 731, 731, 1017, 1017,
            1017, 1017, 1024,
        ];
        assert_eq!(
            fast_cores::of(&gb10),
            Some(vec![5, 6, 7, 8, 9, 15, 16, 17, 18, 19])
        );
        assert_eq!(fast_cores::of(&[1024; 8]), None);
        assert_eq!(fast_cores::of(&[]), None);
        // The work runs, wherever it may.
        assert_eq!(fast_cores::run(|| 7), 7);
    }

    /// The thread runs the work on the fast cores it may use (on this build machine, the
    /// Cortex-X925 ones) and has all its cores back after it.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_burst_runs_on_the_fast_cores_and_the_thread_gets_its_cores_back() {
        let cores = || -> Vec<usize> {
            let status = std::fs::read_to_string("/proc/thread-self/status").expect("status");
            let list = status
                .lines()
                .find_map(|line| line.strip_prefix("Cpus_allowed_list:"))
                .expect("Cpus_allowed_list");
            let range = |part: &str| -> Vec<usize> {
                let mut ends = part.split('-').map(|n| n.parse::<usize>().expect("a core"));
                let first = ends.next().expect("a core");
                (first..=ends.next().unwrap_or(first)).collect()
            };
            list.trim().split(',').flat_map(range).collect()
        };
        let capacity = |cpu: usize| {
            let path = format!("/sys/devices/system/cpu/cpu{cpu}/cpu_capacity");
            std::fs::read_to_string(path).ok()?.trim().parse().ok()
        };
        let capacities: Vec<u32> = (0..1_024).map_while(capacity).collect();
        let before = cores();
        let during = fast_cores::run(cores);
        assert_eq!(cores(), before, "the thread has its cores back");
        let expected: Vec<usize> = match fast_cores::of(&capacities) {
            Some(fast) => before
                .iter()
                .copied()
                .filter(|cpu| fast.contains(cpu))
                .collect(),
            None => before.clone(),
        };
        if !expected.is_empty() {
            assert_eq!(during, expected);
        }
    }

    #[test]
    fn the_detectors_second_sigmoid_is_undone() {
        let reported = |p: f32| 1.0 / (1.0 + (-p).exp());
        for p in [0.0, 0.3, 0.7, 0.8, 1.0] {
            assert!((model_probability(reported(p)) - p).abs() < 1e-4, "{p}");
        }
        // What the detector reports with no audio, and anything outside its range.
        assert_eq!(model_probability(0.0), 0.0);
        assert_eq!(model_probability(0.9), 1.0);
    }

    #[test]
    fn a_stretch_reports_the_level_of_its_voiced_chunks_every_128_ms() {
        let mut tracker = Tracker::new();
        // A soft start: the two onset chunks are 20 dB below the rest of the word.
        tracker.chunk(true, 0.0001);
        assert_eq!(
            tracker.chunk(true, 0.0001),
            [Edge::Onset { level_db: -40.0 }]
        );
        assert_eq!(tracker.chunk(true, 0.01), []);
        let level = |edges: Vec<Edge>| match edges[..] {
            [Edge::Level { level_db }] => level_db,
            _ => panic!("no level report in {edges:?}"),
        };
        // Four voiced chunks: mean square 0.00505 is -23 dBFS.
        assert!((level(tracker.chunk(true, 0.01)) + 22.97).abs() < 0.01);
        // A pause inside the stretch is not part of the level.
        assert_eq!(tracker.chunk(false, 0.0), []);
        for _ in 0..3 {
            assert_eq!(tracker.chunk(true, 0.01), []);
        }
        // Eight voiced chunks: mean square 0.007525 is -21.2 dBFS.
        assert!((level(tracker.chunk(true, 0.01)) + 21.23).abs() < 0.01);
        // The next stretch starts over.
        for _ in 0..7 {
            tracker.chunk(false, 0.0);
        }
        tracker.chunk(true, 0.01);
        assert_eq!(tracker.chunk(true, 0.01), [Edge::Onset { level_db: -20.0 }]);
    }

    /// `ready` waits for the models, never longer than `MODELS_WAIT`: a session whose models
    /// take longer is ready all the same (its audio is kept until they are there).
    #[tokio::test(start_paused = true)]
    async fn ready_waits_for_the_models_at_most_models_wait() {
        let (loaded, models) = oneshot::channel::<()>();
        let start = tokio::time::Instant::now();
        let waiting = tokio::spawn(models_loaded(models));
        tokio::time::sleep(std::time::Duration::from_millis(700)).await;
        loaded.send(()).expect("still waiting");
        assert!(waiting.await.expect("the wait ended"), "loaded in time");
        assert_eq!(start.elapsed(), std::time::Duration::from_millis(700));

        let (_slow, models) = oneshot::channel::<()>();
        let start = tokio::time::Instant::now();
        assert!(!models_loaded(models).await, "past the bound");
        assert_eq!(start.elapsed(), MODELS_WAIT);

        // A model that failed to load settles the wait at once.
        let (failed, models) = oneshot::channel::<()>();
        drop(failed);
        assert!(models_loaded(models).await);
    }

    /// The real models: `spawn_loading` settles once both are loaded, before any audio came.
    #[cfg(all(feature = "silero-vad", feature = "smart-turn"))]
    #[tokio::test]
    #[ignore = "loads the Silero and Smart Turn models (needs ORT_DYLIB_PATH)"]
    async fn live_the_models_settle_the_wait_once_loaded() {
        let (pump_events, _pump_rx) = mpsc::unbounded_channel();
        let ctl = Arc::new(PumpControl::new(pump_events, 0));
        let (_audio, rx) = mpsc::channel::<TapFrame>(8);
        let (tx, _inbox) = mpsc::channel(8);
        let start = std::time::Instant::now();
        let (task, models) = spawn_loading(rx, 16_000, ctl, tx, DuplexConfig::default(), None);
        assert!(models_loaded(models).await, "loaded within MODELS_WAIT");
        eprintln!("models loaded in {} ms", start.elapsed().as_millis());
        task.abort();
    }

    /// A build without the model features has only the stubs: the analyser says so and stops.
    #[cfg(not(all(feature = "silero-vad", feature = "smart-turn")))]
    #[tokio::test]
    async fn a_build_without_the_models_degrades_once_and_stops() {
        let (pump_events, _) = mpsc::unbounded_channel();
        let ctl = Arc::new(PumpControl::new(pump_events, 0));
        let (audio, rx) = mpsc::channel(8);
        let (tx, mut inbox) = mpsc::channel(8);
        let task = spawn(rx, 16_000, ctl, tx, DuplexConfig::default(), None);

        assert!(matches!(
            inbox.recv().await,
            Some(Msg::Duplex(DuplexEvent::Degraded(
                "interaction_missing_build_features"
            )))
        ));
        task.await.expect("the task ended");
        assert!(inbox.recv().await.is_none(), "nothing after it");
        assert!(audio.is_closed(), "the tap finds the channel closed");
    }

    /// The real models on real speech: 24 kHz audio is resampled, the speech gives an onset, and
    /// its end gives the offset, the silence marks and a Smart Turn probability, in that order.
    /// `ORT_DYLIB_PATH=.../libonnxruntime.so cargo test ... live_analyser -- --ignored`
    #[cfg(all(feature = "silero-vad", feature = "smart-turn"))]
    #[tokio::test]
    #[ignore = "loads the Silero and Smart Turn models (needs ORT_DYLIB_PATH)"]
    async fn live_analyser_follows_speech_then_silence() {
        let speech = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/live_testing/audio/tts_output_medium_phrase.raw"
        ))
        .expect("24 kHz PCM16 fixture");
        let (pump_events, _pump_rx) = mpsc::unbounded_channel();
        let ctl = Arc::new(PumpControl::new(pump_events, 0));
        let (audio, rx) = mpsc::channel(1024);
        let (tx, mut inbox) = mpsc::channel(256);
        let task = spawn(rx, 24_000, ctl.clone(), tx, DuplexConfig::default(), None);

        // 20 ms frames of the phrase, then 2 s of silence in real time: Smart Turn runs beside
        // the audio loop, and its answer is only read while the audio lasts.
        for frame in speech.chunks(960) {
            audio
                .send((Bytes::copy_from_slice(frame), false))
                .await
                .expect("analyser alive");
        }
        for _ in 0..100 {
            audio
                .send((Bytes::from_static(&[0u8; 960]), false))
                .await
                .expect("analyser alive");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        drop(audio);

        let mut events = Vec::new();
        while let Some(Msg::Duplex(event)) = inbox.recv().await {
            if !matches!(event, DuplexEvent::Level { .. }) {
                events.push(event);
            }
        }
        task.await.expect("the task ended");

        assert!(
            matches!(
                events
                    .iter()
                    .find(|e| !matches!(e, DuplexEvent::Silence { .. })),
                Some(DuplexEvent::Onset {
                    reflex_ducked: false,
                    ..
                })
            ),
            "speech starts with an onset, and no agent audio was there to duck: {events:?}"
        );
        let end = events
            .iter()
            .rposition(|e| matches!(e, DuplexEvent::Offset { .. }))
            .expect("the speech ended");
        // The probability follows the first silence mark, whenever the model is done.
        let turn = events[end..]
            .iter()
            .position(|e| matches!(e, DuplexEvent::SmartTurn { p, .. } if (0.0..=1.0).contains(p)))
            .expect("a turn probability after the speech");
        assert!(turn >= 2, "{events:?}");
        events.remove(end + turn);
        assert!(
            matches!(
                &events[end..],
                [
                    DuplexEvent::Offset { voiced_ms, .. },
                    DuplexEvent::Silence { ms: 200, .. },
                    DuplexEvent::Silence { ms: 700, .. },
                    DuplexEvent::Silence { ms: 1500, .. },
                ] if *voiced_ms >= 250
            ),
            "{events:?}"
        );
        let (average, max) = ctl.chunk_cost().avg_max_us();
        assert!(average > 0 && max >= average);
    }

    /// The caller speaks while the models load (a cold gateway's first session): the tap holds
    /// only `TAP_FRAMES` (64), yet the speech is heard once the models are there, and the
    /// silence before it is not reported.
    /// `ORT_DYLIB_PATH=.../libonnxruntime.so cargo test ... live_analyser -- --ignored`
    #[cfg(all(feature = "silero-vad", feature = "smart-turn"))]
    #[tokio::test]
    #[ignore = "loads the Silero and Smart Turn models (needs ORT_DYLIB_PATH)"]
    async fn live_analyser_hears_speech_that_came_while_its_models_loaded() {
        let speech = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/live_testing/audio/tts_output_medium_phrase.raw"
        ))
        .expect("24 kHz PCM16 fixture");
        let (pump_events, _pump_rx) = mpsc::unbounded_channel();
        let ctl = Arc::new(PumpControl::new(pump_events, 0));
        let (audio, rx) = mpsc::channel(64);
        let (tx, mut inbox) = mpsc::channel(4096);
        let task = spawn(rx, 24_000, ctl, tx, DuplexConfig::default(), None);
        // 2 s of silence, then the phrase, ten times faster than real time and as the tap
        // sends them (a frame that finds the queue full is dropped): far more than 64 frames
        // before the models are loaded.
        let silence = vec![0u8; 960];
        let frames = std::iter::repeat_n(&silence[..], 100).chain(speech.chunks(960));
        let mut dropped = 0;
        for frame in frames {
            dropped += usize::from(
                audio
                    .try_send((Bytes::copy_from_slice(frame), false))
                    .is_err(),
            );
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        for _ in 0..100 {
            let _ = audio.send((Bytes::from_static(&[0u8; 960]), false)).await;
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        drop(audio);
        let mut events = Vec::new();
        while let Some(Msg::Duplex(event)) = inbox.recv().await {
            events.push(event);
        }
        task.await.expect("the task ended");
        assert_eq!(dropped, 0, "the analyser took every frame while loading");
        // The 2 s of silence before the speech came while loading: no silence ceiling is
        // reported for it, which would flush the speech-to-text's audio of the speech.
        let onset = events
            .iter()
            .position(|e| matches!(e, DuplexEvent::Onset { .. }));
        assert!(
            !events[..onset.unwrap_or(0)]
                .iter()
                .any(|e| matches!(e, DuplexEvent::Silence { ms: 1500, .. })),
            "{events:?}"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, DuplexEvent::Onset { .. }))
                && events.iter().any(
                    |e| matches!(e, DuplexEvent::Offset { voiced_ms, .. } if *voiced_ms >= 250)
                ),
            "{events:?}"
        );
    }

    /// The Smart Turn task answers from the audio up to the question, not what came after it,
    /// and of two questions waiting together only the last.
    /// `ORT_DYLIB_PATH=.../libonnxruntime.so cargo test ... live_analyser -- --ignored`
    #[cfg(all(feature = "silero-vad", feature = "smart-turn"))]
    #[tokio::test]
    #[ignore = "loads the Silero and Smart Turn models (needs ORT_DYLIB_PATH)"]
    async fn live_analyser_answers_the_last_question_from_the_audio_before_it() {
        let speech = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/live_testing/audio/tts_output_medium_phrase.raw"
        ))
        .expect("24 kHz PCM16 fixture");
        let samples = pcm_to_float_simd(&speech);
        let audio = StreamResampler::new()
            .resample(&samples, 24_000, MODEL_RATE)
            .expect("resampled");
        let (before, after) = audio.split_at(audio.len() * 2 / 3);

        let Models { turn, .. } = Models::load().await.expect("the models load");
        let (tell, mut answers) = turn.spawn();
        let heard = |audio: &[f32]| {
            for chunk in audio.chunks(CHUNK) {
                tell.send(ToTurn::Heard(chunk.to_vec()))
                    .expect("task alive");
            }
        };
        heard(&before[..CHUNK * 4]);
        tell.send(ToTurn::Ask(1)).expect("task alive");
        heard(&before[CHUNK * 4..]);
        tell.send(ToTurn::Ask(2)).expect("task alive");
        heard(after);
        drop(tell);
        let (tag, p) = answers.recv().await.expect("an answer");
        assert!(answers.recv().await.is_none(), "one answer");
        assert_eq!(tag, 2);

        // The same audio, up to the question, at once.
        let Models { turn, .. } = Models::load().await.expect("the models load");
        let TurnModel {
            mut mel,
            mut detector,
        } = turn;
        mel.push(before).expect("spectrogram");
        let alone = detector.predict(&mel.frames()).await.expect("inference");
        assert_eq!(p, model_probability(alone.probability));
    }

    /// The real voice activity model on the agent's own audio coming back: the pump's
    /// reference is written 150 ms ahead, and the caller's channel carries the line 200 ms
    /// late and 6 dB down, alone or under a caller who starts 2 s in.
    /// `ORT_DYLIB_PATH=.../libonnxruntime.so cargo test ... live_analyser -- --ignored`
    #[cfg(all(feature = "silero-vad", feature = "smart-turn"))]
    #[tokio::test(start_paused = true)]
    #[ignore = "loads the Silero and Smart Turn models (needs ORT_DYLIB_PATH)"]
    async fn live_analyser_keeps_the_agents_echo_out_of_the_onsets() {
        use crate::core::interaction::config::{EchoConfig, Tri};

        const FRAME: usize = 960;
        let recording = |name: &str| {
            let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/live_testing/audio");
            std::fs::read(format!("{dir}/tts_output_{name}.raw")).expect("24 kHz PCM16 fixture")
        };
        let (agent, voice) = (recording("medium_phrase"), recording("numbers"));
        let sample = |pcm: &[u8], at: usize| {
            let bytes = pcm.get(at * 2..at * 2 + 2);
            bytes.map_or(0, |bytes| i16::from_le_bytes([bytes[0], bytes[1]])) as i32
        };

        // One call of 5 s; returns what the analyser reported, without the level reports.
        let call = async |echo: EchoConfig, phone: bool, caller_from: Option<usize>| {
            let (pump_events, _pump_rx) = mpsc::unbounded_channel();
            let ctl = Arc::new(PumpControl::new(pump_events, 150));
            if phone {
                ctl.set_phone_leg();
            }
            let (audio, rx) = mpsc::channel(1024);
            let (tx, mut inbox) = mpsc::channel(4096);
            let cfg = DuplexConfig {
                echo,
                ..DuplexConfig::default()
            };
            let (fed, mut stt) = mpsc::channel(STT_FEED_FRAMES);
            let on = Arc::new(AtomicBool::new(false));
            let feed = SttFeed {
                on: on.clone(),
                tx: fed,
            };
            let task = spawn(rx, 24_000, ctl.clone(), tx, cfg, Some(feed));
            let began = ctl.now_us();
            for frame in 0..250 {
                // The pump is 150 ms ahead of the playout clock.
                let ahead = frame + 7;
                if let Some(pcm) = agent.chunks(FRAME).nth(ahead) {
                    let at = began + ahead as u64 * 20_000;
                    ctl.record_reference(at, pcm, 24_000);
                }
                let mut heard = Vec::with_capacity(FRAME);
                for at in frame * 480..(frame + 1) * 480 {
                    let echo = at.checked_sub(4800).map_or(0, |at| sample(&agent, at) / 2);
                    let caller = caller_from
                        .and_then(|from| at.checked_sub(from))
                        .map_or(0, |at| sample(&voice, at));
                    let both = (echo + caller).clamp(-32768, 32767) as i16;
                    heard.extend_from_slice(&both.to_le_bytes());
                }
                // Taken for the speech-to-text once the canceller runs.
                let for_stt = on.load(Ordering::Acquire);
                audio
                    .send((Bytes::from(heard), for_stt))
                    .await
                    .expect("analyser alive");
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            drop(audio);
            let mut events = Vec::new();
            while let Some(Msg::Duplex(event)) = inbox.recv().await {
                if !matches!(event, DuplexEvent::Level { .. }) {
                    events.push(event);
                }
            }
            task.await.expect("the task ended");
            assert!(
                !on.load(Ordering::Acquire),
                "the switch is off once the analyser ends"
            );
            // What the speech-to-text got from the canceller, as 24 kHz PCM16.
            let mut cancelled = Vec::new();
            while let Ok(pcm) = stt.try_recv() {
                cancelled.extend_from_slice(&pcm);
            }
            (events, cancelled)
        };
        let onsets = |events: &[DuplexEvent]| -> Vec<u64> {
            let onset = |event: &DuplexEvent| match event {
                DuplexEvent::Onset { at, .. } => Some(*at),
                _ => None,
            };
            events.iter().filter_map(onset).collect()
        };
        let echoes = |events: &[DuplexEvent]| {
            let echo = |event: &&DuplexEvent| matches!(event, DuplexEvent::Echo { .. });
            events.iter().filter(echo).count()
        };
        let none = EchoConfig {
            correlation_gate: false,
            aec: Tri::Off,
            ..EchoConfig::default()
        };

        // With no defence the echo is the caller, again and again.
        let (undefended, fed) = call(none, false, None).await;
        assert!(fed.is_empty(), "no canceller, no audio from it");
        assert!(!onsets(&undefended).is_empty(), "{undefended:?}");
        assert_eq!(echoes(&undefended), 0);

        // The gate: not one onset from the echo, and the actor is told that echo is heard.
        let (gated, _) = call(EchoConfig::default(), false, None).await;
        assert!(onsets(&gated).is_empty(), "{gated:?}");
        assert!(echoes(&gated) >= 10, "{} echo reports", echoes(&gated));

        // A caller who starts 2 s in, over the echo, makes the one onset.
        let (over, _) = call(EchoConfig::default(), false, Some(48_000)).await;
        let first = onsets(&over).first().copied().expect("the caller's onset");
        assert!((2_000..2_500).contains(&first), "onset at {first} ms");

        // A phone line gets the canceller as well, and says nothing about words.
        let (phone, cancelled) = call(EchoConfig::default(), true, None).await;
        // The speech-to-text of a phone line hears what the canceller left of the echo: from
        // the second second on, at least 10 dB less than the echo itself.
        let power = |pcm: &[u8], from: usize, to: usize| {
            let samples = pcm.as_chunks::<2>().0;
            let window = &samples[from.min(samples.len())..to.min(samples.len())];
            window
                .iter()
                .map(|s| f64::from(i16::from_le_bytes(*s)).powi(2))
                .sum::<f64>()
                / window.len().max(1) as f64
        };
        // 24 kHz PCM16 is 48,000 bytes a second: at least 4 of the call's 5 s.
        assert!(cancelled.len() >= 4 * 48_000, "{} bytes", cancelled.len());
        let echo_power = power(&agent, 24_000, 96_000) / 4.0;
        let left = power(&cancelled, 24_000 + 4800, 96_000 + 4800);
        assert!(
            10.0 * (echo_power / left.max(1.0)).log10() >= 10.0,
            "echo {echo_power:.0}, left {left:.0}"
        );
        assert!(onsets(&phone).is_empty(), "{phone:?}");
        assert!(
            !phone
                .iter()
                .any(|e| matches!(e, DuplexEvent::WordsFirst(_)))
        );
        let (over, _) = call(EchoConfig::default(), true, Some(48_000)).await;
        let first = onsets(&over).first().copied().expect("the caller's onset");
        assert!((2_000..2_500).contains(&first), "onset at {first} ms");

        // A phone line with the canceller off: barge-ins open on words, and no reflex duck.
        let (words_first, _) = call(none, true, None).await;
        assert_eq!(words_first.first(), Some(&DuplexEvent::WordsFirst(true)));
        assert!(!onsets(&words_first).is_empty());
    }
}
