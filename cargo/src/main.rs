#[path = "recording/recording.rs"]
mod recording;

#[path = "ui/ui.rs"]
mod ui;

use recording::screencast;

use std::{sync::mpsc, thread};

fn main() {
    // Wait for the portal's screen picker before showing the main window.
    let (ready_sender, ready_receiver) = mpsc::channel();
    let (shutdown_sender, shutdown_receiver) = mpsc::channel();
    thread::Builder::new()
        // pipewire-rs requires its local main loop to be created on a thread
        // named `main`. The UI still stays on the actual process main thread.
        .name("main".into())
        .spawn(move || {
            let result = tokio::runtime::Runtime::new()
                .expect("failed to create tokio runtime")
                .block_on(screencast::run(ready_sender));

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
    if let Err(err) = ui::main::run_app(shutdown_receiver) {
        eprintln!("UI failed: {err}");
        std::process::exit(1);
    }
}
