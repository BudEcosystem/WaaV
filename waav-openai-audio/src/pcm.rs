//! Decoding an uploaded audio file into the PCM a streaming STT provider accepts.
//!
//! `/v1/audio/transcriptions` takes a FILE; every WaaV STT provider takes a stream of raw
//! samples. Something has to bridge the two, and this is it — kept dependency-free and pure
//! so the format edge cases can be tested in milliseconds rather than against a live vendor.
//!
//! WAV and headerless PCM are parsed here directly. The compressed formats OpenAI's API accepts
//! — mp3/mpeg/mpga, m4a/mp4 (AAC), flac, ogg and webm (Vorbis) — are decoded by Symphonia, a
//! pure-Rust decoder, then downmixed and resampled to 16 kHz. Opus, which Symphonia cannot
//! decode, is refused by name. Guessing at a container without a real decoder would not produce
//! a bad transcript, it would produce a confident one from noise, which is far worse than a 400.

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
pub const DECODABLE_EXTENSIONS: &[&str] = &[
    "wav", "wave", "pcm", "raw", "mp3", "mpeg", "mpga", "m4a", "mp4", "flac", "ogg", "oga", "webm",
];

/// The subset that goes through Symphonia.
const COMPRESSED_EXTENSIONS: &[&str] = &[
    "mp3", "mpeg", "mpga", "m4a", "mp4", "flac", "ogg", "oga", "webm",
];

/// Rate compressed uploads are resampled DOWN to (never up).
///
/// A compressed upload has no rate the caller chose for transcription — it is whatever the
/// encoder used, usually 44.1 or 48 kHz — and decoding at that rate costs 3x the memory of the
/// 16 kHz every STT vendor here is built around. WAV uploads keep their own rate.
pub const COMPRESSED_TARGET_RATE: u32 = 16_000;

/// The longest compressed upload decoded, in seconds.
///
/// The 25 MB upload cap bounds a WAV's samples but not an MP3's: at 16 kbps, 25 MB is over three
/// hours, which decodes to hundreds of MB. Thirty minutes covers a 25 MB MP3 at 128 kbps
/// (about 27 minutes) and bounds the decode at ~58 MB of 16 kHz samples.
pub const MAX_DECODED_SECS: u32 = 30 * 60;

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
        // Headerless samples carry no magic, and a small negative first sample (0xFFFF) looks
        // exactly like an MPEG frame sync — so for these the name is the only evidence.
        "pcm" | "raw" => decode_raw_pcm(bytes, DEFAULT_PCM_SAMPLE_RATE),
        // Otherwise the bytes are trusted over the name, in both directions: a real WAV named
        // `audio.bin` is a WAV, and an MP3 named `speech.wav` — a browser recorder's default —
        // is an MP3. Refusing either would be pedantry about a label.
        _ if bytes.starts_with(b"RIFF") => decode_wav(bytes),
        _ if looks_compressed(bytes) => decode_compressed(
            bytes,
            Some(ext.as_str()).filter(|e| COMPRESSED_EXTENSIONS.contains(e)),
        ),
        "wav" | "wave" => decode_wav(bytes),
        e if COMPRESSED_EXTENSIONS.contains(&e) => decode_compressed(bytes, Some(e)),
        other => Err(AudioError::InvalidField {
            field: "file",
            reason: format!(
                "cannot decode {} audio; this gateway transcribes {}. \
                 Convert the file first (e.g. `ffmpeg -i in.{} -ar 16000 -ac 1 out.wav`).",
                if other.is_empty() {
                    "unrecognised"
                } else {
                    other
                },
                DECODABLE_EXTENSIONS.join(", "),
                if other.is_empty() { "audio" } else { other },
            ),
        }),
    }
}

/// Whether the bytes open with a compressed container's magic.
fn looks_compressed(b: &[u8]) -> bool {
    b.starts_with(b"ID3")                                          // mp3 with ID3v2 tag
        || (b.len() > 1 && b[0] == 0xFF && (b[1] & 0xE0) == 0xE0)  // mpeg audio frame sync
        || b.starts_with(b"fLaC")
        || b.starts_with(b"OggS")
        || b.get(4..8) == Some(b"ftyp")                             // mp4 / m4a
        || b.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) // matroska / webm
}

/// Decode a compressed container to mono 16-bit PCM at no more than 16 kHz.
///
/// Streams packet by packet: each is downmixed and fed to the resampler as it is decoded, so the
/// only full-length buffer is the 16 kHz output — never the source-rate float samples, which for
/// a 30-minute 48 kHz file would be 345 MB. The duration cap is checked as samples arrive, so an
/// over-long file is refused before it has been decoded to the end.
fn decode_compressed(bytes: &[u8], ext: Option<&str>) -> Result<PcmAudio, AudioError> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::{CODEC_TYPE_NULL, CODEC_TYPE_OPUS, DecoderOptions};
    use symphonia::core::errors::Error as SymphoniaError;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let label = ext.unwrap_or("compressed");
    let mss = MediaSourceStream::new(
        Box::new(std::io::Cursor::new(bytes.to_vec())),
        Default::default(),
    );
    let mut hint = Hint::new();
    if let Some(e) = ext {
        hint.with_extension(e);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| invalid(format!("could not read the {label} file: {e}")))?;
    let mut format = probed.format;

    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| invalid(format!("the {label} file has no audio track")))?;
    if track.codec_params.codec == CODEC_TYPE_OPUS {
        return Err(invalid(
            "Opus audio cannot be decoded here; send wav, mp3, m4a, flac or ogg (Vorbis), \
             or convert it first (e.g. `ffmpeg -i in.ogg -ar 16000 -ac 1 out.wav`)",
        ));
    }
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| {
            invalid(format!(
                "the {label} file's codec cannot be decoded here: {e}"
            ))
        })?;

    let mut out = Downsampler::default();
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            // The normal end of a stream in Symphonia is an unexpected-EOF I/O error.
            Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                break;
            }
            Err(SymphoniaError::ResetRequired) => break,
            Err(e) => return Err(invalid(format!("the {label} file is unreadable: {e}"))),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            // A single corrupt frame is skipped, as players do; it is not the whole file.
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(e) => {
                return Err(invalid(format!(
                    "the {label} file could not be decoded: {e}"
                )));
            }
        };
        let spec = *decoded.spec();
        let channels = spec.channels.count().max(1);
        let mut buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        buf.copy_interleaved_ref(decoded);
        let mono: Vec<f32> = buf
            .samples()
            .chunks(channels)
            .map(|frame| frame.iter().sum::<f32>() / channels as f32)
            .collect();
        out.push(&mono, spec.rate)?;
    }
    out.finish(label)
}

/// Streaming mono resampler to at most [`COMPRESSED_TARGET_RATE`], with the duration cap.
#[derive(Default)]
struct Downsampler {
    source_rate: u32,
    resampler: Option<rubato::FftFixedIn<f32>>,
    pending: Vec<f32>,
    source_frames: usize,
    output: Vec<i16>,
}

impl Downsampler {
    fn push(&mut self, mono: &[f32], rate: u32) -> Result<(), AudioError> {
        use rubato::Resampler;
        if rate == 0 {
            return Err(invalid("the file declares a sample rate of 0"));
        }
        if self.source_rate == 0 {
            self.source_rate = rate;
            if rate > COMPRESSED_TARGET_RATE {
                self.resampler = Some(
                    rubato::FftFixedIn::<f32>::new(
                        rate as usize,
                        COMPRESSED_TARGET_RATE as usize,
                        1024,
                        2,
                        1,
                    )
                    .map_err(|e| invalid(format!("cannot resample {rate} Hz audio: {e}")))?,
                );
            }
        } else if rate != self.source_rate {
            return Err(invalid(
                "the file changes sample rate mid-stream, which cannot be transcribed as one clip",
            ));
        }
        self.source_frames += mono.len();
        if self.source_frames as u64 > MAX_DECODED_SECS as u64 * rate as u64 {
            return Err(AudioError::TooLarge {
                field: "file",
                limit: format!("{} minutes of audio", MAX_DECODED_SECS / 60),
                actual: "a longer recording; split it and send the parts".to_string(),
            });
        }
        match self.resampler.as_mut() {
            None => self.output.extend(mono.iter().map(|s| to_i16(*s))),
            Some(r) => {
                self.pending.extend_from_slice(mono);
                let chunk = r.input_frames_next();
                while self.pending.len() >= chunk {
                    let take: Vec<f32> = self.pending.drain(..chunk).collect();
                    let resampled = r
                        .process(&[take], None)
                        .map_err(|e| invalid(format!("resampling failed: {e}")))?;
                    if let Some(ch) = resampled.first() {
                        self.output.extend(ch.iter().map(|s| to_i16(*s)));
                    }
                }
            }
        }
        Ok(())
    }

    /// Flush the resampler's tail and trim its filter delay, so the output is exactly the
    /// source's duration at the target rate — no clipped final syllable, no leading silence.
    fn finish(mut self, label: &str) -> Result<PcmAudio, AudioError> {
        use rubato::Resampler;
        if self.source_rate == 0 {
            return Err(invalid(format!("the {label} file contains no audio")));
        }
        let Some(mut r) = self.resampler.take() else {
            return Ok(PcmAudio {
                samples: self.output,
                sample_rate: self.source_rate,
            });
        };
        let delay = r.output_delay();
        let tail = std::mem::take(&mut self.pending);
        let mut flush = |input: Option<&[Vec<f32>]>| -> Result<(), AudioError> {
            let out = r
                .process_partial(input, None)
                .map_err(|e| invalid(format!("resampling failed: {e}")))?;
            if let Some(ch) = out.first() {
                self.output.extend(ch.iter().map(|s| to_i16(*s)));
            }
            Ok(())
        };
        if !tail.is_empty() {
            flush(Some(&[tail]))?;
        }
        flush(None)?;
        let expected = (self.source_frames as u64 * COMPRESSED_TARGET_RATE as u64
            / self.source_rate as u64) as usize;
        let samples: Vec<i16> = self.output.into_iter().skip(delay).take(expected).collect();
        Ok(PcmAudio {
            samples,
            sample_rate: COMPRESSED_TARGET_RATE,
        })
    }
}

fn to_i16(s: f32) -> i16 {
    (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
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
        let size = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
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
        let size = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
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
    fn wav(
        channels: u16,
        sample_rate: u32,
        bits: u16,
        samples: &[i16],
        extra_chunk: bool,
    ) -> Vec<u8> {
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
        assert!(
            err.contains("ffmpeg"),
            "the message must say how to fix it: {err}"
        );
    }

    /// "Hello from Bud." — 44.1 kHz mono MP3, synthesised by the `eleven-v3` deployment.
    const HELLO_MP3: &[u8] = include_bytes!("../tests/fixtures/hello.mp3");
    /// The same sentence as Ogg Opus (ElevenLabs `opus_48000_64`).
    const HELLO_OPUS: &[u8] = include_bytes!("../tests/fixtures/hello.opus");

    #[test]
    fn an_mp3_decodes_to_16khz_mono_of_the_right_length() {
        let audio = decode(HELLO_MP3, "speech.mp3").expect("a real MP3 must decode");
        assert_eq!(audio.sample_rate, COMPRESSED_TARGET_RATE);
        // ~1-2 s of speech; the exact length is the encoder's, the bounds are sanity.
        let secs = audio.duration_secs();
        assert!((0.5..5.0).contains(&secs), "decoded {secs}s");
        // Not silence: a decoder that ran but produced zeros would pass the length check.
        let peak = audio
            .samples
            .iter()
            .map(|s| s.unsigned_abs())
            .max()
            .unwrap_or(0);
        assert!(peak > 1000, "peak amplitude {peak} — decoded to silence?");
    }

    #[test]
    fn a_compressed_file_is_recognised_by_its_bytes_whatever_its_name() {
        let named = decode(HELLO_MP3, "speech.mp3").unwrap();
        for name in ["upload.bin", "noextension", "speech.MP3", "speech.wav"] {
            let other = decode(HELLO_MP3, name).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(other, named, "{name}");
        }
    }

    #[test]
    fn opus_is_refused_by_name_with_a_way_out() {
        let err = decode(HELLO_OPUS, "speech.ogg").unwrap_err().to_string();
        assert!(err.contains("Opus"), "got: {err}");
        assert!(err.contains("mp3") && err.contains("ffmpeg"), "got: {err}");
    }

    #[test]
    fn a_truncated_or_fake_mp3_is_a_clean_error_not_a_panic() {
        assert!(decode(b"\xff\xfbfake mp3 body", "speech.mp3").is_err());
        // A file cut off mid-frame: whatever it yields, it must not panic the worker.
        let _ = decode(&HELLO_MP3[..HELLO_MP3.len() / 2], "speech.mp3");
    }

    #[test]
    fn the_duration_cap_is_enforced_while_decoding() {
        // 8 kHz, so no resampler runs and the test stays fast; the cap is on source duration.
        let mut d = Downsampler::default();
        let minute = vec![0.0f32; 8_000 * 60];
        let mut refused = None;
        for i in 0..=(MAX_DECODED_SECS / 60 + 1) {
            if let Err(e) = d.push(&minute, 8_000) {
                refused = Some((i, e));
                break;
            }
        }
        let (at, err) = refused.expect("a recording past the cap must be refused");
        assert_eq!(
            at,
            MAX_DECODED_SECS / 60,
            "refused on the first minute past the cap"
        );
        assert!(
            matches!(err, AudioError::TooLarge { field: "file", .. }),
            "{err:?}"
        );
    }

    #[test]
    fn an_unknown_container_is_refused_and_names_what_works() {
        let err = decode(b"not audio at all", "speech.xyz")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("xyz") && err.contains("mp3") && err.contains("wav"),
            "got: {err}"
        );
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
        let raw: Vec<u8> = [1i16, -1, 300]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
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
