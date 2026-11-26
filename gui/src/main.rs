//! renameAnt desktop app. The initial state is a dry run: nothing is written
//! before you press Apply.

mod about;
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
    // One window: two instances would overwrite each other's rule list, and the
    // journal is easier to reason about with one writer.
    let journal = match renameant_core::journal::default_path() {
        Ok(journal) => journal,
        Err(error) => {
            return quit(&format!("renameAnt could not find its state directory:\n{error}"));
        }
    };
    let _instance =
        match renameant_core::journal::lock_instance(&journal.with_file_name("instance.lock")) {
            Ok(Some(lock)) => lock,
            Ok(None) => return quit("renameAnt is already running."),
            Err(error) => {
                return quit(&format!("renameAnt could not lock its state directory:\n{error}"));
            }
        };
    // Only the instance that owns the state may touch the menu entry.
    desktop_integration::install();
    let options = eframe::NativeOptions {
        viewport: ViewportBuilder::default()
            .with_app_id("renameant")
            .with_title("renameAnt")
            .with_icon(logo::icon())
            .with_inner_size(WINDOW_SIZE),
        // GL/EGL comes from the host, and wgpu is a large dependency
        // the AppImage does not need.
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native("renameAnt", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
}

/// Shows a startup problem and exits without opening a window.
fn quit(message: &str) -> eframe::Result {
    rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Warning)
        .set_title("renameAnt")
        .set_description(message)
        .show();
    Ok(())
}
