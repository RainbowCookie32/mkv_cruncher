use std::io::Error;
use std::fmt::Display;

#[derive(Debug)]
pub enum ProbeError {
    NumParseError(String),
    UnknownCodecType(String),

    ExecError(Error),
    SerdeError(serde_json::Error),
}

impl std::error::Error for ProbeError {}

impl Display for ProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProbeError::NumParseError(num) => write!(f, "Failed to parse '{num} as a number.'"),
            ProbeError::UnknownCodecType(_type) => write!(f, "Unknown codec name '{_type}.'"),
            ProbeError::ExecError(e) => write!(f, "ffprobe subprocess failed to run: {e}"),
            ProbeError::SerdeError(e) => write!(f, "Serde failed to deserialize the result: {e}"),
        }
    }
}
