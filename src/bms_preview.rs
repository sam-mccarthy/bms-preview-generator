pub mod renderer;
pub use renderer::Renderer;

mod errors;
mod audio;

pub use clap::Parser;

#[derive(Parser, Debug)]
#[command(about, long_about = None)]
pub struct Args {
    /// The directory containing songs to process in a batch.
    #[arg(short, long)]
    pub songs_folder: String,

    /// The starting time of the preview (seconds)
    #[arg(long, default_value_t = 20.0)]
    pub start: f64,

    /// The ending time of the preview (seconds)
    #[arg(long, default_value_t = 40.0)]
    pub end: f64,

    /// The duration to fade in the preview
    #[arg(long, default_value_t = 2.0)]
    pub fade_in: f64,

    /// The duration to fade out the preview
    #[arg(long, default_value_t = 2.0)]
    pub fade_out: f64,

    /// The filename of the preview file
    #[arg(long, default_value = "preview_auto_generated.ogg")]
    pub preview_file: String,
    
    // Render mono instead of stereo preview audio
    #[arg(long, default_value_t = true)]
    pub mono_audio: bool,
    
    // Render mono using only the left audio channel instead of averaging stereo
    #[arg(long, default_value_t = false)]
    pub lazy_mono: bool,
    
    // The sample rate of the preview file. If zero, will default to the sample rate used by the song
    #[arg(long, default_value_t = 0)]
    pub sample_rate: u32,

    // Scale volume (decimal scale, i.e. 0.5 = 50%)
    #[arg(long, default_value_t = 1.0)]
    pub volume: f32,
}

use errors::ProcessError;
use std::path::{Path, PathBuf};
use std::time::Instant;
use std::{fs, io};
use rayon::prelude::*;

fn get_bms_files(files: &mut Vec<PathBuf>, dir: &Path) -> io::Result<()> {
    let valid_extensions = ["bms", "bme", "bml", "pms", "bmson"];

    if dir.is_dir() {
        let items = fs::read_dir(dir)?;
        for item in items {
            let path = item?.path();
            if path.is_dir() {
                get_bms_files(files, &dir)?;
            } else {
                let Some(ext_osstr) = path.extension() else {
                    return Ok(());
                };
                let Some(ext) = ext_osstr.to_str() else {
                    return Ok(());
                };

                if valid_extensions.contains(&ext) {
                    files.push(path);
                    break;
                }
            }
        }
    }

    Ok(())
}

pub fn process_folder(song_folder: &PathBuf, args: &Args) -> Result<(), ProcessError> {
    if !song_folder.exists() || !song_folder.is_dir() {
        return Err(ProcessError::InvalidSongsFolder());
    }

    let mut bms_files = Vec::new();
    get_bms_files(&mut bms_files, song_folder).expect("failed to get BMS files");
    
    bms_files.par_iter().for_each(|file| {
        let start = Instant::now();

        let Ok(render) = Renderer::new(&file) else {
            return;
        };

        if let Err(e) = render.process_bms_file(&args) {
            eprintln!("{}", e);
        }

        let end = Instant::now();
        println!("processed {} in {:.2}s", file.to_str().unwrap(), (end - start).as_secs_f64());
    });
    
    Ok(())
}