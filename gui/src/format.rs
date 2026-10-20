//! Text and labels for the window: the pure functions that turn a plan row, a
//! name or a byte count into the string a widget shows.

use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use renameant_core::plan::{ConflictKind, InvalidKind, Status};

/// What a row's status means, in the user's words.
pub fn status_label(status: Status) -> &'static str {
    match status {
        Status::Ok => "ok",
        Status::Unchanged => "unchanged",
        Status::Conflict(ConflictKind::DuplicateTarget) => "two rows want this name",
        Status::Conflict(ConflictKind::TargetExists) => "name is taken",
        Status::Conflict(ConflictKind::CaseOnly) => "case-only collision",
        Status::Invalid(InvalidKind::Empty) => "empty name",
        Status::Invalid(InvalidKind::IllegalChar) => "illegal character",
        Status::Invalid(InvalidKind::NameTooLong) => "over 255 bytes",
        Status::Invalid(InvalidKind::PathTooLong) => "path too long",
    }
}

/// A name as text. Non-UTF-8 names are shown lossily and flagged; they are
/// never converted on disk.
pub fn name_text(name: &[u8]) -> String {
    String::from_utf8_lossy(name).into_owned()
}

pub fn is_utf8(name: &[u8]) -> bool {
    std::str::from_utf8(name).is_ok()
}

pub fn file_bytes(path: &Path) -> &[u8] {
    path.file_name().unwrap_or(path.as_os_str()).as_bytes()
}

pub fn file_text(path: &Path) -> String {
    name_text(file_bytes(path))
}

/// A path split for the preview: everything up to its name, and the name
/// itself, so the name can be shown stronger than the folders above it.
pub fn path_parts(path: &Path) -> (String, String) {
    let bytes = path.as_os_str().as_bytes();
    let name_at = bytes.len() - file_bytes(path).len();
    (name_text(&bytes[..name_at]), name_text(&bytes[name_at..]))
}

/// A byte count for the preview's Size column.
pub fn size_text(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
    const STEP: f64 = 1024.0;
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= STEP && unit < UNITS.len() - 1 {
        value /= STEP;
        unit += 1;
    }
    if unit == 0 { format!("{bytes} B") } else { format!("{value:.1} {}", UNITS[unit]) }
}

/// A unix timestamp as `yyyy/mm/dd, hh:mm:ss` in UTC. Local time would need a
/// date crate, so the arithmetic is done here against the epoch instead.
pub fn utc_text(seconds: u64) -> String {
    const SECS_PER_DAY: u64 = 24 * 60 * 60;
    let days = (seconds / SECS_PER_DAY) as i64;
    let rest = seconds % SECS_PER_DAY;
    let (year, month, day) = civil_from_days(days);
    let (hour, minute, second) = (rest / 3600, rest / 60 % 60, rest % 60);
    format!("{year:04}/{month:02}/{day:02}, {hour:02}:{minute:02}:{second:02}")
}

/// The civil date of a day number since 1970-01-01: Howard Hinnant's
/// `civil_from_days`, which is exact for the proleptic Gregorian calendar.
fn civil_from_days(days: i64) -> (i64, u64, u64) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 { shifted } else { shifted - 146_096 } / 146_097;
    let day_of_era = (shifted - era * 146_097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_shift = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_shift + 2) / 5 + 1;
    let month = if month_shift < 10 { month_shift + 3 } else { month_shift - 9 };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_in_plain_units() {
        assert_eq!(size_text(999), "999 B");
        assert_eq!(size_text(1024), "1.0 K");
        assert_eq!(size_text(1536), "1.5 K");
    }

    #[test]
    fn timestamps_read_as_utc_date_and_time() {
        assert_eq!(utc_text(0), "1970/01/01, 00:00:00");
        assert_eq!(utc_text(1_000_000_000), "2001/09/09, 01:46:40");
        assert_eq!(utc_text(1_700_000_000), "2023/11/14, 22:13:20");
        // A leap day, so the arithmetic is checked past a plain month boundary.
        assert_eq!(utc_text(1_709_164_800), "2024/02/29, 00:00:00");
    }

    #[test]
    fn a_path_splits_before_its_name() {
        assert_eq!(
            path_parts(Path::new("/d/sub/a.txt")),
            ("/d/sub/".to_owned(), "a.txt".to_owned())
        );
        // A name directly below the root keeps exactly one separator.
        assert_eq!(path_parts(Path::new("/a.txt")), ("/".to_owned(), "a.txt".to_owned()));
    }
}
