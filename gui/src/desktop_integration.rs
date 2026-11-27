//! Desktop integration for the AppImage build and local runs.
//!
//! An AppImage has no installer, so the menu entry and icon are written on
//! launch. KDE reads a Wayland window's icon from the desktop file named after
//! its app id, so a `cargo run` needs one too; that entry is hidden from the
//! menu. Release filenames carry the version, so `Exec` is repointed whenever
//! the running file differs.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

const DESKTOP_FILE: &str = "renameant.desktop";
const ICON_FILE: &str = "renameant.svg";
const ICON_SVG: &[u8] = include_bytes!("../../assets/logo.svg");
/// The same entry `just package` puts inside the AppImage, so names, icon and
/// window class cannot drift.
const DESKTOP_TEMPLATE: &str = include_str!("../../dist/renameant.desktop");
/// Records the executable this entry was generated for, so a stale entry is
/// recognised without parsing the quoted `Exec` value.
const MARKER: &str = "X-RenameAnt-Exec=";
/// A system install already provides the entry; a user copy would shadow it.
const SYSTEM_DESKTOP_FILES: [&str; 2] = [
    "/usr/share/applications/renameant.desktop",
    "/usr/local/share/applications/renameant.desktop",
];

/// What the entry should launch: the AppImage, or a build run directly.
struct Target {
    path: PathBuf,
    appimage: bool,
}

/// Registers the running AppImage, or the binary for a local run, and drops its
/// icon where the theme looks for it.
pub fn install() {
    if let Err(error) = sync() {
        eprintln!("renameAnt: desktop integration failed: {error}");
    }
}

fn sync() -> std::io::Result<()> {
    if SYSTEM_DESKTOP_FILES.iter().any(|path| Path::new(*path).exists()) {
        return Ok(());
    }
    let Some(target) = running_target() else {
        return Ok(());
    };
    let Some(exec) = target.path.to_str() else {
        eprintln!("renameAnt: desktop integration skipped: the executable path is not UTF-8");
        return Ok(());
    };

    let data = renameant_core::xdg::base("XDG_DATA_HOME", ".local/share")?;
    let applications = data.join("applications");
    let icons = data.join("icons/hicolor/scalable/apps");
    std::fs::create_dir_all(&applications)?;
    std::fs::create_dir_all(&icons)?;
    write_if_changed(&icons.join(ICON_FILE), ICON_SVG)?;

    let path = applications.join(DESKTOP_FILE);
    let existing = std::fs::read_to_string(&path).ok();
    // A dev run must not hide a working AppImage entry. Only an AppImage run
    // repoints unconditionally.
    if !keep_existing(target.appimage, existing.as_deref()) {
        let content = entry(exec, target.appimage);
        if existing.as_deref() != Some(content.as_str()) {
            write_atomic(&path, content.as_bytes())?;
            refresh_menu(&applications);
        }
    }
    Ok(())
}

/// Whether an entry can stay: never for an AppImage run, and for a dev run only
/// when the entry's executable still exists.
fn keep_existing(appimage: bool, existing: Option<&str>) -> bool {
    !appimage && existing.is_some_and(is_current)
}

/// The AppImage launcher sets `APPIMAGE` to the file the user runs. Inside the
/// mount `current_exe()` points at a path that disappears on exit.
fn running_target() -> Option<Target> {
    if let Some(path) = std::env::var_os("APPIMAGE").map(PathBuf::from) {
        // Inside the mount `current_exe()` disappears with the process, so only
        // the file the user launched is a usable target.
        return path.is_file().then_some(Target { path, appimage: true });
    }
    let path = std::env::current_exe().ok()?;
    Some(Target { path, appimage: false })
}

/// Whether the entry can stay: one we did not write, or one whose recorded
/// executable still exists.
fn is_current(content: &str) -> bool {
    marker_value(content).is_none_or(|path| Path::new(path).is_file())
}

fn entry(path: &str, appimage: bool) -> String {
    // `%F` is where a file manager puts the picked files and folders.
    let exec = format!("{} %F", exec_quote(path));
    let mut entry = String::new();
    for line in DESKTOP_TEMPLATE.lines() {
        if line.starts_with("Exec=") {
            entry.push_str(&format!("Exec={exec}\n"));
        } else {
            entry.push_str(line);
            entry.push('\n');
        }
    }
    // A menu entry for `target/debug/renameant-gui` is noise, but the icon still
    // has to be resolvable.
    if !appimage {
        entry.push_str("NoDisplay=true\n");
    }
    entry.push_str(&format!("{MARKER}{path}\n"));
    entry
}

fn marker_value(content: &str) -> Option<&str> {
    content.lines().find_map(|line| line.strip_prefix(MARKER))
}

/// Quotes a value for a desktop-entry `Exec` key: double quotes, with `"`, `` ` ``,
/// `$` and `\` escaped by a backslash and `%` doubled, per the freedesktop
/// Desktop Entry specification.
fn exec_quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' | '`' | '$' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            '%' => {
                out.push('%');
                out.push('%');
            }
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

fn write_if_changed(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if std::fs::read(path).ok().as_deref() == Some(bytes) {
        return Ok(());
    }
    std::fs::write(path, bytes)
}

/// Writes beside the target and renames over it, so a second launch never sees
/// half an entry. A stale temp is removed so `create_new` cannot follow a
/// planted symlink.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let temp = path.with_extension("desktop.tmp");
    match std::fs::remove_file(&temp) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut file = OpenOptions::new().write(true).create_new(true).open(&temp)?;
    file.write_all(bytes)?;
    drop(file);
    std::fs::rename(&temp, path)
}

/// Rebuilds the desktop-file cache. Absent on minimal systems; KDE also
/// refreshes through KSycoca, so a failure is not fatal. It runs off the UI
/// thread because the first frame should not wait on a directory scan.
fn refresh_menu(applications: &Path) {
    let applications = applications.to_path_buf();
    let _ = std::thread::spawn(move || {
        let _ = std::process::Command::new("update-desktop-database").arg(applications).status();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry_for(path: &str) -> String {
        entry(path, true)
    }

    #[test]
    fn appimage_entry_registers_the_running_file() {
        let entry = entry_for("/home/user/Apps/renameAnt 2.AppImage");
        assert!(entry.contains("Exec=\"/home/user/Apps/renameAnt 2.AppImage\" %F\n"), "{entry}");
        assert!(
            entry.contains(&format!("{MARKER}/home/user/Apps/renameAnt 2.AppImage\n")),
            "{entry}"
        );
        assert!(!entry.contains("NoDisplay=true"), "{entry}");
        // Everything else in the template survives the `Exec` rewrite.
        assert!(entry.contains("Icon=renameant\n"), "{entry}");
        assert!(entry.contains("StartupWMClass=renameant\n"), "{entry}");
    }

    #[test]
    fn dev_entry_is_hidden_and_points_at_the_binary() {
        let entry = entry("/repo/target/debug/renameant-gui", false);
        assert!(entry.contains("Exec=\"/repo/target/debug/renameant-gui\" %F\n"), "{entry}");
        assert!(entry.contains("NoDisplay=true\n"), "{entry}");
    }

    #[test]
    fn marker_is_read_back_verbatim() {
        let entry = entry_for("/opt/renameAnt-0.1.0-x86_64.AppImage");
        assert_eq!(marker_value(&entry), Some("/opt/renameAnt-0.1.0-x86_64.AppImage"));
        assert_eq!(marker_value("[Desktop Entry]\n"), None);
    }

    #[test]
    fn only_an_entry_whose_executable_survives_is_current() {
        assert!(is_current("[Desktop Entry]\nIcon=renameant\n"));
        let dir = tempfile::tempdir().expect("a home for the appimage");
        let live = dir.path().join("renameAnt-0.1.0-x86_64.AppImage");
        std::fs::write(&live, b"x").expect("the appimage");
        assert!(is_current(&entry_for(&live.to_string_lossy())));
        assert!(!is_current(&entry_for(&dir.path().join("moved-away.AppImage").to_string_lossy())));
    }

    #[test]
    fn an_appimage_repoints_but_a_dev_run_keeps_a_live_entry() {
        let dir = tempfile::tempdir().expect("a home for the appimage");
        let live = dir.path().join("renameAnt-0.1.0-x86_64.AppImage");
        std::fs::write(&live, b"x").expect("the appimage");
        let current = entry_for(&live.to_string_lossy());
        assert!(keep_existing(false, Some(current.as_str())));
        assert!(!keep_existing(true, Some(current.as_str())));
        assert!(!keep_existing(false, None));

        let stale = entry_for(&dir.path().join("moved-away.AppImage").to_string_lossy());
        assert!(!keep_existing(false, Some(stale.as_str())));
    }

    #[test]
    fn an_entry_is_replaced_without_leaving_the_temp() {
        let dir = tempfile::tempdir().expect("a home for the entry");
        let path = dir.path().join(DESKTOP_FILE);
        write_atomic(&path, b"one").expect("the first write");
        write_atomic(&path, b"two").expect("the second write");
        assert_eq!(std::fs::read(&path).expect("the entry"), b"two");
        assert!(!dir.path().join("renameant.desktop.tmp").exists());
    }

    #[test]
    fn exec_quote_escapes_the_specials_and_spaces() {
        assert_eq!(exec_quote("/opt/renameAnt 2.AppImage"), "\"/opt/renameAnt 2.AppImage\"");
        assert_eq!(exec_quote("/opt/a%b.AppImage"), "\"/opt/a%%b.AppImage\"");
        assert_eq!(exec_quote("/opt/a$b.AppImage"), "\"/opt/a\\$b.AppImage\"");
    }
}
