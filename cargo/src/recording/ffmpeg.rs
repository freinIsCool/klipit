use std::{
    fs, io,
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc::{self, Receiver, SyncSender},
    thread::{self, JoinHandle},
};

use ez_ffmpeg::{Output, VideoWriter};

const FPS: usize = 60;
const OUTPUT_PATH: &str = "test.mp4";

pub fn start_segmented_recording(
    segment_duration_seconds: usize,
) -> io::Result<(SyncSender<Vec<u8>>, JoinHandle<Result<(), String>>)> {
    if segment_duration_seconds == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "segment duration must be greater than zero",
        ));
    }

    let frames_per_segment = segment_duration_seconds * FPS;
    let (sender, receiver) = mpsc::sync_channel(2);
    let worker = thread::Builder::new()
        .name("video-segment-writer".into())
        .spawn(move || record_segments(receiver, frames_per_segment))?;

    Ok((sender, worker))
}

fn record_segments(receiver: Receiver<Vec<u8>>, frames_per_segment: usize) -> Result<(), String> {
    let process_id = std::process::id();
    let mut completed_segments = Vec::new();
    let mut writer = None;
    let mut writer_path = None;
    let mut frames_in_segment = 0;
    let mut next_segment_index = 0;

    while let Ok(frame) = receiver.recv() {
        if writer.is_none() {
            let segment_path = PathBuf::from(format!(
                "test_segment_{process_id}_{next_segment_index:06}.mp4"
            ));
            next_segment_index += 1;
            writer = Some(create_writer(&segment_path)?);
            writer_path = Some(segment_path);
        }

        writer
            .as_mut()
            .expect("writer initialized for current segment")
            .write_owned(frame)
            .map_err(|error| format!("Failed to queue video frame: {error}"))?;
        frames_in_segment += 1;

        if frames_in_segment == frames_per_segment {
            finish_segment(
                writer
                    .take()
                    .expect("writer initialized for current segment"),
                writer_path.take().expect("segment path initialized"),
                &mut completed_segments,
            )?;
            frames_in_segment = 0;
        }
    }

    if let Some(writer) = writer {
        finish_segment(
            writer,
            writer_path.expect("segment path initialized"),
            &mut completed_segments,
        )?;
    }

    if completed_segments.is_empty() {
        return Err("No video frames were captured".into());
    }

    concatenate_segments(&completed_segments, process_id)
}

fn finish_segment(
    writer: VideoWriter,
    path: PathBuf,
    completed_segments: &mut Vec<PathBuf>,
) -> Result<(), String> {
    writer
        .finish()
        .map_err(|error| format!("Failed to finalize video segment: {error}"))?;
    completed_segments.push(path);

    Ok(())
}

fn create_writer(path: &Path) -> Result<VideoWriter, String> {
    let output = Output::from(path.to_string_lossy().into_owned()).set_video_codec("h264_nvenc");

    VideoWriter::builder(1920, 1080)
        .pixel_format("bgra")
        .fps(FPS as i32, 1)
        .open(output)
        .map_err(|error| format!("Failed to create video segment: {error}"))
}

fn concatenate_segments(segment_paths: &[PathBuf], process_id: u32) -> Result<(), String> {
    let manifest_path = PathBuf::from(format!("test_concat_{process_id}.txt"));
    let manifest = segment_paths
        .iter()
        .map(|path| {
            let path = path
                .file_name()
                .expect("segment filename")
                .to_string_lossy();
            format!("file '{path}'\n")
        })
        .collect::<String>();
    fs::write(&manifest_path, manifest)
        .map_err(|error| format!("Failed to write concat manifest: {error}"))?;

    let result = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
        ])
        .arg(&manifest_path)
        .args(["-c", "copy", "-movflags", "+faststart"])
        .arg(OUTPUT_PATH)
        .output()
        .map_err(|error| format!("Failed to launch ffmpeg to concatenate segments: {error}"));

    let cleanup_result = fs::remove_file(&manifest_path)
        .map_err(|error| format!("Failed to remove concat manifest: {error}"));
    let output = result?;
    cleanup_result?;

    if !output.status.success() {
        return Err(format!(
            "Failed to concatenate video segments: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    Ok(())
}
