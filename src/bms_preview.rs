pub mod renderer;
use colored::Colorize;
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
    
    /// Show the amount of time spent processing files.
    #[arg(long, default_value_t = false)]
    pub show_process_time: bool,
    
    /// Process files in parallel.
    #[arg(long, default_value_t = true)]
    pub parallel: bool,
}

use errors::ProcessError;
use rayon::prelude::*;
use walkdir::{DirEntry, WalkDir};
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Instant;

fn process_song(args: &Args) -> impl Fn(&DirEntry) {
    move |file| {
        let path = file.path();
        let str_path = path.to_string_lossy();
        let start = Instant::now();
        
        // Setup (parse) the song file as a renderer
        match Renderer::new(path) {
            // Generate the preview file
            Ok(render) => match render.process_bms_file(&args) {
                Ok(_) => {
                    let end = Instant::now();
                    if args.show_process_time {
                        let elapsed = (end - start).as_secs_f64().to_string();
                        
                        println!(
                            "{} {}{}{} in {:.4}{}.",
                            "Success".green(),
                            "[".yellow(),
                            str_path,
                            "]".yellow(),
                            elapsed.green(),
                            "s".green(),
                        );
                    } else {
                        println!(
                            "{} {}{}{}",
                            "Success".green(),
                            "[".yellow(),
                            str_path,
                            "]".yellow(),
                        );
                    }
                }
                Err(e) => eprintln!("{} [{}]: {}.", "Fail".red(), str_path, e.to_string().red()),
            },
            Err(e) => eprintln!("{} [{}]: {}.", "Fail".red(), str_path, e.to_string().red()),
        }
    }
}

/// Process a folder containing BMS songs.
pub fn process_folder(song_folder: &PathBuf, args: &Args) -> Result<(), ProcessError> {
    const VALID_EXTS: [&str; 5] = ["bms", "bme", "bml", "pms", "bmson"];
    
    if !song_folder.exists() || !song_folder.is_dir() {
        return Err(ProcessError::InvalidSongsFolder());
    }
    
    // Track folders that have been explored to avoid rendering same song multiple times
    let mut explored_folders: HashSet<PathBuf> = HashSet::new();
    // Get all song files (by extension) in the song folder
    let bms_files: Vec<DirEntry> = WalkDir::new(song_folder).into_iter().filter_map(|file| {
        let Ok(file) = file else { return None };
        
        let path = file.path();
        let parent = path.parent()?.to_path_buf();
        let Some(extension) = path.extension() else { return None };
        
        // Check if the extension if one of the valid BMS extensions
        let is_valid = VALID_EXTS
            .iter()
            .any(|valid_ext| valid_ext == &extension.to_string_lossy());
        // If the path is a file, is valid, and is in a folder that hasn't been explored, then
        // we'll add it to the collection.
        if path.is_file() && is_valid && !explored_folders.contains(&parent) {
            explored_folders.insert(parent);
            Some(file)
        } else {
            None
        }
    }).collect();
    
    // Iterate over songs in parallel
    if args.parallel {
        bms_files.par_iter().for_each(process_song(args));
    } else {
        bms_files.iter().for_each(process_song(args));
    }
    

    Ok(())
}
