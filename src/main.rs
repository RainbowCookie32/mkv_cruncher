mod ffprobe;

use std::fs;
use std::time::Instant;
use std::path::{Path, PathBuf};

use log::*;
use log4rs::Config;
use log4rs::config::{Appender, Root};
use log4rs::append::file::FileAppender;
use log4rs::append::console::{ConsoleAppender, Target};
use log4rs::encode::pattern::PatternEncoder;
use log4rs::filter::threshold::ThresholdFilter;

use clap::Parser;
use walkdir::WalkDir;
use bytesize::ByteSize;
use ffprobe::mkv::{MkvFile, Stream};

#[derive(Parser, Debug)]
#[clap(author, about)]
struct Args {
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

fn main() {
    let args = Args::parse();

    let input_path = args.input_dir;
    let output_path = args.output_dir;
    let intermediate_path: Option<PathBuf> = args.intermediate_dir;

    let dry_run = args.dry_run;

    let never_transcode_video = args.force_never_transcode;
    let always_transcode_video = args.force_always_transcode;

    configure_log();
    prepare_paths(&input_path, &output_path, &intermediate_path);

    println!();
    info!("Reading directory {}", input_path.as_os_str().to_string_lossy());

    let dir_walker = WalkDir::new(input_path)
        .max_depth(1)
        .into_iter()
        .filter_map(| d | d.ok())
    ;

    let mut clean_exit = true;

    for entry in dir_walker {
        let file_name = entry.file_name().to_string_lossy().to_string();

        if entry.file_type().is_file() && file_name.contains("mkv") {
            info!("Currently processing: {file_name}");

            if let Ok(mkv) = ffprobe::probe_file(entry.path()) {
                let with_video_transcode = {
                    if always_transcode_video {
                        info!("  Video track will be transcoded.");
                        true
                    }
                    else if never_transcode_video || !analyze_video(&mkv) {
                        info!("  Video track will be copied.");
                        false
                    }
                    else {
                        info!("  Video track will be transcoded.");
                        true
                    }
                };

                let subs_to_keep = analyze_sub_tracks(&mkv);
                let audio_to_keep = analyze_audio_tracks(&mkv);
                let attachments_to_keep = analyze_attachments(&mkv);

                if dry_run {
                    info!("Dry run was requested, moving on...");
                    trace!("------");
                    println!();

                    continue;
                }

                let file_buf = std::fs::read(entry.path()).unwrap_or_default();

                let mut ffmpeg_process = subprocess::Exec::cmd("ffmpeg")
                    // Feed the file using stdin.
                    .stdin(file_buf)
                    // Make ffmpeg less noisy.
                    .arg("-hide_banner")
                    .arg("-loglevel").arg("error")
                    .arg("-stats")
                    .arg("-y")
                    // Input file.
                    .arg("-i").arg("pipe:0")
                    // Grab only the first video stream. Skips cover pictures and horrible fuck-ups.
                    .arg("-map").arg("0:v:0")
                ;

                for (sub, _) in subs_to_keep {
                    ffmpeg_process = ffmpeg_process
                        .arg("-map").arg(format!("0:s:{sub}"))
                    ;
                }

                for (audio, track) in audio_to_keep.iter() {
                    ffmpeg_process = ffmpeg_process
                        .arg("-map").arg(format!("0:a:{audio}"))
                    ;

                    if LOSSLESS_AUDIO_CODECS.contains(&track.codec()) {
                        ffmpeg_process = ffmpeg_process
                            .arg(format!("-c:a:{audio}")).arg("libopus")
                            .arg("-ac").arg("2")
                        ;
                    }
                    else {
                        ffmpeg_process = ffmpeg_process
                            .arg(format!("-c:a:{audio}")).arg("copy")
                        ;
                    }
                }

                for (attachment, _) in attachments_to_keep {
                    ffmpeg_process = ffmpeg_process
                        .arg("-map").arg(format!("0:t:{attachment}"))
                    ;
                }

                if with_video_transcode {
                    ffmpeg_process = ffmpeg_process
                        .arg("-c:v").arg("libx265")
                        .arg("-x265-params").arg("log-level=error")
                        .arg("-crf").arg("19")
                        .arg("-preset").arg("medium")
                        .arg("-tune").arg("animation")
                    ;
                }
                else {
                    ffmpeg_process = ffmpeg_process
                        .arg("-c:v").arg("copy")
                    ;
                }

                ffmpeg_process = ffmpeg_process
                    .arg("-c:s").arg("copy")
                    .arg("-metadata").arg("title=")
                    .arg("-metadata:s:v").arg("title=")
                    .arg("-metadata:s:a").arg("title=")
                    .arg("-metadata:s:v").arg("language=und")
                ;

                let mut out_path = {
                    if let Some(intermediate_path) = intermediate_path.as_ref() {
                        intermediate_path.clone()
                    }
                    else {
                        output_path.clone()
                    }
                };

                out_path.push(&file_name);
                ffmpeg_process = ffmpeg_process.arg(out_path.as_os_str());

                let instant = Instant::now();

                match ffmpeg_process.capture() {
                    Ok(r) => {
                        if !r.exit_status.success() {
                            error!("ffmpeg didn't exit successfully, exiting...");
                            clean_exit = false;

                            break;
                        }

                        if let Some(intermediate) = intermediate_path.as_ref() {
                            let time_to_process = instant.elapsed();
                            let mut output_path = output_path.clone();
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

        if !dry_run {
            if let Some(intermediate) = intermediate_path {
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

fn configure_log() {
    if PathBuf::from("cruncher.log").exists() {
        if let Err(e) = fs::copy("cruncher.log", "cruncher.old") {
            println!("Error saving old log! {e}");
        }
    }

    let stdout_log = ConsoleAppender::builder()
        .target(Target::Stdout)
        .encoder(Box::new(PatternEncoder::new("[{d(%Y-%m-%d %H:%M:%S)}] [{l}]: {m}\n")))
        .build()
    ;

    let logfile = FileAppender::builder()
        .encoder(Box::new(PatternEncoder::new("[{d(%Y-%m-%d %H:%M:%S)}] [{l}]: {m}\n")))
        .append(false)
        .build("cruncher.log")
        .expect("Failed to initialize file logging")
    ;

    let log_config = Config::builder()
        .appenders([
            Appender::builder().filter(Box::new(ThresholdFilter::new(LevelFilter::Info))).build("console", Box::new(stdout_log)),
            Appender::builder().build("logfile", Box::new(logfile))
        ])
        .build(
            Root::builder()
                .appender("console")
                .appender("logfile")
                .build(LevelFilter::Trace)
        )
        .expect("Failed to configure logging")
    ;

    log4rs::init_config(log_config).expect("Failed to init logging.");
}

fn prepare_paths(input: &Path, output: &Path, intermediate: &Option<PathBuf>) {
    if !input.exists() {
        panic!("Input path doesn't exist!");
    }

    if !output.exists() {
        fs::create_dir_all(&output).expect("Output directory didn't exist, and couldn't be created!");
    }

    if let Some(intermediate) = intermediate.as_ref() {
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
        // Filter out Signs & Songs sub tracks.
        .filter(| (_, s) | {
            let name = s.stream_title();
            !name.contains("S&S") && !name.contains("Signs") && !name.contains("Songs")
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
            l.is_empty() || l == "jpn" || l == "und"
        })
        // Fallback filter.
        .filter(| (_, s) | {
            let stream_name = s.stream_title().to_lowercase();
            !stream_name.contains("eng") | !stream_name.contains("english")
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
            name.contains("ttf") || name.contains("otf")
        })
        .collect()
    ;

    preserved_attachments.sort_unstable_by_key(| (_, a) | a.stream_title());
    preserved_attachments.dedup_by_key(| (_, a) | a.stream_title());

    if preserved_attachments.len() < attachment_count {
        let mut attachment_list = String::new();

        for (i, (_, a)) in preserved_attachments.iter().enumerate() {
            attachment_list.push_str(&a.stream_title());
            
            if i != preserved_attachments.len() - 1 {
                attachment_list.push_str(", ");
            }
        }

        info!("  Keeping {}/{attachment_count} attachments: {attachment_list}", preserved_attachments.len());
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

const LOSSLESS_AUDIO_CODECS: [&str; 4] = [
    "dts",
    "flac",
    "truehd",
    "pcm_s24le"
];
