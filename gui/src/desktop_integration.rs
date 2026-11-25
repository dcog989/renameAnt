//! Desktop integration for the AppImage build.
//!
//! An AppImage has no installer, so the menu entry and icon are written on
//! launch. Release filenames carry the version, so `Exec` is repointed whenever
//! the running file differs.

use std::path::{Path, PathBuf};

const DESKTOP_FILE: &str = "renameant.desktop";
const ICON_FILE: &str = "renameant.svg";
const ICON_SVG: &[u8] = include_bytes!("../../assets/logo.svg");
/// The same entry `just package` puts inside the AppImage, so names, icon and
/// window class cannot drift.
const DESKTOP_TEMPLATE: &str = include_str!("../../dist/renameant.desktop");
/// Records the AppImage this entry was generated for, so a stale entry is
/// recognised without parsing the quoted `Exec` value.
const MARKER: &str = "X-RenameAnt-AppImage=";

/// Registers the running AppImage, or removes an entry left behind by one that
/// no longer exists.
pub fn install() {
    if let Err(error) = sync() {
        eprintln!("renameAnt: desktop integration failed: {error}");
    }
}

fn sync() -> std::io::Result<()> {
    let data = data_home()?;
    let applications = data.join("applications");

    let Some(appimage) = running_appimage() else {
        remove_generated(&applications);
        return Ok(());
    };

    let icons = data.join("icons/hicolor/scalable/apps");
    std::fs::create_dir_all(&applications)?;
    std::fs::create_dir_all(&icons)?;
    write_if_changed(&icons.join(ICON_FILE), ICON_SVG)?;

    let path = applications.join(DESKTOP_FILE);
    let content = appimage_entry(&appimage);
    if std::fs::read_to_string(&path).ok().as_deref() != Some(content.as_str()) {
        std::fs::write(&path, content.as_bytes())?;
        refresh_menu(&applications);
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
fn running_appimage() -> Option<PathBuf> {
    std::env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
}

fn appimage_entry(appimage: &Path) -> String {
    let exec = exec_quote(&appimage.to_string_lossy());
    let mut entry = String::new();
    for line in DESKTOP_TEMPLATE.lines() {
        if let Some(rest) = line.strip_prefix("Exec=") {
            entry.push_str(&format!("Exec={exec}{rest}\n"));
        } else {
            entry.push_str(line);
            entry.push('\n');
        }
    }
    entry.push_str(&format!("{MARKER}{}\n", appimage.display()));
    entry
}

/// Deletes the entry only when it carries `MARKER`: a system install could
/// otherwise be removed by a run of a different build.
fn remove_generated(applications: &Path) {
    let path = applications.join(DESKTOP_FILE);
    let Ok(content) = std::fs::read_to_string(&path) else {
        return;
    };
    let Some(appimage) = marker_value(&content) else {
        return;
    };
    if Path::new(appimage).is_file() {
        return;
    }
    let _ = std::fs::remove_file(&path);
    refresh_menu(applications);
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

    #[test]
    fn entry_registers_the_running_appimage() {
        let entry = appimage_entry(Path::new("/home/user/Apps/renameAnt 2.AppImage"));
        assert!(
            entry.contains("Exec=\"/home/user/Apps/renameAnt 2.AppImage\"\n"),
            "{entry}"
        );
        assert!(
            entry.contains(&format!("{MARKER}/home/user/Apps/renameAnt 2.AppImage\n")),
            "{entry}"
        );
        // Everything else in the template survives the `Exec` rewrite.
        assert!(entry.contains("Icon=renameant\n"), "{entry}");
        assert!(entry.contains("StartupWMClass=renameant\n"), "{entry}");
    }

    #[test]
    fn marker_is_read_back_verbatim() {
        let entry = appimage_entry(Path::new("/opt/renameAnt-0.1.0-x86_64.AppImage"));
        assert_eq!(
            marker_value(&entry),
            Some("/opt/renameAnt-0.1.0-x86_64.AppImage")
        );
        assert_eq!(marker_value("[Desktop Entry]\n"), None);
    }

    #[test]
    fn exec_quote_escapes_the_specials_and_spaces() {
        assert_eq!(exec_quote("/opt/renameAnt 2.AppImage"), "\"/opt/renameAnt 2.AppImage\"");
        assert_eq!(exec_quote("/opt/a%b.AppImage"), "\"/opt/a%%b.AppImage\"");
        assert_eq!(exec_quote("/opt/a$b.AppImage"), "\"/opt/a\\$b.AppImage\"");
    }
}
