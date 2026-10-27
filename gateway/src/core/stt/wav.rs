//! The WAV writer lives in the segmented speech-to-text crate, shared with its utterance uploads.

pub(crate) use waav_segmented_stt::wav::{
    WavBuildError, create_pcm_wav_header, encode_pcm_wav, encode_pcm16_wav,
};
