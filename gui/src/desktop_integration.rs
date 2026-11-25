//! Desktop integration for the AppImage build and local runs.
//!
//! An AppImage has no installer, so the menu entry and icon are written on
//! launch. KDE reads a Wayland window's icon from the desktop file named after
//! its app id, so a `cargo run` needs one too; that entry is hidden from the
//! menu. Release filenames carry the version, so `Exec` is repointed whenever
//! the running file differs.

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
const SYSTEM_DESKTOP_FILE: &str = "/usr/share/applications/renameant.desktop";

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
    if Path::new(SYSTEM_DESKTOP_FILE).exists() {
        return Ok(());
    }
    let Some(target) = running_target() else {
        return Ok(());
    };

    let data = data_home()?;
    let applications = data.join("applications");
    let icons = data.join("icons/hicolor/scalable/apps");
    std::fs::create_dir_all(&applications)?;
    std::fs::create_dir_all(&icons)?;
    write_if_changed(&icons.join(ICON_FILE), ICON_SVG)?;

    let path = applications.join(DESKTOP_FILE);
    let existing = std::fs::read_to_string(&path).ok();
    // A dev run must not hide a working AppImage entry, and every run keeps an
    // entry it did not write. Only an AppImage run repoints unconditionally.
    let keep = !target.appimage && existing.as_deref().is_some_and(is_current);
    if !keep {
        let content = entry(&target);
        if existing.as_deref() != Some(content.as_str()) {
            std::fs::write(&path, content.as_bytes())?;
            refresh_menu(&applications);
        }
    }
    Ok(())
}

/// `$XDG_DATA_HOME`, conventionally `~/.local/share`. No crate is pulled in for
/// the one directory this app needs.
fn data_home() -> std::io::Result<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_DATA_HOME").filter(|dir| !dir.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    let home = std::env::var_os("HOME").ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "neither XDG_DATA_HOME nor HOME is set",
        )
    })?;
    Ok(PathBuf::from(home).join(".local/share"))
}

/// The AppImage launcher sets `APPIMAGE` to the file the user runs. Inside the
/// mount `current_exe()` points at a path that disappears on exit.
fn running_target() -> Option<Target> {
    if let Some(path) = std::env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
    {
        return Some(Target { path, appimage: true });
    }
    let path = std::env::current_exe().ok()?;
    Some(Target { path, appimage: false })
}

/// Whether the entry can stay: one we did not write, or one whose recorded
/// executable still exists.
fn is_current(content: &str) -> bool {
    marker_value(content).is_none_or(|path| Path::new(path).is_file())
}

fn entry(target: &Target) -> String {
    let exec = exec_quote(&target.path.to_string_lossy());
    let mut entry = String::new();
    for line in DESKTOP_TEMPLATE.lines() {
        if let Some(rest) = line.strip_prefix("Exec=") {
            entry.push_str(&format!("Exec={exec}{rest}\n"));
        } else {
            entry.push_str(line);
            entry.push('\n');
        }
    }
    // A menu entry for `target/debug/renameant-gui` is noise, but the icon still
    // has to be resolvable.
    if !target.appimage {
        entry.push_str("NoDisplay=true\n");
    }
    entry.push_str(&format!("{MARKER}{}\n", target.path.display()));
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

/// Rebuilds the desktop-file cache. Absent on minimal systems; KDE also
/// refreshes through KSycoca, so a failure is not fatal.
fn refresh_menu(applications: &Path) {
    let _ = std::process::Command::new("update-desktop-database")
        .arg(applications)
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry_for(path: &str) -> String {
        entry(&Target { path: PathBuf::from(path), appimage: true })
    }

    #[test]
    fn appimage_entry_registers_the_running_file() {
        let entry = entry_for("/home/user/Apps/renameAnt 2.AppImage");
        assert!(
            entry.contains("Exec=\"/home/user/Apps/renameAnt 2.AppImage\"\n"),
            "{entry}"
        );
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
        let entry = entry(&Target {
            path: PathBuf::from("/repo/target/debug/renameant-gui"),
            appimage: false,
        });
        assert!(
            entry.contains("Exec=\"/repo/target/debug/renameant-gui\"\n"),
            "{entry}"
        );
        assert!(entry.contains("NoDisplay=true\n"), "{entry}");
    }

    #[test]
    fn marker_is_read_back_verbatim() {
        let entry = entry_for("/opt/renameAnt-0.1.0-x86_64.AppImage");
        assert_eq!(
            marker_value(&entry),
            Some("/opt/renameAnt-0.1.0-x86_64.AppImage")
        );
        assert_eq!(marker_value("[Desktop Entry]\n"), None);
    }

    #[test]
    fn only_an_entry_whose_executable_survives_is_current() {
        assert!(is_current("[Desktop Entry]\nIcon=renameant\n"));
        assert!(is_current(&entry_for("/opt/renameAnt-0.1.0-x86_64.AppImage")));
        assert!(!is_current(&entry_for("/opt/moved-away.AppImage")));
    }

    #[test]
    fn exec_quote_escapes_the_specials_and_spaces() {
        assert_eq!(exec_quote("/opt/renameAnt 2.AppImage"), "\"/opt/renameAnt 2.AppImage\"");
        assert_eq!(exec_quote("/opt/a%b.AppImage"), "\"/opt/a%%b.AppImage\"");
        assert_eq!(exec_quote("/opt/a$b.AppImage"), "\"/opt/a\\$b.AppImage\"");
    }
}
