use std::path::PathBuf;

use clap::Parser;

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
        short = 'd',
        long,
        help="Analyze the MKV files on the input directory, but skip processing them."
    )]
    dry_run: bool,

    #[clap(
        short = 'r',
        long,
        help="Whether or not to go into subfolders in the Input directory."
    )]
    recursive: bool,

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
    pub fn recursive(&self) -> bool {
        self.recursive
    }
    
    pub fn dry_run(&self) -> bool {
        self.dry_run
    }

    pub fn forced_transcode(&self) -> bool {
        self.force_always_transcode
    }

    pub fn can_transcode_video(&self) -> bool {
        self.force_always_transcode || !self.force_never_transcode
    }

    pub fn input_dir(&self) -> &PathBuf {
        &self.input_dir
    }

    pub fn output_dir(&self) -> &PathBuf {
        &self.output_dir
    }

    pub fn intermediate_dir(&self) -> Option<&PathBuf> {
        self.intermediate_dir.as_ref()
    }
}
