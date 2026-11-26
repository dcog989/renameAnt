//! File logging under the state directory.
//!
//! The About window owns the level and applies it without a restart. One
//! append-only file beside the journal: enough for a bug report, and nothing is
//! written while the window is idle.

use std::fmt;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, Once};

use crate::config::LogLevel;

/// The directory under the state directory, beside the journal.
pub const DIRECTORY: &str = "logs";
const FILE: &str = "renameant.log";
// shortcut: one append-only file, no rotation; rotate once a long-lived install
// grows it.

static LEVEL: AtomicU8 = AtomicU8::new(LogLevel::Info.rank());
static TARGET: Mutex<Option<PathBuf>> = Mutex::new(None);
static HOOK: Once = Once::new();

/// Points the logger at [`directory`] and installs the panic hook. Called once
/// per window; a test window may call it again to retarget it.
pub fn init(level: LogLevel) {
    set_level(level);
    install_panic_hook();
    let target = directory().map(|dir| {
        // A log that cannot be created must not cost the user the app; the log
        // then stays empty and the status line still carries what matters.
        let _ = std::fs::create_dir_all(&dir);
        dir.join(FILE)
    });
    *lock() = target;
}

/// The directory the logger writes under, beside the journal.
pub fn directory() -> Option<PathBuf> {
    Some(renameant_core::journal::default_path().ok()?.parent()?.join(DIRECTORY))
}

pub fn set_level(level: LogLevel) {
    LEVEL.store(level.rank(), Ordering::Relaxed);
}

/// Appends one line, unless the configured level is quieter than `level`.
pub fn write(level: LogLevel, message: fmt::Arguments<'_>) {
    if !enabled(level) {
        return;
    }
    let Some(target) = lock().clone() else { return };
    let stamp = stamp_text(&jiff::ZonedDateTime::now());
    let record = line(&stamp, level, message);
    // A failed write is not worth interrupting the user; a full disk already
    // shows up where the user acts on it. Opened per line: a rotated or deleted
    // file heals itself, and events are rare.
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&target) {
        let _ = file.write_all(record.as_bytes());
    }
}

fn enabled(level: LogLevel) -> bool {
    level.rank() <= LEVEL.load(Ordering::Relaxed)
}

fn line(stamp: &str, level: LogLevel, message: impl fmt::Display) -> String {
    format!("{stamp} {:<5} {message}\n", level.as_str())
}

/// Local time, matching the journal's stamps.
fn stamp_text(at: &jiff::ZonedDateTime) -> String {
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        at.year(),
        at.month(),
        at.day(),
        at.hour(),
        at.minute(),
        at.second()
    )
}

/// A panic is the one event worth reaching the log even when the build aborts.
fn install_panic_hook() {
    HOOK.call_once(|| {
        let default = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let at = info.location().map(|at| format!(" at {at}")).unwrap_or_default();
            write(LogLevel::Error, format_args!("panic{at}: {info}"));
            default(info);
        }));
    });
}

fn lock() -> std::sync::MutexGuard<'static, Option<PathBuf>> {
    TARGET.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[macro_export]
macro_rules! log_error {
    ($($arg:tt)*) => {
        $crate::logging::write($crate::config::LogLevel::Error, format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_warn {
    ($($arg:tt)*) => {
        $crate::logging::write($crate::config::LogLevel::Warn, format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_info {
    ($($arg:tt)*) => {
        $crate::logging::write($crate::config::LogLevel::Info, format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_debug {
    ($($arg:tt)*) => {
        $crate::logging::write($crate::config::LogLevel::Debug, format_args!($($arg)*))
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_states_the_time_the_level_and_the_message() {
        assert_eq!(
            line("2026-01-02 03:04:05", LogLevel::Warn, "boom"),
            "2026-01-02 03:04:05 warn  boom\n"
        );
    }

    #[test]
    fn a_message_quieter_than_the_level_is_dropped() {
        assert!(enabled_at(LogLevel::Warn, LogLevel::Error));
        assert!(enabled_at(LogLevel::Warn, LogLevel::Warn));
        assert!(!enabled_at(LogLevel::Warn, LogLevel::Info));
    }

    fn enabled_at(configured: LogLevel, message: LogLevel) -> bool {
        message.rank() <= configured.rank()
    }
}
