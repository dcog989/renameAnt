//! Which entries the pipeline runs on.
//!
//! A filter selects entries; it never rewrites a name, so it is not a [`Rule`]
//! A caller builds the list `plan()` sees by keeping what a compiled
//! filter matches, so a filtered-out entry leaves the pipeline while its name
//! stays on disk and in the scan's [`DirIndex`] — a row that wanted that name is
//! still `TargetExists`, exactly as for a skipped row.
//!
//! [`Rule`]: crate::rule::Rule

use std::os::unix::ffi::OsStrExt;

use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

use crate::scan::{Entry, Kind};

/// Which entry kinds a filter keeps: two independent bits, because the window
/// offers a checkbox each and unticking both is a filter that keeps nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KindFilter {
    pub dirs: bool,
    /// Everything that is not a directory: files and symlinks.
    pub files: bool,
}

impl Default for KindFilter {
    fn default() -> Self {
        KindFilter { dirs: true, files: true }
    }
}

/// An entry filter. Empty by default, so it keeps everything.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Filter {
    pub kinds: KindFilter,
    /// Matched against the whole basename. Empty means no pattern. A non-UTF-8
    /// name cannot match a UTF-8 pattern, so a pattern leaves those entries out.
    pub pattern: String,
    /// Match `pattern` as a regex instead of plain text. Off by default, so a
    /// plain word matches a substring of the name.
    #[serde(default)]
    pub regex: bool,
    /// Match the pattern's case exactly. Off by default, so a name is found
    /// whatever its case.
    #[serde(default)]
    pub case_sensitive: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum FilterError {
    #[error("filter pattern is not a valid regex: {0}")]
    Regex(#[from] regex::Error),
}

/// A [`Filter`] with its pattern built once, never per entry.
pub struct CompiledFilter {
    kinds: KindFilter,
    pattern: Option<Regex>,
}

impl Filter {
    /// Whether this filter keeps every entry, so a caller can skip compiling it.
    pub fn is_inactive(&self) -> bool {
        self.kinds == KindFilter::default() && self.pattern.is_empty()
    }

    /// The pattern is compiled as a regex either way — escaped as plain text
    /// when `regex` is off — so one case-folding rule covers both.
    pub fn compile(&self) -> Result<CompiledFilter, FilterError> {
        let pattern = if self.pattern.is_empty() {
            None
        } else {
            let source = match self.regex {
                true => self.pattern.clone(),
                false => regex::escape(&self.pattern),
            };
            let regex =
                RegexBuilder::new(&source).case_insensitive(!self.case_sensitive).build()?;
            Some(regex)
        };
        Ok(CompiledFilter { kinds: self.kinds, pattern })
    }
}

impl CompiledFilter {
    pub fn matches(&self, entry: &Entry) -> bool {
        let kind_matches = match entry.kind {
            Kind::Dir => self.kinds.dirs,
            // A symlink is not a directory, so it is a file here.
            Kind::File | Kind::Symlink => self.kinds.files,
        };
        let Some(pattern) = &self.pattern else {
            return kind_matches;
        };
        // A non-UTF-8 name cannot match a UTF-8 pattern, so a pattern leaves it
        // out.
        let Ok(name) = std::str::from_utf8(entry.name.as_bytes()) else {
            return false;
        };
        kind_matches && pattern.is_match(name)
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    use super::*;

    fn entry(name: &str, kind: Kind) -> Entry {
        Entry { path: format!("/d/{name}").into(), name: OsString::from(name), kind }
    }

    fn keeps(filter: &Filter, entry: &Entry) -> bool {
        filter.compile().expect("filter compiles").matches(entry)
    }

    #[test]
    fn kinds_select_directories_and_files_independently() {
        let file = entry("a.txt", Kind::File);
        let dir = entry("sub", Kind::Dir);
        let link = entry("link", Kind::Symlink);

        // "files" is everything that is not a directory, so it keeps the link.
        let files = Filter { kinds: KindFilter { dirs: false, files: true }, ..Filter::default() };
        assert!(keeps(&files, &file) && keeps(&files, &link) && !keeps(&files, &dir));

        let dirs = Filter { kinds: KindFilter { dirs: true, files: false }, ..Filter::default() };
        assert!(keeps(&dirs, &dir) && !keeps(&dirs, &file) && !keeps(&dirs, &link));

        // Both unticked keeps nothing, which leaves the plan empty rather than
        // falling back to everything.
        let neither =
            Filter { kinds: KindFilter { dirs: false, files: false }, ..Filter::default() };
        assert!(!keeps(&neither, &file) && !keeps(&neither, &dir) && !keeps(&neither, &link));
        assert!(!neither.is_inactive());
        assert!(Filter::default().is_inactive());
    }

    #[test]
    fn a_literal_pattern_matches_a_substring_and_a_regex_only_when_asked() {
        let literal = Filter { pattern: "txt".into(), ..Filter::default() };
        assert!(keeps(&literal, &entry("a.txt", Kind::File)));
        assert!(!keeps(&literal, &entry("a.jpg", Kind::File)));
        // Metacharacters are literal text unless the regex flag is set.
        let dotted = Filter { pattern: "a.b".into(), ..Filter::default() };
        assert!(keeps(&dotted, &entry("a.b", Kind::File)));
        assert!(!keeps(&dotted, &entry("axb", Kind::File)));

        let regex = Filter { pattern: r"\.txt$".into(), regex: true, ..Filter::default() };
        assert!(keeps(&regex, &entry("a.txt", Kind::File)));
        assert!(!keeps(&regex, &entry("a.jpg", Kind::File)));
        assert!(keeps(
            &Filter { pattern: "a.b".into(), regex: true, ..Filter::default() },
            &entry("axb", Kind::File)
        ));
    }

    #[test]
    fn case_is_ignored_until_the_filter_asks_for_it() {
        let insensitive = Filter { pattern: "IMG".into(), ..Filter::default() };
        assert!(keeps(&insensitive, &entry("img_01.jpg", Kind::File)));
        // Case folding reaches past ASCII, so an umlaut matches its pair.
        assert!(keeps(
            &Filter { pattern: "äpfel".into(), ..Filter::default() },
            &entry("ÄPFEL", Kind::File)
        ));

        let sensitive = Filter { pattern: "IMG".into(), case_sensitive: true, ..Filter::default() };
        assert!(keeps(&sensitive, &entry("IMG_01.jpg", Kind::File)));
        assert!(!keeps(&sensitive, &entry("img_01.jpg", Kind::File)));

        // The regex flag case-folds the same way.
        let regex = Filter { pattern: "^img".into(), regex: true, ..Filter::default() };
        assert!(keeps(&regex, &entry("IMG_01.jpg", Kind::File)));
        let exact = Filter {
            pattern: "^img".into(),
            regex: true,
            case_sensitive: true,
            ..Filter::default()
        };
        assert!(!keeps(&exact, &entry("IMG_01.jpg", Kind::File)));
    }

    #[test]
    fn a_pattern_cannot_match_a_non_utf8_name() {
        let raw = OsString::from_vec(b"caf\xe9".to_vec());
        let name = Entry { path: "/d/x".into(), name: raw, kind: Kind::File };

        assert!(!keeps(&Filter { pattern: "a".into(), ..Filter::default() }, &name));
        // With no pattern it is kept: nothing about the kind check is UTF-8.
        assert!(keeps(&Filter::default(), &name));
    }

    #[test]
    fn an_empty_filter_keeps_everything_and_a_bad_pattern_is_an_error() {
        assert!(Filter::default().is_inactive());
        let files = Filter {
            kinds: KindFilter { dirs: false, ..KindFilter::default() },
            ..Filter::default()
        };
        assert!(files.compile().is_ok());
        assert!(
            Filter { pattern: "(".into(), regex: true, ..Filter::default() }.compile().is_err()
        );
        // The same text is a valid literal pattern.
        assert!(Filter { pattern: "(".into(), ..Filter::default() }.compile().is_ok());
    }
}
