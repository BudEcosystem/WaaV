//! The audio front end: wire bytes to 16 kHz mono 512-sample frames, and WAV encoding.
//!
//! The wire `encoding` is a free string, and the gateway's streaming clients accept aliases and
//! fall back to 16-bit PCM for names they do not know. The front end does the same, so a
//! configuration that works on a streaming model works on a file model. Compressed formats cannot
//! be framed for a detector and are refused by name.
//!
//! Time is counted in samples, never in wall-clock time: the resampler keeps its filter state
//! across any gap between chunks (the gateway's own resampler resets after 200 ms of wall time,
//! which would make a network jitter gap change the audio).

use rubato::{FftFixedIn, Resampler};

use crate::types::{FRAME_SAMPLES, SAMPLE_RATE};

/// One 32 ms frame of 16 kHz mono 16-bit PCM.
pub type Frame = Vec<i16>;

/// How the wire bytes are decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireEncoding {
    /// Little-endian signed 16-bit PCM.
    Pcm16,
    /// G.711 μ-law.
    MuLaw,
    /// G.711 A-law.
    ALaw,
}

/// The decoded encoding, and the name that was assumed to be PCM when the gateway did not know it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodingInfo {
    pub encoding: WireEncoding,
    pub assumed_pcm: Option<String>,
}

/// An encoding the front end cannot turn into samples.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the audio encoding '{0}' cannot be cut into utterances by the gateway")]
pub struct UndecodableEncoding(pub String);

/// Compressed formats: a detector needs raw samples.
pub const UNDECODABLE: &[&str] = &[
    "flac",
    "opus",
    "ogg_opus",
    "webm_opus",
    "amr",
    "amr_wb",
    "mp3",
];

const PCM_NAMES: &[&str] = &[
    "linear16",
    "pcm",
    "pcm16",
    "pcm_s16le",
    "lpcm",
    "raw",
    "s16le",
];
const MULAW_NAMES: &[&str] = &[
    "mulaw",
    "ulaw",
    "pcm_mulaw",
    "pcmu",
    "g711u",
    "mu-law",
    "g711_ulaw",
];
const ALAW_NAMES: &[&str] = &["alaw", "pcm_alaw", "pcma", "g711a", "a-law", "g711_alaw"];

/// Read a wire `encoding` name, case-insensitively. An empty or unknown name is 16-bit PCM.
pub fn parse_encoding(name: &str) -> Result<EncodingInfo, UndecodableEncoding> {
    let n = name.trim().to_ascii_lowercase();
    if UNDECODABLE.contains(&n.as_str()) {
        return Err(UndecodableEncoding(name.trim().to_string()));
    }
    if MULAW_NAMES.contains(&n.as_str()) {
        return Ok(EncodingInfo {
            encoding: WireEncoding::MuLaw,
            assumed_pcm: None,
        });
    }
    if ALAW_NAMES.contains(&n.as_str()) {
        return Ok(EncodingInfo {
            encoding: WireEncoding::ALaw,
            assumed_pcm: None,
        });
    }
    let assumed_pcm = (!n.is_empty() && !PCM_NAMES.contains(&n.as_str())).then(|| n.clone());
    Ok(EncodingInfo {
        encoding: WireEncoding::Pcm16,
        assumed_pcm,
    })
}

/// Whether the front end can turn this `encoding` into samples.
pub fn can_decode_encoding(name: &str) -> bool {
    parse_encoding(name).is_ok()
}

/// G.711 μ-law to linear PCM (ITU-T G.711).
pub fn ulaw_to_linear(byte: u8) -> i16 {
    let u = !byte;
    let sign = u & 0x80;
    let exponent = (u >> 4) & 0x07;
    let mantissa = (u & 0x0F) as i32;
    let magnitude = (((mantissa << 3) + 0x84) << exponent) - 0x84;
    if sign != 0 {
        -magnitude as i16
    } else {
        magnitude as i16
    }
}

/// G.711 A-law to linear PCM (ITU-T G.711).
pub fn alaw_to_linear(byte: u8) -> i16 {
    let a = byte ^ 0x55;
    let sign = a & 0x80;
    let exponent = (a >> 4) & 0x07;
    let mantissa = (a & 0x0F) as i32;
    let magnitude = if exponent == 0 {
        (mantissa << 4) + 8
    } else {
        ((mantissa << 4) + 0x108) << (exponent - 1)
    };
    // In A-law a set sign bit is positive.
    if sign != 0 {
        magnitude as i16
    } else {
        -magnitude as i16
    }
}

/// Why the front end cannot be built.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AudioError {
    #[error(transparent)]
    Undecodable(#[from] UndecodableEncoding),
    #[error("unsupported audio format: {0}")]
    Unsupported(String),
}

/// A streaming resampler to 16 kHz that counts time in samples.
struct Resampler16k {
    inner: FftFixedIn<f32>,
    pending: Vec<f32>,
}

impl Resampler16k {
    fn new(in_rate: u32) -> Result<Self, AudioError> {
        let chunk = ((in_rate as usize) / 50).clamp(64, 2048);
        let inner = FftFixedIn::<f32>::new(in_rate as usize, SAMPLE_RATE as usize, chunk, 2, 1)
            .map_err(|e| AudioError::Unsupported(format!("resampler for {in_rate} Hz: {e}")))?;
        Ok(Self {
            inner,
            pending: Vec::new(),
        })
    }

    fn push(&mut self, input: &[f32], out: &mut Vec<f32>) {
        self.pending.extend_from_slice(input);
        loop {
            let chunk = self.inner.input_frames_next().max(1);
            if self.pending.len() < chunk {
                break;
            }
            let take: Vec<f32> = self.pending.drain(..chunk).collect();
            if let Ok(mut resampled) = self.inner.process(&[take], None)
                && let Some(channel) = resampled.pop()
            {
                out.extend_from_slice(&channel);
            }
        }
    }

    /// Push the held tail through with zeros, so the last consonant is not lost at a forced cut.
    fn flush(&mut self, out: &mut Vec<f32>) {
        if self.pending.is_empty() {
            return;
        }
        let chunk = self.inner.input_frames_next().max(1);
        let mut tail = std::mem::take(&mut self.pending);
        tail.resize(chunk, 0.0);
        if let Ok(mut resampled) = self.inner.process(&[tail], None)
            && let Some(channel) = resampled.pop()
        {
            out.extend_from_slice(&channel);
        }
        self.inner.reset();
    }
}

/// Wire bytes in, 16 kHz mono frames out.
pub struct FrontEnd {
    encoding: WireEncoding,
    assumed_pcm: Option<String>,
    wire_rate: u32,
    channels: u16,
    /// An odd trailing byte of a PCM chunk, carried to the next.
    byte_carry: Option<u8>,
    /// Samples of an incomplete multi-channel group, carried to the next chunk.
    channel_carry: Vec<i16>,
    resampler: Option<Resampler16k>,
    /// 16 kHz samples not yet grouped into a frame.
    pending: Vec<i16>,
}

impl std::fmt::Debug for FrontEnd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrontEnd")
            .field("encoding", &self.encoding)
            .field("wire_rate", &self.wire_rate)
            .field("channels", &self.channels)
            .field("pending", &self.pending.len())
            .finish()
    }
}

impl FrontEnd {
    /// `encoding` as the client named it, `wire_rate` in Hz, `channels` 1 or more.
    pub fn new(encoding: &str, wire_rate: u32, channels: u16) -> Result<Self, AudioError> {
        let info = parse_encoding(encoding)?;
        if !(4_000..=192_000).contains(&wire_rate) {
            return Err(AudioError::Unsupported(format!(
                "sample rate {wire_rate} Hz"
            )));
        }
        if channels == 0 || channels > 8 {
            return Err(AudioError::Unsupported(format!("{channels} channels")));
        }
        let resampler = if wire_rate == SAMPLE_RATE {
            None
        } else {
            Some(Resampler16k::new(wire_rate)?)
        };
        Ok(Self {
            encoding: info.encoding,
            assumed_pcm: info.assumed_pcm,
            wire_rate,
            channels,
            byte_carry: None,
            channel_carry: Vec::new(),
            resampler,
            pending: Vec::new(),
        })
    }

    /// The wire rate when it is not 16 kHz.
    pub fn resampled_from(&self) -> Option<u32> {
        (self.wire_rate != SAMPLE_RATE).then_some(self.wire_rate)
    }

    /// An unknown encoding name treated as PCM.
    pub fn assumed_pcm(&self) -> Option<&str> {
        self.assumed_pcm.as_deref()
    }

    /// Decode one chunk and append every complete frame to `frames`.
    pub fn push(&mut self, bytes: &[u8], frames: &mut Vec<Frame>) {
        let samples = self.decode(bytes);
        let mono = self.mix(samples);
        self.resample_into_pending(&mono);
        self.drain_frames(frames);
    }

    /// End of a stretch of audio (a commit or an input stall): push the resampler's tail through
    /// and pad the last partial frame with zeros. Appends what that produced.
    pub fn flush(&mut self, frames: &mut Vec<Frame>) {
        if let Some(r) = self.resampler.as_mut() {
            let mut out = Vec::new();
            r.flush(&mut out);
            self.pending.extend(out.into_iter().map(f32_to_i16));
        }
        self.drain_frames(frames);
        if !self.pending.is_empty() {
            let mut frame = std::mem::take(&mut self.pending);
            frame.resize(FRAME_SAMPLES, 0);
            frames.push(frame);
        }
    }

    fn decode(&mut self, bytes: &[u8]) -> Vec<i16> {
        match self.encoding {
            WireEncoding::MuLaw => bytes.iter().map(|b| ulaw_to_linear(*b)).collect(),
            WireEncoding::ALaw => bytes.iter().map(|b| alaw_to_linear(*b)).collect(),
            WireEncoding::Pcm16 => {
                let mut out = Vec::with_capacity(bytes.len() / 2 + 1);
                let mut rest = bytes;
                if let Some(lo) = self.byte_carry.take() {
                    if let Some((&hi, tail)) = rest.split_first() {
                        out.push(i16::from_le_bytes([lo, hi]));
                        rest = tail;
                    } else {
                        self.byte_carry = Some(lo);
                        return out;
                    }
                }
                let mut pairs = rest.chunks_exact(2);
                out.extend(pairs.by_ref().map(|p| i16::from_le_bytes([p[0], p[1]])));
                if let [odd] = pairs.remainder() {
                    self.byte_carry = Some(*odd);
                }
                out
            }
        }
    }

    fn mix(&mut self, samples: Vec<i16>) -> Vec<i16> {
        if self.channels == 1 {
            return samples;
        }
        let ch = self.channels as usize;
        let mut all = std::mem::take(&mut self.channel_carry);
        all.extend(samples);
        let whole = all.len() / ch * ch;
        let mono = all[..whole]
            .chunks_exact(ch)
            .map(|g| (g.iter().map(|s| *s as i32).sum::<i32>() / ch as i32) as i16)
            .collect();
        self.channel_carry = all[whole..].to_vec();
        mono
    }

    fn resample_into_pending(&mut self, mono: &[i16]) {
        match self.resampler.as_mut() {
            None => self.pending.extend_from_slice(mono),
            Some(r) => {
                let input: Vec<f32> = mono.iter().map(|s| *s as f32 / 32768.0).collect();
                let mut out = Vec::new();
                r.push(&input, &mut out);
                self.pending.extend(out.into_iter().map(f32_to_i16));
            }
        }
    }

    fn drain_frames(&mut self, frames: &mut Vec<Frame>) {
        let whole = self.pending.len() / FRAME_SAMPLES * FRAME_SAMPLES;
        if whole == 0 {
            return;
        }
        frames.extend(
            self.pending
                .drain(..whole)
                .collect::<Vec<_>>()
                .chunks_exact(FRAME_SAMPLES)
                .map(<[i16]>::to_vec),
        );
    }
}

fn f32_to_i16(s: f32) -> i16 {
    if !s.is_finite() {
        return 0;
    }
    (s * 32768.0)
        .round()
        .clamp(i16::MIN as f32, i16::MAX as f32) as i16
}

/// A frame as the detector wants it: f32 in [-1, 1].
pub fn frame_to_f32(frame: &[i16]) -> Vec<f32> {
    frame.iter().map(|s| *s as f32 / 32768.0).collect()
}

/// Root mean square of a frame, in [0, 1].
pub fn rms(frame: &[i16]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }
    let sum: f64 = frame
        .iter()
        .map(|s| {
            let v = *s as f64 / 32768.0;
            v * v
        })
        .sum();
    (sum / frame.len() as f64).sqrt() as f32
}

/// A 16 kHz mono 16-bit PCM WAV file. Groq and ElevenLabs both state that uncompressed audio gives
/// the lowest latency.
pub fn wav_16k_mono(pcm: &[i16]) -> Vec<u8> {
    let data_len = (pcm.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    out.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in pcm {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcm_bytes(samples: &[i16]) -> Vec<u8> {
        samples.iter().flat_map(|s| s.to_le_bytes()).collect()
    }

    fn tone(rate: u32, hz: f32, ms: u32, amp: f32) -> Vec<i16> {
        let n = (rate as u64 * ms as u64 / 1000) as usize;
        (0..n)
            .map(|i| {
                ((2.0 * std::f32::consts::PI * hz * i as f32 / rate as f32).sin() * amp * 32767.0)
                    as i16
            })
            .collect()
    }

    #[test]
    fn encoding_names_decode_pcm_and_g711_and_refuse_compressed_formats() {
        assert_eq!(
            parse_encoding("linear16").unwrap().encoding,
            WireEncoding::Pcm16
        );
        assert_eq!(
            parse_encoding("PCM_S16LE").unwrap().encoding,
            WireEncoding::Pcm16
        );
        assert_eq!(
            parse_encoding("").unwrap(),
            EncodingInfo {
                encoding: WireEncoding::Pcm16,
                assumed_pcm: None
            }
        );
        assert_eq!(
            parse_encoding("mulaw").unwrap().encoding,
            WireEncoding::MuLaw
        );
        assert_eq!(
            parse_encoding("pcmu").unwrap().encoding,
            WireEncoding::MuLaw
        );
        assert_eq!(parse_encoding("alaw").unwrap().encoding, WireEncoding::ALaw);
        assert_eq!(
            parse_encoding("g711a").unwrap().encoding,
            WireEncoding::ALaw
        );
        for compressed in UNDECODABLE {
            assert!(!can_decode_encoding(compressed), "{compressed}");
        }
        assert!(!can_decode_encoding("Opus"));
    }

    #[test]
    fn an_unknown_encoding_name_is_pcm_and_is_recorded() {
        let info = parse_encoding("weird-pcm").unwrap();
        assert_eq!(info.encoding, WireEncoding::Pcm16);
        assert_eq!(info.assumed_pcm.as_deref(), Some("weird-pcm"));
    }

    #[test]
    fn g711_decoding_matches_the_standard_tables() {
        assert_eq!(ulaw_to_linear(0xFF), 0);
        assert_eq!(ulaw_to_linear(0x7F), 0);
        assert_eq!(ulaw_to_linear(0x00), -32124);
        assert_eq!(ulaw_to_linear(0x80), 32124);
        assert_eq!(alaw_to_linear(0xD5), 8);
        assert_eq!(alaw_to_linear(0x55), -8);
        assert_eq!(alaw_to_linear(0xAA), 32256);
        assert_eq!(alaw_to_linear(0x2A), -32256);
    }

    #[test]
    fn sixteen_khz_pcm_is_framed_sample_for_sample() {
        let mut fe = FrontEnd::new("linear16", 16_000, 1).unwrap();
        let samples: Vec<i16> = (0..1200).map(|i| i as i16).collect();
        let mut frames = Vec::new();
        fe.push(&pcm_bytes(&samples), &mut frames);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0], samples[..512].to_vec());
        assert_eq!(frames[1], samples[512..1024].to_vec());
        assert_eq!(fe.resampled_from(), None);
        let mut rest = Vec::new();
        fe.flush(&mut rest);
        assert_eq!(rest.len(), 1);
        assert_eq!(&rest[0][..176], &samples[1024..]);
        assert!(rest[0][176..].iter().all(|s| *s == 0));
    }

    #[test]
    fn an_odd_byte_is_carried_to_the_next_chunk() {
        let mut fe = FrontEnd::new("pcm", 16_000, 1).unwrap();
        let samples: Vec<i16> = (0..512).map(|i| (i * 7) as i16).collect();
        let bytes = pcm_bytes(&samples);
        let mut frames = Vec::new();
        fe.push(&bytes[..3], &mut frames);
        fe.push(&bytes[3..701], &mut frames);
        fe.push(&bytes[701..], &mut frames);
        assert_eq!(frames, vec![samples]);
    }

    #[test]
    fn stereo_is_averaged_to_mono_across_chunk_boundaries() {
        let mut fe = FrontEnd::new("linear16", 16_000, 2).unwrap();
        let interleaved: Vec<i16> = (0..1024)
            .map(|i| if i % 2 == 0 { 100 } else { 300 })
            .collect();
        let bytes = pcm_bytes(&interleaved);
        let mut frames = Vec::new();
        fe.push(&bytes[..6], &mut frames); // a sample and a half of a group
        fe.push(&bytes[6..], &mut frames);
        assert_eq!(frames.len(), 1);
        assert!(frames[0].iter().all(|s| *s == 200));
    }

    #[test]
    fn mulaw_at_eight_khz_is_decoded_and_upsampled_to_twice_the_samples() {
        let mut fe = FrontEnd::new("mulaw", 8_000, 1).unwrap();
        assert_eq!(fe.resampled_from(), Some(8_000));
        let mut frames = Vec::new();
        // One second of μ-law silence-ish bytes.
        fe.push(&vec![0xFFu8; 8_000], &mut frames);
        fe.flush(&mut frames);
        let total: usize = frames.iter().map(Vec::len).sum();
        assert!((15_900..=17_000).contains(&total), "{total}");
        assert!(frames.iter().all(|f| f.len() == FRAME_SAMPLES));
    }

    #[test]
    fn a_resampled_tone_keeps_its_frequency_and_level() {
        let mut fe = FrontEnd::new("linear16", 48_000, 1).unwrap();
        let mut frames = Vec::new();
        for chunk in tone(48_000, 440.0, 1000, 0.5).chunks(960) {
            fe.push(&pcm_bytes(chunk), &mut frames);
        }
        let out: Vec<i16> = frames.concat();
        // Skip the filter's warm-up, then count zero crossings over 0.5 s: about 440.
        let window = &out[1600..9600];
        let crossings = window
            .windows(2)
            .filter(|w| (w[0] < 0) != (w[1] < 0))
            .count();
        assert!((430..=450).contains(&crossings), "{crossings}");
        let level = rms(window);
        assert!((0.32..0.39).contains(&level), "{level}");
    }

    #[test]
    fn a_gap_between_chunks_does_not_change_the_audio() {
        // Same audio pushed in two different chunkings gives the same frames: no clock is read.
        let samples = tone(24_000, 300.0, 600, 0.3);
        let run = |sizes: &[usize]| {
            let mut fe = FrontEnd::new("linear16", 24_000, 1).unwrap();
            let mut frames = Vec::new();
            let mut at = 0;
            for size in sizes.iter().cycle() {
                if at >= samples.len() {
                    break;
                }
                let end = (at + size).min(samples.len());
                fe.push(&pcm_bytes(&samples[at..end]), &mut frames);
                at = end;
            }
            fe.flush(&mut frames);
            frames
        };
        assert_eq!(run(&[480]), run(&[160, 999, 7]));
    }

    #[test]
    fn rejects_formats_it_cannot_frame() {
        assert!(matches!(
            FrontEnd::new("opus", 48_000, 1),
            Err(AudioError::Undecodable(_))
        ));
        assert!(FrontEnd::new("linear16", 0, 1).is_err());
        assert!(FrontEnd::new("linear16", 16_000, 0).is_err());
    }

    #[test]
    fn the_wav_header_says_16_khz_mono_16_bit() {
        let wav = wav_16k_mono(&[1, -1, 2]);
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(wav[4..8].try_into().unwrap()), 36 + 6);
        assert_eq!(&wav[8..16], b"WAVEfmt ");
        assert_eq!(u16::from_le_bytes(wav[22..24].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 16_000);
        assert_eq!(u16::from_le_bytes(wav[34..36].try_into().unwrap()), 16);
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 6);
        assert_eq!(wav.len(), 50);
        assert_eq!(i16::from_le_bytes([wav[46], wav[47]]), -1);
    }

    #[test]
    fn rms_of_a_full_scale_square_is_one() {
        assert!((rms(&[i16::MAX, i16::MIN + 1, i16::MAX]) - 1.0).abs() < 1e-3);
        assert_eq!(rms(&[0; 512]), 0.0);
    }
}
