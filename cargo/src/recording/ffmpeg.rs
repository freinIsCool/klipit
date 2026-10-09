use std::{
    process::Command,
    thread,
    time::Duration,
};

use ez_ffmpeg::{Output, VideoWriter};

pub fn convert() -> Result<(), Box<dyn std::error::Error>> {
    let output = Output::from("test.mp4")
        .set_video_codec("libx264");

    let mut writer =
        VideoWriter::builder(1920, 1080)
            .fps(60, 1)
            .open(output)?;

    // Push raw frames here.
    // Each frame must contain tightly packed pixel data.

    writer.finish()?;

    Ok(())
}