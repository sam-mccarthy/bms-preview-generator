use crate::bms_preview::audio;
use crate::bms_preview::errors::*;
use crate::bms_preview::Args;

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

pub struct Renderer {
    bms: Bms,
    base_path: PathBuf,
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

            if let Ok(codec) = audio::get_wav_codec(name) {
                let length = audio::get_length_from_codec(&codec);

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

    pub fn process_bms_file(&self, args: &Args) -> Result<(), AudioError> {
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
            let Ok(mut wav) = audio::read_wav(&wav_path) else {
                continue;
            };

            if args.sample_rate != wav.sample_rate {
                match audio::resample_audio(
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
                audio::add_audio(
                    &mut render_buf[..],
                    &wav.buffer[..],
                    args.sample_rate,
                    channels,
                    wav.channels,
                    time - args.start,
                )?;
            }
        }

        audio::fade_audio(
            &mut render_buf[..],
            args.sample_rate,
            channels,
            args.fade_in,
            false,
        );
        audio::fade_audio(
            &mut render_buf[..],
            args.sample_rate,
            channels,
            args.fade_out,
            true,
        );

        if args.volume != 1.0 {
            audio::scale_audio(&mut render_buf[..], args.volume);
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

    pub fn new(bms_path: PathBuf) -> Result<Self, RendererError> {
        let path_str = bms_path.to_string_lossy().to_string();
        
        let file_bytes = fs::read(&bms_path)?;
        let encoding = Encoding::for_label(&file_bytes).unwrap_or(UTF_8);

        let (source, _, failed) = encoding.decode(&file_bytes);
        if failed {
            return Err(RendererError::BMSDecodingError(path_str, encoding.name().to_string()));
        }

        let BmsOutput { bms, .. } =
            parse_bms(&source, default_config())?;

        Ok(Self {
            bms,
            base_path: bms_path.parent().unwrap().to_path_buf(),
        })
    }
}
