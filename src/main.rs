mod bms_preview;

use colored::Colorize;

use crate::bms_preview::*;

use std::path::Path;

fn main() {
    let args = Args::parse();

    if let Some(song_folder) = &args.songs_folder {
        let path = Path::new(&song_folder);

        match process_folder(&path.to_path_buf(), &args) {
            Ok(_) => {
                println!("{}", "Finished!".green())
            }
            Err(e) => {
                println!("{}: {}.", "Failed".red(), e.to_string().red());
            }
        }
    }
}
