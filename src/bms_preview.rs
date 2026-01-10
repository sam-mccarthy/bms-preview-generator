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
use std::path::{Path, PathBuf};
use std::time::Instant;
use std::{fs, io};

/// Get all files with a valid BMS extension recursively in a folder.
fn get_bms_files(files: &mut Vec<PathBuf>, dir: &Path) -> io::Result<()> {
    // The set of valid song extensions.
    let valid_extensions = ["bms", "bme", "bml", "pms", "bmson"];
    
    if dir.is_dir() {
        // Read all items in the folder and recurse through directories while adding
        // valid files to the vector.
        
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

/// Process a folder containing BMS songs.
pub fn process_folder(song_folder: &PathBuf, args: &Args) -> Result<(), ProcessError> {
    if !song_folder.exists() || !song_folder.is_dir() {
        return Err(ProcessError::InvalidSongsFolder());
    }
    
    // Get all song files (by extension) in the song folder
    let mut bms_files = Vec::new();
    get_bms_files(&mut bms_files, song_folder)?;
    
    // Iterate over songs in parallel
    bms_files.par_iter().for_each(|file| {
        let start = Instant::now();
        
        // Setup (parse) the song file as a renderer
        match Renderer::new(&file) {
            // Generate the preview file
            Ok(render) => match render.process_bms_file(&args) {
                Ok(_) => {
                    let end = Instant::now();
                    println!(
                        "processed {} in {:.2}s",
                        file.to_str().unwrap(),
                        (end - start).as_secs_f64()
                    );
                }
                Err(e) => {
                    let _end = Instant::now();
                    println!("failed {}: {}", file.to_str().unwrap(), e);
                }
            },
            Err(e) => eprintln!("failed {}: {}", file.to_str().unwrap(), e),
        }
    });

    Ok(())
}
