//! Rule types and their compiled form.
//!
//! A [`RuleSet`] is an ordered pipeline applied to each name in turn.
//! Adding a feature means adding a variant here, not a code path in `plan` or
//! `apply`.

use std::io::Write;

use bstr::ByteSlice;
use regex::Regex;
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

use crate::osname;

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
    /// First character of every word upper, the rest lower.
    Title,
    /// First character of the selection upper, the rest lower.
    Sentence,
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
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "rule", rename_all = "snake_case")]
pub enum Rule {
    Regex {
        pattern: String,
        replacement: String,
        target: Target,
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
    #[error("rule {index}: invalid regex /{pattern}/: {source}")]
    Regex {
        index: usize,
        pattern: String,
        #[source]
        source: regex::Error,
    },
    #[error("rule {index}: literal `find` is empty, so the rule can never match")]
    EmptyLiteral { index: usize },
    #[error("rule {index}: sequence `step` is zero, so every name gets the same number")]
    ZeroStep { index: usize },
    #[error("rule {index}: sequence `pad` is {pad}, above the maximum of {max}")]
    PadTooLarge { index: usize, pad: usize, max: usize },
}

/// A rule with its regex built once per plan call, never per item.
enum CompiledRule {
    Regex { re: Regex, replacement: String, target: Target },
    Literal { find: Vec<u8>, replace: Vec<u8>, target: Target },
    Case { style: CaseStyle, target: Target },
    InsertRemove { op: InsertRemoveOp, target: Target },
    Sequence { start: u64, step: u64, pad: usize, target: Target },
    Normalize { ops: Vec<NormalizeOp>, target: Target },
}

pub(crate) struct CompiledRuleSet {
    rules: Vec<CompiledRule>,
    /// Reused by regex replacement, which otherwise builds a `String` per item.
    expansion: String,
}

impl RuleSet {
    pub(crate) fn compile(&self) -> Result<CompiledRuleSet, RuleError> {
        let mut rules = Vec::with_capacity(self.rules.len());
        for (index, rule) in self.rules.iter().enumerate() {
            rules.push(match rule {
                Rule::Regex { pattern, replacement, target } => CompiledRule::Regex {
                    re: Regex::new(pattern).map_err(|source| RuleError::Regex {
                        index,
                        pattern: pattern.clone(),
                        source,
                    })?,
                    replacement: replacement.clone(),
                    target: *target,
                },
                Rule::Literal { find, replace, target } => {
                    if find.is_empty() {
                        return Err(RuleError::EmptyLiteral { index });
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
                Rule::Sequence { start, step, pad, target } => {
                    if *step == 0 {
                        return Err(RuleError::ZeroStep { index });
                    }
                    if *pad > MAX_SEQUENCE_PAD {
                        return Err(RuleError::PadTooLarge {
                            index,
                            pad: *pad,
                            max: MAX_SEQUENCE_PAD,
                        });
                    }
                    CompiledRule::Sequence {
                        start: *start,
                        step: *step,
                        pad: *pad,
                        target: *target,
                    }
                }
                Rule::Normalize { ops, target } => {
                    CompiledRule::Normalize { ops: ops.clone(), target: *target }
                }
            });
        }
        Ok(CompiledRuleSet { rules, expansion: String::new() })
    }
}

impl CompiledRuleSet {
    /// Whether any rule reads the entry's position in name order. Only
    /// `Sequence` does, so a pipeline without one needs no order taken at all.
    pub(crate) fn needs_rank(&self) -> bool {
        self.rules.iter().any(|rule| matches!(rule, CompiledRule::Sequence { .. }))
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
    ) {
        // Disjoint fields, so the expansion buffer can be reused per item.
        let expansion = &mut self.expansion;
        for rule in &self.rules {
            let target = rule.target();
            sel.clear();
            if !rule.transform(name, rank, sel, expansion) {
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

impl CompiledRule {
    fn target(&self) -> Target {
        match self {
            CompiledRule::Regex { target, .. }
            | CompiledRule::Literal { target, .. }
            | CompiledRule::Case { target, .. }
            | CompiledRule::InsertRemove { target, .. }
            | CompiledRule::Sequence { target, .. }
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
    ) -> bool {
        let sel = osname::select(name, self.target());
        match self {
            CompiledRule::Regex { re, replacement, .. } => {
                let Ok(text) = std::str::from_utf8(sel) else {
                    return false;
                };
                // The same matches and the same `$1` expansion as
                // `Regex::replace_all`, into buffers that are reused.
                let mut rest = 0;
                for captures in re.captures_iter(text) {
                    let Some(whole) = captures.get(0) else {
                        continue;
                    };
                    out.extend_from_slice(&text.as_bytes()[rest..whole.start()]);
                    expansion.clear();
                    captures.expand(replacement, expansion);
                    out.extend_from_slice(expansion.as_bytes());
                    rest = whole.end();
                }
                out.extend_from_slice(&text.as_bytes()[rest..]);
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
                        for ch in text.chars() {
                            if ch.is_alphanumeric() {
                                if start_of_word {
                                    push_upper(out, ch);
                                } else {
                                    push_lower(out, ch);
                                }
                                start_of_word = false;
                            } else {
                                push_char(out, ch);
                                start_of_word = true;
                            }
                        }
                    }
                    CaseStyle::Sentence => {
                        let mut first = true;
                        for ch in text.chars() {
                            if first && ch.is_alphanumeric() {
                                push_upper(out, ch);
                                first = false;
                            } else {
                                push_lower(out, ch);
                            }
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
                    let keep = text.char_indices().rev().nth(*count).map_or(0, |(at, _)| at);
                    out.extend_from_slice(&text.as_bytes()[..keep]);
                    true
                }
            },
            CompiledRule::Sequence { start, step, pad, .. } => {
                let number = start.saturating_add(step.saturating_mul(rank as u64));
                let _ = write!(out, "{number:0width$}", width = *pad);
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
                    };
                }
                out.extend_from_slice(current.as_bytes());
                true
            }
        }
    }
}

/// Uppercase and lowercase map one character to several (`ß` to `SS`), so they
/// are pushed character by character instead of through a `String`.
fn extend_upper(out: &mut Vec<u8>, text: &str) {
    for ch in text.chars() {
        push_upper(out, ch);
    }
}

fn extend_lower(out: &mut Vec<u8>, text: &str) {
    for ch in text.chars() {
        push_lower(out, ch);
    }
}

fn push_upper(out: &mut Vec<u8>, ch: char) {
    for upper in ch.to_uppercase() {
        push_char(out, upper);
    }
}

fn push_lower(out: &mut Vec<u8>, ch: char) {
    for lower in ch.to_lowercase() {
        push_char(out, lower);
    }
}

fn push_char(out: &mut Vec<u8>, ch: char) {
    let mut buffer = [0u8; 4];
    out.extend_from_slice(ch.encode_utf8(&mut buffer).as_bytes());
}

fn collapse_spaces(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_space = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !in_space {
                out.push(' ');
                in_space = true;
            }
        } else {
            out.push(ch);
            in_space = false;
        }
    }
    out
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
        compiled.apply(&mut current, rank, &mut sel, &mut spare);
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
    fn remove_counts_characters_not_bytes() {
        let rules =
            vec![Rule::InsertRemove { op: InsertRemoveOp::RemovePrefix(2), target: Target::Stem }];
        assert_eq!(run(rules, "héllo.txt"), "llo.txt");
    }

    #[test]
    fn sequence_replaces_the_target_with_a_padded_number() {
        let rules = vec![Rule::Sequence { start: 1, step: 2, pad: 3, target: Target::Stem }];
        assert_eq!(run_ranked(rules.clone(), "b.txt", 0), "001.txt");
        assert_eq!(run_ranked(rules, "a.txt", 1), "003.txt");
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
    fn utf8_only_rules_skip_non_utf8_names() {
        let rules = vec![Rule::Case { style: CaseStyle::Upper, target: Target::Whole }];
        let mut compiled = RuleSet { rules }.compile().unwrap();
        let mut current = b"\xff\xfe".to_vec();
        let mut sel = Vec::new();
        let mut spare = Vec::new();
        compiled.apply(&mut current, 0, &mut sel, &mut spare);
        assert_eq!(current, b"\xff\xfe".to_vec());
    }

    #[test]
    fn bad_rules_are_rejected_at_compile_time() {
        assert!(matches!(
            RuleSet {
                rules: vec![Rule::Regex {
                    pattern: "(".into(),
                    replacement: String::new(),
                    target: Target::Whole
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
            Err(RuleError::EmptyLiteral { .. })
        ));
        assert!(matches!(
            RuleSet {
                rules: vec![Rule::Sequence { start: 0, step: 0, pad: 0, target: Target::Whole }]
            }
            .compile(),
            Err(RuleError::ZeroStep { .. })
        ));
        assert!(matches!(
            RuleSet {
                rules: vec![Rule::Sequence {
                    start: 0,
                    step: 1,
                    pad: MAX_SEQUENCE_PAD + 1,
                    target: Target::Whole
                }]
            }
            .compile(),
            Err(RuleError::PadTooLarge { .. })
        ));
    }
}
