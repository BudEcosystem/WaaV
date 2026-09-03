//! Shared WAV container helpers for STT providers that upload PCM as `audio/wav`.

use std::fmt;

pub(crate) const HEADER_SIZE: usize = 44;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WavBuildError {
    ZeroSampleRate,
    ZeroChannels,
    ZeroBitsPerSample,
    InvalidBitsPerSample(u16),
    HeaderArithmeticOverflow(&'static str),
    DataTooLarge,
}

impl fmt::Display for WavBuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroSampleRate => write!(f, "sample rate cannot be zero"),
            Self::ZeroChannels => write!(f, "number of channels cannot be zero"),
            Self::ZeroBitsPerSample => write!(f, "bits per sample cannot be zero"),
            Self::InvalidBitsPerSample(bits) => {
                write!(f, "bits per sample must be byte-aligned, got {bits}")
            }
            Self::HeaderArithmeticOverflow(field) => {
                write!(f, "WAV header arithmetic overflow for {field}")
            }
            Self::DataTooLarge => write!(f, "PCM data exceeds maximum WAV file size"),
        }
    }
}

impl std::error::Error for WavBuildError {}

pub(crate) fn create_pcm_wav_header(
    sample_rate: u32,
    bits_per_sample: u16,
    num_channels: u16,
    data_size: u32,
) -> Result<[u8; HEADER_SIZE], WavBuildError> {
    if sample_rate == 0 {
        return Err(WavBuildError::ZeroSampleRate);
    }
    if num_channels == 0 {
        return Err(WavBuildError::ZeroChannels);
    }
    if bits_per_sample == 0 {
        return Err(WavBuildError::ZeroBitsPerSample);
    }
    if bits_per_sample % 8 != 0 {
        return Err(WavBuildError::InvalidBitsPerSample(bits_per_sample));
    }

    let bytes_per_sample = bits_per_sample / 8;
    let block_align = num_channels
        .checked_mul(bytes_per_sample)
        .ok_or(WavBuildError::HeaderArithmeticOverflow("block_align"))?;
    let byte_rate = sample_rate
        .checked_mul(u32::from(block_align))
        .ok_or(WavBuildError::HeaderArithmeticOverflow("byte_rate"))?;
    let chunk_size = 36u32
        .checked_add(data_size)
        .ok_or(WavBuildError::DataTooLarge)?;

    let mut header = [0u8; HEADER_SIZE];
    header[0..4].copy_from_slice(b"RIFF");
    header[4..8].copy_from_slice(&chunk_size.to_le_bytes());
    header[8..12].copy_from_slice(b"WAVE");
    header[12..16].copy_from_slice(b"fmt ");
    header[16..20].copy_from_slice(&16u32.to_le_bytes());
    header[20..22].copy_from_slice(&1u16.to_le_bytes());
    header[22..24].copy_from_slice(&num_channels.to_le_bytes());
    header[24..28].copy_from_slice(&sample_rate.to_le_bytes());
    header[28..32].copy_from_slice(&byte_rate.to_le_bytes());
    header[32..34].copy_from_slice(&block_align.to_le_bytes());
    header[34..36].copy_from_slice(&bits_per_sample.to_le_bytes());
    header[36..40].copy_from_slice(b"data");
    header[40..44].copy_from_slice(&data_size.to_le_bytes());
    Ok(header)
}

pub(crate) fn encode_pcm_wav(
    pcm_data: &[u8],
    sample_rate: u32,
    bits_per_sample: u16,
    num_channels: u16,
) -> Result<Vec<u8>, WavBuildError> {
    let data_size = u32::try_from(pcm_data.len()).map_err(|_| WavBuildError::DataTooLarge)?;
    let header = create_pcm_wav_header(sample_rate, bits_per_sample, num_channels, data_size)?;
    let capacity = HEADER_SIZE
        .checked_add(pcm_data.len())
        .ok_or(WavBuildError::DataTooLarge)?;
    let mut wav = Vec::with_capacity(capacity);
    wav.extend_from_slice(&header);
    wav.extend_from_slice(pcm_data);
    Ok(wav)
}

pub(crate) fn encode_pcm16_wav(
    pcm_data: &[u8],
    sample_rate: u32,
    num_channels: u16,
) -> Result<Vec<u8>, WavBuildError> {
    encode_pcm_wav(pcm_data, sample_rate, 16, num_channels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_pcm_wav_rejects_invalid_geometry_without_panicking() {
        assert_eq!(
            encode_pcm16_wav(&[0, 0], 0, 1).unwrap_err(),
            WavBuildError::ZeroSampleRate
        );
        assert_eq!(
            encode_pcm16_wav(&[0, 0], 16_000, 0).unwrap_err(),
            WavBuildError::ZeroChannels
        );
        assert_eq!(
            encode_pcm_wav(&[0, 0], 16_000, 7, 1).unwrap_err(),
            WavBuildError::InvalidBitsPerSample(7)
        );
        assert_eq!(
            encode_pcm16_wav(&[0, 0], 16_000, u16::MAX).unwrap_err(),
            WavBuildError::HeaderArithmeticOverflow("block_align")
        );
        assert_eq!(
            encode_pcm16_wav(&[0, 0], u32::MAX, 1).unwrap_err(),
            WavBuildError::HeaderArithmeticOverflow("byte_rate")
        );
    }

    #[test]
    fn encode_pcm_wav_preserves_pcm16_header_layout() {
        let pcm = vec![0u8; 100];
        let wav = encode_pcm16_wav(&pcm, 16_000, 1).expect("valid PCM16 WAV");
        assert_eq!(wav.len(), HEADER_SIZE + pcm.len());
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(u32::from_le_bytes([wav[4], wav[5], wav[6], wav[7]]), 136);
        assert_eq!(
            u32::from_le_bytes([wav[28], wav[29], wav[30], wav[31]]),
            32_000
        );
        assert_eq!(u16::from_le_bytes([wav[32], wav[33]]), 2);
        assert_eq!(
            u32::from_le_bytes([wav[40], wav[41], wav[42], wav[43]]),
            100
        );
    }
}
