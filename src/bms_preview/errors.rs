use audioadapter_buffers::SizeError;
use bms_rs::bms::error::ParseErrorWithRange;
use rubato::{ResampleError, ResamplerConstructionError};
use std::io;
use thiserror::Error;
use vorbis_rs::VorbisError;

#[derive(Error, Debug)]
pub enum RendererError {
    #[error("failed to decode .bms file ({0}) with {1} format")]
    BMSDecodingError(String, String),
    #[error("failed to parse .bms file")]
    BMSParsingError(#[from] ParseErrorWithRange),
    #[error("failed to read .bms file")]
    FileNotFound(#[from] io::Error),
}

#[derive(Error, Debug)]
pub enum AudioError {
    #[error("failed to find audio file: {0}")]
    FileNotFound(String),
    #[error("invalid bounds: {0} with channel size {1}")]
    InvalidSampleOffset(usize, usize),
    #[error("invalid bounds: [{0}..{1}] of len {2}")]
    InvalidDestinationBounds(usize, usize, usize),
    #[error("invalid bounds: [{0}..{1}] of len {2}")]
    InvalidSourceBounds(usize, usize, usize),
    #[error("unsupported number of channels for destination")]
    InvalidChannelCount(),
    #[error("failed to get channel info")]
    MissingChannelInfo(),
    #[error("failed to get sample rate")]
    MissingSampleRateInfo(),
    #[error("resampler construction error")]
    ResamplerConstructionError(#[from] ResamplerConstructionError),
    #[error("invalid audio size")]
    InvalidAudioSize(#[from] SizeError),
    #[error("resampler error")]
    ResamplerError(#[from] ResampleError),
    #[error("I/O error")]
    IOError(#[from] io::Error),
    #[error("audio decoder error")]
    DecodingError(#[from] symphonia::core::errors::Error),
    #[error("vorbis encoder error")]
    VorbisEncodingError(#[from] VorbisError),
}
