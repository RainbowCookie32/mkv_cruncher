use std::path::PathBuf;

use clap::{Parser, ValueEnum};

#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum PreloadMode {
    Auto,
    Force,
    Never
}

#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum TranscodeMode {
    Auto,
    Force,
    Never
}

#[derive(Parser, Debug)]
#[clap(author, about)]
pub struct AppArgs {
    #[clap(
        short = 'i',
        long,
        help="The directory with MKV files to process."
    )]
    input_dir: PathBuf,
    #[clap(
        short = 'o',
        long,
        help="The directory to save processed MKV files to."
    )]
    output_dir: PathBuf,
    #[clap(
        long,
        help="A directory for ffmpeg to write the output files to, which are then moved by the cruncher to output_dir."
    )]
    intermediate_dir: Option<PathBuf>,
    #[clap(
        arg_enum,
        value_parser,
        long,
        help="Whether to force preload of mkv files into memory, read them from disk, or let mkv_cruncher decide."
    )]
    preload_mode: Option<PreloadMode>,
    #[clap(
        arg_enum,
        value_parser,
        long,
        help="Whether to force transcode of video streams, copy them, or let mkv_cruncher decide."
    )]
    transcode_mode: Option<TranscodeMode>
}

impl AppArgs {
    pub fn preload_mode(&self) -> PreloadMode {
        if let Some(mode) = self.preload_mode.clone() {
            mode
        }
        else {
            PreloadMode::Auto
        }
    }

    pub fn transcode_mode(&self) -> TranscodeMode {
        if let Some(mode) = self.transcode_mode {
            mode
        }
        else {
             TranscodeMode::Auto
        }
    }

    pub fn input_dir(&self) -> PathBuf {
        self.input_dir.clone()
    }

    pub fn output_dir(&self) -> PathBuf {
        self.output_dir.clone()
    }

    pub fn intermediate_dir(&self) -> Option<PathBuf> {
        self.intermediate_dir.clone()
    }
}
