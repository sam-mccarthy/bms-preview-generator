use std::{
    fs::File,
    num::{NonZeroU8, NonZeroU32},
    ops::{Add, AddAssign, Mul, MulAssign},
    path::{Path, PathBuf},
};

use audioadapter_buffers::direct::SequentialSliceOfVecs;
use itertools::Itertools;
use rubato::{Fft, FixedSync, Resampler};
use symphonia::core::{
    audio::SampleBuffer,
    codecs::DecoderOptions,
    formats::{FormatOptions, FormatReader, Track},
    io::MediaSourceStream,
    meta::MetadataOptions,
    probe::Hint,
};
use vorbis_rs::VorbisEncoderBuilder;

use crate::bms_preview::errors::AudioError;

const STEREO_CHANNELS: usize = 2;
const RESAMPLING_CHUNK_SIZE: usize = 1024;
const RESAMPLING_SUB_CHUNKS: usize = 1;
const ENCODING_CHUNK_SIZE: usize = 1024;

fn get_audio_fuzzy(path: impl AsRef<Path>) -> Option<PathBuf> {
    let path_ref = path.as_ref();

    if path_ref.exists() {
        return Some(path_ref.to_path_buf());
    }

    const VALID_AUDIO: [&str; 3] = ["wav", "ogg", "mp3"];

    VALID_AUDIO.iter().find_map(|extension| {
        let alternate_path = path_ref.with_extension(extension);
        alternate_path.exists().then_some(alternate_path)
    })
}

#[derive(Copy, Clone, Default)]
pub struct StereoSample {
    pub left: f32,
    pub right: f32,
}

#[derive(Clone)]
pub struct StereoAudio {
    pub buffer: Vec<StereoSample>,
    pub sample_rate: u32,
}

pub struct Probe {
    pub track: Track,
    pub format: Box<dyn FormatReader>,
}

impl Probe {
    pub fn new(path: impl AsRef<Path>) -> Result<Probe, AudioError> {
        let path = get_audio_fuzzy(path).ok_or(AudioError::FileNotFound())?;

        let file = Box::new(File::open(&path)?);
        let mss = MediaSourceStream::new(file, Default::default());

        let mut hint = &mut Hint::new();
        if let Some(ext) = path.extension().and_then(|ext| ext.to_str()) {
            hint = hint.with_extension(ext);
        }

        let format_opts: FormatOptions = Default::default();
        let metadata_opts: MetadataOptions = Default::default();

        let probed =
            symphonia::default::get_probe().format(&hint, mss, &format_opts, &metadata_opts)?;

        let format = probed.format;
        let track = format.default_track().unwrap();

        Ok(Probe {
            track: track.clone(),
            format,
        })
    }

    pub fn get_length(&self) -> Option<f64> {
        let codec = &self.track.codec_params;
        let Some(frames) = codec.n_frames else {
            return None;
        };

        Some(codec.time_base?.calc_time(frames).frac)
    }
}

impl Add for StereoSample {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self {
            left: self.left + rhs.left,
            right: self.right + rhs.right,
        }
    }
}

impl AddAssign for StereoSample {
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

impl Mul<f32> for StereoSample {
    type Output = Self;
    fn mul(self, rhs: f32) -> Self {
        Self {
            left: self.left * rhs,
            right: self.right * rhs,
        }
    }
}

impl MulAssign<f32> for StereoSample {
    fn mul_assign(&mut self, rhs: f32) {
        *self = *self * rhs;
    }
}

impl StereoAudio {
    pub fn new(length: f64, sample_rate: u32) -> Self {
        let samples = length * STEREO_CHANNELS as f64 * sample_rate as f64;

        Self {
            buffer: vec![Default::default(); samples as usize + 1],
            sample_rate: sample_rate,
        }
    }

    pub fn load(mut probe: Probe) -> Result<Self, AudioError> {
        let decoder_opts: DecoderOptions = Default::default();

        let channels = probe
            .track
            .codec_params
            .channels
            .ok_or(AudioError::MissingCodecInfo())?
            .count();
        let sample_rate = probe
            .track
            .codec_params
            .sample_rate
            .ok_or(AudioError::MissingCodecInfo())?;

        let mut decoder =
            symphonia::default::get_codecs().make(&probe.track.codec_params, &decoder_opts)?;

        let track_id = probe.track.id;

        let mut output: Vec<StereoSample> = Vec::new();
        let mut buffer: Option<SampleBuffer<f32>> = None;

        loop {
            let packet = match probe.format.next_packet() {
                Ok(p) => p,
                Err(_) => break,
            };

            if packet.track_id() != track_id {
                continue;
            }

            match decoder.decode(&packet) {
                Ok(audio_buf) => {
                    if buffer.is_none() {
                        let spec = *audio_buf.spec();
                        let duration = audio_buf.capacity() as u64;

                        buffer = Some(SampleBuffer::<f32>::new(duration, spec));
                    }

                    if let Some(buf) = &mut buffer {
                        buf.copy_interleaved_ref(audio_buf);
                        let samples = buf.samples();
                        let count = samples.len();

                        output.reserve(count);
                        for i in (0..count).step_by(channels) {
                            if channels == 1 {
                                output.push(StereoSample {
                                    left: samples[i],
                                    right: samples[i],
                                });
                            } else {
                                output.push(StereoSample {
                                    left: samples[i],
                                    right: samples[i + 1],
                                });
                            }
                        }
                    }
                }
                Err(symphonia::core::errors::Error::DecodeError(_)) => (),
                Err(_) => break,
            }
        }

        Ok(Self {
            buffer: output,
            sample_rate,
        })
    }

    pub fn resample(&mut self, desired_rate: usize) -> Result<(), AudioError> {
        let mut resampler = Fft::<f32>::new(
            self.sample_rate as usize,
            desired_rate as usize,
            RESAMPLING_CHUNK_SIZE,
            RESAMPLING_SUB_CHUNKS,
            STEREO_CHANNELS,
            FixedSync::Input,
        )?;

        let left_in = self.buffer.iter().map(|sample| sample.left).collect();
        let right_in = self.buffer.iter().map(|sample| sample.right).collect();
        let input = &[left_in, right_in];

        let n_input_frames = self.frames_per_channel();
        let input_adapter = SequentialSliceOfVecs::new(input, STEREO_CHANNELS, n_input_frames)?;

        let resample_capacity = resampler.process_all_needed_output_len(n_input_frames);

        let output = &mut [vec![0.0; resample_capacity], vec![0.0; resample_capacity]];
        let mut output_adapter =
            SequentialSliceOfVecs::new_mut(output, STEREO_CHANNELS, resample_capacity)?;

        resampler.process_all_into_buffer(
            &input_adapter,
            &mut output_adapter,
            n_input_frames,
            None,
        )?;

        let left_out = &output[0];
        let right_out = &output[1];
        self.buffer = left_out
            .iter()
            .zip(right_out.iter())
            .map(|(left, right)| StereoSample {
                left: *left,
                right: *right,
            })
            .collect();
        self.sample_rate = desired_rate as u32;

        Ok(())
    }

    pub fn fade(&mut self, fade_in_time: f64, fade_out_time: f64) {
        let in_samples = self.time_to_samples(fade_in_time);
        let out_samples = self.time_to_samples(fade_out_time);

        self.buffer
            .iter_mut()
            .zip(0..in_samples)
            .for_each(|(sample, i)| {
                let ratio = i as f32 / in_samples as f32;
                *sample *= ratio;
            });

        self.buffer
            .iter_mut()
            .rev()
            .zip(0..out_samples)
            .for_each(|(sample, i)| {
                let ratio = i as f32 / in_samples as f32;
                *sample *= ratio;
            });
    }

    pub fn add(&mut self, rhs: &StereoAudio, offset: f64) -> Result<(), AudioError> {
        if self.sample_rate != rhs.sample_rate {
            return Err(AudioError::MismatchedSampleRate());
        }

        let raw_offset = self.time_to_samples(offset);
        let mut dst_offset = raw_offset.abs() as usize;
        let mut src_offset = dst_offset;

        if raw_offset >= 0 && dst_offset < self.buffer.len() {
            src_offset = 0;
        } else if raw_offset < 0 && src_offset < rhs.buffer.len() {
            dst_offset = 0;
        } else {
            return Ok(());
        }

        self.buffer[dst_offset..]
            .iter_mut()
            .zip(&rhs.buffer[src_offset..])
            .for_each(|(left, right)| {
                *left += *right;
            });

        Ok(())
    }
    
    pub fn attenuate(&mut self, volume: f32) {
        if volume == 1.0 {
            return;
        }

        self.buffer.iter_mut().for_each(|sample| {
            *sample *= volume;
        });
    }

    pub fn encode(&mut self, path: impl AsRef<Path>, mono: bool) -> Result<(), AudioError> {
        let channels = if mono { 1 } else { 2 };
        let file = File::create(path)?;
        let mut encoder = VorbisEncoderBuilder::new(
            NonZeroU32::new(self.sample_rate).ok_or(AudioError::InvalidCodecInfo())?,
            NonZeroU8::new(channels as u8).ok_or(AudioError::InvalidCodecInfo())?,
            file,
        )?
        .build()?;

        let missing_samples = self.buffer.len() % ENCODING_CHUNK_SIZE;

        let mut left = self
            .buffer
            .iter()
            .map(|sample| sample.left)
            .chain((0..missing_samples).map(|_| Default::default()));
        let mut right = self
            .buffer
            .iter()
            .map(|sample| sample.right)
            .chain((0..missing_samples).map(|_| Default::default()));
        
        for _ in (0..self.buffer.len()).step_by(ENCODING_CHUNK_SIZE) {
            let Some(left_chunk): Option<[f32; ENCODING_CHUNK_SIZE]> = left.next_array() else {
                continue;
            };
            let Some(right_chunk): Option<[f32; ENCODING_CHUNK_SIZE]> = right.next_array() else {
                continue;
            };

            if !mono {
                let block = &[left_chunk, right_chunk];

                encoder.encode_audio_block(block)?;
            } else {
                let average: [f32; ENCODING_CHUNK_SIZE] = left_chunk
                    .iter()
                    .zip(right_chunk)
                    .map(|(lhs, rhs)| (lhs + rhs) / 2.0)
                    .collect_array()
                    .unwrap();
                let block = &[average];

                encoder.encode_audio_block(block)?;
            }
        }

        Ok(())
    }

    pub fn match_sample_rate(&mut self, rhs: &StereoAudio) -> Result<(), AudioError> {
        if self.sample_rate != rhs.sample_rate {
            self.resample(rhs.sample_rate as usize)?;
        }

        Ok(())
    }

    #[allow(dead_code)]
    pub fn get_length(&self) -> f64 {
        self.samples_to_time(self.buffer.len() as isize)
    }

    #[allow(dead_code)]
    fn samples_to_time(&self, samples: isize) -> f64 {
        return samples as f64 / self.sample_rate as f64;
    }

    fn time_to_samples(&self, time: f64) -> isize {
        return (time * self.sample_rate as f64) as isize;
    }

    fn frames_per_channel(&self) -> usize {
        return self.buffer.len();
    }
}
