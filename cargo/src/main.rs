#[path = "recording/recording.rs"]
mod recording;

use recording::screencast;

use std::{path, thread, time::Duration};

fn main() {
    println!("hello");

    if let Err(err) = tokio::runtime::Runtime::new()
        .expect("failed to create tokio runtime")
        .block_on(screencast::run())
    {
        eprintln!("screencast failed: {err}");
        std::process::exit(1);
    }
    
}