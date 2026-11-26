//! Text and labels for the window: the pure functions that turn a plan row, a
//! name or a byte count into the string a widget shows.

use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use jiff::{Timestamp, ZonedDateTime, tz::TimeZone};
use renameant_core::plan::{ConflictKind, InvalidKind, Status};

/// What a row's status means, in the user's words.
pub fn status_label(status: Status) -> &'static str {
    match status {
        Status::Ok => "match",
        Status::Unchanged => "no match",
        Status::Conflict(ConflictKind::DuplicateTarget) => "two rows want this name",
        Status::Conflict(ConflictKind::TargetExists) => "name is taken",
        Status::Conflict(ConflictKind::CaseOnly) => "case-only collision",
        Status::Invalid(InvalidKind::Empty) => "empty name",
        Status::Invalid(InvalidKind::IllegalChar) => "illegal or reserved name",
        Status::Invalid(InvalidKind::NameTooLong) => "over 255 bytes",
        Status::Invalid(InvalidKind::PathTooLong) => "path too long",
    }
}

/// Sort order for the Status column: problems first, matches last, so the view
/// groups by meaning instead of by label text.
pub fn status_rank(status: Status) -> u8 {
    match status {
        Status::Invalid(_) => 0,
        Status::Conflict(_) => 1,
        Status::Ok => 2,
        Status::Unchanged => 3,
    }
}

/// A name as text. Non-UTF-8 names are shown lossily and flagged; they are
/// never converted on disk.
pub fn name_text(name: &[u8]) -> String {
    String::from_utf8_lossy(name).into_owned()
}

pub fn file_bytes(path: &Path) -> &[u8] {
    path.file_name().unwrap_or(path.as_os_str()).as_bytes()
}

pub fn file_text(path: &Path) -> String {
    name_text(file_bytes(path))
}

/// `n` with the noun that fits it, e.g. `1 record` and `2 records`.
pub fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// A path split for the preview: everything up to its name, and the name
/// itself, so the name can be shown stronger than the folders above it.
pub fn path_parts(path: &Path) -> (String, String) {
    let bytes = path.as_os_str().as_bytes();
    // file_name() ignores a trailing separator, so find the name from the end
    // rather than trusting the path's byte length.
    let end = bytes.iter().rposition(|byte| *byte != b'/').map_or(0, |at| at + 1);
    if end == 0 {
        return (String::new(), name_text(bytes));
    }
    let start = bytes[..end].iter().rposition(|byte| *byte == b'/').map_or(0, |at| at + 1);
    (name_text(&bytes[..start]), name_text(&bytes[start..end]))
}

/// A byte count for the preview's Size column.
pub fn size_text(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    const STEP: f64 = 1024.0;
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= STEP && unit < UNITS.len() - 1 {
        value /= STEP;
        unit += 1;
    }
    // One decimal can round 1023.95 KiB up to "1024.0", so carry it.
    if value >= STEP - 0.05 && unit < UNITS.len() - 1 {
        value /= STEP;
        unit += 1;
    }
    if unit == 0 { format!("{bytes} B") } else { format!("{value:.1} {}", UNITS[unit]) }
}

/// A unix timestamp as `yyyy/mm/dd, hh:mm:ss` in the system's local time zone.
pub fn local_text(seconds: u64) -> String {
    let Ok(seconds) = i64::try_from(seconds) else { return String::new() };
    let Ok(stamp) = Timestamp::from_second(seconds) else { return String::new() };
    zoned_text(&stamp.to_zoned_datetime(TimeZone::system()))
}

/// The same stamp in an explicit zone, so the field layout is testable.
fn zoned_text(zoned: &ZonedDateTime) -> String {
    format!(
        "{:04}/{:02}/{:02}, {:02}:{:02}:{:02}",
        zoned.year(),
        zoned.month(),
        zoned.day(),
        zoned.hour(),
        zoned.minute(),
        zoned.second()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_in_plain_units() {
        assert_eq!(size_text(999), "999 B");
        assert_eq!(size_text(1024), "1.0 KiB");
        assert_eq!(size_text(1536), "1.5 KiB");
        // One decimal would print "1024.0 KiB" without the carry.
        assert_eq!(size_text(1_048_525), "1.0 MiB");
    }

    #[test]
    fn timestamps_read_as_local_date_and_time() {
        let at = |seconds: u64, offset_hours: i8| {
            let zone = TimeZone::fixed(jiff::tz::offset(offset_hours));
            let stamp = Timestamp::from_second(seconds as i64).expect("a valid second");
            zoned_text(&stamp.to_zoned_datetime(zone))
        };
        assert_eq!(at(0, 0), "1970/01/01, 00:00:00");
        assert_eq!(at(1_000_000_000, 0), "2001/09/09, 01:46:40");
        // The same instant two hours east is the next day, which UTC hid.
        assert_eq!(at(1_700_000_000, 2), "2023/11/15, 00:13:20");
        // A leap day, so the calendar is checked past a plain month boundary.
        assert_eq!(at(1_709_164_800, 0), "2024/02/29, 00:00:00");
    }

    #[test]
    fn a_path_splits_before_its_name() {
        assert_eq!(
            path_parts(Path::new("/d/sub/a.txt")),
            ("/d/sub/".to_owned(), "a.txt".to_owned())
        );
        // A name directly below the root keeps exactly one separator.
        assert_eq!(path_parts(Path::new("/a.txt")), ("/".to_owned(), "a.txt".to_owned()));
        // A trailing separator is not part of the name.
        assert_eq!(path_parts(Path::new("/d/sub/")), ("/d/".to_owned(), "sub".to_owned()));
        assert_eq!(path_parts(Path::new("/")), (String::new(), "/".to_owned()));
    }
}
