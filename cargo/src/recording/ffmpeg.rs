use std::{
    process::Command,
    thread,
    time::Duration,
};

pub fn main() -> std::io::Result<()> {
    let mut ffmpeg = Command::new("ffmpeg")
        .args([
            "-h"
        ])
        .spawn()?;

    thread::sleep(Duration::from_secs(5));

    ffmpeg.kill()?;
    ffmpeg.wait()?;

    Ok(())
}