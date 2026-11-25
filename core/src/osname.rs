//! Byte-level name handling: target selection, splicing, NFC detection and
//! plan-time validation.
//!
//! Names stay `OsStr`/`&[u8]` end to end. Nothing here converts a name lossily.

use std::cmp::Ordering;

use unicode_normalization::UnicodeNormalization;

use crate::plan::InvalidKind;
use crate::rule::Target;

/// Longest filename component on Linux.
pub const NAME_MAX: usize = 255;
/// Longest path the kernel accepts.
pub const PATH_MAX: usize = 4096;

/// How a name splits into stem and extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Parts<'a> {
    pub stem: &'a [u8],
    pub ext: &'a [u8],
}

/// Split: the extension is everything after the final `.` when that
/// `.` is neither the first nor the last byte of the name. Otherwise there is
/// no extension and the stem is the whole name, so `.bashrc` and `foo.` have no
/// extension while `archive.tar.gz` splits into `archive.tar` and `gz`.
pub fn parts(name: &[u8]) -> Parts<'_> {
    let none = Parts { stem: name, ext: &[] };
    let Some(dot) = name.iter().rposition(|b| *b == b'.') else {
        return none;
    };
    if dot == 0 || dot + 1 == name.len() {
        return none;
    }
    Parts { stem: &name[..dot], ext: &name[dot + 1..] }
}

/// The bytes a rule rewrites for `target`.
pub fn select(name: &[u8], target: Target) -> &[u8] {
    match target {
        Target::Whole => name,
        Target::Stem => parts(name).stem,
        Target::Ext => parts(name).ext,
    }
}

/// Rebuild `name` with the selection replaced by `sel`.
///
/// An empty `Ext` result drops the dot rather than leaving a trailing one, so
/// clearing an extension turns `foo.txt` into `foo`.
pub fn splice_into(name: &[u8], target: Target, sel: &[u8], out: &mut Vec<u8>) {
    out.clear();
    match target {
        Target::Whole => out.extend_from_slice(sel),
        Target::Stem => {
            let split = parts(name);
            out.extend_from_slice(sel);
            if !split.ext.is_empty() {
                out.push(b'.');
                out.extend_from_slice(split.ext);
            }
        }
        Target::Ext => {
            let split = parts(name);
            out.extend_from_slice(split.stem);
            if !sel.is_empty() {
                out.push(b'.');
                out.extend_from_slice(sel);
            }
        }
    }
}

/// Whether the name is already NFC. Non-UTF-8 names cannot be normalised and
/// are never flagged.
pub fn is_nfc(name: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(name) else {
        return true;
    };
    text.chars().eq(text.nfc())
}

/// Compares two names the way a case-insensitive mount would: Unicode lower
/// case for UTF-8, ASCII only for bytes that are not text. Names that are not
/// UTF-8 sort apart from text, so the two classes never compare as one.
///
/// This is the allocation-free form of [`case_fold`]; the two must agree.
pub(crate) fn folded_order(left: &[u8], right: &[u8]) -> Ordering {
    match (std::str::from_utf8(left), std::str::from_utf8(right)) {
        (Ok(left), Ok(right)) => left
            .chars()
            .flat_map(char::to_lowercase)
            .cmp(right.chars().flat_map(char::to_lowercase)),
        (Ok(_), Err(_)) => Ordering::Less,
        (Err(_), Ok(_)) => Ordering::Greater,
        (Err(_), Err(_)) => {
            let left = left.iter().map(|byte| byte.to_ascii_lowercase());
            let right = right.iter().map(|byte| byte.to_ascii_lowercase());
            left.cmp(right)
        }
    }
}

/// A name folded for comparison on a case-insensitive mount: Unicode lower
/// case for UTF-8, ASCII only for bytes that are not text.
///
/// shortcut: this lower-cases, it does not apply full Unicode case folding
/// (`ß` does not fold to `ss`); add a folding table if those names matter.
pub(crate) fn case_fold(name: &[u8]) -> Vec<u8> {
    match std::str::from_utf8(name) {
        Ok(text) => text.chars().flat_map(char::to_lowercase).collect::<String>().into_bytes(),
        Err(_) => name.to_ascii_lowercase(),
    }
}

/// A rename whose source and destination differ only by case (§8). The
/// destination may sit on a case-insensitive mount, so such a row is staged
/// through a temp name instead of a direct `RENAME_NOREPLACE`.
pub fn is_case_only(src: &[u8], dst: &[u8]) -> bool {
    src != dst && folded_order(src, dst) == Ordering::Equal
}

/// Plan-time validation: a bad destination is an `Invalid` row up
/// front, not an error halfway through a batch.
pub fn validate(name: &[u8], path_len: usize) -> Option<InvalidKind> {
    if name.is_empty() {
        return Some(InvalidKind::Empty);
    }
    if name == b"." || name == b".." {
        return Some(InvalidKind::IllegalChar);
    }
    if name.iter().any(|byte| *byte == b'/' || *byte == 0) {
        return Some(InvalidKind::IllegalChar);
    }
    if name.len() > NAME_MAX {
        return Some(InvalidKind::NameTooLong);
    }
    if path_len >= PATH_MAX {
        return Some(InvalidKind::PathTooLong);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(name: &str) -> (&[u8], &[u8]) {
        let parts = parts(name.as_bytes());
        (parts.stem, parts.ext)
    }

    #[test]
    fn extension_is_after_the_final_inner_dot() {
        assert_eq!(split("archive.tar.gz"), (b"archive.tar".as_slice(), b"gz".as_slice()));
        assert_eq!(split("photo.jpg"), (b"photo".as_slice(), b"jpg".as_slice()));
    }

    #[test]
    fn leading_and_trailing_dots_are_not_extensions() {
        assert_eq!(split(".bashrc"), (b".bashrc".as_slice(), b"".as_slice()));
        assert_eq!(split("foo."), (b"foo.".as_slice(), b"".as_slice()));
        assert_eq!(split(".."), (b"..".as_slice(), b"".as_slice()));
        assert_eq!(split("file"), (b"file".as_slice(), b"".as_slice()));
    }

    #[test]
    fn splicing_an_empty_extension_drops_the_dot() {
        let mut out = Vec::new();
        splice_into(b"foo.txt", Target::Ext, b"", &mut out);
        assert_eq!(out, b"foo");

        splice_into(b"foo", Target::Ext, b"", &mut out);
        assert_eq!(out, b"foo");
    }

    #[test]
    fn splicing_keeps_the_untouched_half() {
        let mut out = Vec::new();
        splice_into(b"archive.tar.gz", Target::Stem, b"backup", &mut out);
        assert_eq!(out, b"backup.gz");

        splice_into(b"archive.tar.gz", Target::Ext, b"xz", &mut out);
        assert_eq!(out, b"archive.tar.xz");

        splice_into(b"archive.tar.gz", Target::Whole, b"other", &mut out);
        assert_eq!(out, b"other");
    }

    #[test]
    fn nfc_detection_ignores_non_utf8() {
        assert!(is_nfc("café".as_bytes()));
        assert!(is_nfc(b"\xff\xfe"));
        assert!(!is_nfc("cafe\u{301}".as_bytes()));
    }

    #[test]
    fn case_only_detection_needs_a_real_difference() {
        assert!(is_case_only(b"readme.md", b"README.md"));
        assert!(!is_case_only(b"readme.md", b"readme.md"));
        assert!(!is_case_only(b"readme.md", b"other.md"));
    }

    #[test]
    fn case_only_detection_folds_unicode_letters() {
        assert!(is_case_only("Ärger.txt".as_bytes(), "ärger.txt".as_bytes()));
        assert!(is_case_only("café.txt".as_bytes(), "CAFÉ.txt".as_bytes()));
        assert!(!is_case_only("Ärger.txt".as_bytes(), "Arger.txt".as_bytes()));
        // Non-UTF-8 bytes can only fold ASCII, but that still counts.
        assert!(is_case_only(b"CAF\xe9", b"caf\xe9"));
    }

    #[test]
    fn folded_order_agrees_with_case_fold() {
        let pairs: [(&[u8], &[u8]); 5] = [
            (b"readme", b"README"),
            ("Ärger".as_bytes(), "ärger".as_bytes()),
            (b"caf\xe9", b"CAF\xe9"),
            (b"cafe", b"caf\xe9"),
            (b"a", b"b"),
        ];
        for (left, right) in pairs {
            assert_eq!(
                folded_order(left, right) == Ordering::Equal,
                case_fold(left) == case_fold(right),
                "{} vs {}",
                String::from_utf8_lossy(left),
                String::from_utf8_lossy(right)
            );
        }
    }

    #[test]
    fn validation_rejects_empty_separators_and_overlong_names() {
        assert_eq!(validate(b"", 1), Some(InvalidKind::Empty));
        assert_eq!(validate(b".", 1), Some(InvalidKind::IllegalChar));
        assert_eq!(validate(b"..", 2), Some(InvalidKind::IllegalChar));
        assert_eq!(validate(b"a/b", 3), Some(InvalidKind::IllegalChar));
        assert_eq!(validate(b"a\0b", 3), Some(InvalidKind::IllegalChar));
        assert_eq!(validate(&[b'a'; NAME_MAX + 1], 0), Some(InvalidKind::NameTooLong));
        assert_eq!(validate(b"ok", PATH_MAX - 1), None);
        assert_eq!(validate(b"ok", PATH_MAX), Some(InvalidKind::PathTooLong));
        assert_eq!(validate(b"ok", 10), None);
    }
}
