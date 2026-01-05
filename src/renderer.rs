use std::error::Error;
use thiserror::Error as TError;
use std::{cmp, fs, io};
use std::collections::HashMap;
use std::fs::File;
use std::num::{NonZeroU32, NonZeroU64, NonZeroU8};
use std::ops::Mul;
use std::path::PathBuf;
use audioadapter_buffers::direct::SequentialSlice;
use bms_rs::bms::{default_config, parse_bms, BmsOutput, Decimal};
use bms_rs::bms::model::Bms;
use bms_rs::bms::prelude::{BpmChangeObj, KeyLayoutBeat};
use bms_rs::command::time::ObjTime;
use encoding_rs::{Encoding, UTF_8};
use itertools::Itertools;
use rubato::{Fft, FixedSync, Indexing, Resampler, SincInterpolationParameters, SincInterpolationType, WindowFunction};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CodecParameters, DecoderOptions};
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use vorbis_rs::VorbisEncoderBuilder;
use crate::Args;

struct AudioFile {
    codec: CodecParameters,
    buffer: Vec<f32>,
    length: f64,
    channels: usize,
    sample_rate: u32
}

pub struct Renderer {
    bms: Bms,
    base_path: PathBuf,
}

#[derive(TError, Debug)]
pub enum AudioIOError {
    #[error("failed to get channel info from codec")]
    MissingChannelInfo()
}

#[derive(TError, Debug)]
pub enum AudioGenError {
    #[error("invalid bounds")]
    InvalidBounds(String),
    #[error("unsupported number of channels for destination")]
    InvalidChannelCount(),
}

fn get_audio_fuzzy(path: PathBuf) -> io::Result<PathBuf> {
    if path.exists() {
        return Ok(path);
    }

    let valid_exts = ["wav", "ogg", "mp3"];
    for ext in valid_exts {
        let alt_path = path.with_extension(ext);
        if alt_path.exists() {
            return Ok(alt_path);
        }
    }

    Err(io::Error::new(io::ErrorKind::NotFound, "failed to find fuzzy file"))
}

fn get_wav_codec(fuzzy_path: PathBuf) -> Result<CodecParameters, Box<dyn Error>> {
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
    let codec = track.codec_params.clone();

    Ok(codec)
}

fn read_wav(fuzzy_path: PathBuf) -> Result<AudioFile, Box<dyn Error>> {
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
    let channels = codec.channels.ok_or(AudioIOError::MissingChannelInfo())?.count();

    let mut decoder =
        symphonia::default::get_codecs().make(&track.codec_params, &decoder_opts)?;

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
            },
            Err(symphonia::core::errors::Error::DecodeError(_)) => (),
            Err(_) => break,
        }
    }
    
    let mut output_buf: Vec<f32> = Vec::new();
    for i in 0..channels {
        output_buf.extend_from_slice(&output_buffers[i][..]);
    }
    
    let sample_rate = codec.sample_rate.unwrap_or(48000);
    let length = (output_buf.len() as f64) / (channels as f64) / (sample_rate as f64);
    Ok(AudioFile { codec, buffer: output_buf, length, sample_rate, channels})
}

fn resample_audio(src: &[f32], dst_sample_rate: u32, src_sample_rate: u32, src_channels: usize) -> Result<Vec<f32>, Box<dyn Error>> {
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
        let (frames_read, frames_written) = resampler
            .process_into_buffer(&input_adapter, &mut output_adapter, Some(&indexing))?;

        indexing.input_offset += frames_read;
        indexing.output_offset += frames_written;
        input_frames_left -= frames_read;
        input_frames_next = resampler.input_frames_next();
    }

    Ok(resampled_data)
}

fn scale_audio() {
    
}

fn fade_audio_in() {
    
}

fn fade_audio_out() {
    
}

fn add_audio_raw(dst: &mut [f32], src: &[f32], dst_ch: usize, src_ch: usize, dst_ch_max: usize, src_ch_max: usize, sample_offset: usize) -> Result<(), AudioGenError> {
    let dst_channel_size: usize = dst.len() / dst_ch_max;
    let src_channel_size: usize = src.len() / src_ch_max;
    
    let dst_base = sample_offset + dst_ch * dst_channel_size;
    let dst_end = (dst_ch + 1) * dst_channel_size;
    
    let src_base = src_ch * src_channel_size;
    let src_end = (src_ch + 1) * src_channel_size;
    
    if dst_base > dst.len() || dst_end > dst.len() {
        return Err(AudioGenError::InvalidBounds("invalid destination bounds".to_string()));
    }
    
    if src_base > dst.len() || src_end > src.len() {
        return Err(AudioGenError::InvalidBounds("invalid source bounds".to_string()));
    }
    
    for (dst_sample, src_sample) in dst[dst_base..dst_end].iter_mut().zip(&src[src_base..src_end]) {
        *dst_sample += src_sample;
    }
    
    Ok(())
}

fn add_audio(dst: &mut [f32], src: &[f32], sample_rate: u32,
             dst_channels: usize, src_channels: usize, offset_sec: f64) -> Result<(), AudioGenError> {
    if dst_channels == 0 || dst_channels > 2 || src_channels == 0 {
        return Err(AudioGenError::InvalidChannelCount());
    }
    
    let sample_offset = (offset_sec * sample_rate as f64) as usize;
    add_audio_raw(dst, src, 0, 0, dst_channels, src_channels, sample_offset)?;
    
    if dst_channels == 2 && src_channels >= 2 {
        add_audio_raw(dst, src, 1, 1, dst_channels, src_channels, sample_offset)?;
    } else if dst_channels == 2 && src_channels == 1 {
        add_audio_raw(dst, src, 1, 0, dst_channels, src_channels, sample_offset)?;
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
    fn get_wav_timings(&self) -> HashMap<PathBuf, Vec<f64>> {
        let bpm_changes = &self.bms.bpm.bpm_changes;
        let section_len_changes = &self.bms.section_len.section_len_changes;

        const DEFAULT_BPM: f64 = 130.0;
        let default_bpm_dec = Decimal::from(DEFAULT_BPM);
        let mut current_bpm: f64 = self.bms.bpm.bpm.clone().unwrap_or(default_bpm_dec).try_into().unwrap_or(DEFAULT_BPM);
        let mut current_section_time = 0.0;
        let mut next_section_time = 0.0;
        let mut previous_section = 0;

        let four = NonZeroU64::new(4).unwrap();
        let first_bpm_change = bpm_changes.range(..ObjTime::new(2, 0, four)).next();
        if let Some((_, BpmChangeObj { bpm: first_bpm, .. })) = first_bpm_change {
            current_bpm = first_bpm.clone().try_into().unwrap_or(DEFAULT_BPM);
        }

        let mut timings: HashMap<PathBuf, Vec<f64>> = HashMap::new();

        for note in self.bms.wav.notes
            .bgms::<KeyLayoutBeat>()
            .sorted_by(|a, b| a.offset.cmp(&b.offset)) {
            let track = note.offset.track();
            let numerator = note.offset.numerator();
            let denominator = note.offset.denominator_u64();

            let one = Decimal::from(1);
            let current_section_len = section_len_changes.get(&note.offset.track()).map_or(one, |obj| obj.length.clone());
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

            let Some(name) = self.bms.wav.wav_files.get(&note.wav_id) else { continue };
            let path = self.base_path.join(name);
            if let Some(timing_vec) = timings.get_mut(&path) {
                timing_vec.push(obj_start_seconds);
            } else {
                timings.insert(path.clone(), vec![obj_start_seconds]);
            }
        }

        timings
    }


    pub fn process_bms_file(&self, _args: &Args) -> Result<(), Box<dyn Error>> {
        if let Some(_) = self.bms.music_info.preview_music {
            return Ok(())
        }

        let timings = self.get_wav_timings();

        // TODO: this can probably be rustified
        let mut first_sample_rate = None;
        let mut song_length = 0f64;
        for (audio_path, offsets) in &timings {
            let Ok(codec) = get_wav_codec(audio_path.clone()) else { continue };
            let Some(length) = get_length_from_codec(&codec) else { continue };

            for time in offsets {
                song_length = song_length.max(time + length);
            }
            
            if first_sample_rate == None {
                first_sample_rate = codec.sample_rate;
            }
        }
        
        const ENCODING_STEP_SIZE: usize = 512;
        
        let sample_rate = first_sample_rate.unwrap_or(48000);
        let channels = 2;
        let req_samples = song_length * sample_rate as f64 * channels as f64;
        let n_samples = ceil_n(req_samples, ENCODING_STEP_SIZE as f64 * channels as f64) as usize;
        let mut render_buf = vec![0f32; n_samples];

        for (wav_path, timings) in &timings {
            println!("{}", wav_path.to_str().unwrap());
            let Ok(wav) = read_wav(wav_path.clone()) else { continue };
            let Ok(resampled) = resample_audio(&wav.buffer[..], sample_rate, wav.sample_rate, wav.channels) else { continue };
            
            for time in timings {
                add_audio(&mut render_buf[..], &resampled[..], sample_rate, channels, wav.channels, *time)?;
            }
        }

        // TODO: implement audio fading
        fade_audio_in();
        fade_audio_out();

        let mut output_buf = Vec::new();
        let mut encoder = VorbisEncoderBuilder::new(
            NonZeroU32::new(sample_rate).unwrap(),
            NonZeroU8::new(channels as u8).unwrap(),
            &mut output_buf
        )?.build()?;
        
        let channel_size = n_samples / channels;
        let mut block: Vec<&[f32]> = vec![&[]; channels];
        
        for i in (0..channel_size).step_by(ENCODING_STEP_SIZE){
            for j in 0..channels {
                let base = i + j * channel_size;
                let end = cmp::min(i + ENCODING_STEP_SIZE, channel_size) + j * channel_size;
                
                block[j] = &render_buf[base..end];
            }
            
            encoder.encode_audio_block(&block)?;
        }
        
        encoder.finish()?;
        fs::write("preview.ogg", output_buf)?;

        Ok(())
    }

    pub fn new(bms_path: PathBuf) -> io::Result<Self> {
        let file_bytes = fs::read(&bms_path)?;
        let encoding = Encoding::for_label(&file_bytes).unwrap_or(UTF_8);

        let (source, _, failed) = encoding.decode(&file_bytes);
        if failed {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "failed to decode BMS file"));
        }

        let BmsOutput { bms, .. } = parse_bms(&source, default_config())
            .expect("failed to parse BMS file");

        Ok(Self {
            bms,
            base_path: bms_path.parent().unwrap().to_path_buf()
        })
    }
}