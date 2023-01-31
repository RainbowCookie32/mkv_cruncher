mod args;
mod ffprobe;

use std::fs;
use std::time::Instant;

use log::*;
use flexi_logger::{Logger, LoggerHandle};

use clap::Parser;
use walkdir::WalkDir;
use bytesize::ByteSize;
use ffprobe::mkv::{MkvFile, Stream};

fn main() {
    let args = args::AppArgs::parse();
    let _logger_handle = configure_log();

    prepare_paths(&args);

    println!();
    info!("Reading directory {}", args.input_dir().as_os_str().to_string_lossy());

    let dir_walker = {
        let mut walker = WalkDir::new(args.input_dir());

        if !args.recursive() {
            walker = walker.max_depth(1);
        }

        walker = walker.sort_by(| a, b | a.file_name().cmp(b.file_name()));
        
        walker
            .into_iter()
            .filter_map(| d | d.ok())
    };

    let base_output_dir = {
        if let Some(intermediate_path) = args.intermediate_dir() {
            intermediate_path.clone()
        }
        else {
            args.output_dir().clone()
        }
    };

    let mut clean_exit = true;

    for entry in dir_walker {
        let file_name = entry.file_name().to_string_lossy().to_string();

        if entry.file_type().is_file() && file_name.contains("mkv") {
            info!("Currently processing: {file_name}");

            if let Ok(mkv) = ffprobe::probe_file(entry.path()) {
                let with_video_transcode = {
                    if args.forced_transcode() || (args.can_transcode_video() && analyze_video(&mkv)) {
                        info!("  Video track will be transcoded.");
                        true
                    }
                    else {
                        info!("  Video track will be copied.");
                        false
                    }
                };

                let subs_to_keep = analyze_sub_tracks(&mkv);
                let audio_to_keep = analyze_audio_tracks(&mkv);
                let attachments_to_keep = analyze_attachments(&mkv);

                if args.dry_run() {
                    info!("Dry run was requested, moving on...");
                    trace!("------");
                    println!();

                    continue;
                }

                let mut stdin = None;
                let mut ffmpeg_args = vec![
                    // Make ffmpeg less noisy.
                    String::from("-hide_banner"), String::from("-loglevel"), String::from("error"),
                    // Preserve progress stats and overwrite existing files.
                    String::from("-stats"), String::from("-y")
                ];

                if ByteSize::b(mkv.size()) < ByteSize::gb(4) {
                    info!("  Loading MKV file into memory.");

                    match fs::read(entry.path()) {
                        Ok(buf) => {
                            info!("  File loaded successfully, launching ffmpeg.");
                            stdin = Some(buf);
                        }
                        Err(e) => {
                            info!("  Failed to load MKV file into memory: {e}");
                            info!("  Falling back to reading from disk.");
                        }
                    }
                }
                else {
                    info!("  MKV file is too big, reading from disk.");

                    ffmpeg_args.push(String::from("-i"));
                    ffmpeg_args.push(entry.path().as_os_str().to_string_lossy().to_string());
                }

                // Grab only the first video stream. Skips cover pictures and horrible fuck-ups.
                ffmpeg_args.push(String::from("-map"));
                ffmpeg_args.push(String::from("0:v:0"));

                if !subs_to_keep.is_empty() && subs_to_keep.len() == mkv.subtitles_streams().len() {
                    ffmpeg_args.push(String::from("-map"));
                    ffmpeg_args.push(String::from("0:s"));
                }
                else {
                    for (sub, _) in subs_to_keep {
                        ffmpeg_args.push(String::from("-map"));
                        ffmpeg_args.push(format!("0:s:{sub}"));
                    }
                }

                for (audio, track) in audio_to_keep.iter() {
                    ffmpeg_args.push(String::from("-map"));
                    ffmpeg_args.push(format!("0:a:{audio}"));

                    if LOSSLESS_AUDIO_CODECS.contains(&track.codec()) {
                        ffmpeg_args.push(String::from("-c:a"));
                        ffmpeg_args.push(String::from("libopus"));
                        ffmpeg_args.push(String::from("-ac"));
                        ffmpeg_args.push(String::from("2"));
                    }
                    else {
                        ffmpeg_args.push(String::from("-c:a"));
                        ffmpeg_args.push(String::from("copy"));
                    }
                }

                if !attachments_to_keep.is_empty() && attachments_to_keep.len() == mkv.attachments().len() {
                    ffmpeg_args.push(String::from("-map"));
                    ffmpeg_args.push(String::from("0:t"));
                }
                else {
                    for (attachment, _) in attachments_to_keep {
                        ffmpeg_args.push(String::from("-map"));
                        ffmpeg_args.push(format!("0:t:{attachment}"));
                    }
                }

                ffmpeg_args.push(String::from("-c:v"));

                if with_video_transcode {
                    ffmpeg_args.push(String::from("libsvtav1"));

                    ffmpeg_args.push(String::from("-crf"));
                    ffmpeg_args.push(String::from("30"));

                    ffmpeg_args.push(String::from("-preset"));
                    ffmpeg_args.push(String::from("7"));

                    ffmpeg_args.push(String::from("-g"));
                    ffmpeg_args.push(String::from("120"));

                    ffmpeg_args.push(String::from("-pix_fmt"));
                    ffmpeg_args.push(String::from("yuv420p10le"));
                }
                else {
                    ffmpeg_args.push(String::from("copy"));
                }

                ffmpeg_args.push(String::from("-c:s"));
                ffmpeg_args.push(String::from("copy"));

                ffmpeg_args.push(String::from("-metadata"));
                ffmpeg_args.push(String::from("title="));

                ffmpeg_args.push(String::from("-metadata:s:v"));
                ffmpeg_args.push(String::from("title="));

                ffmpeg_args.push(String::from("-metadata:s:a"));
                ffmpeg_args.push(String::from("title="));

                ffmpeg_args.push(String::from("-metadata:s:v"));
                ffmpeg_args.push(String::from("language=und"));

                let mut out_path = base_output_dir.clone();
                out_path.push(&file_name);

                ffmpeg_args.push(out_path.to_string_lossy().to_string());

                let ffmpeg_process = {
                    if let Some(stdin) = stdin {
                        ffmpeg_args.push(String::from("-i"));
                        ffmpeg_args.push(String::from("pipe:0"));
                        
                        duct::cmd("ffmpeg", ffmpeg_args)
                            .stdout_capture()
                            .stdin_bytes(stdin)
                    }
                    else {
                        ffmpeg_args.push(String::from("-i"));
                        ffmpeg_args.push(entry.path().as_os_str().to_string_lossy().to_string());

                        duct::cmd("ffmpeg", ffmpeg_args)
                            .stdout_capture()
                    }
                };

                let instant = Instant::now();

                match ffmpeg_process.run() {
                    Ok(out) => {
                        if !out.status.success() {
                            if let Some(exit_code) = out.status.code() {
                                error!("ffmpeg didn't exit successfully (code: {exit_code}), exiting...");
                            }
                            else {
                                error!("ffmpeg didn't exit successfully, exiting...");
                            }
                            
                            clean_exit = false;
                            break;
                        }

                        let time_to_process = instant.elapsed();

                        if let Some(intermediate) = args.intermediate_dir() {
                            let mut output_path = args.output_dir().clone();
                            let mut result_path = intermediate.clone();

                            output_path.push(entry.file_name());
                            result_path.push(entry.file_name());

                            info!("ffmpeg exited successfully, copying result to output dir...");
                            info!("Time to process: {}m{}s", time_to_process.as_secs() / 60, time_to_process.as_secs() % 60);

                            let result_size = result_path
                                .metadata()
                                .expect("Failed to read metadata for result file")
                                .len()
                            ;

                            match fs::copy(&result_path, &output_path) {
                                Ok(bytes_copied) => {
                                    if bytes_copied == result_size {
                                        info!("MKV file '{file_name}' processed successfully!");
                                    }
                                    else {
                                        error!("The file copied to output dir didn't match the intermediate file's size.");
                                        clean_exit = false;
    
                                        break;
                                    }
                                },
                                Err(e) => {
                                    error!("Failed to copy result file file for {file_name} to output dir: {e}");
                                    clean_exit = false;

                                    break;
                                }
                            }

                            if let Err(e) = fs::remove_file(&result_path) {
                                error!("Failed to remove intermediate file for {file_name}: {e}");
                                clean_exit = false;

                                break;
                            }
                            else {
                                info!("Intermediate file for {file_name} removed successfully!");
                            }
                        }
                        else {
                            info!("ffmpeg exited successfully.");
                            info!("Time to process: {}m{}s", time_to_process.as_secs() / 60, time_to_process.as_secs() % 60);
                        }
                    }
                    Err(e) => {
                        error!("Failed to launch ffmpeg or something: {e}");
                        clean_exit = false;
                
                        break;
                    },
                }

                trace!("------");
                println!();
            }
        }
    }

    if !clean_exit {
        error!("Exiting because of an error...");

        if !args.dry_run() {
            if let Some(intermediate) = args.intermediate_dir() {
                for entry in WalkDir::new(intermediate).max_depth(1).into_iter().filter_map(| f | f.ok()) {
                    let file_name = entry.file_name().to_string_lossy().to_string();
    
                    if entry.file_type().is_file() && file_name.to_lowercase().contains("mkv") {
                        fs::remove_file(entry.path()).expect("Failed to remove intermediate file");
                    }
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

fn prepare_paths(args: &args::AppArgs) {
    if !args.input_dir().exists() {
        panic!("Input path doesn't exist!");
    }

    if !args.output_dir().exists() {
        fs::create_dir_all(args.output_dir()).expect("Output directory didn't exist, and couldn't be created!");
    }

    if let Some(intermediate) = args.intermediate_dir() {
        if !intermediate.exists() {
            fs::create_dir_all(intermediate).expect("Intermediate directory didn't exist, and couldn't be created!");
        }
    }
}

fn analyze_video(mkv: &MkvFile) -> bool {
    let track = mkv.video_streams()[0];
    
    let mkv_size = ByteSize::b(mkv.size());
    let mkv_duration = (mkv.duration().floor() as u64) / 60;

    // A lot of guesstimation that'll probably need further tweaking.
    // Non-HEVC I'll likely always want to transcode.
    if track.codec() != HEVC_CODEC {
        true
    }
    // This is aimed at movies mostly.
    else if mkv_duration >= 55 {
        // Not quite convinced at size threshold here.
        mkv_size > ByteSize::gib(5)
    }
    // Show episodes should fall in here.
    else {
        // Not quite convinced here either.
        mkv_size > ByteSize::mib(600)
    }
}

fn analyze_sub_tracks(mkv: &MkvFile) -> Vec<(usize, &Stream)> {
    let all_streams = mkv.subtitles_streams();
    let stream_count = all_streams.len();

    let mut preserved_streams: Vec<(usize, &Stream)> = all_streams
        .into_iter()
        .enumerate()
        .map(| (i, s) | (i, s))
        .collect()
    ;

    preserved_streams.sort_unstable_by_key(|(_, s)| s.stream_title());
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
        info!("  Keeping all subs ({stream_count})");
    }

    preserved_streams
}

fn analyze_audio_tracks(mkv: &MkvFile) -> Vec<(usize, &Stream)> {
    let all_streams = mkv.audio_streams();
    let stream_count = all_streams.len();

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
        info!("  Keeping all audio tracks ({stream_count})");
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
            name.contains("ttf") || name.contains("otf") || !name.contains(".")
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
        info!("  Keeping all attachments ({attachment_count})");
    }

    preserved_attachments
}

const ASS_CODEC: &str = "ass";
const HEVC_CODEC: &str = "hevc";

const OK_SUB_LANGS: [&str; 5] = [
    "eng",
    "enm",
    "jpn",
    "spa",
    "und"
];

const BAD_SUB_WORDS: [&str; 7] = [
    "s&s",
    "signs",
    "songs",
    "spain",
    "closed",
    "captions",
    "closed captions"
];

const LOSSLESS_AUDIO_CODECS: [&str; 4] = [
    "dts",
    "flac",
    "truehd",
    "pcm_s24le"
];
