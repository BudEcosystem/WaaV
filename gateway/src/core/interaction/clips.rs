//! The listening sounds of one session (plan chapter 5, "Clips per language and voice"): the
//! checks every clip passes before it enters the bank, the operator's recorded clips, and the
//! bank while it is built.
//!
//! A clip is checked on its audio: once leading and trailing silence (10 ms frames under
//! -45 dBFS) are trimmed it is at most 600 ms long, and it is not silent. Where the session's
//! speech-to-text answers once per upload (cheap to run on a clip), the recogniser must hear
//! no more words than the clip's text has, which catches a voice that spells "mhm" out
//! ("M.H.M."). Audio that is not linear PCM16 cannot be measured and is taken as it is.
//!
//! The same goes for the lines said when a service fails (plan chapter 10): rendered in the
//! session's voice at session start and checked (not silent, at most [`MAX_LINE_MS`]), kept
//! by provider, voice, language and words in this process and under `clips_dir`, replaced by
//! the operator's recordings, and stood in for by the server's default set.

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use parking_lot::{Mutex, RwLock};
use sha2::{Digest, Sha256};
use tracing::{debug, info, warn};

use super::health::LineKind;

use crate::core::audio::resampler::{StreamResampler, flush_pcm16, resample_pcm16};
use crate::core::stt::{STTConfig, STTResult, create_stt_provider};
use crate::core::tts::AudioData;
use crate::core::tts::sniff::is_linear_pcm16;

/// The longest listening sound once its silence is trimmed.
pub const MAX_CLIP_MS: u64 = 600;
/// A 10 ms frame quieter than this is silence.
const SILENCE_DBFS: f32 = -45.0;
const FRAME_MS: u64 = 10;
/// The recogniser's answer on one clip: slower than this, the clip goes without the check.
const HEARING_MS: u64 = 5_000;

/// The longest failure line once its silence is trimmed.
pub const MAX_LINE_MS: u64 = 12_000;
/// Rendered failure lines kept in this process (all voices); the oldest go first.
const RENDERED_KEPT: usize = 512;

/// The voiced part of a PCM16 clip; why it is refused otherwise.
pub fn trimmed(audio: &AudioData) -> Result<AudioData, String> {
    trimmed_to(audio, MAX_CLIP_MS)
}

/// A failure line's check: its voiced part, at most [`MAX_LINE_MS`] and not silent.
pub fn check_line(audio: &AudioData) -> Result<AudioData, String> {
    trimmed_to(audio, MAX_LINE_MS)
}

fn trimmed_to(audio: &AudioData, max_ms: u64) -> Result<AudioData, String> {
    if !is_linear_pcm16(&audio.format) || audio.sample_rate == 0 {
        return Ok(audio.clone());
    }
    let frame = (u64::from(audio.sample_rate) * FRAME_MS / 1_000).max(1) as usize;
    let floor = 10f32.powf(SILENCE_DBFS / 20.0) * 32_768.0;
    let samples: Vec<f32> = audio
        .data
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| f32::from(i16::from_le_bytes(*b)))
        .collect();
    let voiced: Vec<bool> = samples
        .chunks(frame)
        .map(|c| (c.iter().map(|s| s * s).sum::<f32>() / c.len() as f32).sqrt() > floor)
        .collect();
    let Some(first) = voiced.iter().position(|v| *v) else {
        return Err("silent".to_owned());
    };
    let last = voiced.iter().rposition(|v| *v).unwrap_or(first);
    let (from, to) = (first * frame, ((last + 1) * frame).min(samples.len()));
    let ms = (to - from) as u64 * 1_000 / u64::from(audio.sample_rate);
    if ms > max_ms {
        return Err(format!("{ms} ms long"));
    }
    Ok(AudioData {
        data: audio.data[from * 2..to * 2].to_vec(),
        duration_ms: None,
        ..audio.clone()
    })
}

/// The recogniser heard the clip's text: no more words than the text has (a hyphen or an
/// apostrophe joins a word). A closer match refuses good clips: Whisper small.en hears a
/// well-spoken "okay" as "OK.", "bouquet" or "Bokeh." and a listening hum as nothing at all.
pub fn words_match(text: &str, heard: &str) -> bool {
    let words = |s: &str| {
        s.split(|c: char| !c.is_alphanumeric() && !matches!(c, '-' | '\'' | '\u{2019}'))
            .filter(|word| word.chars().any(char::is_alphanumeric))
            .count()
    };
    words(heard) <= words(text)
}

/// What the session's recogniser hears in the clip, when it answers once per upload; `None`
/// when it streams (no check), fails or is slow.
pub async fn heard(stt: &STTConfig, audio: &AudioData) -> Option<String> {
    if !is_linear_pcm16(&audio.format) || audio.sample_rate == 0 {
        return None;
    }
    let mut provider = create_stt_provider(&stt.provider, stt.clone()).ok()?;
    if !provider.is_request_response() {
        return None;
    }
    let words = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = words.clone();
    let on_result = Arc::new(move |result: STTResult| {
        if result.is_final {
            sink.lock().push(result.transcript);
        }
        Box::pin(async {}) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
    });
    provider.on_result(on_result).await.ok()?;
    provider.connect().await.ok()?;
    let rate = if stt.sample_rate == 0 {
        audio.sample_rate
    } else {
        stt.sample_rate
    };
    let pcm = at_rate(&audio.data, audio.sample_rate, rate);
    provider.send_audio(pcm.into()).await.ok()?;
    let upload = provider.take_utterance()?;
    let answered = tokio::time::timeout(Duration::from_millis(HEARING_MS), upload).await;
    let _ = provider.disconnect().await;
    answered.ok()?.ok()?;
    Some(words.lock().join(" "))
}

/// A clip's checks (see the module). `stt`: the session's recogniser, when it answers once
/// per upload. Returns the trimmed clip, or why it is refused.
pub async fn check(
    audio: AudioData,
    text: &str,
    stt: Option<STTConfig>,
) -> Result<AudioData, String> {
    let clip = trimmed(&audio)?;
    if let Some(stt) = stt
        && let Some(heard) = heard(&stt, &clip).await
    {
        debug!(text, heard, "interaction listening sound heard");
        if !words_match(text, &heard) {
            return Err(format!("heard {heard:?}"));
        }
    }
    Ok(clip)
}

/// An operator's recorded clip `<dir>/<provider>/<voice>/<language>/<clip id>.wav` (the
/// parts, the last one the file's name without `.wav`), as mono PCM16 at `rate` (its own rate
/// when `None`). `None` when there is none, a part of the path is not a plain name, or the
/// file is not a WAV file.
pub fn recorded(dir: &Path, parts: &[&str], rate: Option<u32>) -> Option<AudioData> {
    let path = wav_path(dir, parts)?;
    if !path.is_file() {
        return None;
    }
    let read = || -> Result<(Vec<f32>, hound::WavSpec), hound::Error> {
        let mut reader = hound::WavReader::open(&path)?;
        let spec = reader.spec();
        let samples = match spec.sample_format {
            hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>()?,
            hound::SampleFormat::Int => {
                let full = (1u64 << (spec.bits_per_sample.clamp(1, 32) - 1)) as f32;
                let samples = reader.samples::<i32>();
                samples
                    .map(|s| s.map(|s| s as f32 / full))
                    .collect::<Result<_, _>>()?
            }
        };
        Ok((samples, spec))
    };
    let (samples, spec) = match read() {
        Ok(read) => read,
        Err(error) => {
            warn!(path = %path.display(), %error, "interaction recorded clip unreadable");
            return None;
        }
    };
    let channels = usize::from(spec.channels.max(1));
    let mono: Vec<u8> = samples
        .chunks(channels)
        .flat_map(|frame| {
            let level = frame.iter().sum::<f32>() / frame.len() as f32;
            ((level.clamp(-1.0, 1.0) * 32_767.0) as i16).to_le_bytes()
        })
        .collect();
    let out = rate.unwrap_or(spec.sample_rate);
    Some(AudioData {
        data: at_rate(&mono, spec.sample_rate, out),
        sample_rate: out,
        format: "pcm".to_owned(),
        duration_ms: None,
    })
}

/// `<dir>/<part>/.../<last part>.wav`, when every part is a plain name.
fn wav_path(dir: &Path, parts: &[&str]) -> Option<PathBuf> {
    let plain = |part: &&str| {
        !part.is_empty() && *part != "." && *part != ".." && !part.contains(['/', '\\', '\0'])
    };
    let (file, folders) = parts.split_last()?;
    if !parts.iter().all(plain) {
        return None;
    }
    let folder = folders
        .iter()
        .fold(dir.to_path_buf(), |path, part| path.join(part));
    Some(folder.join(format!("{file}.wav")))
}

/// How long PCM16 audio plays (0 for another format).
pub fn duration_ms(audio: &AudioData) -> u64 {
    if !is_linear_pcm16(&audio.format) || audio.sample_rate == 0 {
        return 0;
    }
    audio.data.len() as u64 / 2 * 1_000 / u64::from(audio.sample_rate)
}

/// The audio at another rate (PCM16; any other format as it is).
pub fn resampled(audio: &AudioData, rate: u32) -> AudioData {
    if !is_linear_pcm16(&audio.format) || audio.sample_rate == rate || rate == 0 {
        return audio.clone();
    }
    AudioData {
        data: at_rate(&audio.data, audio.sample_rate, rate),
        sample_rate: rate,
        duration_ms: None,
        ..audio.clone()
    }
}

/// PCM16 at another rate.
fn at_rate(pcm: &[u8], from: u32, to: u32) -> Vec<u8> {
    let mut resampler = StreamResampler::new();
    let Some(mut out) = resample_pcm16(&mut resampler, pcm, from, to) else {
        return pcm.to_vec();
    };
    out.extend(flush_pcm16(&mut resampler).unwrap_or_default());
    out
}

/// The bank while it is built: each listening sound is tried (recorded, then rendered) until
/// it passes or fails; the first that fails brings in the pack's fallback word, tried the same
/// way. No clip plays before every one is settled.
pub struct Bank {
    lines: Vec<(String, String, Option<Result<AudioData, ()>>)>,
    fallback: Option<(String, String)>,
    installed: bool,
}

impl Bank {
    /// The persona's listening sounds (clip id, words) and the pack's fallback.
    pub fn new(lines: Vec<(String, String)>, fallback: Option<(String, String)>) -> Bank {
        Bank {
            lines: lines
                .into_iter()
                .map(|(id, text)| (id, text, None))
                .collect(),
            fallback,
            installed: false,
        }
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// The clip id and words of line `n`.
    pub fn line(&self, n: usize) -> (&str, &str) {
        let (id, text, _) = &self.lines[n];
        (id, text)
    }

    pub fn passed(&mut self, n: usize, clip: AudioData) {
        if let Some(line) = self.lines.get_mut(n) {
            line.2 = Some(Ok(clip));
        }
    }

    /// Line `n` has no usable clip. Returns the fallback's new line, to be tried, the first
    /// time a line fails when the bank does not hold the fallback yet.
    pub fn failed(&mut self, n: usize) -> Option<usize> {
        self.lines.get_mut(n)?.2 = Some(Err(()));
        let (id, text) = self.fallback.take()?;
        if self.lines.iter().any(|(held, _, _)| *held == id) {
            return None;
        }
        self.lines.push((id, text, None));
        Some(self.lines.len() - 1)
    }

    /// Once, when every line is settled: the clips that passed, in order (none: the session
    /// makes no listening sounds).
    pub fn ready(&mut self) -> Option<Vec<AudioData>> {
        if self.installed || self.lines.iter().any(|(_, _, state)| state.is_none()) {
            return None;
        }
        self.installed = true;
        let passed = self.lines.iter().filter_map(|(_, _, state)| match state {
            Some(Ok(clip)) => Some(clip.clone()),
            _ => None,
        });
        Some(passed.collect())
    }
}

// ---- the lines said when a service fails (chapter 10)

/// One line of the pack's `failure` group: its id (the recording's file name), its kind and
/// its words.
#[derive(Debug, Clone, PartialEq)]
pub struct FailureLine {
    pub id: String,
    pub kind: LineKind,
    pub text: String,
}

/// Whose voice a line is in: the provider, the voice (`default` when the session names
/// none) and the pack's language.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Voice {
    pub provider: String,
    pub voice: String,
    pub language: String,
}

impl Voice {
    fn parts<'a>(&'a self, more: &[&'a str]) -> Vec<&'a str> {
        let own = [self.provider.as_str(), &self.voice, &self.language];
        own.into_iter().chain(more.iter().copied()).collect()
    }

    fn key(&self, text: &str) -> String {
        format!(
            "{}\0{}\0{}\0{text}",
            self.provider, self.voice, self.language
        )
    }
}

/// The file name a rendered line is cached under: its id and a digest of its words, so new
/// words are rendered again.
fn cache_name(line: &FailureLine) -> String {
    let digest = Sha256::digest(line.text.as_bytes());
    format!("{}-{}", line.id, hex::encode(&digest[..8]))
}

/// The lines rendered in this process, by voice and words.
fn rendered() -> &'static Mutex<HashMap<String, AudioData>> {
    static RENDERED: OnceLock<Mutex<HashMap<String, AudioData>>> = OnceLock::new();
    RENDERED.get_or_init(Mutex::default)
}

/// A failure line of `voice` that needs no synthesis, at `rate`: the operator's recording
/// `<dir>/<provider>/<voice>/<language>/failure/<id>.wav`, else the rendering kept in this
/// process or under `<dir>/.../failure/cache/`. Checked as a rendered line is; `true` with a
/// recording.
pub fn failure_ready(
    dir: Option<&Path>,
    voice: &Voice,
    line: &FailureLine,
    rate: Option<u32>,
) -> Option<(AudioData, bool)> {
    let checked = |audio: AudioData, source: &str| match check_line(&audio) {
        Ok(audio) => Some(audio),
        Err(reason) => {
            info!(id = %line.id, source, %reason, "interaction failure line refused");
            None
        }
    };
    if let Some(dir) = dir {
        let recording = recorded(dir, &voice.parts(&["failure", &line.id]), rate);
        if let Some(audio) = recording.and_then(|audio| checked(audio, "recording")) {
            return Some((audio, true));
        }
    }
    let kept = rendered().lock().get(&voice.key(&line.text)).cloned();
    if let Some(audio) = kept {
        return Some((
            rate.map_or_else(|| audio.clone(), |rate| resampled(&audio, rate)),
            false,
        ));
    }
    let name = cache_name(line);
    let cached = recorded(dir?, &voice.parts(&["failure", "cache", &name]), rate)?;
    let audio = checked(cached, "cache")?;
    rendered()
        .lock()
        .insert(voice.key(&line.text), audio.clone());
    Some((audio, false))
}

/// A line rendered in `voice` that passed its checks: kept in this process, and under `dir`
/// (`.../failure/cache/<id>-<digest>.wav`) for the next process. PCM16 only.
pub fn keep_rendered(dir: Option<&Path>, voice: &Voice, line: &FailureLine, audio: &AudioData) {
    if !is_linear_pcm16(&audio.format) || audio.sample_rate == 0 {
        return;
    }
    {
        let mut kept = rendered().lock();
        if kept.len() >= RENDERED_KEPT {
            kept.clear();
        }
        kept.insert(voice.key(&line.text), audio.clone());
    }
    let name = cache_name(line);
    let Some(path) = dir.and_then(|dir| wav_path(dir, &voice.parts(&["failure", "cache", &name])))
    else {
        return;
    };
    let write = || -> Result<(), String> {
        let folder = path.parent().ok_or("no folder")?;
        std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: audio.sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        // Written aside and renamed, so a reader never finds half a file.
        let partial = path.with_extension("wav.part");
        let mut writer = hound::WavWriter::create(&partial, spec).map_err(|e| e.to_string())?;
        for sample in audio.data.as_chunks::<2>().0 {
            writer
                .write_sample(i16::from_le_bytes(*sample))
                .map_err(|e| e.to_string())?;
        }
        writer.finalize().map_err(|e| e.to_string())?;
        std::fs::rename(&partial, &path).map_err(|e| e.to_string())
    };
    match write() {
        Ok(()) => debug!(path = %path.display(), "interaction failure line cached"),
        Err(error) => warn!(path = %path.display(), %error, "interaction failure line not cached"),
    }
}

/// The server's default set of failure lines: rendered once at server start in the server's
/// default voice (`interaction.failure_voice`), or the operator's recordings of it. A session
/// whose own voice cannot give a line uses this one's.
#[derive(Debug, Clone)]
pub struct DefaultSet {
    pub voice: Voice,
    pub lines: Vec<(FailureLine, AudioData)>,
}

impl DefaultSet {
    /// The line that stands in for `line`: the one with its id, else the first of its kind.
    pub fn stand_in(&self, line: &FailureLine) -> Option<&AudioData> {
        let same = |of: &&(FailureLine, AudioData)| of.0.id == line.id;
        let kind = |of: &&(FailureLine, AudioData)| of.0.kind == line.kind;
        let found = self
            .lines
            .iter()
            .find(same)
            .or_else(|| self.lines.iter().find(kind));
        found.map(|(_, audio)| audio)
    }
}

fn default_slot() -> &'static RwLock<Option<Arc<DefaultSet>>> {
    static DEFAULT: OnceLock<RwLock<Option<Arc<DefaultSet>>>> = OnceLock::new();
    DEFAULT.get_or_init(RwLock::default)
}

/// The server's default set, once it is made.
pub fn default_set() -> Option<Arc<DefaultSet>> {
    default_slot().read().clone()
}

/// Installs (or, with `None`, removes) the server's default set.
pub fn install_default_set(set: Option<DefaultSet>) {
    *default_slot().write() = set.map(Arc::new);
}

/// Makes the default set: each line from a recording or a rendering kept before, else
/// rendered with `render` (its words to audio) and checked. Lines that fail are left out.
pub async fn build_default_set<F, Fut>(
    lines: Vec<FailureLine>,
    voice: Voice,
    dir: Option<&Path>,
    render: F,
) -> DefaultSet
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = Result<AudioData, String>>,
{
    let mut set = Vec::new();
    for line in lines {
        if let Some((audio, _)) = failure_ready(dir, &voice, &line, None) {
            set.push((line, audio));
            continue;
        }
        match render(line.text.clone())
            .await
            .and_then(|audio| check_line(&audio))
        {
            Ok(audio) => {
                keep_rendered(dir, &voice, &line, &audio);
                set.push((line, audio));
            }
            Err(reason) => {
                warn!(id = %line.id, %reason, "interaction default failure line refused")
            }
        }
    }
    DefaultSet { voice, lines: set }
}

/// How long a line of the default set waits for the process to have no open session.
const DEFAULT_SET_WAIT: std::time::Duration = std::time::Duration::from_secs(60);

/// Returns once no interaction session is open, or after [`DEFAULT_SET_WAIT`]: a line of
/// the default set is work for no call, and on a speech server shared with the calls it held
/// their lines up (live L1/L3: the first answers of a gateway's first session 4.4 to 6.6 s
/// after the caller's end, behind the default set's nine renders).
async fn idle_or_late() {
    let since = tokio::time::Instant::now();
    while super::actor::sessions_open() > 0 && since.elapsed() < DEFAULT_SET_WAIT {
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

/// Words to audio in one voice of the gateway's own.
pub type Render = Arc<
    dyn Fn(String) -> futures::future::BoxFuture<'static, Result<AudioData, String>> + Send + Sync,
>;

/// A voice of the gateway's own from a `tts_config` object (its key, as for `/speak`, from
/// the server's configuration): who it is, and how it says a line. PCM16 only. The server's
/// default voice (`interaction.failure_voice`) and a speech-to-speech session's
/// `speech.voice` are made so.
pub fn gateway_voice(
    value: &serde_json::Value,
    state: &Arc<crate::state::AppState>,
) -> Result<(Voice, Render), String> {
    let config: crate::handlers::ws::config::TTSWebSocketConfig =
        serde_json::from_value(value.clone())
            .map_err(|error| format!("not a tts_config: {error}"))?;
    let api_key = state
        .config
        .get_api_key(&config.provider)
        .unwrap_or_default();
    let tts = config.to_tts_config(api_key);
    if !is_linear_pcm16(tts.audio_format.as_deref().unwrap_or("pcm")) {
        return Err("the voice must give PCM audio".to_owned());
    }
    let voice = Voice {
        provider: tts.provider.clone(),
        voice: tts.voice_id.clone().unwrap_or_else(|| "default".to_owned()),
        language: "en".to_owned(),
    };
    let state = state.clone();
    let render: Render = Arc::new(move |text: String| {
        let (state, tts) = (state.clone(), tts.clone());
        Box::pin(async move {
            let (data, format, sample_rate) =
                crate::handlers::speak::synthesize_once(&state, tts, &text).await?;
            Ok(AudioData {
                data,
                sample_rate,
                format,
                duration_ms: None,
            })
        })
    });
    Ok((voice, render))
}

/// Makes the server's default set once, off the caller's path: the English pack's lines in
/// `interaction.failure_voice` (a `tts_config` object; its key, as for `/speak`, from the
/// server's configuration), or the operator's recordings of that voice. Nothing without the
/// switch on or a voice that is not PCM16.
pub fn spawn_default_set(state: Arc<crate::state::AppState>) {
    let server = super::config::server();
    let Some(voice) = server.failure_voice.clone().filter(|_| server.enabled) else {
        return;
    };
    let (voice, say) = match gateway_voice(&voice, &state) {
        Ok(voice) => voice,
        Err(error) => {
            return warn!(%error, "interaction.failure_voice cannot be used; no default failure lines");
        }
    };
    let lines = super::persona::Persona::load_in("default", "en").failure_lines();
    let dir = server.clips_dir.clone();
    tokio::spawn(async move {
        let render = |text: String| {
            let say = say.clone();
            async move {
                idle_or_late().await;
                say(text).await
            }
        };
        let dir = dir.as_deref().map(Path::new);
        let set = build_default_set(lines, voice, dir, render).await;
        info!(
            provider = %set.voice.provider,
            voice = %set.voice.voice,
            lines = set.lines.len(),
            "interaction default failure lines ready"
        );
        if !set.lines.is_empty() {
            install_default_set(Some(set));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 16_000;

    /// `silence_ms` of silence, `voiced_ms` of a tone at `amplitude`, `silence_ms` again.
    fn clip(silence_ms: u32, voiced_ms: u32, amplitude: i16) -> AudioData {
        let n = |ms: u32| (RATE / 1_000 * ms) as usize;
        let mut samples = vec![0i16; n(silence_ms)];
        samples.extend((0..n(voiced_ms)).map(|i| if i % 2 == 0 { amplitude } else { -amplitude }));
        samples.extend(vec![0i16; n(silence_ms)]);
        AudioData {
            data: samples.iter().flat_map(|s| s.to_le_bytes()).collect(),
            sample_rate: RATE,
            format: "pcm".to_owned(),
            duration_ms: None,
        }
    }

    fn ms(audio: &AudioData) -> u64 {
        audio.data.len() as u64 / 2 * 1_000 / u64::from(audio.sample_rate)
    }

    #[test]
    fn a_clip_is_trimmed_to_its_sound_and_held_to_600_ms() {
        // 300 ms of silence either side of 450 ms of sound: 450 ms are kept.
        let kept = trimmed(&clip(300, 450, 8_000)).expect("a good clip");
        assert_eq!(ms(&kept), 450);
        // "M.H.M." spelled out: 780 ms of sound.
        assert_eq!(trimmed(&clip(200, 780, 8_000)).unwrap_err(), "780 ms long");
        assert_eq!(trimmed(&clip(200, 600, 8_000)).map(|c| ms(&c)), Ok(600));
        // Silence, and sound under -45 dBFS (180 of 32,768 is -45.2 dBFS).
        assert_eq!(trimmed(&clip(500, 0, 0)).unwrap_err(), "silent");
        assert_eq!(trimmed(&clip(100, 300, 180)).unwrap_err(), "silent");
        // A format it cannot measure is taken as it is.
        let mp3 = AudioData {
            format: "mp3".to_owned(),
            ..clip(0, 900, 8_000)
        };
        assert_eq!(trimmed(&mp3).map(|c| c.data.len()), Ok(mp3.data.len()));
    }

    #[test]
    fn the_recogniser_must_hear_no_more_words_than_the_clip_says() {
        assert!(words_match("okay", "OK."));
        assert!(words_match("okay", "bouquet"));
        assert!(words_match("mhm", ""));
        assert!(words_match("mhm", "Mm-hmm."));
        assert!(words_match("I see", "I see."));
        assert!(words_match("ya veo", "Ya veo."));
        // A voice that spells it out.
        assert!(!words_match("mhm", "M.H.M."));
        assert!(!words_match("mhm", "and they chan."));
        assert!(!words_match("I see", "I see you."));
    }

    #[tokio::test]
    async fn a_streaming_recogniser_is_not_run_on_a_clip() {
        // `deepgram` streams: the check goes on the audio alone, without a connection.
        let stt = STTConfig {
            provider: "deepgram".to_owned(),
            api_key: "unused".to_owned(),
            sample_rate: RATE,
            ..Default::default()
        };
        assert_eq!(heard(&stt, &clip(0, 300, 8_000)).await, None);
        let checked = check(clip(100, 300, 8_000), "mhm", Some(stt)).await;
        assert_eq!(checked.map(|c| ms(&c)), Ok(300));
    }

    #[test]
    fn a_recorded_clip_is_read_as_mono_pcm16_at_the_sessions_rate() {
        let dir = std::env::temp_dir().join(format!("waav-clips-{}", std::process::id()));
        let at = dir.join("openai/alloy/en");
        std::fs::create_dir_all(&at).unwrap();
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(at.join("mhm.wav"), spec).unwrap();
        for _ in 0..48_000 / 5 {
            writer.write_sample(16_000i16).unwrap();
            writer.write_sample(0i16).unwrap();
        }
        writer.finalize().unwrap();
        let read = recorded(&dir, &["openai", "alloy", "en", "mhm"], Some(RATE));
        let missing = recorded(&dir, &["openai", "alloy", "en", "okay"], Some(RATE));
        let escaped = recorded(&dir, &["openai", "..", "alloy/en", "mhm"], Some(RATE));
        std::fs::remove_dir_all(&dir).unwrap();
        let read = read.expect("the recorded clip");
        assert_eq!((read.sample_rate, read.format.as_str()), (RATE, "pcm"));
        // 200 ms, the two channels mixed: about half of 16,000.
        assert!((195..=205).contains(&ms(&read)), "{} ms", ms(&read));
        let middle = &read.data[read.data.len() / 2..read.data.len() / 2 + 2];
        let level = i16::from_le_bytes([middle[0], middle[1]]);
        assert!((7_500..=8_500).contains(&level), "{level}");
        assert!(missing.is_none() && escaped.is_none());
    }

    fn line(id: &str, kind: LineKind, text: &str) -> FailureLine {
        FailureLine {
            id: id.to_owned(),
            kind,
            text: text.to_owned(),
        }
    }

    /// Chapter 10: a line is rendered once per voice and words, kept in the process and under
    /// `clips_dir` (another process reads it there), and rendered again for new words; the
    /// operator's recording wins, and a silent or overlong line is refused.
    #[tokio::test]
    async fn failure_lines_are_rendered_once_and_cached_by_voice() {
        let dir = std::env::temp_dir().join(format!("waav-failure-{}", std::process::id()));
        let voice = |name: &str| Voice {
            provider: format!("cache-test-{}", std::process::id()),
            voice: name.to_owned(),
            language: "en".to_owned(),
        };
        let stall = line("stall-1", LineKind::Stall, "Bear with me a moment.");
        let renders = Arc::new(Mutex::new(Vec::<String>::new()));
        let render = |renders: Arc<Mutex<Vec<String>>>| {
            move |text: String| {
                let renders = renders.clone();
                async move {
                    renders.lock().push(text);
                    Ok(clip(100, 1_500, 8_000))
                }
            }
        };
        let first = build_default_set(
            vec![stall.clone()],
            voice("alloy"),
            Some(&dir),
            render(renders.clone()),
        )
        .await;
        assert_eq!(first.lines.len(), 1);
        assert_eq!(ms(&first.lines[0].1), 1_500, "trimmed");
        // Kept: the same voice renders nothing again, another voice does.
        build_default_set(
            vec![stall.clone()],
            voice("alloy"),
            Some(&dir),
            render(renders.clone()),
        )
        .await;
        assert_eq!(renders.lock().len(), 1);
        build_default_set(
            vec![stall.clone()],
            voice("echo"),
            Some(&dir),
            render(renders.clone()),
        )
        .await;
        assert_eq!(renders.lock().len(), 2);
        // Another process finds it on disk.
        rendered().lock().clear();
        let (cached, recording) =
            failure_ready(Some(&dir), &voice("alloy"), &stall, None).expect("the cached line");
        assert!(!recording);
        assert_eq!((cached.sample_rate, ms(&cached)), (RATE, 1_500));
        // New words are rendered again.
        let reworded = line("stall-1", LineKind::Stall, "One moment, please.");
        assert!(failure_ready(Some(&dir), &voice("alloy"), &reworded, None).is_none());
        // The operator's recording wins.
        let at = dir.join(voice("alloy").provider).join("alloy/en/failure");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: RATE,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(at.join("stall-1.wav"), spec).unwrap();
        for i in 0..RATE {
            writer
                .write_sample(if i % 2 == 0 { 9_000i16 } else { -9_000 })
                .unwrap();
        }
        writer.finalize().unwrap();
        let (recorded, recording) =
            failure_ready(Some(&dir), &voice("alloy"), &stall, None).unwrap();
        assert!(recording);
        assert_eq!(ms(&recorded), 1_000);
        // Refused: silent, or longer than a line may be.
        let silent = build_default_set(
            vec![line(
                "back",
                LineKind::Back,
                "Thanks for waiting, I'm back.",
            )],
            voice("onyx"),
            None,
            |_| async { Ok(clip(500, 0, 0)) },
        )
        .await;
        let long = build_default_set(
            vec![line("exit", LineKind::Exit, "I have to go now.")],
            voice("onyx"),
            None,
            |_| async { Ok(clip(0, 12_500, 8_000)) },
        )
        .await;
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(silent.lines.is_empty() && long.lines.is_empty());
        // A stand-in: the line of the same id, else the first of the same kind.
        let set = DefaultSet {
            voice: voice("alloy"),
            lines: vec![(stall.clone(), clip(0, 100, 8_000))],
        };
        assert!(
            set.stand_in(&line("stall-9", LineKind::Stall, "x"))
                .is_some()
        );
        assert!(set.stand_in(&line("back", LineKind::Back, "x")).is_none());
    }

    #[test]
    fn a_line_that_fails_brings_in_the_fallback_once_and_the_bank_waits_for_all() {
        let line = |id: &str, text: &str| (id.to_owned(), text.to_owned());
        let fallback = Some(line("okay", "okay"));
        let mut bank = Bank::new(vec![line("mhm", "mhm"), line("i-see", "I see")], fallback);
        assert_eq!(bank.len(), 2);
        assert_eq!(bank.failed(0), Some(2));
        assert_eq!(bank.line(2), ("okay", "okay"));
        bank.passed(1, clip(0, 100, 8_000));
        assert!(bank.ready().is_none(), "the fallback is not settled");
        assert_eq!(bank.failed(2), None, "the fallback comes once");
        assert_eq!(bank.ready().map(|clips| clips.len()), Some(1));
        assert!(bank.ready().is_none(), "installed once");
        // A bank that holds its fallback already brings nothing in; none passing is no bank.
        let mut bank = Bank::new(vec![line("mhm", "mhm"), line("okay", "okay")], None);
        bank.fallback = Some(line("okay", "okay"));
        assert_eq!(bank.failed(0), None);
        assert_eq!(bank.failed(1), None);
        assert_eq!(bank.ready().map(|clips| clips.len()), Some(0));
    }
}
