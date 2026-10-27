//! Echo defence for the analyser's copy of the caller's audio. The pump records the agent's
//! audio as it delivers it ([`Reference`]). The probe finds where that audio comes back on the
//! caller's channel, and the correlation gate tells the voiced audio it explains from the
//! caller's own ([`Gate`]). WebRTC's echo canceller takes it out of the audio ([`Aec`]).
//! [`Defence`] runs both ahead of the voice activity model.
//!
//! Only the analyser hears the result: the speech-to-text gets the caller's audio as it came.

use std::collections::VecDeque;

use livekit::webrtc::native::apm::AudioProcessingModule;
use parking_lot::Mutex;
use tracing::debug;

use crate::core::audio::StreamResampler;
use crate::utils::simd_ops::pcm_to_float_simd;

/// Everything here runs at the analyser's rate.
const RATE: u64 = 16_000;
/// The echo canceller takes 10 ms blocks.
const BLOCK: usize = 160;
/// Mean square below which audio is silence (-50 dBFS).
const FLOOR: f32 = 1e-5;

/// The reference holds 2 s: the longest lag searched and the search window.
const RING: usize = 1 << 15;
/// A frame that starts within 20 ms of where the last one ended continues it.
const JOIN: u64 = 320;

/// The agent's audio as the pump delivered it, at 16 kHz, each sample at its playout time on
/// the session clock. What was never delivered reads as silence. Written through a [`Tap`].
pub struct Reference {
    ring: Vec<f32>,
    /// The position after the last sample written.
    end: u64,
}

impl Default for Reference {
    fn default() -> Self {
        Self {
            ring: vec![0.0; RING],
            end: 0,
        }
    }
}

/// The writer's side of a [`Reference`]: it turns each delivered frame into 16 kHz samples
/// before it takes the reference's lock, which the analyser reads under.
#[derive(Default)]
pub struct Tap {
    resampler: StreamResampler,
    /// Where the reference ends, as this tap wrote it.
    end: u64,
}

impl Tap {
    /// One delivered frame (PCM16 at `rate`) whose first sample plays `at_us` after the
    /// session started.
    pub fn write(&mut self, at_us: u64, pcm: &[u8], rate: u32, reference: &Mutex<Reference>) {
        let at = at_us * RATE / 1_000_000;
        if at.abs_diff(self.end) > JOIN {
            // A new stretch of audio, after silence, a pause or a cut.
            self.resampler.reset();
            self.end = at;
        }
        let samples = pcm_to_float_simd(pcm);
        let resampled = self.resampler.resample(&samples, rate, RATE as u32);
        let samples = resampled.as_deref().unwrap_or(&samples);
        reference.lock().put(self.end, samples);
        self.end += samples.len() as u64;
    }
}

impl Reference {
    /// The position after the last sample written; 0 before any.
    #[cfg(test)]
    pub(crate) fn written(&self) -> u64 {
        self.end
    }

    /// 16 kHz samples whose first plays at position `at`.
    fn put(&mut self, at: u64, samples: &[f32]) {
        if at.abs_diff(self.end) > JOIN {
            // Nothing played in between, and what was written past a cut never will.
            for position in self.end..at.min(self.end + RING as u64) {
                self.ring[position as usize % RING] = 0.0;
            }
            self.end = at;
        }
        for sample in samples {
            self.ring[self.end as usize % RING] = *sample;
            self.end += 1;
        }
    }

    /// The samples from position `from` on.
    fn read(&self, from: i64, out: &mut [f32]) {
        for (i, sample) in out.iter_mut().enumerate() {
            let position = from + i as i64;
            let held = position >= 0
                && (position as u64) < self.end
                && position as u64 + RING as u64 > self.end;
            *sample = if held {
                self.ring[position as usize % RING]
            } else {
                0.0
            };
        }
    }

    /// Whether agent audio played in the second before `position`.
    fn recent(&self, position: u64) -> bool {
        self.end > 0 && self.end + RATE > position
    }
}

/// One sample as PCM16, as the echo canceller and the speech-to-text take it.
pub(super) fn to_pcm16(sample: f32) -> i16 {
    (sample * 32768.0).clamp(-32768.0, 32767.0) as i16
}

fn energy(samples: &[f32]) -> f32 {
    samples.iter().map(|sample| sample * sample).sum()
}

/// The dot product of two slices of one length. Indexed, not zipped: the lag search runs it
/// two thousand times per window, and an unoptimised build pays for every adaptor.
#[allow(clippy::needless_range_loop)]
fn dot(a: &[f32], b: &[f32]) -> f32 {
    let mut sum = 0.0;
    for i in 0..a.len() {
        sum += a[i] * b[i];
    }
    sum
}

/// The echo comes back between 60 ms before and 1 s after its playout time on the pump's
/// clock (a client may play a frame the moment it arrives, ahead of its slot).
const LAG_MIN: i64 = -960;
const LAG_MAX: i64 = 16_000;
/// The probe correlates the last 256 ms with the reference at every lag, both decimated to
/// 2 kHz.
const WINDOW: usize = 4096;
const DECIMATE: usize = 8;
/// The share of the window's energy the best lag must explain to be tried (a correlation of
/// 0.5).
const FOUND: f32 = 0.25;
/// The analyser's chunk: the window is judged in these.
const CHUNK: usize = 512;
/// A chunk is fitted within 0.25 ms of the lag, which follows the fit.
const TRACK: i64 = 4;
/// A chunk is echo when the reference leaves at most a share of its energy unexplained. The
/// share is twice what the path leaves of pure echo and a tenth, between these two: a
/// correlation of 0.84 on a clean path (one resampler against another leaves up to 0.2 of a
/// consonant) and of 0.6 on a poor one.
const TIGHT: f32 = 0.3;
const LOOSE: f32 = 0.64;
/// The unexplained shares of the last 64 chunks the lag explained. Their lower quartile is
/// what the path leaves of pure echo.
const SHARES: usize = 64;
/// A lag is taken when it explains half of what the window's chunks carry.
const LOST: f32 = 0.5;
/// A chunk no louder than the echo can be is echo too, however a codec has changed its
/// waveform: at most this much above the path's measured level (3 dB).
const LEVEL_MARGIN: f32 = 2.0;
const LEVEL_REACH: usize = 64;
/// No echo is more than 12 dB louder than what was sent: a lag that says so is another voice.
const GAIN_MAX: f32 = 16.0;
/// With a lag that does not explain a chunk, the probe looks again at once (every line of
/// the agent comes back a few milliseconds earlier or later than the last: a client starts
/// to play on its own tick) and then every 128 ms. After half a second of such chunks the
/// lag is given up.
const RETRY_CHUNKS: u32 = 4;
const LOST_CHUNKS: u32 = 16;
/// A chunk below the floor this soon after explained echo is the echo's tail.
const TAIL_CHUNKS: u32 = 3;

/// The echo probe and the correlation gate. The probe locks the lag at which the reference
/// explains the caller's channel and measures how loud the echo is against the reference.
/// The gate then calls a chunk echo when the reference at that lag explains it about as well
/// as it explains this path's pure echo, or when the chunk is no louder than the echo can
/// be. A caller who speaks over the echo leaves more unexplained, is louder, and passes.
#[derive(Default)]
pub struct Gate {
    /// The last `WINDOW` samples of the caller's channel and the position after them.
    raw: VecDeque<f32>,
    raw_end: u64,
    /// Caller position minus reference position, once locked.
    lag: Option<i64>,
    shares: VecDeque<f32>,
    /// The echo's energy over the reference's, measured on the chunks the lag explained.
    gain: f32,
    /// Chunks in a row that the lag did not explain, and chunks since the reference last
    /// explained one by its waveform: the level alone is trusted only near such a chunk.
    missed: u32,
    since_fit: u32,
    /// Chunks asked about since one was explained.
    since_echo: u32,
    /// Where the last chunk asked about ended, and how many came in a row before it.
    asked: u64,
    run: u32,
    /// The longest lag looked for, in samples (`duplex.echo.max_bulk_delay_ms`); 0 is
    /// `LAG_MAX`.
    max_lag: i64,
}

impl Gate {
    /// The caller's channel as it came, in order; `end` is the position after `block`.
    fn hear(&mut self, block: &[f32], end: u64) {
        self.raw.extend(block);
        let extra = self.raw.len().saturating_sub(WINDOW);
        self.raw.drain(..extra);
        self.raw_end = end;
    }

    /// The chunk of the caller's channel that ends at position `end`, as it came.
    fn came(&self, end: u64) -> Option<Vec<f32>> {
        let behind = self.raw_end.saturating_sub(end) as usize;
        let start = self.raw.len().checked_sub(behind + CHUNK)?;
        Some(self.raw.range(start..start + CHUNK).copied().collect())
    }

    /// Whether the agent's own audio explains the chunk of the caller's channel that ends at
    /// position `end`. Asked for voiced chunks only.
    fn explains(&mut self, end: u64, reference: &Mutex<Reference>) -> bool {
        let Some(chunk) = self.came(end) else {
            return false;
        };
        self.run = match end.checked_sub(self.asked) {
            Some(step) if step == CHUNK as u64 => self.run + 1,
            _ => 0,
        };
        self.asked = end;
        self.since_echo = self.since_echo.saturating_add(1);
        let explained = if energy(&chunk) < FLOOR * CHUNK as f32 {
            // Too quiet to judge, and the voice activity model still hears the echo's end.
            self.lag.is_some() && self.since_echo <= TAIL_CHUNKS
        } else {
            self.fits(&chunk, end, reference)
        };
        if explained {
            self.since_echo = 0;
        }
        explained
    }

    fn fits(&mut self, chunk: &[f32], end: u64, reference: &Mutex<Reference>) -> bool {
        if self.lag.is_some() {
            if self.judge(chunk, end, reference) {
                self.missed = 0;
                return true;
            }
            self.missed += 1;
            if self.missed >= LOST_CHUNKS {
                self.unlock();
            } else if self.missed != 1 && !self.missed.is_multiple_of(RETRY_CHUNKS) {
                return false;
            }
        }
        // No lag, or one that has stopped explaining: the probe runs on voiced audio only.
        // With no lag it runs on the second to fourth chunk of a stretch, so that an echo's
        // first chunks make no onset, and then every 256 ms: a caller who speaks over an
        // agent that makes no echo must not pay for a search with every chunk.
        let due = self.lag.is_some() || matches!(self.run, 1..=3) || self.run % 8 == 7;
        due && self.probe(reference) && self.judge(chunk, end, reference)
    }

    /// Judge a chunk against the locked lag.
    fn judge(&mut self, chunk: &[f32], end: u64, reference: &Mutex<Reference>) -> bool {
        let Some(lag) = self.lag else {
            return false;
        };
        let Some(fit) = unexplained(chunk, end, lag, TRACK, reference) else {
            return false;
        };
        let limit = (2.0 * quantile(&self.shares, 4) + 0.1).clamp(TIGHT, LOOSE);
        let fits = fit.share <= limit;
        // The loudest the echo of this stretch of the reference can be, 4 ms either way (a
        // codec moves the echo by more than the fit follows).
        let mut agent = vec![0.0; CHUNK + 2 * LEVEL_REACH];
        let from = end as i64 - CHUNK as i64 - lag - LEVEL_REACH as i64;
        reference.lock().read(from, &mut agent);
        let loudest = [0, LEVEL_REACH, 2 * LEVEL_REACH]
            .map(|start| energy(&agent[start..start + CHUNK]))
            .into_iter()
            .fold(0.0, f32::max);
        self.since_fit = self.since_fit.saturating_add(1);
        if fits {
            // Only what the lag explained says what the path leaves of pure echo and how
            // loud the echo is: a caller who speaks over it must not move either.
            if self.shares.len() == SHARES {
                self.shares.pop_front();
            }
            self.shares.push_back(fit.share);
            self.lag = Some(fit.lag);
            self.gain = 0.9 * self.gain + 0.1 * fit.gain;
            self.since_fit = 0;
        }
        fits || (self.since_fit <= LOST_CHUNKS
            && energy(chunk) <= LEVEL_MARGIN * self.gain * loudest)
    }

    /// Look for the lag at which the reference explains the last 256 ms of the caller's
    /// channel. Returns whether a new lag was locked.
    fn probe(&mut self, reference: &Mutex<Reference>) -> bool {
        let window: Vec<f32> = self.raw.iter().copied().collect();
        let end = self.raw_end;
        let max_lag = if self.max_lag > 0 {
            self.max_lag
        } else {
            LAG_MAX
        };
        let Some(coarse) = search(&window, end, reference, max_lag) else {
            return false;
        };
        // The search steps 0.5 ms: fit the whole window, then judge it chunk by chunk. The
        // lag holds when it explains half of what the window's chunks carry.
        let Some(whole) = unexplained(&window, end, coarse, DECIMATE as i64, reference) else {
            return false;
        };
        if self.lag.is_some_and(|lag| (lag - whole.lag).abs() <= TRACK) {
            // The lag it has: the caller is speaking over the echo.
            return false;
        }
        let mut shares = VecDeque::new();
        for (i, chunk) in window.as_rchunks::<CHUNK>().1.iter().rev().enumerate() {
            let chunk_end = end - (i * CHUNK) as u64;
            if let Some(fit) = unexplained(chunk, chunk_end, whole.lag, TRACK, reference) {
                shares.push_front(fit.share);
            }
        }
        if shares.len() < 2 || quantile(&shares, 2) > LOST || whole.gain > GAIN_MAX {
            return false;
        }
        debug!(
            lag_ms = whole.lag / 16,
            level_db = 10.0 * whole.gain.max(1e-12).log10(),
            "interaction echo path found"
        );
        if self
            .lag
            .is_none_or(|lag| (lag - whole.lag).abs() > 2 * DECIMATE as i64)
        {
            // Another path: what the old one left of pure echo says nothing about it.
            self.shares = shares;
            self.gain = whole.gain;
        }
        self.lag = Some(whole.lag);
        self.missed = 0;
        self.since_fit = 0;
        true
    }

    /// The lag is no longer known: the caller's channel moved against the clock, or the lag
    /// stopped explaining it.
    fn unlock(&mut self) {
        self.lag = None;
        self.shares.clear();
        self.missed = 0;
    }
}

/// The value a `part`-th of the way up the sorted shares: the lower quartile for 4, the
/// median for 2.
fn quantile(shares: &VecDeque<f32>, part: usize) -> f32 {
    let mut sorted: Vec<f32> = shares.iter().copied().collect();
    sorted.sort_by(f32::total_cmp);
    sorted.get(sorted.len() / part).copied().unwrap_or(1.0)
}

/// The lag at which the reference best explains `window` (which ends at `end`), if it
/// explains enough of it.
fn search(window: &[f32], end: u64, reference: &Mutex<Reference>, max_lag: i64) -> Option<i64> {
    let decimate = |samples: &[f32]| -> Vec<f32> {
        samples
            .as_chunks::<DECIMATE>()
            .0
            .iter()
            .map(|group| group.iter().sum::<f32>() / DECIMATE as f32)
            .collect()
    };
    if window.len() < CHUNK {
        return None;
    }
    let caller = decimate(window);
    let n = caller.len();
    let floor = FLOOR * n as f32;
    let caller_energy = energy(&caller);
    // The caller's side needs one chunk's worth of sound, as a chunk the gate is asked about
    // has: a line's soft first word is all its echo gives the window at first (live S4: the
    // second voiced chunk's window was below the whole window's floor, its lag was found one
    // chunk later, and the two chunks before made an onset).
    let heard = FLOOR * (CHUNK / DECIMATE) as f32;
    let mut agent = vec![0.0; window.len() + (max_lag - LAG_MIN) as usize];
    reference
        .lock()
        .read(end as i64 - window.len() as i64 - max_lag, &mut agent);
    let agent = decimate(&agent);
    if caller_energy < heard || energy(&agent) < floor {
        return None;
    }
    let mut agent_energy = energy(&agent[..n]);
    let mut best = (0.0, 0);
    for start in 0..=agent.len() - n {
        if start > 0 {
            agent_energy += agent[start + n - 1] * agent[start + n - 1];
            agent_energy -= agent[start - 1] * agent[start - 1];
        }
        if agent_energy < floor {
            continue;
        }
        let both = dot(&caller, &agent[start..start + n]);
        let explained = both * both / (caller_energy * agent_energy);
        if explained > best.0 {
            best = (explained, start);
        }
    }
    (best.0 >= FOUND).then(|| max_lag - (best.1 * DECIMATE) as i64)
}

/// How the reference explains a chunk at the best lag near the one asked about.
struct Fit {
    /// The share of the chunk's energy that the reference leaves unexplained.
    share: f32,
    lag: i64,
    /// The chunk's energy over the reference's.
    gain: f32,
}

/// The best fit of `chunk` (a chunk, or the probe's window) within `reach` of `lag`. `None`
/// when it, or the reference there, has less than a chunk's worth of sound.
fn unexplained(
    chunk: &[f32],
    end: u64,
    lag: i64,
    reach: i64,
    reference: &Mutex<Reference>,
) -> Option<Fit> {
    let n = chunk.len();
    // A chunk's worth of sound, also in the probe's whole window ([`search`]).
    let floor = FLOOR * n.min(CHUNK) as f32;
    let chunk_energy = energy(chunk);
    if chunk_energy < floor {
        return None;
    }
    let mut agent = vec![0.0; n + 2 * reach as usize];
    reference
        .lock()
        .read(end as i64 - n as i64 - lag - reach, &mut agent);
    let mut best: Option<Fit> = None;
    for shift in 0..=2 * reach as usize {
        let window = &agent[shift..shift + n];
        let agent_energy = energy(window);
        if agent_energy < floor {
            continue;
        }
        let both = dot(chunk, window);
        let share = 1.0 - both * both / (chunk_energy * agent_energy);
        if best.as_ref().is_none_or(|best| share < best.share) {
            best = Some(Fit {
                share,
                lag: lag + reach - shift as i64,
                gain: chunk_energy / agent_energy,
            });
        }
    }
    best
}

/// WebRTC's echo canceller (AEC3 with its high-pass filter; no gain control, no noise
/// suppression) from the libwebrtc build the gateway already links for LiveKit.
struct Aec {
    apm: AudioProcessingModule,
}

impl Aec {
    fn new() -> Self {
        Self {
            apm: AudioProcessingModule::new(true, false, true, false),
        }
    }

    /// Take the echo of `agent` (the reference at the block's own time) out of one 10 ms
    /// block. Returns whether most of the block's energy went. A block the module refuses is
    /// left as it came. Nothing is allocated: this runs a hundred times a second.
    fn cancel(&mut self, block: &mut [f32; BLOCK], agent: &[f32; BLOCK]) -> bool {
        let mut far = agent.map(to_pcm16);
        let mut near = block.map(to_pcm16);
        if self
            .apm
            .process_reverse_stream(&mut far, RATE as i32, 1)
            .and_then(|()| self.apm.process_stream(&mut near, RATE as i32, 1))
            .is_err()
        {
            return false;
        }
        let before = energy(block);
        for (sample, clean) in block.iter_mut().zip(near) {
            *sample = clean as f32 / 32768.0;
        }
        before >= FLOOR * BLOCK as f32 && energy(block) * 4.0 < before
    }
}

/// The caller's channel may move this far against the session clock before its position is
/// taken from the clock again (250 ms).
const DRIFT: u64 = 4000;

/// The echo canceller finds an echo that comes 20 to 300 ms after the reference it is given
/// and adapts anew whenever that reference moves. When the probe finds the echo outside that
/// range, the canceller gets the reference from 40 ms ahead of the echo.
const AEC_NEAR: i64 = 320;
const AEC_FAR: i64 = 4800;
const AEC_HEADROOM: i64 = 640;
/// A chunk the canceller left less than an eighth of (9 dB) was echo. A caller who speaks
/// over the echo keeps more: the canceller takes about 4 dB off such a voice.
const CANCELLED: f32 = 8.0;

/// The echo defence of one session: the caller's audio in, the audio the voice activity model
/// hears out.
pub struct Defence {
    /// The probe runs whenever a defence is on: it also tells the canceller where the echo is.
    probe: Gate,
    /// The correlation gate is on: what the probe explains is not the caller.
    gate: bool,
    aec: Option<Aec>,
    /// How far behind its playout time the canceller reads the reference.
    shift: i64,
    /// Caller audio short of a block.
    carry: Vec<f32>,
    /// The position of the next sample to come out; `None` before the first audio.
    position: Option<u64>,
    /// Echo was heard since the last [`Self::heard`].
    heard: bool,
}

impl Defence {
    pub fn new(gate: bool) -> Self {
        Self {
            probe: Gate::default(),
            gate,
            aec: None,
            shift: 0,
            carry: Vec::new(),
            position: None,
            heard: false,
        }
    }

    /// The longest delay at which the probe looks for the agent's audio (at most what the
    /// reference keeps less the probe's window).
    pub fn set_max_delay(&mut self, ms: u32) {
        let lag = i64::from(ms) * RATE as i64 / 1000;
        self.probe.max_lag = lag.clamp(LAG_MIN.abs(), RING as i64 - WINDOW as i64 - LAG_MIN.abs());
    }

    /// Switch the echo canceller.
    pub fn set_aec(&mut self, on: bool) {
        if on != self.aec.is_some() {
            self.aec = on.then(Aec::new);
        }
    }

    pub fn aec(&self) -> bool {
        self.aec.is_some()
    }

    /// Whether the audio goes through [`Self::feed`] at all.
    pub fn active(&self) -> bool {
        self.gate || self.aec.is_some()
    }

    /// The position after the last sample `feed` put out.
    pub fn position(&self) -> u64 {
        self.position.unwrap_or(0)
    }

    /// Caller audio that arrived `now_us` after the session started. Whole 10 ms blocks of it
    /// are appended to `out`, with the echo cancelled when the canceller is on.
    pub fn feed(
        &mut self,
        samples: &[f32],
        now_us: u64,
        reference: &Mutex<Reference>,
        out: &mut Vec<f32>,
    ) {
        // The audio ends now: on the clock the reference is written on, it began here.
        let end = now_us * RATE / 1_000_000;
        let began = end.saturating_sub((samples.len() + self.carry.len()) as u64);
        let position = match self.position {
            Some(position) if position.abs_diff(began) <= DRIFT => position,
            _ => {
                self.probe.unlock();
                began
            }
        };
        if let Some(lag) = self.probe.lag
            && !(AEC_NEAR..=AEC_FAR).contains(&(lag - self.shift))
        {
            self.shift = (lag - AEC_HEADROOM).max(0);
        }
        self.carry.extend_from_slice(samples);
        let whole = self.carry.len() / BLOCK * BLOCK;
        let mut agent = [0.0; BLOCK];
        let mut at = position;
        for block in self.carry[..whole].as_chunks_mut::<BLOCK>().0 {
            at += BLOCK as u64;
            self.probe.hear(block, at);
            if let Some(aec) = &mut self.aec {
                let from = at as i64 - BLOCK as i64 - self.shift;
                // One lock per block, held for the copy only.
                let recent = {
                    let reference = reference.lock();
                    reference.read(from, &mut agent);
                    reference.recent(at)
                };
                self.heard |= aec.cancel(block, &agent) && recent;
            }
        }
        out.extend(self.carry.drain(..whole));
        self.position = Some(at);
    }

    /// The chunk of the caller's channel that ends at position `end`, as it came.
    pub fn came(&self, end: u64) -> Option<Vec<f32>> {
        self.probe.came(end)
    }

    /// Whether the agent's own audio explains the voiced chunk of the caller's channel that
    /// ends at position `end`; `clean` is that chunk as `feed` put it out. The gate judges the
    /// channel as it came, also when the canceller is on: what the canceller leaves of an
    /// echo while it adapts no longer looks like it. The canceller explains a chunk it left
    /// less than an eighth of. With the gate off the probe still runs, for the canceller.
    pub fn explains(&mut self, end: u64, clean: &[f32], reference: &Mutex<Reference>) -> bool {
        let gated = self.probe.explains(end, reference) && self.gate;
        let cancelled = self.aec.is_some()
            && self.probe.came(end).is_some_and(|came| {
                energy(clean) * CANCELLED < energy(&came) && reference.lock().recent(end)
            });
        self.heard |= gated || cancelled;
        gated || cancelled
    }

    /// Whether echo was heard since the last call: a chunk the gate explained, or a block the
    /// canceller emptied.
    pub fn heard(&mut self) -> bool {
        std::mem::take(&mut self.heard)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AGENT_RATE: u32 = 24_000;
    /// A 20 ms frame of the agent's audio, in bytes and in reference samples.
    const FRAME_BYTES: usize = 960;
    const FRAME: usize = 320;

    /// A 24 kHz PCM16 recording of the synthesiser.
    fn recording(name: &str) -> Vec<u8> {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/live_testing/audio");
        std::fs::read(format!("{dir}/tts_output_{name}.raw")).expect("24 kHz PCM16 fixture")
    }

    /// The pump of a test: it delivers a recording from the start of the session, each
    /// frame 150 ms ahead of its playout time.
    struct Pump {
        reference: Mutex<Reference>,
        tap: Tap,
        pcm: Vec<u8>,
        delivered: usize,
    }

    impl Pump {
        fn new(name: &str) -> Self {
            Self {
                reference: Mutex::default(),
                tap: Tap::default(),
                pcm: recording(name),
                delivered: 0,
            }
        }

        /// Deliver what the pump has sent by the time the caller's channel is at `position`.
        fn run_to(&mut self, position: usize) {
            while self.delivered * FRAME < position + 2400
                && let Some(frame) = self.pcm.as_chunks::<FRAME_BYTES>().0.get(self.delivered)
            {
                let at_us = self.delivered as u64 * 20_000;
                self.tap.write(at_us, frame, AGENT_RATE, &self.reference);
                self.delivered += 1;
            }
        }
    }

    /// A recording as the reference holds it: what the client plays, at 16 kHz.
    fn played(name: &str) -> Vec<f32> {
        let mut pump = Pump::new(name);
        let mut samples = Vec::new();
        let mut frame = [0.0; FRAME];
        while samples.len() + FRAME <= pump.pcm.len() / 3 {
            pump.run_to(samples.len());
            pump.reference.lock().read(samples.len() as i64, &mut frame);
            samples.extend(frame);
        }
        samples
    }

    /// The caller's channel with the agent's audio in it `lag` samples late and 6 dB down.
    fn echo(agent: &[f32], lag: usize) -> Vec<f32> {
        let mut caller = vec![0.0; lag];
        caller.extend(agent.iter().map(|sample| sample * 0.5));
        caller
    }

    /// Run the caller's channel through a gate while the agent's line plays: for each chunk
    /// that is not silence, whether the gate explained it.
    fn gated(caller: &[f32]) -> Vec<Option<bool>> {
        gated_with(caller, Gate::default())
    }

    fn gated_with(caller: &[f32], mut gate: Gate) -> Vec<Option<bool>> {
        let mut pump = Pump::new("medium_phrase");
        let mut explained = Vec::new();
        for (i, chunk) in caller.as_chunks::<CHUNK>().0.iter().enumerate() {
            let start = i * CHUNK;
            pump.run_to(start + CHUNK);
            for (j, block) in chunk.chunks(BLOCK).enumerate() {
                let end = start + ((j + 1) * BLOCK).min(CHUNK);
                gate.hear(block, end as u64);
            }
            let end = (start + CHUNK) as u64;
            let voiced = energy(chunk) >= FLOOR * CHUNK as f32;
            explained.push(voiced.then(|| gate.explains(end, &pump.reference)));
        }
        explained
    }

    #[test]
    fn the_reference_holds_what_was_delivered_at_its_playout_time() {
        let pcm = recording("medium_phrase");
        let delivered = |at_us: u64| {
            let (mut tap, reference) = (Tap::default(), Mutex::default());
            for (i, frame) in pcm[..96_000].chunks(FRAME_BYTES).enumerate() {
                tap.write(at_us + i as u64 * 20_000, frame, AGENT_RATE, &reference);
            }
            (tap, reference)
        };
        // The first frame plays 0.5 s into the session.
        let (_, reference) = delivered(500_000);
        let reference = reference.lock();
        let mut before = vec![1.0; 8000];
        reference.read(0, &mut before);
        assert!(before.iter().all(|sample| *sample == 0.0), "silence first");
        let mut during = vec![0.0; 16_000];
        reference.read(8000, &mut during);
        assert!(energy(&during) > FLOOR * 16_000.0, "then the line");
        assert!(reference.recent(8000 + 30_000) && !reference.recent(8000 + 60_000));

        // A cut restarts the playout clock before the end of what was written: the rest was
        // never played.
        let (mut tap, cut) = delivered(0);
        tap.write(1_000_000, &[0u8; FRAME_BYTES], AGENT_RATE, &cut);
        let mut after = vec![1.0; 8000];
        cut.lock().read(16_320, &mut after);
        assert!(after.iter().all(|sample| *sample == 0.0));
        // And a line that starts after a silence leaves silence behind it.
        tap.write(2_000_000, &pcm[..FRAME_BYTES], AGENT_RATE, &cut);
        let mut between = vec![1.0; 15_000];
        cut.lock().read(16_500, &mut between);
        assert!(between.iter().all(|sample| *sample == 0.0));
    }

    #[test]
    fn the_gate_explains_the_agents_own_audio_and_not_a_caller() {
        // The line comes back 200 ms late, 6 dB down. The probe runs on its first chunks: at
        // most one of them passes, and an onset takes two in a row.
        let explained = gated(&echo(&played("medium_phrase"), 3200));
        let judged: Vec<bool> = explained.iter().flatten().copied().collect();
        assert!(judged.len() >= 100, "{} chunks judged", judged.len());
        let missed = judged.iter().filter(|explained| !**explained).count();
        assert!(
            missed <= 1,
            "{missed} of {} chunks of pure echo",
            judged.len()
        );

        // Another voice while the agent speaks, and no echo: nothing is explained.
        let caller = played("numbers");
        let explained = gated(&caller[..80_000]);
        let judged: Vec<bool> = explained.iter().flatten().copied().collect();
        assert!(judged.len() >= 100, "{} chunks judged", judged.len());
        let taken = judged.iter().filter(|explained| **explained).count();
        assert_eq!(taken, 0, "of {} chunks of a caller", judged.len());
    }

    #[test]
    fn an_echo_later_than_the_max_bulk_delay_is_not_looked_for() {
        // The line comes back 600 ms late: found with the default 1 s, not within 400 ms.
        let caller = echo(&played("medium_phrase"), 9600);
        let share = |explained: Vec<Option<bool>>| {
            let judged: Vec<bool> = explained.iter().flatten().copied().collect();
            judged.iter().filter(|explained| **explained).count() * 100 / judged.len().max(1)
        };
        assert!(share(gated(&caller)) >= 90);
        let mut defence = Defence::new(true);
        defence.set_max_delay(400);
        assert_eq!(defence.probe.max_lag, 6400);
        assert_eq!(share(gated_with(&caller, defence.probe)), 0);
    }

    #[test]
    fn a_caller_who_speaks_over_the_echo_passes_the_gate() {
        let mut caller = echo(&played("medium_phrase"), 3200);
        // From 1.28 s on the caller speaks too, at the level of the echo.
        let from = 40 * CHUNK;
        let voice = played("numbers");
        for (i, sample) in caller.iter_mut().enumerate().skip(from) {
            *sample += 0.5 * voice[i - from];
        }
        let explained = gated(&caller);
        let echo_only: Vec<bool> = explained[24..40].iter().flatten().copied().collect();
        assert!(echo_only.len() >= 10 && echo_only.iter().all(|explained| *explained));
        // An onset is two voiced chunks in a row that echo does not explain: it comes
        // within the caller's first 320 ms.
        let both = &explained[40..];
        let onset = both
            .windows(2)
            .position(|pair| pair == [Some(false), Some(false)]);
        assert!(onset.is_some_and(|chunk| chunk < 10), "{onset:?}: {both:?}");
        // A chunk in which the caller is at least as loud as the echo is the caller's; in the
        // caller's pauses it is the agent's echo again.
        let agent = echo(&played("medium_phrase"), 3200);
        let levels = |chunk: usize| {
            let at = from + chunk * CHUNK;
            let caller = 0.25 * energy(&voice[chunk * CHUNK..][..CHUNK]);
            (caller, energy(&agent[at..at + CHUNK]))
        };
        let verdicts = |wanted: &dyn Fn(f32, f32) -> bool| -> Vec<bool> {
            (0..both.len())
                .filter(|chunk| {
                    let (caller, agent) = levels(*chunk);
                    wanted(caller, agent)
                })
                .filter_map(|chunk| both[chunk])
                .collect()
        };
        // The level rule takes a chunk for echo up to 3 dB above the echo's own level.
        let louder = verdicts(&|caller, agent| caller >= 2.0 * agent);
        let passed = louder.iter().filter(|explained| !**explained).count();
        assert!(louder.len() >= 40, "{} chunks", louder.len());
        assert_eq!(
            passed,
            louder.len(),
            "of the chunks the caller is louder in"
        );
        let pauses = verdicts(&|caller, _| caller < FLOOR * CHUNK as f32);
        let explained = pauses.iter().filter(|explained| **explained).count();
        assert!(pauses.len() >= 20, "{} chunks", pauses.len());
        assert!(
            explained * 10 >= pauses.len() * 9,
            "{explained} of {} chunks of echo in the caller's pauses",
            pauses.len()
        );
    }

    /// A client starts each line on its own tick: the echo of the next line comes back a
    /// few milliseconds earlier or later than that of the last.
    #[test]
    fn the_next_line_is_found_again_at_its_own_lag() {
        let agent = played("medium_phrase");
        // The echo of the first 2.5 s at 200 ms, nothing for a second, then 205 ms.
        let mut caller = vec![0.0; agent.len() + 3280];
        for (i, sample) in agent.iter().enumerate() {
            match i {
                0..40_000 => caller[i + 3200] = 0.5 * sample,
                56_000.. => caller[i + 3280] = 0.5 * sample,
                _ => {}
            }
        }
        let explained = gated(&caller);
        let again = &explained[(56_000 + 3280) / CHUNK..];
        let judged: Vec<bool> = again.iter().flatten().copied().collect();
        let missed = judged.iter().filter(|explained| !**explained).count();
        assert!(judged.len() >= 40, "{} chunks", judged.len());
        assert!(
            missed <= 1,
            "{missed} of {} chunks of the second line",
            judged.len()
        );
    }

    /// Live S4 (gate, request recogniser, 3 of 5 runs): the echo of a line from its soft first
    /// word ("While I look that up", the synthesiser's own start, `line_start`). The probe of
    /// its second voiced chunk had a window below the whole window's floor and found nothing;
    /// the lag came a chunk later, after two unexplained chunks had made an onset (340-380 ms
    /// ducked). With a lag locked on the line before, a few milliseconds off, or none.
    #[test]
    fn a_new_lines_first_chunks_make_no_onset_while_its_lag_is_found_again() {
        let pcm = recording("line_start");
        let line = &pcm[..96_000];
        let second_us = 3_000_000;
        for (offset, first_echoed) in (0..CHUNK).step_by(64).flat_map(|o| [(o, true), (o, false)]) {
            // The first 2 s of the recording, then again from its start at 3 s.
            let (mut tap, reference) = (Tap::default(), Mutex::default());
            for at_us in [0, second_us] {
                for (i, frame) in line.chunks(FRAME_BYTES).enumerate() {
                    tap.write(at_us + i as u64 * 20_000, frame, AGENT_RATE, &reference);
                }
            }
            // Its echo at 200 ms, then 205 ms, the second line moved by `offset` samples
            // against the analyser's chunks.
            let first = 3200;
            let second = (second_us * RATE / 1_000_000) as usize + 3280 + offset;
            let mut held = vec![0.0; 2 * 32_000];
            reference.lock().read(0, &mut held[..32_000]);
            let mut caller = vec![0.0; second + 32_000];
            // The first line comes back too, or (live S4: the line before was not fed back)
            // the second is the first echo of the session.
            for (i, sample) in held[..32_000].iter().enumerate().filter(|_| first_echoed) {
                caller[i + first] = 0.5 * sample;
            }
            reference
                .lock()
                .read((second - 3280 - offset) as i64, &mut held[32_000..]);
            for (i, sample) in held[32_000..].iter().enumerate() {
                caller[second + i] = 0.5 * sample;
            }
            let mut gate = Gate::default();
            let mut explained = Vec::new();
            for (i, chunk) in caller.as_chunks::<CHUNK>().0.iter().enumerate() {
                let end = ((i + 1) * CHUNK) as u64;
                gate.hear(chunk, end);
                let voiced = energy(chunk) >= FLOOR * CHUNK as f32;
                explained.push(voiced.then(|| gate.explains(end, &reference)));
            }
            let again = &explained[second / CHUNK..];
            let onset = again
                .windows(2)
                .position(|pair| pair == [Some(false), Some(false)]);
            assert!(
                onset.is_none(),
                "offset {offset}, first line echoed {first_echoed}: onset at chunk {onset:?} \
                 of the second line: {:?}",
                &again[..12]
            );
        }
    }

    /// A codec keeps the level of what it carries and not always its waveform.
    #[test]
    fn a_chunk_a_codec_has_garbled_is_echo_by_its_level() {
        let agent = played("medium_phrase");
        let mut caller = echo(&agent, 3200);
        // From 0.96 s on, two chunks in four keep their energy and lose their waveform: every
        // sample's sign is drawn anew.
        let mut state = 0x2545_f491u32;
        for (i, sample) in caller.iter_mut().enumerate().skip(30 * CHUNK) {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            if (i / CHUNK) % 4 >= 2 && state >> 31 == 1 {
                *sample = -*sample;
            }
        }
        let explained = gated(&caller);
        let garbled: Vec<bool> = (30..explained.len())
            .filter(|chunk| chunk % 4 >= 2)
            .filter_map(|chunk| explained[chunk])
            .collect();
        let taken = garbled.iter().filter(|explained| **explained).count();
        assert!(garbled.len() >= 40, "{} chunks", garbled.len());
        assert!(
            taken * 10 >= garbled.len() * 9,
            "{taken} of {} garbled chunks of echo",
            garbled.len()
        );

        // A caller 6 dB above such an echo is still heard.
        let voice = played("numbers");
        let from = 60 * CHUNK;
        for (i, sample) in caller.iter_mut().enumerate().skip(from) {
            *sample += voice[i - from];
        }
        let explained = gated(&caller);
        let loud = |chunk: usize| {
            let at = (chunk - 60) * CHUNK;
            energy(&voice[at..at + CHUNK]) >= energy(&agent[chunk * CHUNK - 3200..][..CHUNK])
        };
        let over: Vec<bool> = (60..explained.len())
            .filter(|chunk| loud(*chunk))
            .filter_map(|chunk| explained[chunk])
            .collect();
        let passed = over.iter().filter(|explained| !**explained).count();
        assert!(over.len() >= 30, "{} chunks", over.len());
        // The level of one chunk is the caller's and the echo's and what they share: now and
        // then a chunk still passes for echo. An onset takes two in a row.
        assert!(
            passed * 20 >= over.len() * 19,
            "{passed} of {} chunks the caller is 6 dB louder in",
            over.len()
        );
    }

    /// Run the caller's channel through the echo canceller while the agent's line plays.
    fn cancelled(caller: &[f32], agent: Option<&str>) -> (Vec<f32>, bool) {
        let mut pump = agent.map(Pump::new);
        let silent = Mutex::default();
        let mut defence = Defence::new(false);
        defence.set_aec(true);
        let mut clean = Vec::new();
        for (i, frame) in caller.chunks(FRAME).enumerate() {
            let end = (i + 1) * FRAME;
            if let Some(pump) = &mut pump {
                pump.run_to(end);
            }
            let reference = pump.as_ref().map_or(&silent, |pump| &pump.reference);
            defence.feed(frame, end as u64 * 1_000_000 / RATE, reference, &mut clean);
        }
        (clean, defence.heard())
    }

    #[test]
    fn aec_reduces_synthetic_echo() {
        // 120 ms late, 6 dB down, with a second path 5 ms behind the first.
        let mut caller = echo(&played("medium_phrase"), 1920);
        for i in 80..caller.len() {
            caller[i] += 0.3 * caller[i - 80];
        }
        let (clean, heard) = cancelled(&caller, Some("medium_phrase"));
        // The canceller has had 2 s to find the path.
        let tail = 32_000..clean.len();
        let (before, after) = (energy(&caller[tail.clone()]), energy(&clean[tail]));
        let reduction_db = 10.0 * (before / after.max(1e-12)).log10();
        assert!(reduction_db >= 20.0, "echo down {reduction_db:.1} dB");
        assert!(heard, "and it says that echo was heard");

        // A caller with no agent audio passes whole.
        let voice = &played("numbers")[..64_000];
        let (kept, heard) = cancelled(voice, None);
        let loss_db = 10.0 * (energy(voice) / energy(&kept)).log10();
        assert!(
            loss_db.abs() < 3.0,
            "the caller's voice moved {loss_db:.1} dB"
        );
        assert!(!heard, "nothing was playing");

        // A caller who speaks over the echo is still there when the echo is gone.
        let from = 32_000;
        let mut both = caller.clone();
        for (i, sample) in both.iter_mut().enumerate().skip(from) {
            *sample += 0.5 * voice[i - from];
        }
        let (clean, _) = cancelled(&both, Some("medium_phrase"));
        let span = from..from + 48_000;
        let voice_energy = 0.25 * energy(&voice[..48_000]);
        let kept_db = 10.0 * (energy(&clean[span]) / voice_energy).log10();
        assert!(
            kept_db.abs() < 6.0,
            "the caller comes out {kept_db:.1} dB off"
        );
    }

    #[test]
    fn the_defence_keeps_the_callers_place_on_the_clock() {
        let reference = Mutex::default();
        let mut defence = Defence::new(true);
        let mut out = Vec::new();
        // 25 ms of audio: two blocks come out, 5 ms waits.
        defence.feed(&[0.0; 400], 1_000_000, &reference, &mut out);
        assert_eq!((out.len(), defence.position()), (320, 16_000 - 400 + 320));
        // The next 20 ms arrive a millisecond late: the count goes on from the audio, not
        // from the clock.
        defence.feed(&[0.0; 320], 1_021_000, &reference, &mut out);
        assert_eq!((out.len(), defence.position()), (640, 16_240));
        // After a gap of a second the place is taken from the clock again.
        defence.feed(&[0.0; 320], 2_100_000, &reference, &mut out);
        assert_eq!(defence.position(), 33_520);
        assert!(!defence.aec() && defence.active());
        assert!(!Defence::new(false).active());
    }
}
