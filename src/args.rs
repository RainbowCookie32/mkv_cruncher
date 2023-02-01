use std::path::PathBuf;

use clap::Parser;

use crate::TranscodeMode;

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
        long,
        help="Never transcode the video stream of the input files."
    )]
    force_never_transcode: bool,
    #[clap(
        long,
        help="Always transcode the video stream of the input files."
    )]
    force_always_transcode: bool
}

impl AppArgs {
    pub fn transcode_mode(&self) -> TranscodeMode {
        if self.force_always_transcode {
            TranscodeMode::Force
        }
        else if self.force_never_transcode {
            TranscodeMode::Never
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
