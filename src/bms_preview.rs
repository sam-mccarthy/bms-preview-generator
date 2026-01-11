pub mod renderer;
pub use renderer::Renderer;

mod errors;
mod stereo_audio;

pub use clap::Parser;

#[derive(Parser, Debug)]
#[command(about, long_about = None)]
pub struct Args {
    /// The directory containing songs to process in a batch.
    #[arg(short = 'f', long, required = true)]
    pub songs_folder: Option<String>,

    /// The starting time of the preview (seconds)
    #[arg(short = 's', long, default_value_t = 20.0)]
    pub start: f64,

    /// The ending time of the preview (seconds)
    #[arg(short = 'e', long, default_value_t = 40.0)]
    pub end: f64,

    /// The starting time of the preview (percentage of song)
    #[arg(long)]
    pub start_p: Option<f64>,

    /// The starting time of the preview (percentage of song)
    #[arg(long)]
    pub end_p: Option<f64>,

    /// The duration to fade in the preview
    #[arg(long, default_value_t = 2.0)]
    pub fade_in: f64,

    /// The duration to fade out the preview
    #[arg(long, default_value_t = 2.0)]
    pub fade_out: f64,

    /// The filename of the preview file
    #[arg(short = 'o', long, default_value = "preview_auto_generated.ogg")]
    pub preview_file: String,

    /// Render mono instead of stereo preview audio
    #[arg(short = 'm', long, default_value_t = false)]
    pub mono_audio: bool,

    /// The sample rate of the preview file. Defaults to sample rate of the song.
    #[arg(short = 'r', long)]
    pub sample_rate: Option<u32>,

    /// Scale volume by percentage.
    #[arg(short = 'v', long, default_value_t = 100.0)]
    pub volume: f32,

    /// Overwrite existing preview files.
    #[arg(long, default_value_t = true)]
    pub overwrite: bool,
}

use errors::ProcessError;
use rayon::prelude::*;
use walkdir::{DirEntry, WalkDir};
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Instant;

/// Process a folder containing BMS songs.
pub fn process_folder(song_folder: &PathBuf, args: &Args) -> Result<(), ProcessError> {
    const VALID_EXTS: [&str; 5] = ["bms", "bme", "bml", "pms", "bmson"];
    
    if !song_folder.exists() || !song_folder.is_dir() {
        return Err(ProcessError::InvalidSongsFolder());
    }
    
    let mut explored_folders: HashSet<PathBuf> = HashSet::new();
    // Get all song files (by extension) in the song folder
    let bms_files: Vec<DirEntry> = WalkDir::new(song_folder).into_iter().filter_map(|file| {
        let Ok(file) = file else { return None };
        
        let path = file.path();
        let parent = path.parent()?.to_path_buf();
        let Some(extension) = path.extension() else { return None };
        
        let is_valid = VALID_EXTS
            .iter()
            .any(|valid_ext| valid_ext == &extension.to_string_lossy());
        if path.is_file() && is_valid && !explored_folders.contains(&parent) {
            explored_folders.insert(parent);
            Some(file)
        } else {
            None
        }
    }).collect();
    
    // Iterate over songs in parallel
    bms_files.par_iter().for_each(|file| {
        let path = file.path();
        let str_path = path.to_str();
        let start = Instant::now();
        
        // Setup (parse) the song file as a renderer
        match Renderer::new(path) {
            // Generate the preview file
            Ok(render) => match render.process_bms_file(&args) {
                Ok(_) => {
                    let end = Instant::now();
                    println!(
                        "processed {} in {:.2}s",
                        str_path.unwrap(),
                        (end - start).as_secs_f64()
                    );
                }
                Err(e) => {
                    let _end = Instant::now();
                    println!("failed {}: {}", str_path.unwrap(), e);
                }
            },
            Err(e) => eprintln!("failed {}: {}", str_path.unwrap(), e),
        }
    });

    Ok(())
}
