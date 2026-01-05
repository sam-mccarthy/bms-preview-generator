use crate::Args;
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
use thiserror::Error as TError;
use vorbis_rs::VorbisEncoderBuilder;

struct AudioFile {
    buffer: Vec<f32>,
    channels: usize,
    sample_rate: u32,
}

pub struct Renderer {
    bms: Bms,
    base_path: PathBuf,
}

#[derive(TError, Debug)]
pub enum AudioIOError {
    #[error("failed to get {0} from audio")]
    MissingInfo(String),
}

#[derive(TError, Debug)]
pub enum AudioGenError {
    #[error("invalid bounds: {0}: [{1}..{2}] of len {3}")]
    InvalidBounds(String, usize, usize, usize),
    #[error("unsupported number of channels for destination")]
    InvalidChannelCount(),
}

fn get_audio_fuzzy(path: &PathBuf) -> io::Result<PathBuf> {
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

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "failed to find fuzzy file",
    ))
}

fn get_wav_codec(fuzzy_path: &PathBuf) -> Result<CodecParameters, Box<dyn Error>> {
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

fn read_wav(fuzzy_path: &PathBuf) -> Result<AudioFile, Box<dyn Error>> {
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
        .ok_or(AudioIOError::MissingInfo("channel info".to_string()))?
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
        .ok_or(AudioIOError::MissingInfo("sample rate".to_string()))?;
    Ok(AudioFile {
        buffer: output_buf,
        sample_rate,
        channels,
    })
}

fn resample_audio(
    src: &[f32],
    dst_sample_rate: u32,
    src_sample_rate: u32,
    src_channels: usize,
) -> Result<Vec<f32>, Box<dyn Error>> {
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

fn scale_audio(audio: &mut [f32], scale_by: f32) {
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

fn fade_audio(
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
) -> Result<(), AudioGenError> {
    let dst_channel_size: usize = dst.len() / dst_ch_max;
    let src_channel_size: usize = src.len() / src_ch_max;

    let dst_base = dst_offset + dst_ch * dst_channel_size;
    let dst_end = (dst_ch + 1) * dst_channel_size;

    let src_base = src_offset + src_ch * src_channel_size;
    let src_end = (src_ch + 1) * src_channel_size;

    if dst_offset > dst_channel_size {
        return Err(AudioGenError::InvalidBounds(
            "invalid sample offset".to_string(),
            src_base,
            src_end,
            src.len(),
        ));
    }

    if dst_base > dst.len() || dst_end > dst.len() {
        return Err(AudioGenError::InvalidBounds(
            "invalid destination bounds".to_string(),
            src_base,
            src_end,
            src.len(),
        ));
    }

    if src_end < src_base || src_base > src.len() || src_end > src.len() {
        return Err(AudioGenError::InvalidBounds(
            "invalid source bounds".to_string(),
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

fn add_audio(
    dst: &mut [f32],
    src: &[f32],
    sample_rate: u32,
    dst_channels: usize,
    src_channels: usize,
    offset_sec: f64,
) -> Result<(), AudioGenError> {
    if dst_channels == 0 || dst_channels > 2 || src_channels == 0 {
        return Err(AudioGenError::InvalidChannelCount());
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

fn get_length_from_codec(codec: &CodecParameters) -> Option<f64> {
    let n_frames = codec.n_frames?;
    let channels = codec.channels?;
    let sample_rate = codec.sample_rate?;

    Some(n_frames as f64 / channels.count() as f64 / sample_rate as f64)
}

fn ceil_n(number: f64, ceiling: f64) -> f64 {
    let remainder = number % ceiling;
    number + ceiling - remainder
}

impl Renderer {
    // referenced from https://github.com/approvers/bms-bounce/blob/master/bms-rs-wasm/src/lib.rs
    fn get_wav_timings(&self, start: f64, end: f64) -> HashMap<PathBuf, Vec<f64>> {
        let bpm_changes = &self.bms.bpm.bpm_changes;
        let section_len_changes = &self.bms.section_len.section_len_changes;

        const DEFAULT_BPM: f64 = 130.0;
        let default_bpm_dec = Decimal::from(DEFAULT_BPM);
        let mut current_bpm: f64 = self
            .bms
            .bpm
            .bpm
            .clone()
            .unwrap_or(default_bpm_dec)
            .try_into()
            .unwrap_or(DEFAULT_BPM);
        let mut current_section_time = 0.0;
        let mut next_section_time = 0.0;
        let mut previous_section = 0;

        let four = NonZeroU64::new(4).unwrap();
        let first_bpm_change = bpm_changes.range(..ObjTime::new(2, 0, four)).next();
        if let Some((_, BpmChangeObj { bpm: first_bpm, .. })) = first_bpm_change {
            current_bpm = first_bpm.clone().try_into().unwrap_or(DEFAULT_BPM);
        }

        let mut timings: HashMap<PathBuf, Vec<f64>> = HashMap::new();

        for note in self
            .bms
            .wav
            .notes
            .bgms::<KeyLayoutBeat>()
            .sorted_by(|a, b| a.offset.cmp(&b.offset))
        {
            let track = note.offset.track();
            let numerator = note.offset.numerator();
            let denominator = note.offset.denominator_u64();

            let one = Decimal::from(1);
            let current_section_len = section_len_changes
                .get(&note.offset.track())
                .map_or(one, |obj| obj.length.clone());
            let section_beats: f64 = current_section_len.mul(4).try_into().unwrap();
            let seconds_per_beat = 60.0 / current_bpm;
            let section_seconds = section_beats * seconds_per_beat;

            if previous_section < track.0 {
                current_section_time = next_section_time;
                previous_section = track.0;
            }

            let obj_offset_seconds = section_seconds * numerator as f64 / denominator as f64;
            let obj_start_seconds = current_section_time + obj_offset_seconds;
            next_section_time = current_section_time + section_seconds;

            let first_bpm_change = bpm_changes.range(note.offset..).next();
            if let Some((_, BpmChangeObj { bpm: first_bpm, .. })) = first_bpm_change {
                current_bpm = first_bpm.clone().try_into().unwrap_or(current_bpm);
            }

            if obj_start_seconds > end {
                continue;
            }

            let Some(name) = self.bms.wav.wav_files.get(&note.wav_id) else {
                continue;
            };

            if let Ok(codec) = get_wav_codec(name) {
                let length = get_length_from_codec(&codec);

                if length.is_some_and(|len| (obj_start_seconds + len) < start) {
                    continue;
                }
            }

            let path = self.base_path.join(name);
            if let Some(timing_vec) = timings.get_mut(&path) {
                timing_vec.push(obj_start_seconds);
            } else {
                timings.insert(path.clone(), vec![obj_start_seconds]);
            }
        }

        timings
    }

    pub fn process_bms_file(&self, args: &Args) -> Result<(), Box<dyn Error>> {
        if let Some(_) = self.bms.music_info.preview_music {
            return Ok(());
        }

        const ENCODING_STEP_SIZE: usize = 1024;

        let channels = if args.mono_audio { 1 } else { 2 };
        let song_length = args.end - args.start;

        let req_samples = song_length * args.sample_rate as f64 * channels as f64;
        let n_samples = ceil_n(req_samples, ENCODING_STEP_SIZE as f64 * channels as f64) as usize;

        let mut render_buf = vec![0f32; n_samples];

        let timings = self.get_wav_timings(args.start, args.end);

        for (wav_path, timings) in timings {
            let Ok(mut wav) = read_wav(&wav_path) else {
                continue;
            };

            if args.sample_rate != wav.sample_rate {
                match resample_audio(
                    &wav.buffer[..],
                    args.sample_rate,
                    wav.sample_rate,
                    wav.channels,
                ) {
                    Ok(resampled) => wav.buffer = resampled,
                    Err(_) => continue,
                }
                wav.sample_rate = args.sample_rate;
            }

            for time in timings {
                add_audio(
                    &mut render_buf[..],
                    &wav.buffer[..],
                    args.sample_rate,
                    channels,
                    wav.channels,
                    time - args.start,
                )?;
            }
        }

        fade_audio(
            &mut render_buf[..],
            args.sample_rate,
            channels,
            args.fade_in,
            false,
        );
        fade_audio(
            &mut render_buf[..],
            args.sample_rate,
            channels,
            args.fade_out,
            true,
        );

        if args.volume != 1.0 {
            scale_audio(&mut render_buf[..], args.volume);
        }

        let mut output_buf = Vec::new();
        let mut encoder = VorbisEncoderBuilder::new(
            NonZeroU32::new(args.sample_rate).unwrap(),
            NonZeroU8::new(channels as u8).unwrap(),
            &mut output_buf,
        )?
        .build()?;

        let channel_size = n_samples / channels;
        let mut block: Vec<&[f32]> = vec![&[]; channels];

        for i in (0..channel_size).step_by(ENCODING_STEP_SIZE) {
            for j in 0..channels {
                let base = i + j * channel_size;
                let end = cmp::min(i + ENCODING_STEP_SIZE, channel_size) + j * channel_size;

                block[j] = &render_buf[base..end];
            }

            encoder.encode_audio_block(&block)?;
        }

        encoder.finish()?;
        fs::write(self.base_path.join(&args.preview_file), output_buf)?;

        Ok(())
    }

    pub fn new(bms_path: PathBuf) -> io::Result<Self> {
        let file_bytes = fs::read(&bms_path)?;
        let encoding = Encoding::for_label(&file_bytes).unwrap_or(UTF_8);

        let (source, _, failed) = encoding.decode(&file_bytes);
        if failed {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "failed to decode BMS file",
            ));
        }

        let BmsOutput { bms, .. } =
            parse_bms(&source, default_config()).expect("failed to parse BMS file");

        Ok(Self {
            bms,
            base_path: bms_path.parent().unwrap().to_path_buf(),
        })
    }
}
