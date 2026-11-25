//! Rule types and their compiled form.
//!
//! A [`RuleSet`] is an ordered pipeline applied to each name in turn.
//! Adding a feature means adding a variant here, not a code path in `plan` or
//! `apply`.

use std::ffi::OsStr;
use std::fmt::Write;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use bstr::ByteSlice;
use jiff::ZonedDateTime;
use regex::Regex;
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

use crate::osname;
use text::{
    collapse_spaces, extend_lower, extend_upper, is_word_char, push_capitalized, push_char,
    push_lower, push_title, split_words,
};

mod text;

/// Which part of the name a rule rewrites.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    Stem,
    Ext,
    Whole,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaseStyle {
    Lower,
    Upper,
    /// First character of every word upper, the rest lower. An apostrophe
    /// inside a word does not start a new word, except after a one-letter
    /// prefix (`O'Brien`).
    Title,
    /// First character of the selection upper, the rest lower.
    Sentence,
    /// Words lowercased, joined by `-`.
    Kebab,
    /// Words lowercased, joined by `_`.
    Snake,
    /// Words joined with no separator; the first is lowercase.
    Camel,
    /// Words joined with no separator; every word starts uppercase.
    Pascal,
}

/// Where a rule that generates text puts it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SequencePlace {
    /// Before the selected part.
    Prefix,
    /// After the selected part.
    Suffix,
    /// In place of the selected part.
    #[default]
    Replace,
}

/// Byte fields carry base64 in JSON, because a JSON string cannot hold an
/// arbitrary name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InsertRemoveOp {
    AddPrefix(#[serde(with = "crate::bytes_serde")] Vec<u8>),
    AddSuffix(#[serde(with = "crate::bytes_serde")] Vec<u8>),
    /// Drop the first `n` characters. UTF-8 names only; a character count has
    /// no meaning in a byte string.
    RemovePrefix(usize),
    /// Drop the last `n` characters. UTF-8 names only.
    RemoveSuffix(usize),
    /// Insert `text` before the character at `at`, or at the end when `at` is
    /// past it. UTF-8 names only.
    InsertAt {
        at: usize,
        #[serde(with = "crate::bytes_serde")]
        text: Vec<u8>,
    },
    /// Drop the characters in `start..end`, clamped to the selection. UTF-8
    /// names only.
    RemoveRange {
        start: usize,
        end: usize,
    },
}

/// The date stamp formats a date rule can write.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DateFormat {
    Yyyymmdd,
    Ddmmyyyy,
    MmDdYyyy,
    /// Date and time of day, seconds included.
    YyyymmddHhmmss,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NormalizeOp {
    /// Unicode NFC. Non-UTF-8 names are skipped like every other Normalize op.
    Nfc,
    Trim,
    CollapseSpaces,
    SpacesToUnderscores,
    StripControl,
    /// Spell the text in ASCII, dropping a character that has no ASCII form.
    Ascii,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "rule", rename_all = "snake_case")]
pub enum Rule {
    Regex {
        pattern: String,
        replacement: String,
        target: Target,
        /// Match the path as listed instead of the selected part of the name.
        /// The result may only change the basename: a path that leaves its
        /// directory is invalid rather than a move.
        #[serde(default)]
        path: bool,
    },
    Literal {
        #[serde(with = "crate::bytes_serde")]
        find: Vec<u8>,
        #[serde(with = "crate::bytes_serde")]
        replace: Vec<u8>,
        target: Target,
    },
    Case {
        style: CaseStyle,
        target: Target,
    },
    InsertRemove {
        op: InsertRemoveOp,
        target: Target,
    },
    Sequence {
        start: u64,
        step: u64,
        pad: usize,
        #[serde(default)]
        place: SequencePlace,
        target: Target,
    },
    /// Today's date, captured once when the rule set compiles, so every row in
    /// one preview gets the same stamp.
    Date {
        format: DateFormat,
        place: SequencePlace,
        target: Target,
    },
    Normalize {
        ops: Vec<NormalizeOp>,
        target: Target,
    },
}

/// An ordered pipeline. A new rule set starts empty: the common jobs
/// are reachable without typing a pattern.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleSet {
    pub rules: Vec<Rule>,
}

/// Longest zero-pad a sequence rule may ask for. A larger value builds names no
/// filesystem accepts and, per rule, per row, burns memory before failing.
pub(crate) const MAX_SEQUENCE_PAD: usize = 255;

#[derive(Debug, thiserror::Error)]
pub enum RuleError {
    #[error("replace rule has an invalid pattern: {source}")]
    Regex {
        #[source]
        source: regex::Error,
    },
    #[error("replace rule has an empty find text")]
    EmptyLiteral,
    #[error("number rule is active but its step is zero, so every name gets the same number")]
    ZeroStep,
    #[error("number rule is active with {pad} digits; the most is {max}")]
    PadTooLarge { pad: usize, max: usize },
    #[error("number rule overflows u64 with {entries} entries; lower its start or step")]
    SequenceOverflow { entries: usize },
}

/// A rule with its regex built once per plan call, never per item.
enum CompiledRule {
    Regex { re: Regex, replacement: String, target: Target, path: bool },
    Literal { find: Vec<u8>, replace: Vec<u8>, target: Target },
    Case { style: CaseStyle, target: Target },
    InsertRemove { op: InsertRemoveOp, target: Target },
    Sequence { start: u64, step: u64, pad: usize, place: SequencePlace, target: Target },
    Date { stamp: String, place: SequencePlace, target: Target },
    Normalize { ops: Vec<NormalizeOp>, target: Target },
}

pub(crate) struct CompiledRuleSet {
    rules: Vec<CompiledRule>,
    /// Reused by regex replacement, which otherwise builds a `String` per item.
    expansion: String,
    /// Reused by a path regex, which matches `parent/name` per item.
    path: PathBuf,
}

impl RuleSet {
    pub(crate) fn compile(&self) -> Result<CompiledRuleSet, RuleError> {
        // One clock read per compile keeps every Date rule in a preview on the
        // same stamp.
        let now = ZonedDateTime::now();
        let mut rules = Vec::with_capacity(self.rules.len());
        for rule in &self.rules {
            rules.push(match rule {
                Rule::Regex { pattern, replacement, target, path } => CompiledRule::Regex {
                    re: Regex::new(pattern).map_err(|source| RuleError::Regex { source })?,
                    replacement: replacement.clone(),
                    target: *target,
                    path: *path,
                },
                Rule::Literal { find, replace, target } => {
                    if find.is_empty() {
                        return Err(RuleError::EmptyLiteral);
                    }
                    CompiledRule::Literal {
                        find: find.clone(),
                        replace: replace.clone(),
                        target: *target,
                    }
                }
                Rule::Case { style, target } => {
                    CompiledRule::Case { style: *style, target: *target }
                }
                Rule::InsertRemove { op, target } => {
                    CompiledRule::InsertRemove { op: op.clone(), target: *target }
                }
                Rule::Sequence { start, step, pad, place, target } => {
                    if *step == 0 {
                        return Err(RuleError::ZeroStep);
                    }
                    if *pad > MAX_SEQUENCE_PAD {
                        return Err(RuleError::PadTooLarge { pad: *pad, max: MAX_SEQUENCE_PAD });
                    }
                    CompiledRule::Sequence {
                        start: *start,
                        step: *step,
                        pad: *pad,
                        place: *place,
                        target: *target,
                    }
                }
                Rule::Date { format, place, target } => CompiledRule::Date {
                    stamp: date_stamp(*format, &now),
                    place: *place,
                    target: *target,
                },
                Rule::Normalize { ops, target } => {
                    CompiledRule::Normalize { ops: ops.clone(), target: *target }
                }
            });
        }
        Ok(CompiledRuleSet { rules, expansion: String::new(), path: PathBuf::new() })
    }
}

impl CompiledRuleSet {
    /// Whether any rule reads the entry's position in name order. Only
    /// `Sequence` does, so a pipeline without one needs no order taken at all.
    pub(crate) fn needs_rank(&self) -> bool {
        self.rules.iter().any(|rule| matches!(rule, CompiledRule::Sequence { .. }))
    }

    /// Rejects a Sequence rule whose number would overflow `u64` before the
    /// last entry. Compile cannot know how many entries the plan has.
    pub(crate) fn check_ranks(&self, entries: usize) -> Result<(), RuleError> {
        if entries == 0 {
            return Ok(());
        }
        let last = (entries - 1) as u64;
        for rule in &self.rules {
            if let CompiledRule::Sequence { start, step, .. } = rule {
                let Some(product) = step.checked_mul(last) else {
                    return Err(RuleError::SequenceOverflow { entries });
                };
                if start.checked_add(product).is_none() {
                    return Err(RuleError::SequenceOverflow { entries });
                }
            }
        }
        Ok(())
    }

    /// Runs the pipeline over one name, in order.
    ///
    /// `rank` is the entry's position in name order, which only `Sequence`
    /// uses. The three buffers are borrowed so that a pipeline costs no
    /// allocation beyond the name it produces.
    pub(crate) fn apply(
        &mut self,
        name: &mut Vec<u8>,
        rank: usize,
        sel: &mut Vec<u8>,
        spare: &mut Vec<u8>,
        parent: &Path,
    ) {
        // Disjoint fields, so the expansion and path buffers can be reused per
        // item.
        let expansion = &mut self.expansion;
        let path = &mut self.path;
        for rule in &self.rules {
            let target = rule.target();
            sel.clear();
            if !rule.transform(name, rank, sel, expansion, parent, path) {
                continue;
            }
            if osname::select(name, target) == sel.as_slice() {
                continue;
            }
            osname::splice_into(name, target, sel, spare);
            std::mem::swap(name, spare);
        }
    }
}

/// Appends the generated `token` and the selection in the order `place` asks
/// for, or the token alone when it replaces the selection.
fn push_placed(out: &mut Vec<u8>, sel: &[u8], token: &[u8], place: SequencePlace) {
    match place {
        SequencePlace::Prefix => {
            out.extend_from_slice(token);
            out.extend_from_slice(sel);
        }
        SequencePlace::Suffix => {
            out.extend_from_slice(sel);
            out.extend_from_slice(token);
        }
        SequencePlace::Replace => out.extend_from_slice(token),
    }
}

/// The stamp `format` writes for `now`, read in the datetime's own time zone.
fn date_stamp(format: DateFormat, now: &ZonedDateTime) -> String {
    let (year, month, day) = (now.year(), now.month(), now.day());
    match format {
        DateFormat::Yyyymmdd => format!("{year:04}{month:02}{day:02}"),
        DateFormat::Ddmmyyyy => format!("{day:02}{month:02}{year:04}"),
        DateFormat::MmDdYyyy => format!("{month:02}{day:02}{year:04}"),
        DateFormat::YyyymmddHhmmss => {
            let (hour, minute, second) = (now.hour(), now.minute(), now.second());
            format!("{year:04}{month:02}{day:02}-{hour:02}{minute:02}{second:02}")
        }
    }
}

/// Appends `text` with every match replaced: the same matches and the same
/// `$1` expansion as `Regex::replace_all`, into buffers that are reused.
fn append_replaced(
    re: &Regex,
    replacement: &str,
    text: &str,
    out: &mut Vec<u8>,
    expansion: &mut String,
) {
    let mut rest = 0;
    for captures in re.captures_iter(text) {
        let whole = captures.get(0).expect("capture group 0 always exists");
        out.extend_from_slice(&text.as_bytes()[rest..whole.start()]);
        expansion.clear();
        captures.expand(replacement, expansion);
        out.extend_from_slice(expansion.as_bytes());
        rest = whole.end();
    }
    out.extend_from_slice(&text.as_bytes()[rest..]);
}

/// Keeps a path-form replacement only when it stays in the entry's directory:
/// its basename becomes the name. A different directory keeps its slash, which
/// `plan` rejects as an illegal character instead of moving the file.
fn keep_in_parent(out: &mut Vec<u8>, parent: &Path) {
    if !out.contains(&b'/') {
        return;
    }
    let result = Path::new(OsStr::from_bytes(out));
    if let (Some(dir), Some(name)) = (result.parent(), result.file_name())
        && dir == parent
    {
        let name = name.to_owned();
        out.clear();
        out.extend_from_slice(name.as_bytes());
    }
}

impl CompiledRule {
    fn target(&self) -> Target {
        match self {
            // A path regex replaces the whole name: its result is the name, or
            // a path whose basename becomes it.
            CompiledRule::Regex { path: true, .. } => Target::Whole,
            CompiledRule::Regex { target, .. }
            | CompiledRule::Literal { target, .. }
            | CompiledRule::Case { target, .. }
            | CompiledRule::InsertRemove { target, .. }
            | CompiledRule::Sequence { target, .. }
            | CompiledRule::Date { target, .. }
            | CompiledRule::Normalize { target, .. } => *target,
        }
    }

    /// Writes the rewritten selection into `out`. Returns `false` when the rule
    /// does not apply to this name at all — the UTF-8-only rules skip a name
    /// that is not UTF-8 rather than converting it lossily.
    fn transform(
        &self,
        name: &[u8],
        rank: usize,
        out: &mut Vec<u8>,
        expansion: &mut String,
        parent: &Path,
        path: &mut PathBuf,
    ) -> bool {
        let sel = osname::select(name, self.target());
        match self {
            CompiledRule::Regex { re, replacement, path: use_path, .. } => {
                let text: &str = if *use_path {
                    path.clear();
                    path.push(parent);
                    path.push(OsStr::from_bytes(name));
                    let Some(text) = path.to_str() else {
                        return false;
                    };
                    text
                } else {
                    let Ok(text) = std::str::from_utf8(sel) else {
                        return false;
                    };
                    text
                };
                append_replaced(re, replacement, text, out, expansion);
                if *use_path {
                    keep_in_parent(out, parent);
                }
                true
            }
            CompiledRule::Literal { find, replace, .. } => {
                if find.is_empty() {
                    out.extend_from_slice(sel);
                    return true;
                }
                let mut rest = sel;
                while let Some(at) = rest.find(find.as_slice()) {
                    out.extend_from_slice(&rest[..at]);
                    out.extend_from_slice(replace);
                    rest = &rest[at + find.len()..];
                }
                out.extend_from_slice(rest);
                true
            }
            CompiledRule::Case { style, .. } => {
                let Ok(text) = std::str::from_utf8(sel) else {
                    return false;
                };
                match style {
                    CaseStyle::Lower => extend_lower(out, text),
                    CaseStyle::Upper => extend_upper(out, text),
                    CaseStyle::Title => {
                        let mut start_of_word = true;
                        // One-letter prefixes keep the capital (O'Brien); longer
                        // words do not (don't).
                        let mut word_len = 0usize;
                        for ch in text.chars() {
                            if is_word_char(ch) {
                                if start_of_word {
                                    push_title(out, ch);
                                } else {
                                    push_lower(out, ch);
                                }
                                start_of_word = false;
                                if !is_combining_mark(ch) {
                                    word_len += 1;
                                }
                            } else if ch == '\'' || ch == '’' {
                                push_char(out, ch);
                                start_of_word = word_len <= 1;
                                word_len = 0;
                            } else {
                                push_char(out, ch);
                                start_of_word = true;
                                word_len = 0;
                            }
                        }
                    }
                    CaseStyle::Sentence => {
                        let mut first = true;
                        for ch in text.chars() {
                            if first && ch.is_alphanumeric() {
                                push_title(out, ch);
                                first = false;
                            } else {
                                push_lower(out, ch);
                            }
                        }
                    }
                    CaseStyle::Kebab => {
                        for (at, word) in split_words(text).iter().enumerate() {
                            if at > 0 {
                                out.push(b'-');
                            }
                            extend_lower(out, word);
                        }
                    }
                    CaseStyle::Snake => {
                        for (at, word) in split_words(text).iter().enumerate() {
                            if at > 0 {
                                out.push(b'_');
                            }
                            extend_lower(out, word);
                        }
                    }
                    CaseStyle::Camel => {
                        for (at, word) in split_words(text).iter().enumerate() {
                            if at == 0 {
                                extend_lower(out, word);
                            } else {
                                push_capitalized(out, word);
                            }
                        }
                    }
                    CaseStyle::Pascal => {
                        for word in split_words(text) {
                            push_capitalized(out, word);
                        }
                    }
                }
                true
            }
            CompiledRule::InsertRemove { op, .. } => match op {
                InsertRemoveOp::AddPrefix(prefix) => {
                    out.extend_from_slice(prefix);
                    out.extend_from_slice(sel);
                    true
                }
                InsertRemoveOp::AddSuffix(suffix) => {
                    out.extend_from_slice(sel);
                    out.extend_from_slice(suffix);
                    true
                }
                InsertRemoveOp::RemovePrefix(count) => {
                    let Ok(text) = std::str::from_utf8(sel) else {
                        return false;
                    };
                    let keep = text.char_indices().nth(*count).map_or(text.len(), |(at, _)| at);
                    out.extend_from_slice(&text.as_bytes()[keep..]);
                    true
                }
                InsertRemoveOp::RemoveSuffix(count) => {
                    let Ok(text) = std::str::from_utf8(sel) else {
                        return false;
                    };
                    // The first character that stays is the one before the last
                    // `count`; `nth(0)` is the last character itself.
                    let keep = if *count == 0 {
                        text.len()
                    } else {
                        text.char_indices().rev().nth(*count - 1).map_or(0, |(at, _)| at)
                    };
                    out.extend_from_slice(&text.as_bytes()[..keep]);
                    true
                }
                InsertRemoveOp::InsertAt { at, text } => {
                    let Ok(selection) = std::str::from_utf8(sel) else {
                        return false;
                    };
                    let split =
                        selection.char_indices().nth(*at).map_or(selection.len(), |(at, _)| at);
                    out.extend_from_slice(&selection.as_bytes()[..split]);
                    out.extend_from_slice(text);
                    out.extend_from_slice(&selection.as_bytes()[split..]);
                    true
                }
                InsertRemoveOp::RemoveRange { start, end } => {
                    let Ok(selection) = std::str::from_utf8(sel) else {
                        return false;
                    };
                    let len = selection.len();
                    let from = selection.char_indices().nth(*start).map_or(len, |(at, _)| at);
                    let to = selection.char_indices().nth(*end).map_or(len, |(at, _)| at).max(from);
                    out.extend_from_slice(&selection.as_bytes()[..from]);
                    out.extend_from_slice(&selection.as_bytes()[to..]);
                    true
                }
            },
            CompiledRule::Sequence { start, step, pad, place, .. } => {
                let number = start.saturating_add(step.saturating_mul(rank as u64));
                expansion.clear();
                let _ = write!(expansion, "{number:0width$}", width = *pad);
                push_placed(out, sel, expansion.as_bytes(), *place);
                true
            }
            CompiledRule::Date { stamp, place, .. } => {
                push_placed(out, sel, stamp.as_bytes(), *place);
                true
            }
            CompiledRule::Normalize { ops, .. } => {
                let Ok(text) = std::str::from_utf8(sel) else {
                    return false;
                };
                let mut current = String::from(text);
                for op in ops {
                    current = match op {
                        NormalizeOp::Nfc => current.as_str().nfc().collect(),
                        NormalizeOp::Trim => current.trim().to_string(),
                        NormalizeOp::CollapseSpaces => collapse_spaces(&current),
                        NormalizeOp::SpacesToUnderscores => current.replace(' ', "_"),
                        NormalizeOp::StripControl => {
                            current.chars().filter(|ch| !ch.is_control()).collect()
                        }
                        NormalizeOp::Ascii => ascii_text(&current),
                    };
                }
                out.extend_from_slice(current.as_bytes());
                true
            }
        }
    }
}

/// `text` spelled in ASCII. NFC first, so a decomposed accent is spelled from
/// its composed letter instead of leaving the combining mark behind; a
/// character with no ASCII form (private use, rare scripts) is dropped.
fn ascii_text(text: &str) -> String {
    text.nfc().filter_map(deunicode::deunicode_char).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(rules: Vec<Rule>, name: &str) -> String {
        run_ranked(rules, name, 0)
    }

    fn run_ranked(rules: Vec<Rule>, name: &str, rank: usize) -> String {
        let mut compiled = RuleSet { rules }.compile().expect("rules compile");
        let mut current = name.as_bytes().to_vec();
        let mut sel = Vec::new();
        let mut spare = Vec::new();
        compiled.apply(&mut current, rank, &mut sel, &mut spare, Path::new("/d"));
        String::from_utf8(current).expect("still utf-8")
    }

    #[test]
    fn rules_apply_in_order() {
        let rules = vec![
            Rule::Literal { find: b"IMG_".to_vec(), replace: Vec::new(), target: Target::Stem },
            Rule::Case { style: CaseStyle::Lower, target: Target::Stem },
        ];
        assert_eq!(run(rules, "IMG_Holiday.JPG"), "holiday.JPG");
    }

    #[test]
    fn regex_replacement_matches_replace_all() {
        let cases = [
            (r"IMG_(\d+)", "photo-$1", "IMG_1 IMG_22"),
            (r"\b", "-", "ab cd"),
            (r"a", "b", "aa"),
            (r"x", "y", "ab"),
        ];
        for (pattern, replacement, text) in cases {
            let re = Regex::new(pattern).expect("pattern");
            let mut out = Vec::new();
            let mut expansion = String::new();
            append_replaced(&re, replacement, text, &mut out, &mut expansion);
            assert_eq!(
                String::from_utf8(out).expect("utf-8"),
                re.replace_all(text, replacement).to_string(),
                "pattern {pattern}"
            );
        }
    }

    #[test]
    fn stem_only_rules_leave_the_extension_alone() {
        let rules = vec![Rule::InsertRemove {
            op: InsertRemoveOp::AddPrefix(b"2024-".to_vec()),
            target: Target::Stem,
        }];
        assert_eq!(run(rules, "photo.jpg"), "2024-photo.jpg");
    }

    #[test]
    fn case_title_and_sentence() {
        assert_eq!(
            run(
                vec![Rule::Case { style: CaseStyle::Title, target: Target::Whole }],
                "hello wide world"
            ),
            "Hello Wide World"
        );
        assert_eq!(
            run(
                vec![Rule::Case { style: CaseStyle::Sentence, target: Target::Whole }],
                "HELLO Wide World"
            ),
            "Hello wide world"
        );
    }

    #[test]
    fn title_keeps_apostrophes_inside_words() {
        let title = |name| {
            run(vec![Rule::Case { style: CaseStyle::Title, target: Target::Whole }], name)
        };
        assert_eq!(title("don't stop"), "Don't Stop");
        assert_eq!(title("O'Brien's file"), "O'Brien's File");
        assert_eq!(title("rock'n'roll"), "Rock'n'Roll");
        assert_eq!(title("'tis the season"), "'Tis The Season");
    }

    #[test]
    fn title_case_lowercases_extra_uppercase_chars() {
        let case = |style, name| run(vec![Rule::Case { style, target: Target::Whole }], name);
        assert_eq!(case(CaseStyle::Title, "ßeta"), "Sseta");
        assert_eq!(case(CaseStyle::Sentence, "ßETA"), "Sseta");
        assert_eq!(case(CaseStyle::Pascal, "ßeta"), "Sseta");
    }

    #[test]
    fn case_styles_keep_combining_marks() {
        let case = |style, name| run(vec![Rule::Case { style, target: Target::Stem }], name);
        assert_eq!(case(CaseStyle::Snake, "cafe\u{301}.txt"), "cafe\u{301}.txt");
        assert_eq!(case(CaseStyle::Snake, "cafe\u{301}Bar.txt"), "cafe\u{301}_bar.txt");
        assert_eq!(case(CaseStyle::Title, "cafe\u{301}bar.txt"), "Cafe\u{301}bar.txt");
    }

    #[test]
    fn remove_counts_characters_not_bytes() {
        let rules =
            vec![Rule::InsertRemove { op: InsertRemoveOp::RemovePrefix(2), target: Target::Stem }];
        assert_eq!(run(rules, "héllo.txt"), "llo.txt");
    }

    #[test]
    fn sequence_replaces_the_target_with_a_padded_number() {
        let rules = vec![Rule::Sequence {
            start: 1,
            step: 2,
            pad: 3,
            place: SequencePlace::Replace,
            target: Target::Stem,
        }];
        assert_eq!(run_ranked(rules.clone(), "b.txt", 0), "001.txt");
        assert_eq!(run_ranked(rules, "a.txt", 1), "003.txt");
    }

    #[test]
    fn date_stamps_use_the_chosen_format() {
        let at: ZonedDateTime = "2026-03-04T05:06:07+01:00[Europe/Berlin]".parse().unwrap();
        assert_eq!(date_stamp(DateFormat::Yyyymmdd, &at), "20260304");
        assert_eq!(date_stamp(DateFormat::Ddmmyyyy, &at), "04032026");
        assert_eq!(date_stamp(DateFormat::MmDdYyyy, &at), "03042026");
        assert_eq!(date_stamp(DateFormat::YyyymmddHhmmss, &at), "20260304-050607");
    }

    #[test]
    fn date_rule_places_its_stamp() {
        let stamped = |place| {
            let mut compiled = CompiledRuleSet {
                rules: vec![CompiledRule::Date {
                    stamp: "20260304".into(),
                    place,
                    target: Target::Stem,
                }],
                expansion: String::new(),
                path: PathBuf::new(),
            };
            let mut current = b"photo.jpg".to_vec();
            let mut sel = Vec::new();
            let mut spare = Vec::new();
            compiled.apply(&mut current, 0, &mut sel, &mut spare, Path::new("/d"));
            String::from_utf8(current).unwrap()
        };
        assert_eq!(stamped(SequencePlace::Prefix), "20260304photo.jpg");
        assert_eq!(stamped(SequencePlace::Suffix), "photo20260304.jpg");
    }

    #[test]
    fn normalize_ops_run_in_order() {
        let rules = vec![Rule::Normalize {
            ops: vec![
                NormalizeOp::CollapseSpaces,
                NormalizeOp::Trim,
                NormalizeOp::SpacesToUnderscores,
            ],
            target: Target::Whole,
        }];
        assert_eq!(run(rules, "  a   b  "), "a_b");
    }

    #[test]
    fn normalize_nfc_composes_decomposed_input() {
        let rules = vec![Rule::Normalize { ops: vec![NormalizeOp::Nfc], target: Target::Whole }];
        assert_eq!(run(rules, "cafe\u{301}.txt"), "café.txt");
    }

    #[test]
    fn normalize_ascii_spells_names_in_ascii() {
        let rule = vec![Rule::Normalize { ops: vec![NormalizeOp::Ascii], target: Target::Stem }];
        // A composed and a decomposed accent land on the same spelling.
        assert_eq!(run(rule.clone(), "café.txt"), "cafe.txt");
        assert_eq!(run(rule.clone(), "cafe\u{301}.txt"), "cafe.txt");
        assert_eq!(run(rule.clone(), "Straße.txt"), "Strasse.txt");
        assert_eq!(run(rule.clone(), "Привет.txt"), "Privet.txt");
        // ASCII already passes through untouched.
        assert_eq!(run(rule.clone(), "plain_name.txt"), "plain_name.txt");
        // A character with no ASCII form is dropped, not left as a placeholder.
        assert_eq!(run(rule, "a\u{e000}b.txt"), "ab.txt");
    }

    #[test]
    fn normalize_ascii_turns_chinese_into_ascii_too() {
        let rule = vec![Rule::Normalize { ops: vec![NormalizeOp::Ascii], target: Target::Whole }];
        let spelled = run(rule, "中文.txt");
        assert!(spelled.is_ascii(), "the whole name is ASCII: {spelled}");
        assert_ne!(spelled, ".txt", "the name keeps something");
    }

    #[test]
    fn utf8_only_rules_skip_non_utf8_names() {
        let rules = vec![
            Rule::Case { style: CaseStyle::Upper, target: Target::Whole },
            Rule::Normalize { ops: vec![NormalizeOp::Ascii], target: Target::Whole },
        ];
        let mut compiled = RuleSet { rules }.compile().unwrap();
        let mut current = b"\xff\xfe".to_vec();
        let mut sel = Vec::new();
        let mut spare = Vec::new();
        compiled.apply(&mut current, 0, &mut sel, &mut spare, Path::new("/d"));
        assert_eq!(current, b"\xff\xfe".to_vec());
    }

    #[test]
    fn remove_suffix_drops_exactly_the_count() {
        let trim = |count| {
            vec![Rule::InsertRemove {
                op: InsertRemoveOp::RemoveSuffix(count),
                target: Target::Stem,
            }]
        };
        assert_eq!(run(trim(0), "abcde"), "abcde");
        assert_eq!(run(trim(1), "abcde"), "abcd");
        assert_eq!(run(trim(2), "abcde"), "abc");
        assert_eq!(run(trim(9), "abcde"), "");
        assert_eq!(run(trim(1), "héllo"), "héll");
    }

    #[test]
    fn remove_range_drops_the_characters_between_two_positions() {
        let range = |start, end| {
            vec![Rule::InsertRemove {
                op: InsertRemoveOp::RemoveRange { start, end },
                target: Target::Whole,
            }]
        };
        assert_eq!(run(range(1, 3), "abcdef"), "adef");
        assert_eq!(run(range(1, 2), "héllo"), "hllo");
        assert_eq!(run(range(4, 99), "abc"), "abc");
        assert_eq!(run(range(3, 1), "abc"), "abc");
    }

    #[test]
    fn insert_at_counts_characters_and_clamps() {
        let insert = |at, text: &[u8]| {
            vec![Rule::InsertRemove {
                op: InsertRemoveOp::InsertAt { at, text: text.to_vec() },
                target: Target::Whole,
            }]
        };
        assert_eq!(run(insert(2, b"XY"), "abcd"), "abXYcd");
        assert_eq!(run(insert(0, b"X"), "ab"), "Xab");
        assert_eq!(run(insert(99, b"X"), "ab"), "abX");
        assert_eq!(run(insert(1, b"X"), "héllo"), "hXéllo");
    }

    #[test]
    fn sequence_prefix_and_suffix_place_the_number() {
        let sequence =
            |place, target| vec![Rule::Sequence { start: 7, step: 1, pad: 2, place, target }];
        assert_eq!(
            run_ranked(sequence(SequencePlace::Prefix, Target::Stem), "photo.jpg", 0),
            "07photo.jpg"
        );
        assert_eq!(
            run_ranked(sequence(SequencePlace::Suffix, Target::Stem), "photo.jpg", 0),
            "photo07.jpg"
        );
        assert_eq!(
            run_ranked(sequence(SequencePlace::Suffix, Target::Whole), "photo.jpg", 0),
            "photo.jpg07"
        );
        assert_eq!(
            run_ranked(sequence(SequencePlace::Replace, Target::Stem), "photo.jpg", 0),
            "07.jpg"
        );
    }

    #[test]
    fn case_styles_split_separators_and_camel_humps() {
        let case = |style| vec![Rule::Case { style, target: Target::Whole }];
        assert_eq!(run(case(CaseStyle::Snake), "camelCase"), "camel_case");
        assert_eq!(run(case(CaseStyle::Kebab), "camelCase"), "camel-case");
        assert_eq!(run(case(CaseStyle::Camel), "hello wide world"), "helloWideWorld");
        assert_eq!(run(case(CaseStyle::Pascal), "hello_wide world"), "HelloWideWorld");
        assert_eq!(run(case(CaseStyle::Snake), "HTTPServer file2name"), "httpserver_file2name");
    }

    #[test]
    fn bad_rules_are_rejected_at_compile_time() {
        assert!(matches!(
            RuleSet {
                rules: vec![Rule::Regex {
                    pattern: "(".into(),
                    replacement: String::new(),
                    target: Target::Whole,
                    path: false
                }]
            }
            .compile(),
            Err(RuleError::Regex { .. })
        ));
        assert!(matches!(
            RuleSet {
                rules: vec![Rule::Literal {
                    find: Vec::new(),
                    replace: Vec::new(),
                    target: Target::Whole
                }]
            }
            .compile(),
            Err(RuleError::EmptyLiteral)
        ));
        assert!(matches!(
            RuleSet {
                rules: vec![Rule::Sequence {
                    start: 0,
                    step: 0,
                    pad: 0,
                    place: SequencePlace::Replace,
                    target: Target::Whole,
                }]
            }
            .compile(),
            Err(RuleError::ZeroStep)
        ));
        assert!(matches!(
            RuleSet {
                rules: vec![Rule::Sequence {
                    start: 0,
                    step: 1,
                    pad: MAX_SEQUENCE_PAD + 1,
                    place: SequencePlace::Replace,
                    target: Target::Whole
                }]
            }
            .compile(),
            Err(RuleError::PadTooLarge { .. })
        ));
    }
}
