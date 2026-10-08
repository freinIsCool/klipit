#[path = "recording/ffmpeg.rs"]
mod ffmpeg;

#[path ="recording/screencast.rs"]
mod screencast;

use std::{thread, time::Duration};

fn main() {
    println!("hello");

    if let Err(err) = ffmpeg::main() {
        eprintln!("ffmpeg failed: {err}");
        std::process::exit(1);
    }
}