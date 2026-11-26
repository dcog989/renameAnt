//! XDG base directory lookup.
//!
//! A relative value is ignored: the spec says implementations must not use one,
//! and it would resolve against the working directory, so a launch from a menu
//! and from a terminal would read different files.

use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};

/// `$<var>`, or `$HOME/<fallback>`. Both must be absolute; a relative value
/// falls back rather than resolving against the current directory.
pub fn base(var: &str, fallback: &str) -> io::Result<PathBuf> {
    resolve(std::env::var_os(var).as_deref(), std::env::var_os("HOME").as_deref(), var, fallback)
}

fn resolve(
    xdg: Option<&OsStr>,
    home: Option<&OsStr>,
    var: &str,
    fallback: &str,
) -> io::Result<PathBuf> {
    if let Some(dir) = absolute(xdg) {
        return Ok(dir.to_path_buf());
    }
    let home = absolute(home).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("neither {var} nor HOME is set to an absolute path"),
        )
    })?;
    Ok(home.join(fallback))
}

fn absolute(value: Option<&OsStr>) -> Option<&Path> {
    let path = Path::new(value?);
    path.is_absolute().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absolute_override_wins() {
        let base = resolve(
            Some(OsStr::new("/data")),
            Some(OsStr::new("/home/user")),
            "XDG_DATA_HOME",
            ".local/share",
        )
        .expect("an absolute override");
        assert_eq!(base, PathBuf::from("/data"));
    }

    #[test]
    fn a_relative_override_falls_back_to_home() {
        let base = resolve(
            Some(OsStr::new("data")),
            Some(OsStr::new("/home/user")),
            "XDG_DATA_HOME",
            ".local/share",
        )
        .expect("an absolute home");
        assert_eq!(base, PathBuf::from("/home/user/.local/share"));
    }

    #[test]
    fn an_empty_override_falls_back_to_home() {
        let base = resolve(
            Some(OsStr::new("")),
            Some(OsStr::new("/home/user")),
            "XDG_CONFIG_HOME",
            ".config",
        )
        .expect("an absolute home");
        assert_eq!(base, PathBuf::from("/home/user/.config"));
    }

    #[test]
    fn a_relative_home_is_an_error() {
        assert!(resolve(None, Some(OsStr::new("home")), "XDG_STATE_HOME", ".local/state").is_err());
        assert!(resolve(None, None, "XDG_STATE_HOME", ".local/state").is_err());
    }
}
