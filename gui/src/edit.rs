//! A rule as the editor holds it, and its mapping to a `core` `Rule`.
//!
//! A rule is edited as plain fields rather than as a `Rule` enum, so switching
//! its kind never throws away what is already typed.

use renameant_core::rule::{CaseStyle, InsertRemoveOp, NormalizeOp, Rule, Target};

/// The normalisations the editor offers, in the order `Normalize` applies them.
pub const NORMALIZE_OPS: [(NormalizeOp, &str); 5] = [
    (NormalizeOp::Nfc, "Unicode NFC"),
    (NormalizeOp::Trim, "trim"),
    (NormalizeOp::CollapseSpaces, "collapse spaces"),
    (NormalizeOp::SpacesToUnderscores, "spaces to underscores"),
    (NormalizeOp::StripControl, "strip control characters"),
];

/// The rule types the editor can build. One `Rule` variant each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleKind {
    Regex,
    Literal,
    Case,
    InsertRemove,
    Sequence,
    Normalize,
}

impl RuleKind {
    pub const ALL: [RuleKind; 6] = [
        RuleKind::Regex,
        RuleKind::Literal,
        RuleKind::Case,
        RuleKind::InsertRemove,
        RuleKind::Sequence,
        RuleKind::Normalize,
    ];

    pub fn label(self) -> &'static str {
        match self {
            RuleKind::Regex => "regex replace",
            RuleKind::Literal => "literal replace",
            RuleKind::Case => "change case",
            RuleKind::InsertRemove => "add or remove text",
            RuleKind::Sequence => "number in sequence",
            RuleKind::Normalize => "normalize",
        }
    }
}

/// Which `InsertRemoveOp` the editor is building.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InsertKind {
    AddPrefix,
    AddSuffix,
    RemovePrefix,
    RemoveSuffix,
}

impl InsertKind {
    pub const ALL: [InsertKind; 4] = [
        InsertKind::AddPrefix,
        InsertKind::AddSuffix,
        InsertKind::RemovePrefix,
        InsertKind::RemoveSuffix,
    ];

    pub fn label(self) -> &'static str {
        match self {
            InsertKind::AddPrefix => "prefix",
            InsertKind::AddSuffix => "suffix",
            InsertKind::RemovePrefix => "remove prefix",
            InsertKind::RemoveSuffix => "remove suffix",
        }
    }

    /// Whether the op is byte-level, so it also reaches a name that is not
    /// UTF-8.
    pub fn is_byte(self) -> bool {
        matches!(self, InsertKind::AddPrefix | InsertKind::AddSuffix)
    }
}

/// One rule as the editor holds it: flat fields, so no widget has to own half a
/// `Rule` enum and every field stays editable in place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleEdit {
    /// A rule that is off stays in the list and in its place; it is skipped when
    /// the pipeline is compiled.
    pub enabled: bool,
    pub kind: RuleKind,
    pub target: Target,
    /// The regex pattern, the literal `find`, or the added prefix/suffix.
    pub value: String,
    /// The regex or literal replacement.
    pub replacement: String,
    pub case: CaseStyle,
    pub insert: InsertKind,
    pub count: u64,
    pub start: u64,
    pub step: u64,
    pub pad: u64,
    pub normalize: [bool; NORMALIZE_OPS.len()],
}

impl Default for RuleEdit {
    /// A new rule changes nothing until it is filled in, and needs no pattern
    /// typed first.
    fn default() -> Self {
        RuleEdit {
            enabled: true,
            kind: RuleKind::InsertRemove,
            target: Target::Whole,
            value: String::new(),
            replacement: String::new(),
            case: CaseStyle::Lower,
            insert: InsertKind::AddPrefix,
            count: 1,
            start: 1,
            step: 1,
            pad: 3,
            normalize: [false; NORMALIZE_OPS.len()],
        }
    }
}

impl RuleEdit {
    /// The `Rule` this editor stands for. Infallible: a pattern that will not
    /// compile is `plan`'s error to report.
    pub fn to_rule(&self) -> Rule {
        match self.kind {
            RuleKind::Regex => Rule::Regex {
                pattern: self.value.clone(),
                replacement: self.replacement.clone(),
                target: self.target,
            },
            RuleKind::Literal => Rule::Literal {
                find: self.value.clone().into_bytes(),
                replace: self.replacement.clone().into_bytes(),
                target: self.target,
            },
            RuleKind::Case => Rule::Case { style: self.case, target: self.target },
            RuleKind::InsertRemove => Rule::InsertRemove {
                op: match self.insert {
                    InsertKind::AddPrefix => {
                        InsertRemoveOp::AddPrefix(self.value.clone().into_bytes())
                    }
                    InsertKind::AddSuffix => {
                        InsertRemoveOp::AddSuffix(self.value.clone().into_bytes())
                    }
                    InsertKind::RemovePrefix => InsertRemoveOp::RemovePrefix(self.count as usize),
                    InsertKind::RemoveSuffix => InsertRemoveOp::RemoveSuffix(self.count as usize),
                },
                target: self.target,
            },
            RuleKind::Sequence => Rule::Sequence {
                start: self.start,
                step: self.step,
                pad: self.pad as usize,
                target: self.target,
            },
            RuleKind::Normalize => Rule::Normalize {
                ops: NORMALIZE_OPS
                    .iter()
                    .enumerate()
                    .filter(|(at, _)| self.normalize[*at])
                    .map(|(_, (op, _))| *op)
                    .collect(),
                target: self.target,
            },
        }
    }

    /// The inverse of [`RuleEdit::to_rule`], for a rule loaded from disk. A
    /// literal's bytes come back as text: the editor can only ever produce
    /// UTF-8 there, so nothing it saved is lost.
    pub fn from_rule(rule: &Rule, enabled: bool) -> RuleEdit {
        let mut edit = RuleEdit { enabled, ..RuleEdit::default() };
        match rule {
            Rule::Regex { pattern, replacement, target } => {
                edit.kind = RuleKind::Regex;
                edit.value = pattern.clone();
                edit.replacement = replacement.clone();
                edit.target = *target;
            }
            Rule::Literal { find, replace, target } => {
                edit.kind = RuleKind::Literal;
                edit.value = String::from_utf8_lossy(find).into_owned();
                edit.replacement = String::from_utf8_lossy(replace).into_owned();
                edit.target = *target;
            }
            Rule::Case { style, target } => {
                edit.kind = RuleKind::Case;
                edit.case = *style;
                edit.target = *target;
            }
            Rule::InsertRemove { op, target } => {
                edit.kind = RuleKind::InsertRemove;
                edit.target = *target;
                match op {
                    InsertRemoveOp::AddPrefix(bytes) => {
                        edit.insert = InsertKind::AddPrefix;
                        edit.value = String::from_utf8_lossy(bytes).into_owned();
                    }
                    InsertRemoveOp::AddSuffix(bytes) => {
                        edit.insert = InsertKind::AddSuffix;
                        edit.value = String::from_utf8_lossy(bytes).into_owned();
                    }
                    InsertRemoveOp::RemovePrefix(count) => {
                        edit.insert = InsertKind::RemovePrefix;
                        edit.count = *count as u64;
                    }
                    InsertRemoveOp::RemoveSuffix(count) => {
                        edit.insert = InsertKind::RemoveSuffix;
                        edit.count = *count as u64;
                    }
                }
            }
            Rule::Sequence { start, step, pad, target } => {
                edit.kind = RuleKind::Sequence;
                edit.start = *start;
                edit.step = *step;
                edit.pad = *pad as u64;
                edit.target = *target;
            }
            Rule::Normalize { ops, target } => {
                edit.kind = RuleKind::Normalize;
                edit.target = *target;
                for op in ops {
                    if let Some(at) = NORMALIZE_OPS.iter().position(|(known, _)| known == op) {
                        edit.normalize[at] = true;
                    }
                }
            }
        }
        edit
    }
}

/// The pipeline a fresh start shows: one card per rule type, all off, so they
/// always display without changing anything until the user turns one on. The
/// user adds custom rules on top.
pub fn default_rules() -> Vec<RuleEdit> {
    RuleKind::ALL
        .iter()
        .map(|kind| RuleEdit { enabled: false, kind: *kind, ..RuleEdit::default() })
        .collect()
}

pub fn target_label(target: Target) -> &'static str {
    match target {
        Target::Stem => "stem",
        Target::Ext => "extension",
        Target::Whole => "whole name",
    }
}

pub fn case_label(style: CaseStyle) -> &'static str {
    match style {
        CaseStyle::Lower => "lower",
        CaseStyle::Upper => "upper",
        CaseStyle::Title => "title",
        CaseStyle::Sentence => "sentence",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_editor_kind_maps_to_its_rule() {
        assert_eq!(
            RuleEdit {
                kind: RuleKind::Regex,
                value: "^(.*)$".into(),
                replacement: "x$1".into(),
                target: Target::Stem,
                ..RuleEdit::default()
            }
            .to_rule(),
            Rule::Regex {
                pattern: "^(.*)$".into(),
                replacement: "x$1".into(),
                target: Target::Stem
            }
        );
        assert_eq!(
            RuleEdit {
                kind: RuleKind::Literal,
                value: "IMG_".into(),
                target: Target::Whole,
                ..RuleEdit::default()
            }
            .to_rule(),
            Rule::Literal { find: b"IMG_".to_vec(), replace: Vec::new(), target: Target::Whole }
        );
        assert_eq!(
            RuleEdit {
                kind: RuleKind::Case,
                case: CaseStyle::Title,
                target: Target::Whole,
                ..RuleEdit::default()
            }
            .to_rule(),
            Rule::Case { style: CaseStyle::Title, target: Target::Whole }
        );
        assert_eq!(
            RuleEdit {
                kind: RuleKind::InsertRemove,
                insert: InsertKind::RemovePrefix,
                count: 3,
                target: Target::Stem,
                ..RuleEdit::default()
            }
            .to_rule(),
            Rule::InsertRemove { op: InsertRemoveOp::RemovePrefix(3), target: Target::Stem }
        );
        assert_eq!(
            RuleEdit {
                kind: RuleKind::InsertRemove,
                insert: InsertKind::AddSuffix,
                value: "_x".into(),
                target: Target::Stem,
                ..RuleEdit::default()
            }
            .to_rule(),
            Rule::InsertRemove {
                op: InsertRemoveOp::AddSuffix(b"_x".to_vec()),
                target: Target::Stem
            }
        );
        assert_eq!(
            RuleEdit {
                kind: RuleKind::Sequence,
                start: 5,
                step: 2,
                pad: 4,
                target: Target::Stem,
                ..RuleEdit::default()
            }
            .to_rule(),
            Rule::Sequence { start: 5, step: 2, pad: 4, target: Target::Stem }
        );

        // Normalize ops come out in the canonical order, not the click order.
        let mut normalize =
            RuleEdit { kind: RuleKind::Normalize, target: Target::Whole, ..RuleEdit::default() };
        normalize.normalize[2] = true;
        normalize.normalize[0] = true;
        assert_eq!(
            normalize.to_rule(),
            Rule::Normalize {
                ops: vec![NormalizeOp::Nfc, NormalizeOp::CollapseSpaces],
                target: Target::Whole,
            }
        );
    }

    #[test]
    fn a_rule_survives_a_save_and_load_round_trip() {
        let mut normalize =
            RuleEdit { kind: RuleKind::Normalize, target: Target::Stem, ..RuleEdit::default() };
        normalize.normalize[1] = true;
        normalize.normalize[4] = true;
        let edits = [
            RuleEdit {
                kind: RuleKind::Regex,
                value: "^(.*)$".into(),
                replacement: "x$1".into(),
                target: Target::Stem,
                ..RuleEdit::default()
            },
            RuleEdit {
                kind: RuleKind::Literal,
                value: "IMG_".into(),
                replacement: "img-".into(),
                target: Target::Whole,
                ..RuleEdit::default()
            },
            RuleEdit { kind: RuleKind::Case, case: CaseStyle::Title, ..RuleEdit::default() },
            RuleEdit {
                kind: RuleKind::InsertRemove,
                insert: InsertKind::RemoveSuffix,
                count: 4,
                ..RuleEdit::default()
            },
            RuleEdit {
                kind: RuleKind::InsertRemove,
                insert: InsertKind::AddPrefix,
                value: "2024-".into(),
                ..RuleEdit::default()
            },
            RuleEdit {
                kind: RuleKind::Sequence,
                start: 5,
                step: 2,
                pad: 4,
                target: Target::Stem,
                ..RuleEdit::default()
            },
            normalize,
            RuleEdit { enabled: false, kind: RuleKind::Case, ..RuleEdit::default() },
        ];

        for edit in edits {
            assert_eq!(RuleEdit::from_rule(&edit.to_rule(), edit.enabled), edit);
        }
    }
}
