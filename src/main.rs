mod args;
mod ffprobe;

use std::fs;
use std::path::PathBuf;
use std::time::Instant;
use std::process::Command;
use std::io::{BufRead, BufReader, Write};

use log::*;
use flexi_logger::{Logger, LoggerHandle};
use indicatif::{ProgressBar, ProgressStyle};

use clap::Parser;
use walkdir::WalkDir;
use bytesize::ByteSize;

use args::{PreloadMode, TranscodeMode};
use ffprobe::mkv::{MkvFile, Stream};

pub struct Cruncher {
    output: PathBuf,
    intermediate: Option<PathBuf>,

    files: Vec<PathBuf>,

    preload_mode: PreloadMode,
    transcode_mode: TranscodeMode
}

impl Cruncher {
    fn init(cfg: args::AppArgs) -> Cruncher {
        if !cfg.input_dir().exists() {
            panic!("Input directory doesn't exist!");
        }

        if let Some(intermediate) = cfg.intermediate_dir().as_ref() {
            if !intermediate.exists() {
                if let Err(e) = fs::create_dir_all(intermediate) {
                    panic!("Failed to create intermediate directory! {e}");
                }
                else {
                    warn!("Created intermediate directory at {}", intermediate.to_string_lossy())
                }
            }
        }

        if !cfg.output_dir().exists() {
            if let Err(e) = fs::create_dir_all(&cfg.output_dir()) {
                panic!("Failed to create output directory! {e}");
            }
            else {
                info!("Created output directory at {}", cfg.output_dir().to_string_lossy())
            }
        }

        info!("Reading directory {}", cfg.input_dir().as_os_str().to_string_lossy());

        let files = WalkDir::new(&cfg.input_dir())
            .max_depth(1)
            .sort_by_file_name()
            .into_iter()
            .filter_map(| entry | entry.ok())
            .filter(| entry | entry.file_type().is_file())
            .filter(| entry | entry.file_name().to_string_lossy().contains(".mkv"))
            .map(| entry | entry.into_path())
            .collect::<Vec<PathBuf>>()
        ;

        Cruncher {
            output: cfg.output_dir(),
            intermediate: cfg.intermediate_dir(),

            files,
            preload_mode: cfg.preload_mode(),
            transcode_mode: cfg.transcode_mode(),
        }
    }

    fn start_cruncher(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let total_timer = Instant::now();

        for file in self.files.iter() {
            let file_name = file.file_name().unwrap().to_str().unwrap_or_default();

            info!("Processing file '{file_name}'");

            let mkv = ffprobe::probe_file(file)?;

            let transcode_video = match self.transcode_mode {
                TranscodeMode::Auto => analyze_video(&mkv),
                TranscodeMode::Force => true,
                TranscodeMode::Never => false
            };

            let preload_file = match self.preload_mode {
                PreloadMode::Auto => transcode_video,
                PreloadMode::Force => true,
                PreloadMode::Never => false
            };

            let kept_subs = analyze_sub_tracks(&mkv);
            let kept_audio = analyze_audio_tracks(&mkv);
            let kept_attachments = analyze_attachments(&mkv);

            let mut ffmpeg_arguments = vec![
                // Silence ffmpeg.
                String::from("-hide_banner"), String::from("-loglevel"), String::from("error"),
                // Print progress stats to stdout, always overwrite existing files.
                String::from("-progress"), String::from("pipe:1"), String::from("-y"),
            ];

            let mut file_buffer = Vec::new();

            // Avoid locking up my system by loading massive files.
            // Also, don't load files into memory if we are not transcoding video,
            // it usually ends up taking longer to load it up than to crunch the file.
            if transcode_video && preload_file && ByteSize::b(mkv.size()) < ByteSize::gb(3) {
                info!("  Loading MKV file into memory.");

                match fs::read(file) {
                    Ok(buf) => {
                        info!("  File loaded successfully, launching ffmpeg.");

                        file_buffer = buf;
                        ffmpeg_arguments.push(String::from("-i"));
                        ffmpeg_arguments.push(String::from("pipe:0"));
                    }
                    Err(e) => {
                        info!("  Failed to load MKV file into memory: {e}");
                        info!("  Falling back to reading from disk.");
                    }
                }
            }
            else {
                if transcode_video && preload_file {
                    info!("  MKV file is too big, reading from disk.");
                }
                else if !preload_file {
                    info!("  Preload was disabled in configuration, reading from disk.");
                }
                else {
                    info!("  Preload is disabled when video isn't transcoded, reading from disk.");
                }

                ffmpeg_arguments.push(String::from("-i"));
                ffmpeg_arguments.push(file.to_str().unwrap_or_default().to_owned());
            }

            // Grab only the first video stream. Skips cover pictures and horrible fuck-ups.
            ffmpeg_arguments.push(String::from("-map"));
            ffmpeg_arguments.push(String::from("0:v:0"));

            // Use -map 0:s if all subs are being kept instead of mapping one by one.
            // The is_empty check is a failsafe to avoid mapping when there are *no* subtitles.
            // IIRC, ffmpeg doesn't like that, so don't remove it, future me.
            if !kept_subs.is_empty() && kept_subs.len() == mkv.subtitles_streams().len() {
                ffmpeg_arguments.push(String::from("-map"));
                ffmpeg_arguments.push(String::from("0:s"));
            }
            else {
                for (stream_idx, _) in kept_subs {
                    ffmpeg_arguments.push(String::from("-map"));
                    ffmpeg_arguments.push(format!("0:s:{stream_idx}"));
                }
            }

            for (stream_idx, stream) in kept_audio.iter() {
                ffmpeg_arguments.push(String::from("-map"));
                ffmpeg_arguments.push(format!("0:a:{stream_idx}"));

                if LOSSLESS_AUDIO_CODECS.contains(&stream.codec()) {
                    ffmpeg_arguments.push(String::from("-c:a"));
                    ffmpeg_arguments.push(String::from("libopus"));
                    ffmpeg_arguments.push(String::from("-ac"));
                    ffmpeg_arguments.push(String::from("2"));
                }
                else {
                    ffmpeg_arguments.push(String::from("-c:a"));
                    ffmpeg_arguments.push(String::from("copy"));
                }
            }

            // Same deal as subs mapping, no removing the is_empty check. It's important.
            if !kept_attachments.is_empty() && kept_attachments.len() == mkv.attachments().len() {
                ffmpeg_arguments.push(String::from("-map"));
                ffmpeg_arguments.push(String::from("0:t"));
            }
            else {
                for (attachment, _) in kept_attachments {
                    ffmpeg_arguments.push(String::from("-map"));
                    ffmpeg_arguments.push(format!("0:t:{attachment}"));
                }
            }
            
            if transcode_video {
                ffmpeg_arguments.push(String::from("-c:v"));
                ffmpeg_arguments.push(String::from("libsvtav1"));

                ffmpeg_arguments.push(String::from("-crf"));
                ffmpeg_arguments.push(String::from("30"));

                ffmpeg_arguments.push(String::from("-preset"));
                ffmpeg_arguments.push(String::from("7"));

                ffmpeg_arguments.push(String::from("-g"));
                ffmpeg_arguments.push(String::from("120"));

                ffmpeg_arguments.push(String::from("-pix_fmt"));
                ffmpeg_arguments.push(String::from("yuv420p10le"));
            }
            else {
                ffmpeg_arguments.push(String::from("-c:v"));
                ffmpeg_arguments.push(String::from("copy"));
            }

            // Copy the "codec" of the subtitle tracks.
            ffmpeg_arguments.push(String::from("-c:s"));
            ffmpeg_arguments.push(String::from("copy"));

            // Remove title metadata from the file
            ffmpeg_arguments.push(String::from("-metadata"));
            ffmpeg_arguments.push(String::from("title="));

            // and the video track
            ffmpeg_arguments.push(String::from("-metadata:s:v"));
            ffmpeg_arguments.push(String::from("title="));

            // *and* the audio track.
            ffmpeg_arguments.push(String::from("-metadata:s:a"));
            ffmpeg_arguments.push(String::from("title="));

            // Some people add language metadata to video streams for some reason.
            // Don't be like those people, you throw off my shit scripts.
            ffmpeg_arguments.push(String::from("-metadata:s:v"));
            ffmpeg_arguments.push(String::from("language=und"));

            let mut target_path = {
                if let Some(intermediate) = self.intermediate.as_ref() {
                    intermediate.clone()
                }
                else {
                    self.output.clone()
                }
            };

            target_path.push(file_name);
            ffmpeg_arguments.push(target_path.to_str().unwrap_or_default().to_owned());

            let mut ffmpeg_process = Command::new("ffmpeg");

            if !file_buffer.is_empty() {
                ffmpeg_process.stdin(std::process::Stdio::piped());
            }

            ffmpeg_process
                .args(ffmpeg_arguments)
                .env("SVT_LOG", "fatal")
                .stdout(std::process::Stdio::piped());

            if let Ok(mut handle) = ffmpeg_process.spawn() {
                // Moving the duration down from seconds to microseconds.
                let bar = ProgressBar::new((mkv.duration() as u64 * 1000) * 1000);

                bar.set_style(
                    ProgressStyle::with_template("Processing... {percent}% {wide_bar} ({msg} - Elapsed: {elapsed_precise})")
                    .unwrap()
                    .progress_chars("##-")
                );

                if let Some(mut stdin) = handle.stdin.take() {
                    std::thread::spawn(move || {
                        stdin.write_all(&file_buffer).expect("Failed to write file to stdin");
                    });
                }

                if let Some(stdout) = handle.stdout.take() {
                    let stdout_reader = BufReader::new(stdout);
                    let stdout_lines = stdout_reader.lines();

                    for line in stdout_lines.flatten() {
                        if let Some((key, value)) = line.split_once('=') {
                            match key {
                                "speed" => bar.set_message(value.to_owned()),
                                "out_time_ms" => bar.set_position(value.parse().unwrap_or_default()),
                                _ => {}
                            }
                        }
                    }
                }

                if handle.wait().is_ok() {
                    if self.intermediate.is_some() {
                        let mut output_path = self.output.clone();
                        output_path.push(file_name);

                        fs::copy(&target_path, &output_path).expect("Failed to copy processed file from intermediate dir");

                        if transcode_video {
                            let source_hash = seahash::hash(&fs::read(&target_path).unwrap_or_default());
                            let target_hash = seahash::hash(&fs::read(&output_path).unwrap_or_default());

                            if source_hash != target_hash {
                                panic!("Hash mismatch on output file!");
                            }
                        }

                        fs::remove_file(&target_path).expect("Failed to remove processed file from intermediate dir");
                    }

                    bar.finish();
                    println!("\n");
                }
                else if target_path.exists() {
                    fs::remove_file(&target_path).expect("Failed to remove output file");
                }
            }
        }

        if self.files.len() > 1 {
            let elapsed_secs = total_timer.elapsed().as_secs();
            info!("Finished processing all files in {}m{}s", elapsed_secs / 60, elapsed_secs % 60);
        }

        Ok(())
    }
}

fn main() {
    let args = args::AppArgs::parse();
    let _logger_handle = configure_log();

    info!("Starting cruncher...\n");

    let intermediate = args.intermediate_dir().clone();
    let mut cruncher = Cruncher::init(args);

    if cruncher.start_cruncher().is_err() {
        error!("Exiting because of an error...");

        if let Some(intermediate) = intermediate {
            for entry in WalkDir::new(intermediate).max_depth(1).into_iter().filter_map(| f | f.ok()) {
                let file_name = entry.file_name().to_string_lossy().to_string();

                if entry.file_type().is_file() && file_name.to_lowercase().contains("mkv") {
                    fs::remove_file(entry.path()).expect("Failed to remove intermediate file");
                }
            }
        }
    }
}

fn configure_log() -> LoggerHandle {
    Logger::try_with_str("info")
        .expect("Failed to create Logger")
        .log_to_file(flexi_logger::FileSpec::default())
        .duplicate_to_stdout(flexi_logger::Duplicate::Info)
        .write_mode(flexi_logger::WriteMode::BufferAndFlush)
        .format_for_files(flexi_logger::detailed_format)
        .start()
        .expect("Failed to start Logger")
}

fn analyze_video(mkv: &MkvFile) -> bool {
    // Don't transcode stuff that's too small, will probably nuke quality.
    if ByteSize::b(mkv.size()) < ByteSize::mib(600) {
        false
    }
    // If it has some size, only transcode if it's not on the target video codec.
    else {
        mkv.video_streams()[0].codec() != TARGET_CODEC
    }
}

fn analyze_sub_tracks(mkv: &MkvFile) -> Vec<(usize, &Stream)> {
    let all_streams = mkv.subtitles_streams();
    let stream_count = all_streams.len();

    if stream_count == 1 {
        return all_streams
            .into_iter()
            .enumerate()
            .collect()
    }

    let mut preserved_streams: Vec<(usize, &Stream)> = all_streams
        .into_iter()
        .enumerate()
        .map(| (i, s) | (i, s))
        .collect()
    ;

    preserved_streams.sort_unstable_by_key(|(_, s)| {
        if s.stream_title().is_empty() {
            s.stream_language()
        }
        else {
            s.stream_title()
        }
    });
    preserved_streams.dedup_by_key(| (_, s) | {
        if s.stream_title().is_empty() {
            s.stream_language()
        }
        else {
            s.stream_title()
        }
    });

    let has_ass = preserved_streams.iter()
        .filter(|(_, s)| s.codec() == ASS_CODEC)
        .count() > 0
    ;

    preserved_streams = preserved_streams
        .into_iter()
        // Filter out unwanted sub tracks.
        .filter(| (_, s) | {
            let name = s.stream_title().to_lowercase();

            if name.contains("jap") || name.contains("jpn") || s.stream_language() == "jpn" {
                true
            }
            else {
                let mut keep = true;

                for bad_word in BAD_SUB_WORDS {
                    if name == bad_word || name.contains(bad_word) {
                        keep = false;
                        break;
                    }
                }
                
                keep
            }
        })
        // Filter out unused languages.
        .filter(| (_, s) | {
            OK_SUB_LANGS.contains(&s.stream_language().as_str())
        })
        // Filter out PGS and other formats if we have ASS subs.
        .filter(| (_, s) | {
            if has_ass {
                s.codec() == ASS_CODEC || s.stream_language() == "jpn"
            }
            else {
                true
            }
        })
        .collect()
    ;

    if preserved_streams.len() < stream_count {
        info!("  Keeping {}/{} subs.", preserved_streams.len(), stream_count);

        for (_, s) in preserved_streams.iter() {
            let stream_title = s.stream_title();
            
            let stream_name = {
                if stream_title.is_empty() {
                    "Untitled track"
                }
                else {
                    stream_title.as_str()
                }
            };

            info!("      {stream_name} ({})", s.codec());
        }
    }
    else {
        info!("  Keeping all subs ({stream_count}).");
    }

    preserved_streams
}

fn analyze_audio_tracks(mkv: &MkvFile) -> Vec<(usize, &Stream)> {
    let all_streams = mkv.audio_streams();
    let stream_count = all_streams.len();

    if stream_count == 1 {
        return all_streams
            .into_iter()
            .enumerate()
            .collect()
    }

    let mut preserved_streams: Vec<(usize, &Stream)> = all_streams
        .into_iter()
        .enumerate()
        // Filter non-japanese, leave undefined just in case.
        .filter(| (_, s) | {
            let l = s.stream_language();
            l.is_empty() || l == "jpn" || l == "chi" || l == "und"
        })
        // Fallback filter + nuke commentary tracks.
        .filter(| (_, s) | {
            let stream_name = s.stream_title().to_lowercase();
            !stream_name.contains("commentary") && !stream_name.contains("description") && (!stream_name.contains("eng") || !stream_name.contains("english"))
        })
        .collect()
    ;

    // Try to nuke potential 5.1 tracks if we still have more than one track.
    if preserved_streams.len() > 1 {
        let jpn_stereo: Vec<(usize, &Stream)> = preserved_streams.clone()
            .into_iter()
            .filter( | (_, s) | {
                // == 0 is a fallback in case parsing drops the ball.
                s.channels() == 2 || s.channels() == 0
            })
            .collect()
        ;

        if !jpn_stereo.is_empty() {
            preserved_streams = jpn_stereo;
        }
    }

    if preserved_streams.len() < stream_count {
        info!("  Keeping {}/{stream_count} audio tracks.", preserved_streams.len());

        for (_, s) in preserved_streams.iter() {
            let stream_title = s.stream_title();

            let stream_name = {
                if stream_title.is_empty() {
                    "Untitled track"
                }
                else {
                    stream_title.as_str()
                }
            };

            info!("      {stream_name} ({})", s.codec());
        }
    }
    else {
        info!("  Keeping all audio tracks ({stream_count}).");
    }

    preserved_streams
}

fn analyze_attachments(mkv: &MkvFile) -> Vec<(usize, &Stream)> {
    let all_attachments = mkv.attachments();
    let attachment_count = all_attachments.len();

    let mut preserved_attachments: Vec<(usize, &Stream)> = all_attachments
        .into_iter()
        .enumerate()
        .filter(| (_, a) | {
            let name = a.stream_title().to_lowercase();
            // Preserve fonts and files without extensions.
            name.contains("ttf") || name.contains("otf") || !name.contains('.')
        })
        .collect()
    ;

    let preserved = preserved_attachments.len();

    preserved_attachments.sort_unstable_by_key(| (_, a) | a.stream_title());
    preserved_attachments.dedup_by_key(| (_, a) | a.stream_title());

    if preserved < attachment_count {
        info!("  Keeping {preserved}/{attachment_count} attachments.");
    }
    else {
        info!("  Keeping all attachments ({attachment_count}).");
    }

    preserved_attachments
}

const ASS_CODEC: &str = "ass";
const TARGET_CODEC: &str = "av1";

const OK_SUB_LANGS: [&str; 5] = [
    "eng",
    "enm",
    "jpn",
    "spa",
    "und"
];

const BAD_SUB_WORDS: [&str; 8] = [
    "s&s",
    "signs",
    "songs",
    "spain",
    "closed",
    "captions",
    "closed captions",
    "commentary"
];

const LOSSLESS_AUDIO_CODECS: [&str; 4] = [
    "dts",
    "flac",
    "truehd",
    "pcm_s24le"
];
