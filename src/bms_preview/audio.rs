use crate::bms_preview::errors::AudioError;

use audioadapter_buffers::direct::SequentialSlice;
use bms_rs::bms::model::Bms;
use bms_rs::bms::prelude::{BpmChangeObj, KeyLayoutBeat};
use bms_rs::bms::{BmsOutput, Decimal, default_config, parse_bms};
use bms_rs::command::time::ObjTime;
use encoding_rs::{Encoding, UTF_8};
use itertools::Itertools;
use rubato::{Fft, FixedSync, Indexing, Resampler};
use std::collections::HashMap;
use std::error::Error;
use std::fs::File;
use std::num::{NonZeroU8, NonZeroU32, NonZeroU64};
use std::ops::Mul;
use std::path::PathBuf;
use std::{cmp, fs, io};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CodecParameters, DecoderOptions};
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use thiserror::Error;
use vorbis_rs::VorbisEncoderBuilder;

pub struct AudioFile {
    pub buffer: Vec<f32>,
    pub channels: usize,
    pub sample_rate: u32,
}

pub fn get_audio_fuzzy(path: &PathBuf) -> Result<PathBuf, AudioError> {
    if path.exists() {
        return Ok(path.clone());
    }

    let valid_exts = ["wav", "ogg", "mp3"];
    for ext in valid_exts {
        let alt_path = path.with_extension(ext);
        if alt_path.exists() {
            return Ok(alt_path);
        }
    }

    Err(AudioError::FileNotFound(path.to_str().unwrap().to_string()))
}

pub fn get_wav_codec(fuzzy_path: &PathBuf) -> Result<CodecParameters, AudioError> {
    let path = get_audio_fuzzy(fuzzy_path)?;

    let file = Box::new(File::open(&path)?);
    let mss = MediaSourceStream::new(file, Default::default());
    let hint = Hint::new();

    let format_opts: FormatOptions = Default::default();
    let metadata_opts: MetadataOptions = Default::default();

    let probed =
        symphonia::default::get_probe().format(&hint, mss, &format_opts, &metadata_opts)?;

    let format = probed.format;
    let track = format.default_track().unwrap();

    Ok(track.codec_params.clone())
}

pub fn read_wav(fuzzy_path: &PathBuf) -> Result<AudioFile, AudioError> {
    let path = get_audio_fuzzy(fuzzy_path)?;

    let file = Box::new(File::open(&path)?);
    let mss = MediaSourceStream::new(file, Default::default());
    let hint = Hint::new();

    let format_opts: FormatOptions = Default::default();
    let metadata_opts: MetadataOptions = Default::default();
    let decoder_opts: DecoderOptions = Default::default();

    let probed =
        symphonia::default::get_probe().format(&hint, mss, &format_opts, &metadata_opts)?;

    let mut format = probed.format;
    let track = format.default_track().unwrap();
    let codec = track.codec_params.clone();
    // TODO: This would probably be better served with a separate error
    let channels = codec
        .channels
        .ok_or(AudioError::MissingChannelInfo())?
        .count();

    let mut decoder = symphonia::default::get_codecs().make(&track.codec_params, &decoder_opts)?;

    let track_id = track.id;
    let mut sample_buf = None;
    let mut output_buffers: Vec<Vec<f32>> = vec![vec![]; channels];

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(_) => break,
        };

        if packet.track_id() != track_id {
            continue;
        }

        match decoder.decode(&packet) {
            Ok(audio_buf) => {
                if sample_buf.is_none() {
                    let spec = *audio_buf.spec();
                    let duration = audio_buf.capacity() as u64;

                    sample_buf = Some(SampleBuffer::<f32>::new(duration, spec));
                }

                if let Some(buf) = &mut sample_buf {
                    buf.copy_planar_ref(audio_buf);
                    let samples = buf.samples();
                    let channel_size = samples.len() / channels;

                    for i in 0..channels {
                        let base = i * channel_size;
                        let end = cmp::min((i + 1) * channel_size, samples.len());
                        output_buffers[i].extend_from_slice(&samples[base..end]);
                    }
                }
            }
            Err(symphonia::core::errors::Error::DecodeError(_)) => (),
            Err(_) => break,
        }
    }

    let mut output_buf: Vec<f32> = Vec::new();
    for i in 0..channels {
        output_buf.extend_from_slice(&output_buffers[i][..]);
    }

    let sample_rate = codec
        .sample_rate
        .ok_or(AudioError::MissingSampleRateInfo())?;
    Ok(AudioFile {
        buffer: output_buf,
        sample_rate,
        channels,
    })
}

pub fn resample_audio(
    src: &[f32],
    dst_sample_rate: u32,
    src_sample_rate: u32,
    src_channels: usize,
) -> Result<Vec<f32>, AudioError> {
    let mut resampler = Fft::<f32>::new(
        src_sample_rate as usize,
        dst_sample_rate as usize,
        1024,
        1,
        src_channels,
        FixedSync::Input,
    )?;

    // wrap it with an InterleavedSlice Adapter
    let nbr_input_frames = src.len() / src_channels;
    let input_adapter = SequentialSlice::new(&src, src_channels, nbr_input_frames)?;

    // create a buffer for the output
    let mut resampled_data = vec![0.0; src.len() * 2];
    let resample_capacity = resampled_data.len() / src_channels;
    let mut output_adapter =
        SequentialSlice::new_mut(&mut resampled_data, src_channels, resample_capacity)?;

    let mut indexing = Indexing {
        input_offset: 0,
        output_offset: 0,
        active_channels_mask: None,
        partial_len: None,
    };

    let mut input_frames_left = nbr_input_frames;
    let mut input_frames_next = resampler.input_frames_next();

    while input_frames_left >= input_frames_next {
        let (frames_read, frames_written) =
            resampler.process_into_buffer(&input_adapter, &mut output_adapter, Some(&indexing))?;

        indexing.input_offset += frames_read;
        indexing.output_offset += frames_written;
        input_frames_left -= frames_read;
        input_frames_next = resampler.input_frames_next();
    }

    Ok(resampled_data)
}

pub fn scale_audio(audio: &mut [f32], scale_by: f32) {
    audio.iter_mut().for_each(|val| {
        *val *= scale_by;
    });
}

fn fade_audio_sch(
    audio: &mut [f32],
    sample_rate: u32,
    channel: usize,
    n_channels: usize,
    fade_length: f64,
    fade_end: bool,
) {
    let channel_size = audio.len() / n_channels;
    let fade_n_samples = cmp::min((fade_length * sample_rate as f64) as usize, channel_size);

    let base = if fade_end {
        channel_size - fade_n_samples - 1
    } else {
        0
    } + channel * channel_size;
    let end = if fade_end {
        channel_size
    } else {
        fade_n_samples
    } + channel * channel_size;

    if fade_end {
        audio[base..end]
            .iter_mut()
            .rev()
            .enumerate()
            .for_each(|(i, val)| {
                *val *= i as f32 / fade_n_samples as f32;
            });
    } else {
        audio[base..end]
            .iter_mut()
            .enumerate()
            .for_each(|(i, val)| {
                *val *= i as f32 / fade_n_samples as f32;
            });
    }
}

pub fn fade_audio(
    audio: &mut [f32],
    sample_rate: u32,
    n_channels: usize,
    fade_length: f64,
    fade_end: bool,
) {
    for i in 0..n_channels {
        fade_audio_sch(audio, sample_rate, i, n_channels, fade_length, fade_end);
    }
}

fn add_audio_sch(
    dst: &mut [f32],
    src: &[f32],
    dst_ch: usize,
    src_ch: usize,
    dst_ch_max: usize,
    src_ch_max: usize,
    dst_offset: usize,
    src_offset: usize,
) -> Result<(), AudioError> {
    let dst_channel_size: usize = dst.len() / dst_ch_max;
    let src_channel_size: usize = src.len() / src_ch_max;

    let dst_base = dst_offset + dst_ch * dst_channel_size;
    let dst_end = (dst_ch + 1) * dst_channel_size;

    let src_base = src_offset + src_ch * src_channel_size;
    let src_end = (src_ch + 1) * src_channel_size;

    if dst_offset > dst_channel_size {
        return Err(AudioError::InvalidSampleOffset(
            dst_offset,
            dst_channel_size
        ));
    }
    
    if src_offset > src_channel_size {
        return Err(AudioError::InvalidSampleOffset(
            src_offset,
            src_channel_size
        ));
    }

    if dst_base > dst.len() || dst_end > dst.len() {
        return Err(AudioError::InvalidDestinationBounds(
            src_base,
            src_end,
            src.len()
        ));
    }

    if src_end < src_base || src_base > src.len() || src_end > src.len() {
        return Err(AudioError::InvalidSourceBounds(
            src_base,
            src_end,
            src.len()
        ));
    }

    dst[dst_base..dst_end]
        .iter_mut()
        .zip(&src[src_base..src_end])
        .for_each(|(dst_sample, src_sample)| {
            *dst_sample += src_sample;
        });

    Ok(())
}

pub fn add_audio(
    dst: &mut [f32],
    src: &[f32],
    sample_rate: u32,
    dst_channels: usize,
    src_channels: usize,
    offset_sec: f64,
) -> Result<(), AudioError> {
    if dst_channels == 0 || dst_channels > 2 || src_channels == 0 {
        return Err(AudioError::InvalidChannelCount());
    }

    let mut dst_offset = (offset_sec * sample_rate as f64) as usize;
    let mut src_offset = 0;

    if offset_sec < 0.0 {
        dst_offset = 0;
        src_offset = ((-offset_sec) * sample_rate as f64) as usize;

        if src_offset >= src.len() / src_channels {
            return Ok(());
        }
    }

    add_audio_sch(
        dst,
        src,
        0,
        0,
        dst_channels,
        src_channels,
        dst_offset,
        src_offset,
    )?;

    if dst_channels == 2 && src_channels >= 2 {
        add_audio_sch(
            dst,
            src,
            1,
            1,
            dst_channels,
            src_channels,
            dst_offset,
            src_offset,
        )?;
    } else if dst_channels == 2 && src_channels == 1 {
        add_audio_sch(
            dst,
            src,
            1,
            0,
            dst_channels,
            src_channels,
            dst_offset,
            src_offset,
        )?;
    }

    Ok(())
}

pub fn get_length_from_codec(codec: &CodecParameters) -> Option<f64> {
    let n_frames = codec.n_frames?;
    let channels = codec.channels?;
    let sample_rate = codec.sample_rate?;

    Some(n_frames as f64 / channels.count() as f64 / sample_rate as f64)
}