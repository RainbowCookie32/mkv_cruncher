use std::fmt::Display;
use std::error::Error;

use subprocess::PopenError;

#[derive(Debug)]
pub enum ProbeError {
    NumParseError(String),
    UnknownCodecType(String),

    ExecError(PopenError),
    SerdeError(serde_json::Error),
}

impl Error for ProbeError {}

impl Display for ProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProbeError::NumParseError(num) => write!(f, "Failed to parse '{num} as a number.'"),
            ProbeError::UnknownCodecType(_type) => write!(f, "Unknown codec name '{_type}.'"),
            ProbeError::ExecError(e) => write!(f, "FFProbe subprocess failed to run: {e}"),
            ProbeError::SerdeError(e) => write!(f, "Serde failed to deserialize the result: {e}"),
        }
    }
}
