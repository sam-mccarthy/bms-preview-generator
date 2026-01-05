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

    #[arg(long, default_value_t = false)]
    pub mono_audio: bool,
    
    // The sample rate of the preview file. If zero, will default to the sample rate used by the song
    #[arg(long, default_value_t = 0)]
    pub sample_rate: u32,

    #[arg(long, default_value_t = 1.0)]
    pub volume: f32,
}