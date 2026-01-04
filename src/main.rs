mod renderer;

use crate::renderer::Renderer;
use clap::Parser;
use std::fs::metadata;
use std::path::{Path, PathBuf};
use std::{fs, io};

#[derive(Parser, Debug)]
#[command(about, long_about = None)]
struct Args {
    /// The directory containing songs to process in a batch.
    #[arg(short, long)]
    songs_folder: String,

    /// The starting time of the preview
    #[arg(long, default_value_t = 20000)]
    start: u64,

    /// The ending time of the preview
    #[arg(long, default_value_t = 40000)]
    end: u64,

    /// The duration to fade in the preview
    #[arg(long, default_value_t = 1000)]
    fade_in: u64,

    /// The duration to fade out the preview
    #[arg(long, default_value_t = 2000)]
    fade_out: u64,

    /// The filename of the preview file
    #[arg(long, default_value = "preview_auto_generated.ogg")]
    preview_file: String,

    #[arg(long, default_value_t = false)]
    mono_audio: bool,
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

    if !metadata(&args.songs_folder)
        .expect("songs folder missing")
        .is_dir()
    {
        return;
    }

    let song_folder = Path::new(&args.songs_folder);
    let mut bms_files = Vec::new();
    get_bms_files(&mut bms_files, song_folder).expect("failed to get BMS files");

    for file in bms_files {
        let Ok(render) = Renderer::new(file) else {
            continue;
        };
        if let Err(e) = render.process_bms_file(&args) {
            eprintln!("{}", e);
        }
        return;
    }
}
