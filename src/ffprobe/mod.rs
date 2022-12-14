pub mod mkv;
pub mod error;

use std::path::Path;

use serde::Deserialize;
use subprocess::Exec;

use error::ProbeError;

#[derive(Deserialize)]
struct FFProbeResult {
    format: FFProbeFormat,
    streams: Vec<FFProbeStream>
}

#[derive(Deserialize)]
struct FFProbeStream {
    index: u64,
    #[serde(default)]
    codec_name: String,
    codec_type: String,

    #[serde(default)]
    channels: u64,

    disposition: FFProbeStreamDisposition,
    #[serde(default)]
    tags: FFProbeStreamTags
}

#[derive(Deserialize)]
struct FFProbeStreamDisposition {
    default: u8,
    forced: u8
}

#[derive(Deserialize, Default)]
struct FFProbeStreamTags {
    language: Option<String>,
    title: Option<String>,

    filename: Option<String>,
    mimetype: Option<String>,
}

#[derive(Deserialize)]
struct FFProbeFormat {
    filename: String,
    nb_streams: u64,
    duration: String,
    size: String,
    bit_rate: String
}

pub fn probe_file(path: &Path) -> Result<mkv::MkvFile, ProbeError> {
    let ffprobe = Exec::cmd("ffprobe")
        .args(&[
            "-v", "quiet",
            "-print_format", "json",
            "-show_format",
            "-show_streams"
        ])
        .arg(path)
    ;

    let data = ffprobe.capture().map_err(ProbeError::ExecError)?;
    let probe = serde_json::from_slice::<FFProbeResult>(&data.stdout).map_err(ProbeError::SerdeError)?;

    mkv::MkvFile::parse_result(probe)
}
