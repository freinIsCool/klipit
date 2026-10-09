use std::{
    fs,
    hash::{Hash, Hasher},
    io,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread::{self, JoinHandle},
    time::{SystemTime, UNIX_EPOCH},
};

use ez_ffmpeg::{Output, VideoWriter};

const FPS: usize = 60;
const OUTPUT_PATH: &str = "test.mp4";

pub(crate) enum RecorderCommand {
    Frame(Vec<u8>),
    SaveClip,
}

pub(crate) fn start_segmented_recording(
    segment_duration_seconds: usize,
) -> io::Result<(SyncSender<RecorderCommand>, JoinHandle<Result<(), String>>)> {
    if segment_duration_seconds == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "segment duration must be greater than zero",
        ));
    }

    cleanup_stale_segments()?;

    let frames_per_segment = segment_duration_seconds * FPS;
    let (sender, receiver) = mpsc::sync_channel(3);
    let worker = thread::Builder::new()
        .name("video-segment-writer".into())
        .spawn(move || record_segments(receiver, frames_per_segment))?;

    Ok((sender, worker))
}

fn record_segments(
    receiver: Receiver<RecorderCommand>,
    frames_per_segment: usize,
) -> Result<(), String> {
    let process_id = std::process::id();
    let mut completed_segments = Vec::new();
    let mut writer = None;
    let mut writer_path = None;
    let mut frames_in_segment = 0;
    let mut next_segment_index = 0;

    while let Ok(command) = receiver.recv() {
        let frame = match command {
            RecorderCommand::Frame(frame) => frame,
            RecorderCommand::SaveClip => {
                if let Some(writer) = writer.take() {
                    finish_segment(
                        writer,
                        writer_path.take().expect("segment path initialized"),
                        &mut completed_segments,
                    )?;
                    frames_in_segment = 0;
                }
                if !completed_segments.is_empty() {
                    let output_path = next_clip_path()?;
                    match concatenate_segments(&completed_segments, &output_path, process_id) {
                        Ok(()) => println!("Saved rolling clip to {}", output_path.display()),
                        Err(error) => eprintln!("Failed to compile rolling clip: {error}"),
                    }
                } else {
                    eprintln!("No completed frames are available to save yet.");
                }
                continue;
            }
        };

        if writer.is_none() {
            if completed_segments.len() == SEGMENT_COUNT {
                let oldest = completed_segments.remove(0);
                fs::remove_file(&oldest)
                    .map_err(|error| format!("Failed to remove expired video segment: {error}"))?;
            }

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

    concatenate_segments(&completed_segments, Path::new(OUTPUT_PATH), process_id)?;

    for path in completed_segments {
        fs::remove_file(&path).map_err(|error| {
            format!(
                "Failed to remove compiled segment {}: {error}",
                path.display()
            )
        })?;
    }

    Ok(())
}

const SEGMENT_COUNT: usize = 30;

fn cleanup_stale_segments() -> io::Result<()> {
    for entry in fs::read_dir(".")? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(stem) = name
            .strip_prefix("test_segment_")
            .and_then(|name| name.strip_suffix(".mp4"))
        else {
            continue;
        };
        let Some((pid, index)) = stem.split_once('_') else {
            continue;
        };
        if pid.parse::<u32>().is_err() || index.parse::<u64>().is_err() {
            continue;
        }

        let process_path = Path::new("/proc").join(pid);
        if !process_path.exists() {
            fs::remove_file(entry.path())?;
        }
    }

    Ok(())
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
    let output = Output::from(path.to_string_lossy().into_owned())
        .set_video_codec("h264_nvenc")
        .set_video_codec_opts(vec![
            ("preset", "p7"),
            ("rc", "vbr"),
            ("cq", "18"),
            ("b", "0"),
            ("spatial_aq", "1"),
            ("temporal_aq", "1"),
        ]);

    VideoWriter::builder(1920, 1080)
        .pixel_format("bgra")
        .fps(FPS as i32, 1)
        .open(output)
        .map_err(|error| format!("Failed to create video segment: {error}"))
}

fn concatenate_segments(
    segment_paths: &[PathBuf],
    output_path: &Path,
    process_id: u32,
) -> Result<(), String> {
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
        .arg(output_path)
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

    println!(
        "Compiled {} video segments into {}",
        segment_paths.len(),
        output_path.display()
    );
    Ok(())
}

fn next_clip_path() -> Result<PathBuf, String> {
    static CLIP_COUNTER: AtomicU64 = AtomicU64::new(0);

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("System clock is before Unix epoch: {error}"))?;
    let (year, month, day) = civil_date_from_days((now.as_secs() / 86_400) as i64);
    let seconds_today = now.as_secs() % 86_400;
    let hour = seconds_today / 3_600;
    let minute = (seconds_today % 3_600) / 60;
    let second = seconds_today % 60;

    loop {
        let counter = CLIP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        now.as_nanos().hash(&mut hasher);
        std::process::id().hash(&mut hasher);
        counter.hash(&mut hasher);
        let random = hasher.finish();
        let path = PathBuf::from(format!(
            "clip_{year:04}_{month:02}_{day:02}_{hour:02}_{minute:02}_{second:02}_{random:016x}.mp4"
        ));
        if !path.exists() {
            return Ok(path);
        }
    }
}

fn civil_date_from_days(days_since_epoch: i64) -> (i64, u32, u32) {
    let days = days_since_epoch + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);

    (year, month as u32, day as u32)
}

#[cfg(test)]
mod tests {
    use super::civil_date_from_days;

    #[test]
    fn converts_unix_epoch_to_civil_date() {
        assert_eq!(civil_date_from_days(0), (1970, 1, 1));
    }

    #[test]
    fn handles_leap_day() {
        assert_eq!(civil_date_from_days(11_016), (2000, 2, 29));
    }
}
