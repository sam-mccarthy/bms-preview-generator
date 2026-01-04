# BMS Preview Generator
A **work-in-progress** tool that generates preview audio files in .ogg format for Be-Music Source (.bms) files, intended for use with [Beatoraja](https://github.com/exch-bms2/beatoraja).

# Why?
The current popular option ([MikiraSora/BmsPreviewAudioGenerator](https://github.com/MikiraSora/BmsPreviewAudioGenerator)) is Windows-only. I tried for a short time to get it working with the .dylib files provided by Un4Seen for the Bass library, but ran into issues and ultimately decided it would be a fun project to write a similar tool in Rust. [5argon/bms-preview-maker](https://github.com/5argon/bms-preview-maker) is also an option for Mac, but I only found it part-way through development, and was having fun anyway. My hope is to finish this project to bring it to feature parity with MikiraSora's work.

# Work-in-progress?
There are currently a lot of things that need to be improved. A bit of the code is lazily written (results with generic boxed errors, cloned values) for the sake of getting a prototype working. The current iteration seems to render and output the song properly, but does not support any of the passed arguments.

# Releases?
Once this project is in a decent state, I'll setup a GitHub Actions process for Windows / Mac / Linux builds and releases. 

# Contribution
For now, I'd like to keep the repository closed to contribution for the sake of my own learning. In the future, I will consider allowing pull requests.

# Credit
The [bms-bounce](https://github.com/approvers/bms-bounce) project was used as reference for calculating the timing of notes from seconds.
The [bms-rs](https://github.com/MikuroXina/bms-rs) project has been invaluable in writing this project, and is used for processing .bms files.
[vorbis-rs](ComunidadAylas/vorbis-rs) made the .ogg encoding much easier than it otherwise seemed to be.