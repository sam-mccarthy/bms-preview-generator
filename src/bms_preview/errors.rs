use audioadapter_buffers::SizeError;
use bms_rs::bms::error::ParseErrorWithRange;
use rubato::{ResampleError, ResamplerConstructionError};
use std::io;
use thiserror::Error;
use vorbis_rs::VorbisError;

#[derive(Error, Debug)]
pub enum ProcessError {
    #[error("songs folder is invalid")]
    InvalidSongsFolder(),
    #[error("failed to get songs: {0}")]
    FailedSongIO(#[from] io::Error),
    #[error("renderer failed: {0}")]
    RendererFailed(#[from] RendererError),
}

#[derive(Error, Debug)]
pub enum RendererError {
    #[error("failed to decode .bms file ({0}) with {1} format")]
    BMSDecodingError(String, String),
    #[error("failed to parse .bms file: {0}")]
    BMSParsingError(#[from] ParseErrorWithRange),
    #[error("failed to read .bms file: {0}")]
    FileNotFound(#[from] io::Error),
}

#[derive(Error, Debug)]
pub enum AudioError {
    #[error("mismatched sample rate")]
    MismatchedSampleRate(),
    #[error("failed to find audio file")]
    FileNotFound(),
    #[error("failed to get vital codec info")]
    MissingCodecInfo(),
    #[error("invalid codec info")]
    InvalidCodecInfo(),
    #[error("resampler construction error: {0}")]
    ResamplerConstructionError(#[from] ResamplerConstructionError),
    #[error("invalid audio size: {0}")]
    InvalidAudioSize(#[from] SizeError),
    #[error("resampler error: {0}")]
    ResamplerError(#[from] ResampleError),
    #[error("I/O error: {0}")]
    IOError(#[from] io::Error),
    #[error("audio decoder error: {0}")]
    DecodingError(#[from] symphonia::core::errors::Error),
    #[error("vorbis encoder error: {0}")]
    VorbisEncodingError(#[from] VorbisError),
}
