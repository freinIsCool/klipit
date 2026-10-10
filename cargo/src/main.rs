#[path = "recording/recording.rs"]
mod recording;

#[path = "ui/ui.rs"]
mod ui;

use recording::ffmpeg::{
    cleanup_current_process_segments, cleanup_stale_segments, clips_directory,
};
use recording::screencast;

use std::{sync::mpsc, thread};

fn main() {
    // Wait for the portal's screen picker before showing the main window.
    let (ready_sender, ready_receiver) = mpsc::channel();
    let (shutdown_sender, shutdown_receiver) = mpsc::channel();
    let (capture_shutdown_sender, capture_shutdown_receiver) = mpsc::channel();
    let capture_thread = thread::Builder::new()
        // pipewire-rs requires its local main loop to be created on a thread
        // named `main`. The UI still stays on the actual process main thread.
        .name("main".into())
        .spawn(move || {
            let result = tokio::runtime::Runtime::new()
                .expect("failed to create tokio runtime")
                .block_on(screencast::run(ready_sender, capture_shutdown_receiver));

            if let Err(err) = result {
                eprintln!("screencast failed: {err}");
            }

            // Closing PipeWire (including via Ctrl+C) must also release the
            // eframe event loop running on the process main thread.
            let _ = shutdown_sender.send(());
        })
        .expect("failed to start screencast thread");

    match ready_receiver.recv() {
        Ok(Ok(())) => {}
        Ok(Err(err)) => {
            eprintln!("screencast setup failed: {err}");
            std::process::exit(1);
        }
        Err(err) => {
            eprintln!("screencast setup stopped unexpectedly: {err}");
            std::process::exit(1);
        }
    }

    // eframe must own the main thread to create and run its native window.
    let ui_result = ui::main::main(capture_shutdown_sender.clone());
    // `on_exit` sends this during a normal close. Send it here as well so a
    // failure while creating the UI cannot leave the capture thread running.
    let _ = capture_shutdown_sender.send(());

    if capture_thread.join().is_err() {
        eprintln!("Capture thread panicked during shutdown.");
    }

    if let Ok(clips_dir) = clips_directory() {
        if let Err(error) = cleanup_stale_segments(&clips_dir) {
            eprintln!("Failed to clean stale recording segments: {error}");
        }
        if let Err(error) = cleanup_current_process_segments(&clips_dir) {
            eprintln!("Failed to clean recording segments: {error}");
        }
    }

    if let Err(err) = ui_result {
        eprintln!("UI failed: {err}");
        std::process::exit(1);
    }
}
