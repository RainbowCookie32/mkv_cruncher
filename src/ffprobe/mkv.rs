use super::{FFProbeResult, FFProbeStream};
use super::error::ProbeError;

pub struct MkvFile {
    size: u64,
    duration: f64,

    streams: Vec<Stream>
}

impl MkvFile {
    pub(super) fn parse_result(probe: FFProbeResult) -> Result<MkvFile, ProbeError> {
        let size = probe.format.size.parse::<u64>().map_err(|_| ProbeError::NumParseError(probe.format.size))?;
        let duration = probe.format.duration.parse::<f64>().map_err(|_| ProbeError::NumParseError(probe.format.duration))?;

        let mut streams = Vec::new();

        for stream_probe in probe.streams {
            streams.push(Stream::parse_result(stream_probe)?);
        }

        Ok(
            MkvFile {
                size,
                duration,

                streams
            }
        )
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn duration(&self) -> f64 {
        self.duration
    }

    pub fn attachments(&self) -> Vec<&Stream> {
        self.streams.iter()
            .filter(| s | {
                matches!(&s.codec_type, CodecType::Attachment { filename: _, mime_type: _ })
            })
            .collect()
    }

    pub fn audio_streams(&self) -> Vec<&Stream> {
        self.streams.iter()
            .filter(| s | {
                matches!(&s.codec_type, CodecType::Audio { language: _, title: _, channels: _ })
            })
            .collect()
    }

    pub fn video_streams(&self) -> Vec<&Stream> {
        self.streams.iter()
            .filter(| s | {
                matches!(&s.codec_type, CodecType::Video { language: _, title: _ })
            })
            .collect()
    }

    pub fn subtitles_streams(&self) -> Vec<&Stream> {
        self.streams.iter()
            .filter(| s | {
                matches!(&s.codec_type, CodecType::Subtitle { language: _, title: _ })
            })
            .collect()
    }
}

pub struct Stream {
    codec: String,
    codec_type: CodecType,
}

impl Stream {
    fn parse_result(probe: FFProbeStream) -> Result<Stream, ProbeError> {
        let codec_type = {
            let title = probe.tags.title.unwrap_or_default();
            let language = probe.tags.language.unwrap_or_else(|| String::from("und"));

            let filename = probe.tags.filename.unwrap_or_default();
            let mime_type = probe.tags.mimetype.unwrap_or_default();

            match probe.codec_type.as_str() {
                "audio" => CodecType::Audio { language, title, channels: probe.channels },
                "video" => CodecType::Video { language, title },
                "subtitle" => CodecType::Subtitle { language, title },
                "attachment" => CodecType::Attachment { filename, mime_type },
    
                _ => return Err(ProbeError::UnknownCodecType(probe.codec_type))
            }
        };

        Ok(
            Stream {
                codec: probe.codec_name,
                codec_type,
            }
        )
    }

    pub fn codec(&self) -> &str {
        self.codec.as_str()
    }

    pub fn channels(&self) -> u64 {
        if let CodecType::Audio { channels, .. } = self.codec_type {
            channels
        }
        else {
            0
        }
    }

    pub fn stream_title(&self) -> String {
        match &self.codec_type {
            CodecType::Audio { title, .. } => title.clone(),
            CodecType::Video { title, .. } => title.clone(),
            CodecType::Subtitle { title, .. } => title.clone(),
            CodecType::Attachment { filename, .. } => filename.clone(),
        }
    }

    pub fn stream_language(&self) -> String {
        match &self.codec_type {
            CodecType::Audio { language, .. } => language.clone(),
            CodecType::Video { language, .. } => language.clone(),
            CodecType::Subtitle { language, .. } => language.clone(),
            _ => String::new(),
        }
    }
}

#[derive(PartialEq)]
pub enum CodecType {
    Audio { language: String, title: String, channels: u64 },
    Video { language: String, title: String },
    Subtitle { language: String, title: String },
    Attachment { filename: String, mime_type: String }
}
