# BMS Preview Generator

Generate preview audio files in .ogg format for Be-Music Source (.bms) files, intended for use with [Beatoraja](https://github.com/exch-bms2/beatoraja).

# Why?

The current popular option ([MikiraSora/BmsPreviewAudioGenerator](https://github.com/MikiraSora/BmsPreviewAudioGenerator)) is Windows-only. [5argon/bms-preview-maker](https://github.com/5argon/bms-preview-maker) is also an option for Mac, but seems much less feature-rich. The goal of this project is to bring it to (at least) feature parity with MikiraSora's work while maintaining cross-platform compatibility and high performance. That goal has more or less been reached - there are some details I'd like to iron out (it seems that .bmson files fail, and it would be nice to have an elapsed time tracker), but it's in a solid working state and can process folders in batches.

# Credit

The [bms-bounce](https://github.com/approvers/bms-bounce) project was used as reference for calculating the timing of notes from seconds.
The [bms-rs](https://github.com/MikuroXina/bms-rs) project has been invaluable in writing this project, and is used for processing .bms files.
[vorbis-rs](ComunidadAylas/vorbis-rs) made the .ogg encoding much easier than it otherwise seemed to be.
