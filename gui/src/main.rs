//! renameAnt desktop app. The initial state is a dry run: nothing is written
//! before you press Apply.

mod app;
mod config;
mod desktop_integration;
mod dialogs;
mod edit;
mod editor;
mod format;
mod history;
mod logo;
mod model;
mod style;
mod table;
mod worker;

use egui::ViewportBuilder;

/// The panes fit a narrow screen and the table scrolls sideways past this.
const WINDOW_SIZE: [f32; 2] = [1200.0, 720.0];

fn main() -> eframe::Result {
    desktop_integration::install();
    let options = eframe::NativeOptions {
        viewport: ViewportBuilder::default()
            .with_app_id("renameant")
            .with_title("renameAnt")
            .with_inner_size(WINDOW_SIZE),
        // GL/EGL comes from the host, and wgpu is a large dependency
        // the AppImage does not need.
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native("renameAnt", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
}
