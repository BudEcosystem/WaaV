//! The tool-call tone: a gentle pulse a voice agent plays while it works on a tool call (from the
//! call until it speaks again, the model's thinking around the tool included), after the call's
//! phrase ("One moment."), so the caller hears that the agent is working rather than dead air.
//!
//! Generated rather than recorded, so there is no asset to license or ship: a soft 440 Hz tone with
//! its octave, eased in and out over [`PULSE_MS`], at about -24 dBFS peak. One pulse is played every
//! [`PERIOD_MS`]. The quiet between pulses is not audio: nothing is queued there, so an answer that
//! arrives between pulses is heard at once, and one that arrives during a pulse waits at most
//! [`PULSE_MS`].

use crate::core::tts::sniff::{is_g711, is_linear_pcm16};

/// From the start of one pulse to the start of the next.
pub const PERIOD_MS: u64 = 1_600;
/// One pulse.
pub const PULSE_MS: u32 = 360;
/// Peak amplitude, as a fraction of full scale (about -24 dBFS): under speech level.
pub const PEAK: f32 = 0.063;
const FUNDAMENTAL_HZ: f32 = 440.0;

/// One pulse at `rate`, 16-bit mono PCM.
pub fn pulse(rate: u32) -> Vec<i16> {
    let n = (u64::from(rate) * u64::from(PULSE_MS) / 1000) as usize;
    let rate = rate as f32;
    (0..n)
        .map(|i| {
            let x = i as f32 / n as f32;
            let env = (std::f32::consts::PI * x).sin().powi(2);
            let secs = i as f32 / rate;
            let wave = 0.8 * (std::f32::consts::TAU * FUNDAMENTAL_HZ * secs).sin()
                + 0.2 * (std::f32::consts::TAU * 2.0 * FUNDAMENTAL_HZ * secs).sin();
            (PEAK * env * wave * f32::from(i16::MAX)) as i16
        })
        .collect()
}

/// Whether a generated tone can be written in the session's TTS output `format` (`None` is the
/// gateway default, 16-bit PCM). A compressed format cannot: the session keeps its phrases.
pub fn can_encode(format: Option<&str>) -> bool {
    let f = format.unwrap_or("linear16");
    is_linear_pcm16(f) || is_g711(f)
}

/// `pcm` in the session's TTS output `format`; `None` when [`can_encode`] is false.
pub fn encode(pcm: &[i16], format: Option<&str>) -> Option<Vec<u8>> {
    match format.unwrap_or("linear16") {
        f if is_linear_pcm16(f) => Some(pcm.iter().flat_map(|s| s.to_le_bytes()).collect()),
        "mulaw" | "ulaw" => Some(pcm.iter().map(|&s| linear_to_mulaw(s)).collect()),
        "alaw" => Some(pcm.iter().map(|&s| linear_to_alaw(s)).collect()),
        _ => None,
    }
}

fn linear_to_mulaw(sample: i16) -> u8 {
    const BIAS: i32 = 0x84;
    const CLIP: i32 = 32_635;
    let mut s = i32::from(sample);
    let sign = if s < 0 { 0x80 } else { 0 };
    if s < 0 {
        s = -s;
    }
    s = s.min(CLIP) + BIAS;
    let exponent = (7 - (s.leading_zeros() as i32 - 17)).clamp(0, 7);
    let mantissa = (s >> (exponent + 3)) & 0x0F;
    !((sign | (exponent << 4) | mantissa) as u8)
}

fn linear_to_alaw(sample: i16) -> u8 {
    let mut s = i32::from(sample) >> 3;
    let sign = if s >= 0 {
        0xD5
    } else {
        s = -s - 1;
        0x55
    };
    let seg_ends = [0x1F, 0x3F, 0x7F, 0xFF, 0x1FF, 0x3FF, 0x7FF, 0xFFF];
    let seg = seg_ends.iter().position(|&end| s <= end).unwrap_or(8) as i32;
    let aval = if seg >= 8 {
        0x7F
    } else {
        let shift = if seg < 2 { 1 } else { seg };
        (seg << 4) | ((s >> shift) & 0x0F)
    };
    (aval ^ sign) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peak(s: &[i16]) -> i32 {
        s.iter().map(|v| i32::from(*v).abs()).max().unwrap_or(0)
    }

    #[test]
    fn a_pulse_is_soft_and_eased_in_and_out() {
        let p = pulse(16_000);
        assert_eq!(p.len(), 16_000 * PULSE_MS as usize / 1000);
        let ceiling = (PEAK * f32::from(i16::MAX)) as i32 + 1;
        assert!(peak(&p) > ceiling / 2, "audible");
        assert!(peak(&p) <= ceiling, "never above the gentle peak");
        assert!(
            peak(&p[..16]) < 200 && peak(&p[p.len() - 16..]) < 200,
            "no click"
        );
        assert_eq!(pulse(24_000).len(), 24_000 * PULSE_MS as usize / 1000);
    }

    #[test]
    fn it_is_written_in_the_session_format_or_not_at_all() {
        let pcm = [0i16, 1000, -1000];
        let le = encode(&pcm, None).unwrap();
        assert_eq!(
            le,
            vec![0, 0, 0xE8, 0x03, 0x18, 0xFC],
            "default: 16-bit little-endian"
        );
        for f in ["linear16", "pcm", "pcm16"] {
            assert_eq!(encode(&pcm, Some(f)).unwrap(), le, "{f}");
        }
        assert_eq!(
            encode(&pcm, Some("mulaw")).unwrap().len(),
            3,
            "one byte per sample"
        );
        assert_eq!(encode(&pcm, Some("ulaw")), encode(&pcm, Some("mulaw")));
        assert_eq!(encode(&pcm, Some("alaw")).unwrap().len(), 3);
        for f in ["mp3", "opus", "wav", "ogg"] {
            assert!(
                encode(&pcm, Some(f)).is_none() && !can_encode(Some(f)),
                "{f}"
            );
        }
        assert!(can_encode(None) && can_encode(Some("mulaw")));
    }

    #[test]
    fn g711_encodes_silence_and_sign() {
        assert_eq!(encode(&[0], Some("mulaw")).unwrap(), vec![0xFF]);
        assert_eq!(encode(&[0], Some("alaw")).unwrap(), vec![0xD5]);
        let loud = encode(&[8_000, -8_000], Some("mulaw")).unwrap();
        assert_ne!(loud[0], loud[1], "the sign is kept");
        // The standard's reference points: full-scale positive and negative.
        assert_eq!(
            encode(&[i16::MAX, i16::MIN + 1], Some("mulaw")).unwrap(),
            vec![0x80, 0x00]
        );
        assert_eq!(
            encode(&[i16::MAX, i16::MIN], Some("alaw")).unwrap(),
            vec![0xAA, 0x2A]
        );
    }
}
