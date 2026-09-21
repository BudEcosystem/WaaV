//! Decoding an uploaded audio file into the PCM a streaming STT provider accepts.
//!
//! `/v1/audio/transcriptions` takes a FILE; every WaaV STT provider takes a stream of raw
//! samples. Something has to bridge the two, and this is it — kept dependency-free and pure
//! so the format edge cases can be tested in milliseconds rather than against a live vendor.
//!
//! **Scope is deliberately narrow.** WAV and headerless PCM are decoded here; mp3, m4a, ogg,
//! flac and webm are REFUSED with a message naming what does work. Guessing at a compressed
//! container without a real decoder would not produce a bad transcript, it would produce a
//! confident one from noise, which is far worse than a 400. Widening this means adding a
//! decoder crate (symphonia), not loosening the check.

use crate::AudioError;

/// Mono 16-bit samples plus the rate they were captured at.
///
/// Mono because every STT provider WaaV drives expects a single channel; carrying stereo
/// further would push the downmix decision into each provider, where it would be made
/// differently in each one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PcmAudio {
    pub samples: Vec<i16>,
    pub sample_rate: u32,
}

impl PcmAudio {
    /// Little-endian bytes, which is what every provider's socket wants.
    pub fn to_bytes_le(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.samples.len() * 2);
        for s in &self.samples {
            out.extend_from_slice(&s.to_le_bytes());
        }
        out
    }

    /// Duration in seconds. Used for billing (STT is priced per second of audio) and for the
    /// `duration` field of a verbose_json response.
    pub fn duration_secs(&self) -> f64 {
        if self.sample_rate == 0 {
            return 0.0;
        }
        self.samples.len() as f64 / self.sample_rate as f64
    }
}

/// Containers this module can actually turn into samples.
pub const DECODABLE_EXTENSIONS: &[&str] = &["wav", "wave", "pcm", "raw"];

/// Rate assumed for a headerless PCM upload, matching the rate WaaV's own pipeline uses.
pub const DEFAULT_PCM_SAMPLE_RATE: u32 = 16_000;

/// Decode an uploaded file into mono 16-bit PCM.
///
/// `filename` selects the container; its extension is the only hint OpenAI's API gives.
pub fn decode(bytes: &[u8], filename: &str) -> Result<PcmAudio, AudioError> {
    let ext = filename
        .rsplit('.')
        .next()
        .filter(|e| !e.is_empty() && *e != filename)
        .unwrap_or("")
        .to_ascii_lowercase();

    if bytes.is_empty() {
        return Err(AudioError::InvalidField {
            field: "file",
            reason: "the uploaded file is empty".to_string(),
        });
    }

    match ext.as_str() {
        "wav" | "wave" => decode_wav(bytes),
        "pcm" | "raw" => decode_raw_pcm(bytes, DEFAULT_PCM_SAMPLE_RATE),
        // A RIFF header is trusted over an absent or wrong extension: a caller that uploads a
        // real WAV named `audio.bin` is unambiguous, and refusing it would be pedantry.
        _ if bytes.starts_with(b"RIFF") => decode_wav(bytes),
        other => Err(AudioError::InvalidField {
            field: "file",
            reason: format!(
                "cannot decode {} audio; this gateway transcribes {} only. \
                 Convert the file first (e.g. `ffmpeg -i in.{} -ar 16000 -ac 1 out.wav`).",
                if other.is_empty() { "unrecognised" } else { other },
                DECODABLE_EXTENSIONS.join(", "),
                if other.is_empty() { "mp3" } else { other },
            ),
        }),
    }
}

fn invalid(reason: impl Into<String>) -> AudioError {
    AudioError::InvalidField {
        field: "file",
        reason: reason.into(),
    }
}

fn decode_raw_pcm(bytes: &[u8], sample_rate: u32) -> Result<PcmAudio, AudioError> {
    if bytes.len() % 2 != 0 {
        return Err(invalid(
            "headerless PCM must be 16-bit little-endian, so its length must be even",
        ));
    }
    let samples = bytes
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect();
    Ok(PcmAudio {
        samples,
        sample_rate,
    })
}

/// Parse a RIFF/WAVE file.
///
/// Chunks are WALKED rather than assumed to sit at fixed offsets. Real files from real
/// recorders carry `LIST`/`INFO`/`fact` chunks before `data`, and a parser that reads `fmt `
/// at 12 and `data` at 36 decodes those as silence or noise without erroring — the failure
/// mode being an empty transcript nobody can explain.
/// Duration of a RIFF/WAVE upload, read from its header without decoding the samples.
///
/// `audio_seconds` is the billing dimension for transcription, the way `characters` is for
/// synthesis. The self-hosted STT path forwards the upload verbatim and never decodes it, so
/// `PcmAudio::duration_secs` is not available there and the column was permanently NULL.
/// This reads the `fmt ` and `data` chunk headers only: a one-hour file is answered without
/// allocating a sample.
///
/// Returns `None` for anything it cannot read with certainty — a non-RIFF container, a
/// truncated header, a zero sample rate. NULL is the honest answer there; putting a guessed
/// number into a billing column is worse than putting nothing.
pub fn wav_duration_secs(bytes: &[u8]) -> Option<f64> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }

    let mut pos = 12usize;
    let mut rate: Option<u32> = None;
    let mut block_align: Option<u32> = None;
    let mut data_len: Option<u32> = None;

    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size =
            u32::from_le_bytes([bytes[pos + 4], bytes[pos + 5], bytes[pos + 6], bytes[pos + 7]])
                as usize;
        let body = pos + 8;
        match id {
            // channels at +2, rate at +4, bits at +14 — enough to derive the frame size
            // without parse_fmt, which validates things a duration read does not care about.
            b"fmt " if size >= 16 && body + 16 <= bytes.len() => {
                let channels = u16::from_le_bytes([bytes[body + 2], bytes[body + 3]]) as u32;
                rate = Some(u32::from_le_bytes([
                    bytes[body + 4],
                    bytes[body + 5],
                    bytes[body + 6],
                    bytes[body + 7],
                ]));
                let bits = u16::from_le_bytes([bytes[body + 14], bytes[body + 15]]) as u32;
                block_align = Some(channels * bits / 8);
            }
            // The DECLARED size, not what arrived: a truncated upload should not silently
            // report a shorter clip than the caller sent.
            b"data" => data_len = Some(size as u32),
            _ => {}
        }
        let next = body.saturating_add(size).min(bytes.len());
        if next <= pos {
            break;
        }
        pos = next + (size & 1);
    }

    let (rate, align, len) = (rate?, block_align?, data_len?);
    if rate == 0 || align == 0 {
        return None;
    }
    // Frames over rate. Bytes over rate would report stereo as twice its length, which in a
    // billing column is an overcharge rather than a rounding error.
    Some((len / align) as f64 / rate as f64)
}

fn decode_wav(bytes: &[u8]) -> Result<PcmAudio, AudioError> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(invalid("not a RIFF/WAVE file"));
    }

    let mut pos = 12usize;
    let mut fmt: Option<WavFmt> = None;
    let mut data: Option<&[u8]> = None;

    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes([bytes[pos + 4], bytes[pos + 5], bytes[pos + 6], bytes[pos + 7]]) as usize;
        let body_start = pos + 8;
        // A declared size past the end means a truncated file; clamp so a partially-uploaded
        // recording still transcribes what arrived rather than failing outright.
        let body_end = body_start.saturating_add(size).min(bytes.len());

        match id {
            b"fmt " => fmt = Some(parse_fmt(&bytes[body_start..body_end])?),
            b"data" => data = Some(&bytes[body_start..body_end]),
            _ => {}
        }

        // Chunks are word-aligned: an odd size is followed by one pad byte.
        pos = body_end + (size & 1);
        if size == 0 && id != b"data" {
            pos = body_end;
        }
        if pos <= body_start && size == 0 {
            break; // zero-size chunk with no advance; stop rather than spin
        }
    }

    let fmt = fmt.ok_or_else(|| invalid("WAVE file has no `fmt ` chunk"))?;
    let data = data.ok_or_else(|| invalid("WAVE file has no `data` chunk"))?;

    if fmt.bits_per_sample != 16 {
        return Err(invalid(format!(
            "{}-bit WAVE is not supported; re-encode as 16-bit PCM (`ffmpeg -i in.wav -acodec pcm_s16le out.wav`)",
            fmt.bits_per_sample
        )));
    }
    if fmt.channels == 0 {
        return Err(invalid("WAVE header declares zero channels"));
    }
    if fmt.sample_rate == 0 {
        return Err(invalid("WAVE header declares a zero sample rate"));
    }

    let interleaved: Vec<i16> = data
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect();

    let samples = if fmt.channels == 1 {
        interleaved
    } else {
        downmix(&interleaved, fmt.channels as usize)
    };

    Ok(PcmAudio {
        samples,
        sample_rate: fmt.sample_rate,
    })
}

struct WavFmt {
    channels: u16,
    sample_rate: u32,
    bits_per_sample: u16,
}

fn parse_fmt(body: &[u8]) -> Result<WavFmt, AudioError> {
    if body.len() < 16 {
        return Err(invalid("WAVE `fmt ` chunk is truncated"));
    }
    let format_tag = u16::from_le_bytes([body[0], body[1]]);
    // 1 = PCM, 0xFFFE = WAVE_FORMAT_EXTENSIBLE (whose sub-format is PCM for our purposes).
    // Anything else is compressed, and decoding it as PCM yields noise, not an error.
    if format_tag != 1 && format_tag != 0xFFFE {
        return Err(invalid(format!(
            "WAVE format tag {format_tag} is compressed, not PCM; re-encode as 16-bit PCM"
        )));
    }
    Ok(WavFmt {
        channels: u16::from_le_bytes([body[2], body[3]]),
        sample_rate: u32::from_le_bytes([body[4], body[5], body[6], body[7]]),
        bits_per_sample: u16::from_le_bytes([body[14], body[15]]),
    })
}

/// Average the channels rather than summing them.
///
/// Summing two channels of a loud stereo recording overflows i16 and wraps, which is audible
/// as harsh clipping and measurably wrecks recognition accuracy. Averaging in i32 and
/// narrowing once cannot overflow.
fn downmix(interleaved: &[i16], channels: usize) -> Vec<i16> {
    interleaved
        .chunks_exact(channels)
        .map(|frame| {
            let sum: i32 = frame.iter().map(|s| *s as i32).sum();
            (sum / channels as i32) as i16
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a WAV in memory, optionally with a junk chunk before `data`.
    fn wav(channels: u16, sample_rate: u32, bits: u16, samples: &[i16], extra_chunk: bool) -> Vec<u8> {
        let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&0u32.to_le_bytes()); // size patched below
        out.extend_from_slice(b"WAVE");

        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes()); // PCM
        out.extend_from_slice(&channels.to_le_bytes());
        out.extend_from_slice(&sample_rate.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes()); // byte rate (unread)
        out.extend_from_slice(&0u16.to_le_bytes()); // block align (unread)
        out.extend_from_slice(&bits.to_le_bytes());

        if extra_chunk {
            // Exactly what a recorder writes and a fixed-offset parser trips over.
            out.extend_from_slice(b"LIST");
            out.extend_from_slice(&10u32.to_le_bytes());
            out.extend_from_slice(b"INFOhello");
            out.push(0); // pad to even
        }

        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&data);

        let total = (out.len() - 8) as u32;
        out[4..8].copy_from_slice(&total.to_le_bytes());
        out
    }

    #[test]
    fn decodes_a_mono_16bit_wav() {
        let bytes = wav(1, 16_000, 16, &[0, 100, -100, 32_767], false);
        let pcm = decode(&bytes, "a.wav").unwrap();
        assert_eq!(pcm.samples, vec![0, 100, -100, 32_767]);
        assert_eq!(pcm.sample_rate, 16_000);
    }

    #[test]
    fn preserves_the_headers_sample_rate() {
        // Reporting 16k for a 44.1k file makes every provider transcribe chipmunk audio.
        let bytes = wav(1, 44_100, 16, &[1, 2, 3, 4], false);
        assert_eq!(decode(&bytes, "a.wav").unwrap().sample_rate, 44_100);
    }

    #[test]
    fn walks_chunks_instead_of_assuming_data_is_at_offset_36() {
        // THE realistic failure: a LIST/INFO chunk before `data`. A fixed-offset parser reads
        // the chunk header as samples and returns plausible-looking noise, so the request
        // succeeds and the transcript is wrong — the worst possible outcome.
        let bytes = wav(1, 16_000, 16, &[7, 8, 9, 10], true);
        let pcm = decode(&bytes, "a.wav").unwrap();
        assert_eq!(pcm.samples, vec![7, 8, 9, 10]);
    }

    #[test]
    fn downmixes_stereo_by_averaging_not_summing() {
        // Summing these wraps i16 and clips audibly; averaging cannot.
        let bytes = wav(2, 16_000, 16, &[30_000, 30_000, -30_000, -30_000], false);
        let pcm = decode(&bytes, "a.wav").unwrap();
        assert_eq!(pcm.samples, vec![30_000, -30_000]);
    }

    #[test]
    fn a_compressed_wav_is_refused_rather_than_read_as_noise() {
        let mut bytes = wav(1, 16_000, 16, &[1, 2], false);
        // Flip the format tag to A-law; the bytes still parse, they just are not PCM.
        bytes[20] = 6;
        let err = decode(&bytes, "a.wav").unwrap_err().to_string();
        assert!(err.contains("compressed"), "got: {err}");
    }

    #[test]
    fn a_non_16_bit_wav_is_refused_with_the_fix_in_the_message() {
        let bytes = wav(1, 16_000, 24, &[1, 2], false);
        let err = decode(&bytes, "a.wav").unwrap_err().to_string();
        assert!(err.contains("24-bit"), "got: {err}");
        assert!(err.contains("ffmpeg"), "the message must say how to fix it: {err}");
    }

    #[test]
    fn an_mp3_is_refused_and_the_message_names_what_does_work() {
        let err = decode(b"\xff\xfbfake mp3 body", "speech.mp3")
            .unwrap_err()
            .to_string();
        assert!(err.contains("mp3"), "got: {err}");
        assert!(err.contains("wav"), "the message must name a working format: {err}");
    }

    #[test]
    fn an_empty_upload_is_refused() {
        assert!(decode(b"", "a.wav").is_err());
    }

    #[test]
    fn a_truncated_header_is_an_error_not_a_panic() {
        assert!(decode(b"RIFF", "a.wav").is_err());
        assert!(decode(b"RIFFxxxxWAVE", "a.wav").is_err());
    }

    #[test]
    fn a_declared_data_size_past_the_end_decodes_what_arrived() {
        // A partially-uploaded recording should transcribe as far as it got.
        let mut bytes = wav(1, 16_000, 16, &[1, 2, 3, 4], false);
        let n = bytes.len();
        bytes[n - 8 - 0 - 4..n - 8].copy_from_slice(&9_999u32.to_le_bytes());
        let pcm = decode(&bytes, "a.wav").unwrap();
        assert_eq!(pcm.samples, vec![1, 2, 3, 4]);
    }

    #[test]
    fn a_real_wav_with_the_wrong_extension_is_still_decoded() {
        let bytes = wav(1, 16_000, 16, &[5, 6], false);
        assert_eq!(decode(&bytes, "recording.bin").unwrap().samples, vec![5, 6]);
    }

    #[test]
    fn headerless_pcm_is_accepted_at_the_default_rate() {
        let raw: Vec<u8> = [1i16, -1, 300].iter().flat_map(|s| s.to_le_bytes()).collect();
        let pcm = decode(&raw, "a.pcm").unwrap();
        assert_eq!(pcm.samples, vec![1, -1, 300]);
        assert_eq!(pcm.sample_rate, DEFAULT_PCM_SAMPLE_RATE);
    }

    #[test]
    fn odd_length_headerless_pcm_is_refused() {
        assert!(decode(b"\x01\x02\x03", "a.raw").is_err());
    }

    #[test]
    fn duration_is_samples_over_rate() {
        let pcm = PcmAudio {
            samples: vec![0; 32_000],
            sample_rate: 16_000,
        };
        assert!((pcm.duration_secs() - 2.0).abs() < 1e-9);
    }

    #[test]
    fn duration_of_a_zero_rate_clip_is_zero_not_a_division_by_zero() {
        let pcm = PcmAudio {
            samples: vec![0; 10],
            sample_rate: 0,
        };
        assert_eq!(pcm.duration_secs(), 0.0);
    }

    #[test]
    fn bytes_round_trip_little_endian() {
        let pcm = PcmAudio {
            samples: vec![1, -2, 300],
            sample_rate: 16_000,
        };
        assert_eq!(pcm.to_bytes_le(), vec![1, 0, 254, 255, 44, 1]);
    }
}

#[cfg(test)]
mod wav_duration_tests {
    use super::*;

    /// A minimal but valid RIFF/WAVE with the given rate, channels and sample count.
    fn wav(rate: u32, channels: u16, bits: u16, frames: u32) -> Vec<u8> {
        let block_align = channels * bits / 8;
        let data_len = frames * block_align as u32;
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data_len).to_le_bytes());
        b.extend_from_slice(b"WAVE");
        b.extend_from_slice(b"fmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes()); // PCM
        b.extend_from_slice(&channels.to_le_bytes());
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * block_align as u32).to_le_bytes());
        b.extend_from_slice(&block_align.to_le_bytes());
        b.extend_from_slice(&bits.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&data_len.to_le_bytes());
        b.resize(b.len() + data_len as usize, 0);
        b
    }

    #[test]
    fn reads_duration_from_the_header_alone() {
        // 16000 Hz, mono, 16-bit, 32000 frames => exactly 2 seconds.
        let d = wav_duration_secs(&wav(16000, 1, 16, 32000)).unwrap();
        assert!((d - 2.0).abs() < 1e-9, "got {d}");
    }

    #[test]
    fn accounts_for_channels_and_bit_depth() {
        // Duration is FRAMES over rate. Counting bytes instead would report stereo as twice
        // its real length — and audio_seconds is a billing dimension, so that is an
        // overcharge, not a rounding error.
        let stereo = wav_duration_secs(&wav(16000, 2, 16, 16000)).unwrap();
        assert!((stereo - 1.0).abs() < 1e-9, "stereo: got {stereo}");
        let deep = wav_duration_secs(&wav(16000, 1, 32, 16000)).unwrap();
        assert!((deep - 1.0).abs() < 1e-9, "32-bit: got {deep}");
    }

    #[test]
    fn does_not_allocate_the_samples() {
        // The point of a header read on the passthrough: a 60-second upload must not be
        // materialised as samples just to be measured. A 1-hour file is answered instantly.
        let big = wav(48000, 2, 16, 48000 * 3600);
        assert!(wav_duration_secs(&big).is_some());
    }

    #[test]
    fn returns_none_rather_than_guessing() {
        // NULL is the honest answer for a container this cannot read. Reporting a wrong
        // number into a billing column is worse than reporting nothing.
        assert_eq!(wav_duration_secs(b"ID3\x04\x00mp3 data here"), None);
        assert_eq!(wav_duration_secs(b""), None);
        assert_eq!(wav_duration_secs(b"RIFF"), None);
        assert_eq!(wav_duration_secs(&wav(16000, 1, 16, 100)[..20]), None);
    }

    #[test]
    fn a_zero_rate_header_is_none_not_a_division_by_zero() {
        assert_eq!(wav_duration_secs(&wav(0, 1, 16, 1000)), None);
    }
}
