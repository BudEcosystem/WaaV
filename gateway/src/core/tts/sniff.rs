//! Magic-byte audio container sniffing (P0.1 / RC1 "trust-by-declaration").
//!
//! The TTS chunker historically framed provider bytes by the *declared*
//! `audio_format`; providers that actually return a container (Speechify WAV,
//! UnrealSpeech/WellSaid MP3) had their headers sliced into "PCM" frames →
//! audible clicks/noise. This module inspects the first bytes of every TTS
//! payload so the pipeline operates on what the bytes *are*, fleet-wide:
//! HTTP responses, cache writes, and cache hits all pass through here.
//!
//! Zero-dependency by design — four magic prefixes cover every container any
//! current provider emits; raw PCM has no magic and yields `None`.

/// A container format identified from leading bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SniffedContainer {
    Wav,
    Mp3,
    Ogg,
    Flac,
}

impl SniffedContainer {
    /// Canonical `AudioData.format` label for the container.
    pub fn as_format_str(self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::Mp3 => "mp3",
            Self::Ogg => "ogg",
            Self::Flac => "flac",
        }
    }
}

/// Identify a container from the first bytes of an audio payload.
///
/// `None` means "no known container magic" — consistent with raw PCM (which
/// has none). Needs ≥ 12 bytes for a confident WAV check; shorter slices are
/// checked against what they can prove.
pub fn sniff_container(bytes: &[u8]) -> Option<SniffedContainer> {
    // RIFF....WAVE
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE" {
        return Some(SniffedContainer::Wav);
    }
    // ID3 tag (MP3 with metadata)
    if bytes.len() >= 3 && &bytes[0..3] == b"ID3" {
        return Some(SniffedContainer::Mp3);
    }
    // Bare MPEG audio frame sync. CRITICAL false-positive guard (found by the
    // adversarial review): raw PCM whose first sample is -1 is 0xFF 0xFF and
    // matches a naive sync check. Validate the full header nibble set:
    //   byte1: 11-bit sync + version not reserved (0b01) + layer not reserved
    //   byte2: bitrate nibble not invalid (0xF) + sample-rate bits not
    //          reserved (0b11)
    // An all-0xFF PCM run fails on bitrate=0xF; real MP3 frames pass.
    if bytes.len() >= 4
        && bytes[0] == 0xFF
        && (bytes[1] & 0xE0) == 0xE0
        && (bytes[1] & 0x18) != 0x08 // version: reserved
        && (bytes[1] & 0x06) != 0x00 // layer: reserved
        && (bytes[2] & 0xF0) != 0xF0 // bitrate: invalid
        && (bytes[2] & 0xF0) != 0x00 // bitrate: "free" — legal but unused by real encoders; PCM noise hits it
        && (bytes[2] & 0x0C) != 0x0C
    // sample rate: reserved
    {
        return Some(SniffedContainer::Mp3);
    }
    // Ogg
    if bytes.len() >= 4 && &bytes[0..4] == b"OggS" {
        return Some(SniffedContainer::Ogg);
    }
    // FLAC
    if bytes.len() >= 4 && &bytes[0..4] == b"fLaC" {
        return Some(SniffedContainer::Flac);
    }
    None
}

/// True when the declared format is a raw-PCM family the chunker would slice.
pub fn is_pcm_family(declared: &str) -> bool {
    matches!(
        declared,
        "linear16" | "pcm" | "pcm16" | "mulaw" | "ulaw" | "alaw"
    )
}

/// True ONLY for 16-bit linear PCM — the formats whose bytes ARE i16 LE
/// samples. G.711 (mulaw/ulaw/alaw) is in the PCM *family* (headerless,
/// sliceable) but its bytes are 8-bit COMPANDED samples: feeding them to
/// PCM16 byte math (resampling, duration) produces noise/2x errors
/// (review wf_85659e16 #6/#12).
pub fn is_linear_pcm16(declared: &str) -> bool {
    matches!(declared, "linear16" | "pcm" | "pcm16")
}

/// True for 8-bit G.711 companded formats (1 byte per sample).
pub fn is_g711(declared: &str) -> bool {
    matches!(declared, "mulaw" | "ulaw" | "alaw")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav_header() -> Vec<u8> {
        let mut v = b"RIFF".to_vec();
        v.extend_from_slice(&36u32.to_le_bytes());
        v.extend_from_slice(b"WAVEfmt ");
        v.extend_from_slice(&[0u8; 32]);
        v
    }

    #[test]
    fn sniffs_wav_riff_header() {
        assert_eq!(sniff_container(&wav_header()), Some(SniffedContainer::Wav));
    }

    #[test]
    fn sniffs_mp3_id3_and_framesync() {
        assert_eq!(
            sniff_container(b"ID3\x04\x00rest"),
            Some(SniffedContainer::Mp3)
        );
        assert_eq!(
            sniff_container(&[0xFF, 0xFB, 0x90, 0x00]),
            Some(SniffedContainer::Mp3)
        );
        assert_eq!(
            sniff_container(&[0xFF, 0xF3, 0x18, 0xC4]),
            Some(SniffedContainer::Mp3)
        );
    }

    /// Adversarial-review regression: PCM whose first samples are -1 (all
    /// 0xFF bytes) must NOT be sniffed as MP3 — bitrate nibble 0xF is invalid.
    #[test]
    fn pcm_negative_one_samples_are_not_mp3() {
        assert_eq!(sniff_container(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]), None);
        // full-scale click then normal audio
        assert_eq!(sniff_container(&[0xFF, 0xFF, 0x00, 0x01, 0x12, 0x00]), None);
        // reserved version / reserved layer / reserved sample-rate variants
        assert_eq!(sniff_container(&[0xFF, 0xEB, 0x90, 0x00]), None); // version reserved
        assert_eq!(sniff_container(&[0xFF, 0xF9, 0x9C, 0x00]), None); // layer reserved
        assert_eq!(sniff_container(&[0xFF, 0xFB, 0x9C, 0x00]), None); // samplerate reserved
    }

    #[test]
    fn sniffs_ogg_and_flac() {
        assert_eq!(
            sniff_container(b"OggS\x00\x02more"),
            Some(SniffedContainer::Ogg)
        );
        assert_eq!(
            sniff_container(b"fLaC\x00\x00\x00\x22"),
            Some(SniffedContainer::Flac)
        );
    }

    #[test]
    fn raw_pcm_and_silence_yield_none() {
        assert_eq!(sniff_container(&[0u8; 64]), None);
        // typical small-amplitude little-endian PCM
        assert_eq!(sniff_container(&[0x12, 0x00, 0xF4, 0xFF, 0x03, 0x00]), None);
        assert_eq!(sniff_container(b""), None);
    }

    #[test]
    fn pcm_family_classification() {
        for f in ["linear16", "pcm", "pcm16", "mulaw", "ulaw", "alaw"] {
            assert!(is_pcm_family(f), "{f}");
        }
        for f in ["mp3", "wav", "ogg", "opus", "aac", "flac", "wav_48000"] {
            assert!(!is_pcm_family(f), "{f}");
        }
    }

    /// The exact failure mode from the audit: WAV/MP3 bytes declared linear16
    /// must be detected so the chunker never slices them as PCM.
    #[test]
    fn audit_regression_container_vs_declared_pcm() {
        for (bytes, name) in [
            (wav_header(), "speechify wav"),
            (b"ID3\x03\x00\x00mp3-data".to_vec(), "wellsaid mp3"),
        ] {
            let sniffed = sniff_container(&bytes);
            assert!(sniffed.is_some(), "{name} must be detected");
            assert!(is_pcm_family("linear16"));
        }
    }
}
