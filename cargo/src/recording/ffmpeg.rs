use std::{
    collections::VecDeque,
    fs,
    hash::{Hash, Hasher},
    io::{self, Write},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender, SyncSender},
    },
    thread::{self, JoinHandle},
    time::{SystemTime, UNIX_EPOCH},
};

const FPS: usize = 60;

pub(crate) enum RecorderCommand {
    Frame(Vec<u8>),
    Audio(Vec<u8>),
    SaveClip,
}

struct RecordedSegment {
    video_path: PathBuf,
    audio: Vec<u8>,
}

pub struct ClipSaveJob {
    segments: Vec<Arc<RecordedSegment>>,
    output_path: PathBuf,
}

struct SegmentMuxer {
    child: Child,
    stdin: ChildStdin,
}

impl Drop for RecordedSegment {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_file(&self.video_path) {
            eprintln!(
                "Failed to remove video segment {}: {error}",
                self.video_path.display()
            );
        }
    }
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

    let clips_dir: PathBuf = clips_directory().map_err(io::Error::other)?;
    fs::create_dir_all(&clips_dir)?;
    cleanup_stale_segments(&clips_dir)?;

    let frames_per_segment = segment_duration_seconds * FPS;
    let (sender, receiver) = mpsc::sync_channel(3);
    let worker = thread::Builder::new()
        .name("video-segment-writer".into())
        .spawn(move || {
            record_segments(
                receiver,
                segment_duration_seconds,
                frames_per_segment,
                clips_dir,
            )
        })?;

    Ok((sender, worker))
}

fn record_segments(
    receiver: Receiver<RecorderCommand>,
    segment_duration_seconds: usize,
    frames_per_segment: usize,
    clips_dir: PathBuf,
) -> Result<(), String> {
    let (save_sender, save_receiver) = mpsc::channel();
    let save_worker = thread::Builder::new()
        .name("clip-saver".into())
        .spawn(move || save_clips(save_receiver))
        .map_err(|error| format!("Failed to start clip saver: {error}"))?;
    let recording_result = record_segments_inner(
        receiver,
        segment_duration_seconds,
        frames_per_segment,
        &clips_dir,
        &save_sender,
    );
    drop(save_sender);
    let save_result = save_worker
        .join()
        .map_err(|_| "Clip saver thread panicked".to_owned())?;
    recording_result?;
    save_result
}

fn record_segments_inner(
    receiver: Receiver<RecorderCommand>,
    segment_duration_seconds: usize,
    frames_per_segment: usize,
    clips_dir: &Path,
    save_sender: &Sender<ClipSaveJob>,
) -> Result<(), String> {
    let process_id = std::process::id();
    let mut completed_segments: Vec<Arc<RecordedSegment>> = Vec::new();
    let mut pending_segments = VecDeque::new();
    let mut segment_muxer = start_segment_muxer(process_id, segment_duration_seconds, clips_dir)?;
    let mut frames_written = 0;
    let mut audio_in_segment = Vec::new();

    let capture_result = (|| -> Result<(), String> {
        while let Ok(command) = receiver.recv() {
            let frame = match command {
                RecorderCommand::Frame(frame) => frame,
                RecorderCommand::Audio(audio) => {
                    audio_in_segment.extend_from_slice(&audio);
                    promote_finalized_segments(
                        &mut pending_segments,
                        &mut completed_segments,
                        process_id,
                        clips_dir,
                    );
                    continue;
                }
                RecorderCommand::SaveClip => {
                    promote_finalized_segments(
                        &mut pending_segments,
                        &mut completed_segments,
                        process_id,
                        clips_dir,
                    );
                    if completed_segments.len() == SEGMENT_COUNT {
                        match next_clip_path(clips_dir) {
                            Ok(output_path) => {
                                if let Err(error) = save_sender.send(ClipSaveJob {
                                    segments: completed_segments.clone(),
                                    output_path: output_path.clone(),
                                }) {
                                    eprintln!("Failed to queue rolling clip: {error}");
                                } else {
                                    println!("Queued rolling clip for {}", output_path.display());
                                }
                            }
                            Err(error) => {
                                eprintln!("Failed to choose clip output path: {error}")
                            }
                        }
                    } else {
                        eprintln!("A 10-second clip is not available yet.");
                    }
                    continue;
                }
            };

            if frames_written > 0 && frames_written % frames_per_segment == 0 {
                let completed_index = frames_written / frames_per_segment - 1;
                pending_segments
                    .push_back((completed_index, std::mem::take(&mut audio_in_segment)));
            }

            segment_muxer
                .stdin
                .write_all(&frame)
                .map_err(|error| format!("Failed to send video frame to FFmpeg: {error}"))?;
            frames_written += 1;
            promote_finalized_segments(
                &mut pending_segments,
                &mut completed_segments,
                process_id,
                clips_dir,
            );
            while completed_segments.len() > SEGMENT_COUNT {
                completed_segments.remove(0);
            }
        }
        Ok(())
    })();

    drop(segment_muxer.stdin);
    let status = segment_muxer
        .child
        .wait()
        .map_err(|error| format!("Failed to wait for FFmpeg segment muxer: {error}"))?;
    if !status.success() {
        return Err(format!(
            "FFmpeg segment muxer exited unsuccessfully: {status}"
        ));
    }
    capture_result?;

    while let Some((index, audio)) = pending_segments.pop_front() {
        completed_segments.push(Arc::new(RecordedSegment {
            video_path: segment_path(clips_dir, process_id, index),
            audio,
        }));
    }
    if frames_written > 0 && frames_written % frames_per_segment == 0 {
        let index = frames_written / frames_per_segment - 1;
        completed_segments.push(Arc::new(RecordedSegment {
            video_path: segment_path(clips_dir, process_id, index),
            audio: audio_in_segment,
        }));
    } else {
        let incomplete_path =
            segment_path(clips_dir, process_id, frames_written / frames_per_segment);
        if incomplete_path.exists() {
            fs::remove_file(&incomplete_path).map_err(|error| {
                format!(
                    "Failed to remove incomplete video segment {}: {error}",
                    incomplete_path.display()
                )
            })?;
        }
    }
    while completed_segments.len() > SEGMENT_COUNT {
        completed_segments.remove(0);
    }

    Ok(())
}

pub fn save_clips(receiver: Receiver<ClipSaveJob>) -> Result<(), String> {
    let process_id = std::process::id();
    for job in receiver {
        match compile_segments_with_audio(&job.segments, &job.output_path, process_id) {
            Ok(()) => println!("Saved rolling clip to {}", job.output_path.display()),
            Err(error) => eprintln!("Failed to compile rolling clip: {error}"),
        }
    }

    Ok(())
}

const SEGMENT_COUNT: usize = 10;

pub fn cleanup_stale_segments(clips_dir: &Path) -> io::Result<()> {
    for entry in fs::read_dir(clips_dir)? {
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

/// Removes temporary segments created by this process during normal shutdown.
pub fn cleanup_current_process_segments(clips_dir: &Path) -> io::Result<()> {
    let process_id = std::process::id();
    let prefix = format!("test_segment_{process_id}_");

    for entry in fs::read_dir(clips_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };

        if name.starts_with(&prefix) && name.ends_with(".mp4") {
            fs::remove_file(entry.path())?;
        }
    }

    Ok(())
}

fn segment_path(clips_dir: &Path, process_id: u32, index: usize) -> PathBuf {
    clips_dir.join(format!("test_segment_{process_id}_{index:06}.mp4"))
}

fn promote_finalized_segments(
    pending_segments: &mut VecDeque<(usize, Vec<u8>)>,
    completed_segments: &mut Vec<Arc<RecordedSegment>>,
    process_id: u32,
    clips_dir: &Path,
) {
    while let Some((index, _)) = pending_segments.front()
        && segment_path(clips_dir, process_id, index + 1).exists()
    {
        let (index, audio) = pending_segments
            .pop_front()
            .expect("front segment was checked");
        completed_segments.push(Arc::new(RecordedSegment {
            video_path: segment_path(clips_dir, process_id, index),
            audio,
        }));
    }
    while completed_segments.len() > SEGMENT_COUNT {
        completed_segments.remove(0);
    }
}

fn start_segment_muxer(
    process_id: u32,
    segment_duration_seconds: usize,
    clips_dir: &Path,
) -> Result<SegmentMuxer, String> {
    let duration = segment_duration_seconds.to_string();
    let keyframe_interval = (FPS * segment_duration_seconds).to_string();
    let force_keyframes = format!("expr:gte(t,n_forced*{duration})");
    let segment_pattern = clips_dir
        .join(format!("test_segment_{process_id}_%06d.mp4"))
        .to_string_lossy()
        .into_owned();
    let mut child = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "rawvideo",
            "-pixel_format",
            "bgra",
            "-video_size",
            "1920x1080",
            "-framerate",
            "60",
            "-i",
            "pipe:0",
            "-an",
            "-c:v",
            "h264_nvenc",
            "-preset",
            "p7",
            "-rc",
            "vbr",
            "-cq",
            "18",
            "-b:v",
            "0",
            "-spatial-aq",
            "1",
            "-temporal-aq",
            "1",
            "-g",
            &keyframe_interval,
            "-force_key_frames",
            &force_keyframes,
            "-f",
            "segment",
            "-segment_time",
            &duration,
            "-reset_timestamps",
            "1",
            "-segment_format",
            "mp4",
        ])
        .arg(segment_pattern)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .process_group(0)
        .spawn()
        .map_err(|error| format!("Failed to start FFmpeg segment muxer: {error}"))?;
    let Some(stdin) = child.stdin.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("FFmpeg segment muxer stdin was not available".to_owned());
    };

    Ok(SegmentMuxer { child, stdin })
}

fn concatenate_segments(
    segment_paths: &[PathBuf],
    clips_dir: &Path,
    output_path: &Path,
    process_id: u32,
    compile_id: u64,
) -> Result<(), String> {
    let manifest_path = clips_dir.join(format!("test_concat_{process_id}_{compile_id}.txt"));
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

fn compile_segments_with_audio(
    segments: &[Arc<RecordedSegment>],
    output_path: &Path,
    process_id: u32,
) -> Result<(), String> {
    static COMPILE_COUNTER: AtomicU64 = AtomicU64::new(0);
    let compile_id = COMPILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let clips_dir = output_path
        .parent()
        .ok_or_else(|| "Clip output path has no parent directory".to_owned())?;
    let video_path = clips_dir.join(format!("test_concat_video_{process_id}_{compile_id}.mp4"));
    let segment_paths = segments
        .iter()
        .map(|segment| segment.video_path.clone())
        .collect::<Vec<_>>();
    concatenate_segments(
        &segment_paths,
        clips_dir,
        &video_path,
        process_id,
        compile_id,
    )?;

    let audio = segments
        .iter()
        .flat_map(|segment| segment.audio.iter().copied())
        .collect::<Vec<_>>();
    let mux_result = mux_aac_audio(&video_path, &audio, output_path);
    let cleanup_result = fs::remove_file(&video_path)
        .map_err(|error| format!("Failed to remove temporary video: {error}"));
    mux_result?;
    cleanup_result?;
    Ok(())
}

fn mux_aac_audio(video_path: &Path, audio: &[u8], output_path: &Path) -> Result<(), String> {
    if audio.is_empty() {
        return Err("No desktop audio samples were captured for this clip".to_owned());
    }

    let sample_rate = crate::recording::audio::SAMPLE_RATE.to_string();
    let channels = crate::recording::audio::CHANNELS.to_string();
    let mut child = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(video_path)
        .args([
            "-f",
            "s16le",
            "-ar",
            &sample_rate,
            "-ac",
            &channels,
            "-i",
            "pipe:0",
            "-map",
            "0:v:0",
            "-map",
            "1:a:0",
            "-c:v",
            "copy",
            "-c:a",
            "aac",
            "-b:a",
            "192k",
            "-shortest",
            "-movflags",
            "+faststart",
        ])
        .arg(output_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Failed to launch ffmpeg to encode audio: {error}"))?;

    let write_result = child
        .stdin
        .take()
        .expect("ffmpeg stdin was configured as a pipe")
        .write_all(audio);
    if let Err(error) = write_result {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!("Failed to send audio samples to ffmpeg: {error}"));
    }

    let output = child
        .wait_with_output()
        .map_err(|error| format!("Failed to wait for ffmpeg audio muxing: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Failed to mux AAC audio into video: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    Ok(())
}

pub fn clips_directory() -> Result<PathBuf, String> {
    let videos_dir = Command::new("xdg-user-dir")
        .arg("VIDEOS")
        .output()
        .map_err(|error| format!("Failed to locate the Videos directory: {error}"))?;
    if !videos_dir.status.success() {
        return Err(format!(
            "Failed to locate the Videos directory: {}",
            String::from_utf8_lossy(&videos_dir.stderr).trim()
        ));
    }
    let videos_dir = String::from_utf8(videos_dir.stdout)
        .map_err(|error| format!("Videos directory path is not valid UTF-8: {error}"))?;
    let videos_dir = videos_dir.trim();
    if videos_dir.is_empty() {
        return Err("xdg-user-dir returned an empty Videos directory path".to_owned());
    }

    let videos_dir = PathBuf::from(videos_dir);
    let videos_dir = if videos_dir.is_absolute() {
        videos_dir
    } else {
        let home_dir = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| "HOME is not set; cannot resolve the Videos directory".to_owned())?;
        home_dir.join(videos_dir)
    };
    let clips_dir = videos_dir.join("clips");
    Ok(clips_dir)
}

fn next_clip_path(clips_dir: &Path) -> Result<PathBuf, String> {
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
        let path = clips_dir.join(format!(
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
