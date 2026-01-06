use crate::bms_preview::Args;
use crate::bms_preview::errors::AudioError;

use audioadapter_buffers::direct::SequentialSlice;
use rubato::{Fft, FixedSync, Resampler};
use std::collections::HashMap;
use std::fs::File;
use std::path::PathBuf;
use std::cmp;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CodecParameters, DecoderOptions};
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use std::num::{NonZeroU8, NonZeroU32};
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
    let mut output_adapter = SequentialSlice::new_mut(&mut resampled_data, src_channels, resample_capacity)?;
    
    resampler.process_all_into_buffer(&input_adapter, &mut output_adapter, nbr_input_frames, None)?;
    
    Ok(resampled_data)
}

pub fn scale_audio(audio: &mut [f32], scale_by_percent: f32) {
    audio.iter_mut().for_each(|val| {
        *val *= scale_by_percent / 100.0;
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

    if dst_offset >= dst_channel_size {
        return Ok(());
    }

    if src_offset >= src_channel_size {
        return Ok(());
    }

    if dst_base > dst.len() || dst_end > dst.len() {
        return Err(AudioError::InvalidDestinationBounds(
            src_base,
            src_end,
            src.len(),
        ));
    }

    if src_end < src_base || src_base > src.len() || src_end > src.len() {
        return Err(AudioError::InvalidSourceBounds(
            src_base,
            src_end,
            src.len(),
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
    lazy_mono: bool,
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

    if dst_channels == 1 && src_channels >= 2 && !lazy_mono {
        add_audio_sch(
            dst,
            src,
            0,
            1,
            dst_channels,
            src_channels,
            dst_offset,
            src_offset,
        )?;
        scale_audio(dst, 0.5);
    } else if dst_channels == 2 && src_channels >= 2 {
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

pub fn encode_vorbis(raw_buffer: &[f32], channels: usize, sample_rate: u32, encoding_step_size: usize ) -> Result<Vec<u8>, AudioError> {
    let mut output_buf = Vec::new();
    let mut encoder = VorbisEncoderBuilder::new(
        NonZeroU32::new(sample_rate).unwrap(),
        NonZeroU8::new(channels as u8).unwrap(),
        &mut output_buf,
    )?
    .build()?;

    let channel_size = raw_buffer.len() / channels;
    let mut block: Vec<&[f32]> = vec![&[]; channels];

    for i in (0..channel_size).step_by(encoding_step_size) {
        for j in 0..channels {
            let base = i + j * channel_size;
            let end = cmp::min(i + encoding_step_size, channel_size) + j * channel_size;
            
            if end - base < encoding_step_size {
                encoder.finish()?;
                return Ok(output_buf)
            }

            block[j] = &raw_buffer[base..end];
        }

        encoder.encode_audio_block(&block)?;
    }
    
    encoder.finish()?;
    Ok(output_buf)
}

fn ceil_n(number: f64, ceiling: f64) -> f64 {
    let remainder = number % ceiling;
    number + ceiling - remainder
}

pub fn render_audios(timings: HashMap<PathBuf, Vec<f64>>, args: &Args, start: f64, end: f64) -> Result<AudioFile, AudioError> {
    let channels = if args.mono_audio { 1 } else { 2 };

    let mut sample_rate = 0;
    let mut render_buf = vec![];

    for (wav_path, timings) in timings {
        let Ok(mut wav) = read_wav(&wav_path) else {
            continue;
        };
        
        if sample_rate == 0 {
            sample_rate = if args.sample_rate == 0 { 
                wav.sample_rate 
            } else { 
                args.sample_rate 
            };
            
            let req_samples = (end - start) * sample_rate as f64 * channels as f64;
            let n_samples = ceil_n(req_samples, args.encoding_step_size as f64 * channels as f64) as usize;
    
            render_buf.resize(n_samples, 0.0);
        }

        if sample_rate != wav.sample_rate {
            match resample_audio(
                &wav.buffer[..],
                sample_rate,
                wav.sample_rate,
                wav.channels,
            ) {
                Ok(resampled) => wav.buffer = resampled,
                Err(_) => continue,
            }
            wav.sample_rate = sample_rate;
        }

        for time in timings {
            add_audio(
                &mut render_buf[..],
                &wav.buffer[..],
                sample_rate,
                channels,
                wav.channels,
                time - start,
                args.lazy_mono,
            )?;
        }
    }
    
    Ok(AudioFile { buffer: render_buf, channels, sample_rate })
}

pub fn get_length_from_codec(codec: &CodecParameters) -> Option<f64> {
    Some(codec.time_base?.calc_time(codec.n_frames?).frac)
}
