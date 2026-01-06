mod bms_preview;

use crate::bms_preview::*;

use std::path::{Path};

fn main() {
    let args = Args::parse();

    let song_folder = Path::new(&args.songs_folder);
    
    match process_folder(&song_folder.to_path_buf(), &args) {
        Ok(_) => {
            println!("finished processing")
        },
        Err(e) => { 
            println!("failed: {}", e);
        },
    }
}
