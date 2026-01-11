use crate::bms_preview::Args;
use crate::bms_preview::errors::*;
use crate::bms_preview::stereo_audio::Probe;
use crate::bms_preview::stereo_audio::StereoAudio;

use bms_rs::bms::model::Bms;
use bms_rs::bms::prelude::ObjTime;
use bms_rs::bms::prelude::{BpmChangeObj, KeyLayoutBeat};
use bms_rs::bms::{Decimal, default_config, parse_bms};
use bms_rs::bmson::parse_bmson;
use encoding_rs::{Encoding, SHIFT_JIS};
use itertools::Itertools;
use std::collections::HashMap;
use std::fs;
use std::ops::Mul;
use std::path::Path;
use std::path::PathBuf;

pub struct Renderer {
    bms: Bms,
    base_path: PathBuf,
}

impl Renderer {
    // referenced from https://github.com/approvers/bms-bounce/blob/master/bms-rs-wasm/src/lib.rs
    /// Get the timings of sounds in a BMS file along with their paths.
    fn get_wav_timings(&self) -> HashMap<PathBuf, Vec<f64>> {
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

        let first_bpm_change = bpm_changes.range(..ObjTime::new(2, 0, 4).unwrap()).next();
        if let Some((_, BpmChangeObj { bpm: first_bpm, .. })) = first_bpm_change {
            current_bpm = first_bpm.clone().try_into().unwrap_or(DEFAULT_BPM);
        }

        let mut timings: HashMap<PathBuf, Vec<f64>> = HashMap::new();

        self.bms
            .wav
            .notes
            .bgms::<KeyLayoutBeat>()
            .sorted_by(|a, b| a.offset.cmp(&b.offset))
            .for_each(|note| {
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

                let Some(name) = self.bms.wav.wav_files.get(&note.wav_id) else {
                    return;
                };

                let path = self.base_path.join(name);

                if let Some(timing_vec) = timings.get_mut(&path) {
                    timing_vec.push(obj_start_seconds);
                } else {
                    timings.insert(path.clone(), vec![obj_start_seconds]);
                }
            });

        timings
    }

    pub fn process_bms_file(&self, args: &Args) -> Result<(), AudioError> {
        let preview_path = self.base_path.join(&args.preview_file);
        // If the BMS file has a preview set, then that'll be played by default, regardless of if we generate a preview.
        if let Some(_) = self.bms.music_info.preview_music {
            return Ok(());
        }
        // If we don't allow overwrites, and a preview already exists by the same name, we'll skip.
        if !args.overwrite && preview_path.exists() {
            return Ok(());
        }

        let mut sample_rate = args.sample_rate;
        let mut song_length: f64 = 0.0;

        // Convert the HashMap of paths and timings into a vector of probes and timings.
        // Getting the probes before actually loading audio allows us to filter notes based on
        // play time and sound length before putting effort into decoding.
        let probes: Vec<(Probe, Vec<f64>)> = self
            .get_wav_timings()
            .into_iter()
            .filter_map(|(path, time_vec)| {
                let Ok(probe) = Probe::new(&path) else {
                    return None;
                };

                let Some(length) = probe.get_length() else {
                    return None;
                };

                // If the sample rate is none, then we'll set it as the sample rate of the first
                // sound that we come across here.
                if sample_rate.is_none()
                    && let Some(probe_rate) = probe.track.codec_params.sample_rate
                {
                    sample_rate = Some(probe_rate);
                }

                // The length of the song will be the maximum end time of any sound.
                time_vec.iter().for_each(|time| {
                    song_length = song_length.max(*time + length);
                });

                Some((probe, time_vec.clone()))
            })
            .collect();

        // Get the desired start and end of the preview.
        // If start / end percentages are passed, then we'll set the start and end according to song length.
        let mut start = args.start;
        let mut end = args.end;
        if let (Some(start_p), Some(end_p)) = (args.start_p, args.end_p) {
            start = (start_p / 100.0) * song_length;
            end = (end_p / 100.0) * song_length;
        }

        // If the start and end aren't ordered, we'll swap them around.
        if start > end {
            let tmp = start;
            start = end;
            end = tmp;
        }

        // Create a new stereo buffer for our preview.
        let mut render = StereoAudio::new(end - start, sample_rate.unwrap_or(48000));
        // Iterate over all of the probes and play their timings.
        probes.into_iter().for_each(|probe_time| {
            let (probe, timings) = probe_time;
            let Some(length) = probe.get_length() else {
                return;
            };

            // Filter out times that don't fit within the preview.
            let mut filtered_times = timings
                .iter()
                .filter(|time| **time < end && (**time + length) > start)
                .peekable();

            // If no filtered times exist, then this sound isn't played during the preview,
            // so we'll just return.
            if filtered_times.peek().is_none() {
                return;
            }

            let Ok(mut audio) = StereoAudio::load(probe) else {
                return;
            };

            if let Err(_) = audio.match_sample_rate(&render) {
                return;
            }

            filtered_times.for_each(|time| {
                let _ = render.add(&audio, *time - start);
            });
        });

        // Fade the start and end, set the volume, and output the final preview audio.
        render.fade(args.fade_in, args.fade_out);
        render.attenuate(args.volume / 100.0);
        render.encode(preview_path, args.mono_audio)?;

        Ok(())
    }

    /// Create a new renderer, parsing the BMS file.
    pub fn new(bms_path: impl AsRef<Path>) -> Result<Self, RendererError> {
        // Convert the AsRef into an actual path, and get its string for potential error
        let path_ref = bms_path.as_ref();
        let path_str = path_ref.to_string_lossy().to_string();
        let extension = path_ref.extension().ok_or(RendererError::BMSPathError())?;

        // Read the BMS file and find its encoding.
        // Default as SHIFT_JIS seems to work best. UTF-8 default breaks significantly.
        let file_bytes = fs::read(path_ref)?;
        let encoding = Encoding::for_label(&file_bytes[..]).unwrap_or(SHIFT_JIS);

        // Decode the file with the proper encoding.
        let (source, _, failed) = encoding.decode(&file_bytes);
        if failed {
            return Err(RendererError::BMSDecodingError(
                path_str,
                encoding.name().to_string(),
            ));
        }

        // Parse the BMS file.
        // We handle BMSON files separately, and then convert to BMS.
        let bms;
        if extension == "bmson" {
            let bmson = parse_bmson(&source).bmson.ok_or(RendererError::BMSONParsingError())?;
            bms = Bms::from_bmson(bmson).bms;
        } else {
            bms = parse_bms(&source, default_config()).bms?;
        }

        Ok(Self {
            bms,
            base_path: path_ref.parent().unwrap().to_path_buf(),
        })
    }
}
