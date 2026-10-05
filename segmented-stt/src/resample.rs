//! The one streaming resampler core: rubato's FFT resampler, mono, fed in whatever sizes the audio
//! arrives in and drained in whole chunks.
//!
//! Segmented sessions' front end (any rate to 16 kHz) and the gateway's `StreamResampler` (any rate
//! to any rate, on the egress and the realtime facade) wrap it with their own policies: when to
//! start fresh, what an error does, how samples become PCM.

use rubato::{FftFixedIn, Resampler};

#[derive(Debug, thiserror::Error)]
#[error("resampling {from} Hz to {to} Hz: {message}")]
pub struct ResampleError {
    pub from: u32,
    pub to: u32,
    pub message: String,
}

/// The frames a resampler from `from` Hz processes per chunk: about 20 ms (`from / 50`), at least
/// 64 and at most `max_chunk`, so 8 kHz telephony never waits 128 ms for a chunk to fill.
pub fn chunk_frames(from: u32, max_chunk: usize) -> usize {
    ((from as usize) / 50).clamp(64, max_chunk.max(64))
}

pub struct MonoResampler {
    inner: FftFixedIn<f32>,
    pending: Vec<f32>,
    from: u32,
    to: u32,
}

impl std::fmt::Debug for MonoResampler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MonoResampler")
            .field("from", &self.from)
            .field("to", &self.to)
            .field("pending", &self.pending.len())
            .finish()
    }
}

impl MonoResampler {
    /// `from` Hz to `to` Hz, processing about 20 ms per chunk (`from / 50` frames, at least 64
    /// and at most `max_chunk`).
    pub fn new(from: u32, to: u32, max_chunk: usize) -> Result<Self, ResampleError> {
        let chunk = chunk_frames(from, max_chunk);
        let inner =
            FftFixedIn::<f32>::new(from as usize, to as usize, chunk, 2, 1).map_err(|e| {
                ResampleError {
                    from,
                    to,
                    message: e.to_string(),
                }
            })?;
        Ok(Self {
            inner,
            pending: Vec::new(),
            from,
            to,
        })
    }

    pub fn rates(&self) -> (u32, u32) {
        (self.from, self.to)
    }

    /// Samples held until the next whole chunk.
    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    /// Resample every whole chunk `input` completes into `out`; the rest waits for the next push.
    /// Every chunk is tried; the first failure is returned after the others are processed.
    pub fn push(&mut self, input: &[f32], out: &mut Vec<f32>) -> Result<(), ResampleError> {
        self.pending.extend_from_slice(input);
        let mut first_error = None;
        loop {
            let chunk = self.inner.input_frames_next().max(1);
            if self.pending.len() < chunk {
                break;
            }
            let take: Vec<f32> = self.pending.drain(..chunk).collect();
            match self.inner.process(&[take], None) {
                Ok(mut resampled) => {
                    if let Some(channel) = resampled.pop() {
                        out.extend_from_slice(&channel);
                    }
                }
                Err(e) => {
                    first_error.get_or_insert_with(|| self.error(e.to_string()));
                }
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    /// Push the held tail through with zeros, so the last sound is not lost, then start fresh.
    /// Nothing held is nothing to do.
    pub fn flush(&mut self, out: &mut Vec<f32>) -> Result<(), ResampleError> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let chunk = self.inner.input_frames_next().max(1);
        let mut tail = std::mem::take(&mut self.pending);
        tail.resize(chunk, 0.0);
        let result = match self.inner.process(&[tail], None) {
            Ok(mut resampled) => {
                if let Some(channel) = resampled.pop() {
                    out.extend_from_slice(&channel);
                }
                Ok(())
            }
            Err(e) => Err(self.error(e.to_string())),
        };
        self.inner.reset();
        result
    }

    /// Drop what is held and the filter's state: the next push starts a new stream.
    pub fn reset(&mut self) {
        self.inner.reset();
        self.pending.clear();
    }

    fn error(&self, message: String) -> ResampleError {
        ResampleError {
            from: self.from,
            to: self.to,
            message,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: u32, ms: u32) -> Vec<f32> {
        let n = (rate * ms / 1000) as usize;
        (0..n)
            .map(|i| (std::f32::consts::TAU * 440.0 * i as f32 / rate as f32).sin() * 0.5)
            .collect()
    }

    #[test]
    fn pieces_of_any_size_give_the_same_audio_as_one_push() {
        let input = tone(48_000, 500);
        let mut whole = MonoResampler::new(48_000, 16_000, 2048).unwrap();
        let mut a = Vec::new();
        whole.push(&input, &mut a).unwrap();
        whole.flush(&mut a).unwrap();
        let mut pieces = MonoResampler::new(48_000, 16_000, 2048).unwrap();
        let mut b = Vec::new();
        for piece in input.chunks(333) {
            pieces.push(piece, &mut b).unwrap();
        }
        pieces.flush(&mut b).unwrap();
        assert_eq!(a, b);
        let expected = input.len() / 3;
        assert!(
            a.len() >= expected && a.len() <= expected + 1024,
            "{} samples for {expected}",
            a.len()
        );
    }

    #[test]
    fn a_reset_drops_what_is_held() {
        let mut r = MonoResampler::new(24_000, 16_000, 1024).unwrap();
        let mut out = Vec::new();
        r.push(&tone(24_000, 5), &mut out).unwrap();
        r.reset();
        r.flush(&mut out).unwrap();
        assert!(out.is_empty(), "nothing was held after the reset");
        assert_eq!(r.rates(), (24_000, 16_000));
    }

    #[test]
    fn an_impossible_rate_is_an_error_not_a_panic() {
        assert!(MonoResampler::new(0, 16_000, 1024).is_err());
    }
}
