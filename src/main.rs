mod renderer;

use crate::renderer::Renderer;
use clap::Parser;
use std::fs::metadata;
use std::path::{Path, PathBuf};
use std::{fs, io};
use std::time::Instant;

#[derive(Parser, Debug)]
#[command(about, long_about = None)]
struct Args {
    /// The directory containing songs to process in a batch.
    #[arg(short, long)]
    songs_folder: String,

    /// The starting time of the preview (seconds)
    #[arg(long, default_value_t = 20.0)]
    start: f64,

    /// The ending time of the preview (seconds)
    #[arg(long, default_value_t = 40.0)]
    end: f64,

    /// The duration to fade in the preview
    #[arg(long, default_value_t = 2.0)]
    fade_in: f64,

    /// The duration to fade out the preview
    #[arg(long, default_value_t = 2.0)]
    fade_out: f64,

    /// The filename of the preview file
    #[arg(long, default_value = "preview_auto_generated.ogg")]
    preview_file: String,

    #[arg(long, default_value_t = false)]
    mono_audio: bool,
    
    #[arg(long, default_value_t = 48000)]
    sample_rate: u32
}

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
                }
            }
        }
    }

    Ok(())
}

fn main() {
    let args = Args::parse();
    
    let song_folder = Path::new(&args.songs_folder);
    if !song_folder.exists() || !song_folder.is_dir() {
        println!("bad songs folder");
        return;
    }

    let mut bms_files = Vec::new();
    get_bms_files(&mut bms_files, song_folder).expect("failed to get BMS files");

    for file in bms_files {
        print!("processing {}", file.to_str().unwrap());
        let start = Instant::now();
        
        let Ok(render) = Renderer::new(file) else {
            continue;
        };
        
        if let Err(e) = render.process_bms_file(&args) {
            eprintln!("{}", e);
        }
        
        let end = Instant::now();
        println!(" - took {:.2}s", (end - start).as_secs_f64());
        
        return;
    }
}
