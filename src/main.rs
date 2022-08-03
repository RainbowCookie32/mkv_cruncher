use std::fs;
use std::fs::File;
use std::path::{Path, PathBuf};

use log::*;
use log4rs::Config;
use log4rs::config::{Appender, Root};
use log4rs::append::file::FileAppender;
use log4rs::append::console::{ConsoleAppender, Target};
use log4rs::encode::pattern::PatternEncoder;

use clap::Parser;
use walkdir::WalkDir;
use matroska::{Matroska, Track, Language, Attachment, Settings};

#[derive(Parser, Debug)]
#[clap(author, about)]
struct Args {
    #[clap(short = 'i', long)]
    input_dir: PathBuf,
    #[clap(short = 'o', long)]
    output_dir: PathBuf,
    #[clap(long)]
    intermediate_dir: Option<PathBuf>,

    #[clap(long)]
    transcode_video: bool
}

fn main() {
    let args = Args::parse();

    let input_path = args.input_dir;
    let output_path = args.output_dir;
    let intermediate_path: Option<PathBuf> = args.intermediate_dir;

    let with_video_transcode = args.transcode_video;

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
            if let Ok(file) = File::open(entry.path()) {
                info!("Currently processing: {file_name}");

                if let Ok(mkv) = Matroska::open(file) {
                    let subs_to_keep = analyze_sub_tracks(&mkv);
                    let audio_to_keep = analyze_audio_tracks(&mkv);
                    let attachments_to_keep = analyze_attachments(&mkv);

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

                        if LOSSLESS_AUDIO_CODECS.contains(&track.codec_id.as_str()) {
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

                    match ffmpeg_process.capture() {
                        Ok(r) => {
                            if !r.exit_status.success() {
                                error!("ffmpeg didn't exit successfully, exiting...");
                                clean_exit = false;

                                break;
                            }

                            if let Some(intermediate) = intermediate_path.as_ref() {
                                let mut output_path = output_path.clone();
                                let mut result_path = intermediate.clone();

                                output_path.push(entry.file_name());
                                result_path.push(entry.file_name());

                                info!("ffmpeg exited successfully, copying result to output dir...");

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

                    println!();
                }
            }
        }
    }

    if !clean_exit {
        error!("Exiting because of an error...");

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
            Appender::builder().build("console", Box::new(stdout_log)),
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

fn analyze_sub_tracks(mkv: &Matroska) -> Vec<(usize, &Track)> {
    let track_count = mkv.subtitle_tracks().count();

    let mut preserved_tracks: Vec<(usize, &Track)> = mkv.subtitle_tracks()
        .enumerate()
        .map(| (i, s) | (i, s))
        .collect()
    ;

    preserved_tracks.sort_unstable_by_key(|(_, s)| {
        if let Some(name) = s.name.as_ref() {
            name.clone()
        }
        else {
            String::new()
        }
    });

    preserved_tracks.dedup_by_key(| (_, s) | {
        if let Some(name) = s.name.as_ref() {
            name.clone()
        }
        else {
            String::new()
        }
    });

    let has_ass = preserved_tracks.iter()
        .filter(|(_, s)| s.codec_id == ASS_CODEC)
        .count() > 0
    ;

    preserved_tracks = preserved_tracks
        .into_iter()
        // Filter out Signs & Songs sub tracks.
        .filter(| (_, s) | {
            let name = s.name.clone().unwrap_or_default();
            !name.contains("S&S") && !name.contains("Signs") && !name.contains("Songs")
        })
        // Filter out unused languages.
        .filter(| (_, s) | {
            if let Some(l) = s.language.as_ref() {
                match l {
                    Language::IETF(l) => {
                        OK_SUB_LANGS_IETF.contains(&l.as_str())
                    }
                    Language::ISO639(l) => {
                        OK_SUB_LANGS.contains(&l.as_str())
                    }
                }
            }
            else {
                true
            }
        })
        // Filter out PGS and other formats if we have ASS subs.
        .filter(| (_, s) | {
            if has_ass {
                s.codec_id == ASS_CODEC
            }
            else {
                true
            }
        })
        .collect()
    ;

    if preserved_tracks.len() < track_count {
        info!("  Keeping {}/{} subs.", preserved_tracks.len(), track_count);

        for (_, s) in preserved_tracks.iter() {
            let track_name = s.name
                .clone()
                .unwrap_or_else(|| String::from("Untitled track"))
            ;

            info!("      {track_name} ({})", s.codec_id);
        }
    }
    else {
        info!("  Keeping all subs ({track_count})");
    }

    preserved_tracks
}

fn analyze_audio_tracks(mkv: &Matroska) -> Vec<(usize, &Track)> {
    let track_count = mkv.audio_tracks().count();

    let mut preserved_tracks: Vec<(usize, &Track)> = mkv.audio_tracks()
        .enumerate()
        // Filter non-japanese, leave undefined just in case.
        .filter(| (_, t) | {
            if let Some(l) = t.language.as_ref() {
                match l {
                    Language::IETF(l) => {
                        l == "ja"
                    }
                    Language::ISO639(l) => {
                        l == "jpn" || l == "und"
                    }
                }
            }
            else {
                true
            }
        })
        .collect()
    ;

    // Try to nuke potential 5.1 tracks if we still have more than one track.
    if preserved_tracks.len() > 1 {
        let jpn_stereo: Vec<(usize, &Track)> = preserved_tracks.clone().into_iter()
            .filter( | (_, t) | {
                if let Settings::Audio(settings) = &t.settings {
                    settings.channels == 2
                }
                else {
                    unreachable!()
                }
            })
            .collect()
        ;

        if !jpn_stereo.is_empty() {
            preserved_tracks = jpn_stereo;
        }
    }

    if preserved_tracks.len() < track_count {
        info!("  Keeping {}/{track_count} audio tracks.", preserved_tracks.len());

        for (_, t) in preserved_tracks.iter() {
            let track_name = t.name
                .clone()
                .unwrap_or_else(|| String::from("Untitled track"))
            ;

            info!("      {track_name} ({})", t.codec_id);
        }
    }
    else {
        info!("  Keeping all audio tracks ({track_count})");
    }

    preserved_tracks
}

fn analyze_attachments(mkv: &Matroska) -> Vec<(usize, &Attachment)> {
    let attachment_count = mkv.attachments.len();
    let mut preserved_attachments: Vec<(usize, &Attachment)> = mkv.attachments
        .iter()
        .enumerate()
        .filter(| (_, a) | {
            let name = a.name.to_lowercase();
            name.contains("ttf") || name.contains("otf")
        })
        .collect()
    ;

    preserved_attachments.sort_unstable_by_key(| (_, a) | a.name.clone());
    preserved_attachments.dedup_by_key(| (_, a) | a.name.clone());

    if preserved_attachments.len() < attachment_count {
        let mut attachment_list = String::new();

        for (i, (_, s)) in preserved_attachments.iter().enumerate() {
            attachment_list.push_str(&s.name);
            
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

const ASS_CODEC: &str = "S_TEXT/ASS";

const OK_SUB_LANGS: [&str; 5] = [
    "eng",
    "enm",
    "jpn",
    "spa",
    "und"
];

const OK_SUB_LANGS_IETF: [&str; 3] = [
    "en",
    "ja",
    "es"
];

const LOSSLESS_AUDIO_CODECS: [&str; 4] = [
    "A_DTS",
    "A_FLAC",
    "A_TRUEHD",
    "A_PCM/INT/LIT"
];
