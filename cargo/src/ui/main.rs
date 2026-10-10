#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // hide console window on Windows in release
#![expect(rustdoc::missing_crate_level_docs)] // it's an example

use crate::recording::ffmpeg::{
    cleanup_current_process_segments, cleanup_stale_segments, clips_directory,
};

use eframe::egui;
use egui::Color32;
use std::sync::mpsc::Sender;

pub fn main(shutdown_sender: Sender<()>, save_clip_sender: Sender<()>) -> eframe::Result {
    env_logger::init(); // Log to stderr (if you run with `RUST_LOG=debug`).

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_position([600.0, 0.0])
            .with_inner_size([320.0, 1031.0])
            .with_decorations(false)
            .with_resizable(false),
        ..Default::default()
    };
    eframe::run_native(
        "Klipit",
        options,
        Box::new(|cc| {
            // This gives us image support:
            egui_extras::install_image_loaders(&cc.egui_ctx);

            Ok(Box::new(MyApp::new(shutdown_sender, save_clip_sender)))
        }),
    )
}

struct MyApp {
    allowed_to_close: bool,
    shutdown_sender: Sender<()>,
    save_clip_sender: Sender<()>,
}

impl MyApp {
    fn new(shutdown_sender: Sender<()>, save_clip_sender: Sender<()>) -> Self {
        Self {
            allowed_to_close: true,
            shutdown_sender,
            save_clip_sender,
        }
    }
}

impl eframe::App for MyApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("My egui Application");
            ui.horizontal(|ui| {});
            if ui.button("save clip").clicked() {
                if let Err(error) = self.save_clip_sender.send(()) {
                    eprintln!("Video recorder stopped; cannot save a clip: {error}");
                }
            }
        });
    }

    fn on_exit(&mut self) {
        let _ = self.shutdown_sender.send(());

        let clips_dir = match clips_directory() {
            Ok(clips_dir) => clips_dir,
            Err(error) => {
                eprintln!("Failed to locate temporary segment directory during shutdown: {error}");
                return;
            }
        };

        if let Err(error) = cleanup_stale_segments(&clips_dir) {
            eprintln!(
                "Failed to clean stale recording segments in {}: {error}",
                clips_dir.display()
            );
        }

        if let Err(error) = cleanup_current_process_segments(&clips_dir) {
            eprintln!(
                "Failed to clean this process's recording segments in {}: {error}",
                clips_dir.display()
            );
        }
    }
}
