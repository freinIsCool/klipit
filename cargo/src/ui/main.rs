#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // hide console window on Windows in release
#![expect(rustdoc::missing_crate_level_docs)] // it's an example

use eframe::egui;
use std::{
    sync::mpsc::{Receiver, TryRecvError},
    time::Duration,
};

pub fn run_app(shutdown_events: Receiver<()>) -> eframe::Result {
    env_logger::init(); // Log to stderr (if you run with `RUST_LOG=debug`).

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([320.0, 240.0]),
        ..Default::default()
    };

    // Our application state:
    let mut name = "Arthur".to_owned();
    let mut age = 42;
    let mut shutdown_requested = false;

    eframe::run_ui_native("My egui App", options, move |ui, _frame| {
        if !shutdown_requested {
            match shutdown_events.try_recv() {
                Ok(()) | Err(TryRecvError::Disconnected) => shutdown_requested = true,
                Err(TryRecvError::Empty) => {}
            }
        }

        if shutdown_requested {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        } else if ui.ctx().input(|input| input.viewport().close_requested()) {
            // Keep the recorder alive when the user closes the window. A later
            // shortcut or tray action can make this viewport visible again.
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }

        // Keep checking for a Ctrl+C-driven recorder shutdown even when the
        // user has hidden the window.
        ui.ctx().request_repaint_after(Duration::from_millis(100));

        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("My egui Application");
            ui.horizontal(|ui| {
                let name_label = ui.label("Your name: ");
                ui.text_edit_singleline(&mut name)
                    .labelled_by(name_label.id);
            });
            ui.add(egui::Slider::new(&mut age, 0..=120).text("age"));
            if ui.button("Increment").clicked() {
                age += 1;
            }
            ui.label(format!("Hello '{name}', age {age}"));
        });
    })
}
