//! The About window: the logo, the version, links and the paths the app keeps.
//!
//! A deferred viewport, not an immediate one: an immediate viewport's input is
//! taken by the main window's pass first, so nothing in it is clickable.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use egui::{
    CursorIcon, Margin, RichText, Sense, Stroke, TextStyle, Vec2, ViewportBuilder, ViewportId,
};

use crate::config;
use crate::config::LogLevel;
use crate::logo;

const WINDOW_ID: &str = "renameant-about";
const OPENER: &str = "xdg-open";
const WINDOW_MARGIN: i8 = 12;
const LOGO_SIZE: f32 = 144.0;
/// The one gap between every row on the page, so the rhythm does not drift.
const ROW_SPACING: f32 = 8.0;
const NAME_FONT_SIZE: f32 = 28.0;
const WINDOW_WIDTH: f32 = 560.0;
/// The opening size is also the minimum: the collapsed content fits it.
const WINDOW_HEIGHT: f32 = 600.0;
const LABEL_COLUMN_WIDTH: f32 = 76.0;
const PATH_PADDING: f32 = 2.0;
const DESCRIPTION: &str = "Linux bulk rename utility.";
const SYSTEM_INFO_TITLE: &str = "System information";
const SYSTEM_INFO_COPY: &str = "Copy environment info";
/// How long the copy confirmation stays under the header.
const COPY_FEEDBACK: f64 = 2.0;
const LOG_ROW_LABEL: &str = "Logs";
const LOG_LEVEL_LABEL: &str = "Log level";
const LOG_DIR: &str = "logs";
const COPYRIGHT: &str = "© 2026 dcog989";
const LINK_WEBSITE: &str = "Website";
const LINK_RELEASES: &str = "Releases";
const LINK_ISSUES: &str = "Report a bug";

#[derive(Default)]
pub struct About {
    open: bool,
    /// Shared with the About viewport: its callback is `'static`, so it cannot
    /// borrow this window.
    panel: Arc<Mutex<Panel>>,
}

#[derive(Default)]
struct Panel {
    logo: Option<egui::TextureHandle>,
    /// Gathered once per opening.
    info: Option<String>,
    /// When the environment info was last copied, for the confirmation.
    copied_at: Option<f64>,
    level: LogLevel,
    level_changed: bool,
    close_requested: bool,
}

impl About {
    pub fn toggle(&mut self) {
        self.open = !self.open;
        if self.open {
            let mut panel = lock(&self.panel);
            panel.info = None;
            panel.copied_at = None;
        }
    }

    /// Returns whether the panel changed the saved log level.
    pub fn show(&mut self, ctx: &egui::Context, level: &mut LogLevel) -> bool {
        if !self.open {
            return false;
        }

        let shared = Arc::clone(&self.panel);
        let mut viewport = ViewportBuilder::default()
            .with_title("About renameAnt")
            .with_app_id("renameant")
            .with_inner_size([WINDOW_WIDTH, WINDOW_HEIGHT])
            .with_min_inner_size([WINDOW_WIDTH, WINDOW_HEIGHT]);
        let first_frame = {
            let mut panel = lock(&shared);
            if panel.logo.is_none() {
                panel.logo = Some(logo::texture(ctx));
            }
            if panel.info.is_none() {
                panel.info = Some(info());
                panel.level = *level;
                panel.level_changed = false;
                panel.close_requested = false;
                true
            } else {
                false
            }
        };
        // The window is created on the first frame, so the icon only has to be
        // handed over then.
        if first_frame {
            viewport = viewport.with_icon(logo::icon());
        }

        let callback = Arc::clone(&shared);
        ctx.show_viewport_deferred(
            ViewportId::from_hash_of(WINDOW_ID),
            viewport,
            move |ui, _class| viewport_ui(ui, &callback),
        );

        let mut level_changed = false;
        {
            let mut panel = lock(&self.panel);
            if panel.close_requested {
                self.open = false;
            }
            if panel.level_changed {
                *level = panel.level;
                panel.level_changed = false;
                level_changed = true;
            }
        }
        level_changed
    }
}

/// Runs in the About viewport's own pass, so the pointer input is its own.
fn viewport_ui(ui: &mut egui::Ui, shared: &Mutex<Panel>) {
    let mut panel = lock(shared);
    if ui.ctx().input(|input| input.viewport().close_requested())
        || ui.ctx().input(|input| input.key_pressed(egui::Key::Escape))
    {
        panel.close_requested = true;
    }

    let frame = egui::Frame::central_panel(ui.style()).inner_margin(Margin::same(WINDOW_MARGIN));
    egui::CentralPanel::default().frame(frame).show(ui, |ui| {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            panel_ui(ui, &mut panel);
        });
    });
    // The panel only repaints its own window; the main window closes it and
    // saves the level.
    if panel.close_requested || panel.level_changed {
        ui.ctx().request_repaint_of(ViewportId::ROOT);
    }
}

fn panel_ui(ui: &mut egui::Ui, panel: &mut Panel) {
    let info = panel.info.clone().unwrap_or_default();
    // Every gap is explicit; egui's own item spacing would otherwise add to it.
    ui.spacing_mut().item_spacing.y = 0.0;

    ui.vertical_centered(|ui| {
        if let Some(logo) = panel.logo.as_ref() {
            let image = egui::Image::new(logo)
                .fit_to_exact_size(Vec2::splat(LOGO_SIZE))
                .alt_text("renameAnt");
            ui.add(image);
        }
        ui.add_space(ROW_SPACING);
        ui.label(RichText::new("renameAnt").size(NAME_FONT_SIZE).strong());
        ui.add_space(ROW_SPACING);
        ui.weak(DESCRIPTION);
        ui.add_space(ROW_SPACING);
        ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
        ui.add_space(ROW_SPACING);
        links_row(ui);
    });

    ui.add_space(ROW_SPACING);
    ui.separator();
    ui.add_space(ROW_SPACING);

    for (index, (label, path, open_folder)) in paths().into_iter().enumerate() {
        if index > 0 {
            ui.add_space(ROW_SPACING);
        }
        ui.horizontal(|ui| {
            label_column(ui, label);
            path_value(ui, path, open_folder);
        });
        if label == LOG_ROW_LABEL {
            ui.add_space(ROW_SPACING);
            ui.horizontal(|ui| {
                label_column(ui, LOG_LEVEL_LABEL);
                panel.level_changed |= log_level_combo(ui, &mut panel.level);
            });
        }
    }

    ui.add_space(ROW_SPACING);
    system_information(ui, &info, &mut panel.copied_at);

    ui.add_space(ROW_SPACING);
    ui.separator();
    ui.add_space(ROW_SPACING);
    ui.vertical_centered(|ui| {
        ui.weak(format!("{} · {COPYRIGHT}", env!("CARGO_PKG_LICENSE")));
    });
}

/// The header toggles the section; its copy button sits at the right edge.
fn system_information(ui: &mut egui::Ui, info: &str, copied_at: &mut Option<f64>) {
    let id = ui.make_persistent_id(SYSTEM_INFO_TITLE);
    let mut state =
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);
    let mut toggled = false;
    ui.horizontal(|ui| {
        let (_, arrow) = ui.allocate_exact_size(
            egui::vec2(ui.spacing().indent, ui.spacing().icon_width),
            egui::Sense::click(),
        );
        let arrow = arrow.on_hover_cursor(egui::CursorIcon::PointingHand);
        let openness = if state.is_open() { 1.0 } else { 0.0 };
        egui::collapsing_header::paint_default_icon(ui, openness, &arrow);
        let title = ui
            .add(egui::Label::new(SYSTEM_INFO_TITLE).sense(egui::Sense::click()))
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        toggled = arrow.clicked() || title.clicked();
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Copy").on_hover_text(SYSTEM_INFO_COPY).clicked() {
                ui.ctx().copy_text(info.to_owned());
                *copied_at = Some(ui.ctx().input(|input| input.time));
            }
        });
    });
    if toggled {
        state.toggle(ui);
    }
    if state.is_open() {
        ui.add_space(ROW_SPACING);
        ui.indent(id, |ui| ui.label(info));
    }
    state.store(ui.ctx());

    if let Some(at) = *copied_at {
        let elapsed = ui.ctx().input(|input| input.time) - at;
        if elapsed < COPY_FEEDBACK {
            ui.add_space(ROW_SPACING);
            ui.weak("Copied environment info");
            let remaining = COPY_FEEDBACK - elapsed;
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(remaining));
        } else {
            *copied_at = None;
        }
    }
}

/// The title column: every value in the table starts at the same x.
fn label_column(ui: &mut egui::Ui, text: &str) {
    let font = TextStyle::Body.resolve(ui.style());
    let color = ui.visuals().strong_text_color();
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, color);
    let width = galley.size().x.max(LABEL_COLUMN_WIDTH);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, galley.size().y), Sense::hover());
    ui.painter().galley(rect.left_top(), galley, color);
}

/// Centred like the version, and each link names its destination on hover.
fn links_row(ui: &mut egui::Ui) {
    let repository = env!("CARGO_PKG_REPOSITORY");
    let links = [
        (LINK_WEBSITE, repository.to_owned()),
        (LINK_RELEASES, format!("{repository}/releases")),
        (LINK_ISSUES, format!("{repository}/issues/new")),
    ];
    let font = TextStyle::Body.resolve(ui.style());
    let color = ui.visuals().hyperlink_color;
    let width: f32 = links
        .iter()
        .map(|(label, _)| {
            ui.painter().layout_no_wrap((*label).to_owned(), font.clone(), color).size().x
        })
        .sum::<f32>()
        + ui.spacing().item_spacing.x * (links.len() - 1) as f32;

    ui.horizontal(|ui| {
        ui.add_space(((ui.available_width() - width) / 2.0).max(0.0));
        for (label, url) in links {
            if ui.link(label).on_hover_text(url.as_str()).clicked() {
                open_target(std::ffi::OsStr::new(&url));
            }
        }
    });
}

/// `open_folder` marks a path that must not be launched: the binary itself
/// would start a second copy of the app.
fn paths() -> [(&'static str, Option<PathBuf>, bool); 4] {
    [
        ("Config", config::default_path().ok(), false),
        ("Journal", renameant_core::journal::default_path().ok(), false),
        ("Binary", std::env::current_exe().ok(), true),
        (LOG_ROW_LABEL, log_directory(), false),
    ]
}

/// The directory the logger will write under, beside the journal.
fn log_directory() -> Option<PathBuf> {
    Some(renameant_core::journal::default_path().ok()?.parent()?.join(LOG_DIR))
}

fn path_value(ui: &mut egui::Ui, path: Option<PathBuf>, open_folder: bool) {
    let Some(path) = path else {
        ui.weak("unavailable");
        return;
    };

    let text = path.display().to_string();
    let font = TextStyle::Body.resolve(ui.style());
    let color = ui.visuals().hyperlink_color;
    let mut job = egui::text::LayoutJob::simple(text, font, color, f32::INFINITY);
    let wrap_width = ui.available_width() - 2.0 * PATH_PADDING;
    job.wrap = egui::text::TextWrapping::truncate_at_width(wrap_width);
    let galley = ui.painter().layout_job(job);

    // The row is hard to hit at glyph height, so pad the hit area vertically.
    let size = galley.size() + Vec2::splat(2.0 * PATH_PADDING);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    // A long path is truncated, so the hover carries the whole thing.
    let response = response
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(format!("{}\nClick to open.", path.display()));
    let underline = if response.hovered() {
        Stroke::new(ui.style().visuals.widgets.hovered.fg_stroke.width, color)
    } else {
        Stroke::NONE
    };
    ui.painter().add(
        egui::epaint::TextShape::new(rect.left_top() + Vec2::splat(PATH_PADDING), galley, color)
            .with_underline(underline),
    );
    if response.clicked() {
        let target = if open_folder { path.parent().unwrap_or(&path) } else { &path };
        open_target(target.as_os_str());
    }
}

fn log_level_combo(ui: &mut egui::Ui, level: &mut LogLevel) -> bool {
    let mut changed = false;
    egui::ComboBox::from_id_salt("log-level")
        .selected_text(level.as_str())
        .show_ui(ui, |ui| {
            for candidate in LogLevel::ALL {
                if ui.selectable_value(level, candidate, candidate.as_str()).changed() {
                    changed = true;
                }
            }
        })
        .response
        .on_hover_text("Log level: saved now, used once logging is enabled.");
    changed
}

/// One spawn-and-reap for files, folders and URLs: the opener hands off and
/// returns.
fn open_target(target: &std::ffi::OsStr) {
    match Command::new(OPENER).arg(target).spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(error) => {
            eprintln!("renameAnt: could not open {}: {error}", Path::new(target).display());
        }
    }
}

fn info() -> String {
    let mut lines = vec![
        format!("renameAnt {}", env!("CARGO_PKG_VERSION")),
        format!("Build: {}", if cfg!(debug_assertions) { "debug" } else { "release" }),
        format!("Install: {}", install_kind()),
    ];
    let os = os_name().unwrap_or_else(|| "unknown".to_owned());
    lines.push(format!("OS: {os} ({})", std::env::consts::ARCH));
    if let Some(kernel) = read_trimmed("/proc/sys/kernel/osrelease") {
        lines.push(format!("Kernel: {kernel}"));
    }
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").ok().map(desktop_name);
    let session = std::env::var("XDG_SESSION_TYPE").ok();
    match (desktop, session) {
        (Some(desktop), Some(session)) => lines.push(format!("Desktop: {desktop} ({session})")),
        (Some(desktop), None) => lines.push(format!("Desktop: {desktop}")),
        (None, Some(session)) => lines.push(format!("Session: {session}")),
        (None, None) => {}
    }
    if let Some(cpu) = cpu_name() {
        lines.push(format!("CPU: {cpu}"));
    }
    if let Some(memory) = memory_gib() {
        lines.push(format!("Memory: {memory:.1} GiB"));
    }
    lines.join("\n")
}

/// KDE names no release in `XDG_CURRENT_DESKTOP`, so ask `plasmashell`.
fn desktop_name(desktop: String) -> String {
    if !desktop.eq_ignore_ascii_case("kde") {
        return desktop;
    }
    match plasma_version() {
        Some(version) => format!("{desktop} Plasma {version}"),
        None => desktop,
    }
}

/// `plasmashell --version` prints `plasmashell 6.7.3`; the version is the last
/// word.
fn plasma_version() -> Option<String> {
    let output = Command::new("plasmashell").arg("--version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    Some(text.split_whitespace().last()?.to_owned())
}

/// shortcut: only the AppImage variable and the /usr prefix are checked; a
/// package installed elsewhere reports as a local build.
fn install_kind() -> &'static str {
    if std::env::var_os("APPIMAGE").is_some() {
        "AppImage"
    } else if std::env::current_exe().is_ok_and(|path| path.starts_with("/usr/")) {
        "system package"
    } else {
        "local build"
    }
}

fn read_trimmed(path: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn os_name() -> Option<String> {
    os_name_from(&std::fs::read_to_string("/etc/os-release").ok()?)
}

fn os_name_from(os_release: &str) -> Option<String> {
    let line = os_release.lines().find(|line| line.starts_with("PRETTY_NAME="))?;
    Some(line.trim_start_matches("PRETTY_NAME=").trim_matches('"').to_owned())
}

fn cpu_name() -> Option<String> {
    let info = std::fs::read_to_string("/proc/cpuinfo").ok()?;
    let line = info.lines().find(|line| line.starts_with("model name"))?;
    Some(line.split_once(':')?.1.trim().to_owned())
}

fn memory_gib() -> Option<f64> {
    let info = std::fs::read_to_string("/proc/meminfo").ok()?;
    let line = info.lines().find(|line| line.starts_with("MemTotal:"))?;
    let kib: f64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib / 1024.0 / 1024.0)
}

fn lock(panel: &Mutex<Panel>) -> std::sync::MutexGuard<'_, Panel> {
    panel.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::os_name_from;

    #[test]
    fn reads_the_pretty_os_name() {
        assert_eq!(
            os_name_from("NAME=\"CachyOS\"\nPRETTY_NAME=\"CachyOS Linux\"\n"),
            Some("CachyOS Linux".to_owned())
        );
        assert_eq!(os_name_from("NAME=\"CachyOS\"\n"), None);
    }
}
