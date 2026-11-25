//! A rule as the editor holds it, and its mapping to a `core` `Rule`.
//!
//! A rule is edited as plain fields rather than as a `Rule` enum, so switching
//! its kind never throws away what is already typed. One kind is one card the
//! user sees; a kind may pick a different `Rule` variant from its settings.

use renameant_core::rule::{
    CaseStyle, DateFormat, InsertRemoveOp, NormalizeOp, Rule, SequencePlace, Target,
};

/// The date formats the editor offers.
pub const DATE_FORMATS: [DateFormat; 4] = [
    DateFormat::Yyyymmdd,
    DateFormat::Ddmmyyyy,
    DateFormat::MmDdYyyy,
    DateFormat::YyyymmddHhmmss,
];

/// The normalisations the editor offers, in the order `Normalize` applies them.
pub const NORMALIZE_OPS: [(NormalizeOp, &str); 5] = [
    (NormalizeOp::Nfc, "Unicode NFC"),
    (NormalizeOp::Trim, "trim"),
    (NormalizeOp::CollapseSpaces, "collapse spaces"),
    (NormalizeOp::SpacesToUnderscores, "spaces to underscores"),
    (NormalizeOp::StripControl, "strip control characters"),
];

/// The case styles the editor offers.
pub const CASE_STYLES: [CaseStyle; 8] = [
    CaseStyle::Upper,
    CaseStyle::Lower,
    CaseStyle::Title,
    CaseStyle::Sentence,
    CaseStyle::Kebab,
    CaseStyle::Snake,
    CaseStyle::Camel,
    CaseStyle::Pascal,
];

/// The rule types the editor can build. One card each, in the order a fresh
/// pipeline shows them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleKind {
    Replace,
    Remove,
    Trim,
    Insert,
    Number,
    Date,
    Case,
    Normalize,
}

impl RuleKind {
    pub const ALL: [RuleKind; 8] = [
        RuleKind::Replace,
        RuleKind::Remove,
        RuleKind::Trim,
        RuleKind::Insert,
        RuleKind::Number,
        RuleKind::Date,
        RuleKind::Case,
        RuleKind::Normalize,
    ];

    pub fn label(self) -> &'static str {
        match self {
            RuleKind::Replace => "replace",
            RuleKind::Remove => "remove",
            RuleKind::Trim => "trim",
            RuleKind::Insert => "insert",
            RuleKind::Number => "number",
            RuleKind::Date => "date",
            RuleKind::Case => "change case",
            RuleKind::Normalize => "normalize",
        }
    }
}

/// What a number rule does with the number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumberPlace {
    Prefix,
    Suffix,
    Whole,
    Stem,
    Extension,
}

impl NumberPlace {
    pub const ALL: [NumberPlace; 5] = [
        NumberPlace::Prefix,
        NumberPlace::Suffix,
        NumberPlace::Whole,
        NumberPlace::Stem,
        NumberPlace::Extension,
    ];

    pub fn label(self) -> &'static str {
        match self {
            NumberPlace::Prefix => "as prefix",
            NumberPlace::Suffix => "as suffix",
            NumberPlace::Whole => "replace whole name",
            NumberPlace::Stem => "replace stem",
            NumberPlace::Extension => "replace extension",
        }
    }
}

/// Where a date rule puts its stamp.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DatePlace {
    Prefix,
    Suffix,
}

impl DatePlace {
    pub const ALL: [DatePlace; 2] = [DatePlace::Prefix, DatePlace::Suffix];

    pub fn label(self) -> &'static str {
        match self {
            DatePlace::Prefix => "as prefix",
            DatePlace::Suffix => "as suffix",
        }
    }
}

/// Where a text rule puts its text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InsertKind {
    Prefix,
    Suffix,
    Position,
}

impl InsertKind {
    pub const ALL: [InsertKind; 3] = [InsertKind::Prefix, InsertKind::Suffix, InsertKind::Position];

    pub fn label(self) -> &'static str {
        match self {
            InsertKind::Prefix => "prefix",
            InsertKind::Suffix => "suffix",
            InsertKind::Position => "position",
        }
    }
}

/// One rule as the editor holds it: flat fields, so no widget has to own half a
/// `Rule` enum and every field stays editable in place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleEdit {
    /// A default card is always in the list; it has no remove button.
    pub builtin: bool,
    /// A rule that is off stays in the list and in its place; it is skipped when
    /// the pipeline is compiled.
    pub enabled: bool,
    pub kind: RuleKind,
    pub target: Target,
    /// A replace rule's pattern, or "find" when `regex` is off.
    pub value: String,
    /// The replace rule's replacement.
    pub replacement: String,
    /// Whether `value` is a regular expression.
    pub regex: bool,
    pub case: CaseStyle,
    pub insert: InsertKind,
    /// The character index an `InsertKind::Position` inserts at.
    pub at: u64,
    /// The first character a remove rule drops.
    pub remove_start: u64,
    /// The character a remove rule stops before.
    pub remove_end: u64,
    /// How many characters a trim rule drops from the end.
    pub count: u64,
    pub start: u64,
    pub step: u64,
    pub pad: u64,
    pub number_place: NumberPlace,
    pub date_place: DatePlace,
    pub date_format: DateFormat,
    pub normalize: [bool; NORMALIZE_OPS.len()],
}

impl Default for RuleEdit {
    /// A new rule changes nothing until it is switched on and filled in.
    fn default() -> Self {
        RuleEdit {
            builtin: false,
            enabled: false,
            kind: RuleKind::Replace,
            target: Target::Whole,
            value: String::new(),
            replacement: String::new(),
            regex: false,
            case: CaseStyle::Lower,
            insert: InsertKind::Prefix,
            at: 0,
            remove_start: 0,
            remove_end: 1,
            count: 1,
            start: 1,
            step: 1,
            pad: 3,
            number_place: NumberPlace::Stem,
            date_place: DatePlace::Prefix,
            date_format: DateFormat::Yyyymmdd,
            normalize: [false; NORMALIZE_OPS.len()],
        }
    }
}

impl RuleEdit {
    /// A card that carries nothing: off, every field at its fresh value. The
    /// regex toggle is ignored, because with an empty pattern it changes nothing.
    fn is_blank(&self) -> bool {
        let fresh = RuleEdit {
            kind: self.kind,
            builtin: self.builtin,
            regex: self.regex,
            ..RuleEdit::default()
        };
        *self == fresh
    }

    /// The `Rule` this editor stands for. Infallible: a pattern that will not
    /// compile is `plan`'s error to report.
    pub fn to_rule(&self) -> Rule {
        match self.kind {
            RuleKind::Replace if self.regex => Rule::Regex {
                pattern: self.value.clone(),
                replacement: self.replacement.clone(),
                target: self.target,
            },
            RuleKind::Replace => Rule::Literal {
                find: self.value.clone().into_bytes(),
                replace: self.replacement.clone().into_bytes(),
                target: self.target,
            },
            RuleKind::Remove => Rule::InsertRemove {
                op: InsertRemoveOp::RemoveRange {
                    start: self.remove_start as usize,
                    end: self.remove_end as usize,
                },
                target: self.target,
            },
            RuleKind::Trim => Rule::InsertRemove {
                op: InsertRemoveOp::RemoveSuffix(self.count as usize),
                target: self.target,
            },
            RuleKind::Insert => Rule::InsertRemove {
                op: match self.insert {
                    InsertKind::Prefix => {
                        InsertRemoveOp::AddPrefix(self.value.clone().into_bytes())
                    }
                    InsertKind::Suffix => {
                        InsertRemoveOp::AddSuffix(self.value.clone().into_bytes())
                    }
                    InsertKind::Position => InsertRemoveOp::InsertAt {
                        at: self.at as usize,
                        text: self.value.clone().into_bytes(),
                    },
                },
                target: self.target,
            },
            RuleKind::Number => {
                let (place, target) = match self.number_place {
                    NumberPlace::Prefix => (SequencePlace::Prefix, Target::Whole),
                    NumberPlace::Suffix => (SequencePlace::Suffix, Target::Stem),
                    NumberPlace::Whole => (SequencePlace::Replace, Target::Whole),
                    NumberPlace::Stem => (SequencePlace::Replace, Target::Stem),
                    NumberPlace::Extension => (SequencePlace::Replace, Target::Ext),
                };
                Rule::Sequence {
                    start: self.start,
                    step: self.step,
                    pad: self.pad as usize,
                    place,
                    target,
                }
            }
            RuleKind::Date => Rule::Date {
                format: self.date_format,
                place: match self.date_place {
                    DatePlace::Prefix => SequencePlace::Prefix,
                    DatePlace::Suffix => SequencePlace::Suffix,
                },
                target: self.target,
            },
            RuleKind::Case => Rule::Case { style: self.case, target: self.target },
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
    pub fn from_rule(rule: &Rule, enabled: bool, builtin: bool) -> RuleEdit {
        let mut edit = RuleEdit { enabled, builtin, ..RuleEdit::default() };
        match rule {
            Rule::Regex { pattern, replacement, target } => {
                edit.kind = RuleKind::Replace;
                edit.regex = true;
                edit.value = pattern.clone();
                edit.replacement = replacement.clone();
                edit.target = *target;
            }
            Rule::Literal { find, replace, target } => {
                edit.kind = RuleKind::Replace;
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
                edit.target = *target;
                match op {
                    InsertRemoveOp::AddPrefix(bytes) => {
                        edit.kind = RuleKind::Insert;
                        edit.insert = InsertKind::Prefix;
                        edit.value = String::from_utf8_lossy(bytes).into_owned();
                    }
                    InsertRemoveOp::AddSuffix(bytes) => {
                        edit.kind = RuleKind::Insert;
                        edit.insert = InsertKind::Suffix;
                        edit.value = String::from_utf8_lossy(bytes).into_owned();
                    }
                    InsertRemoveOp::InsertAt { at, text } => {
                        edit.kind = RuleKind::Insert;
                        edit.insert = InsertKind::Position;
                        edit.at = *at as u64;
                        edit.value = String::from_utf8_lossy(text).into_owned();
                    }
                    InsertRemoveOp::RemoveRange { start, end } => {
                        edit.kind = RuleKind::Remove;
                        edit.remove_start = *start as u64;
                        edit.remove_end = *end as u64;
                    }
                    InsertRemoveOp::RemovePrefix(count) => {
                        edit.kind = RuleKind::Remove;
                        edit.remove_start = 0;
                        edit.remove_end = *count as u64;
                    }
                    InsertRemoveOp::RemoveSuffix(count) => {
                        edit.kind = RuleKind::Trim;
                        edit.count = *count as u64;
                    }
                }
            }
            Rule::Sequence { start, step, pad, place, target } => {
                edit.kind = RuleKind::Number;
                edit.number_place = match (*place, *target) {
                    (SequencePlace::Prefix, _) => NumberPlace::Prefix,
                    (SequencePlace::Suffix, _) => NumberPlace::Suffix,
                    (SequencePlace::Replace, Target::Whole) => NumberPlace::Whole,
                    (SequencePlace::Replace, Target::Ext) => NumberPlace::Extension,
                    (SequencePlace::Replace, Target::Stem) => NumberPlace::Stem,
                };
                edit.start = *start;
                edit.step = *step;
                edit.pad = *pad as u64;
            }
            Rule::Date { format, place, target } => {
                edit.kind = RuleKind::Date;
                edit.date_format = *format;
                edit.date_place = match place {
                    SequencePlace::Prefix => DatePlace::Prefix,
                    SequencePlace::Suffix | SequencePlace::Replace => DatePlace::Suffix,
                };
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
/// always display without changing anything until the user switches one on.
pub fn default_rules() -> Vec<RuleEdit> {
    with_defaults(Vec::new())
}

/// Adds a default card for every rule kind the list does not already have one
/// of, in the order a fresh pipeline shows them. Default cards are never
/// removed, so a saved list that lost one gets it back.
pub fn with_defaults(mut edits: Vec<RuleEdit>) -> Vec<RuleEdit> {
    // A blank card carries no intent: it is what an old default seed became
    // when the kinds changed. Drop it before topping the list up.
    edits.retain(|edit| edit.builtin || !edit.is_blank());
    for kind in RuleKind::ALL {
        match edits.iter_mut().find(|edit| edit.kind == kind) {
            Some(edit) => edit.builtin = true,
            None => edits.push(RuleEdit { builtin: true, kind, ..RuleEdit::default() }),
        }
    }
    edits
}

pub fn target_label(target: Target) -> &'static str {
    match target {
        Target::Stem => "stem",
        Target::Ext => "extension",
        Target::Whole => "whole name",
    }
}

pub fn date_format_label(format: DateFormat) -> &'static str {
    match format {
        DateFormat::Yyyymmdd => "yyyymmdd",
        DateFormat::Ddmmyyyy => "ddmmyyyy",
        DateFormat::MmDdYyyy => "mmddyyyy",
        DateFormat::YyyymmddHhmmss => "yyyymmdd-hhmmss",
    }
}

pub fn case_label(style: CaseStyle) -> &'static str {
    match style {
        CaseStyle::Lower => "lower",
        CaseStyle::Upper => "upper",
        CaseStyle::Title => "title",
        CaseStyle::Sentence => "sentence",
        CaseStyle::Kebab => "kebab",
        CaseStyle::Snake => "snake",
        CaseStyle::Camel => "camel",
        CaseStyle::Pascal => "pascal",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trips(edit: RuleEdit) {
        assert_eq!(RuleEdit::from_rule(&edit.to_rule(), edit.enabled, edit.builtin), edit);
    }

    #[test]
    fn every_kind_survives_a_save_and_load_round_trip() {
        round_trips(RuleEdit {
            kind: RuleKind::Replace,
            regex: true,
            value: "a+".into(),
            replacement: "b".into(),
            target: Target::Stem,
            ..RuleEdit::default()
        });
        round_trips(RuleEdit {
            kind: RuleKind::Replace,
            value: "IMG_".into(),
            ..RuleEdit::default()
        });
        round_trips(RuleEdit {
            kind: RuleKind::Remove,
            remove_start: 2,
            remove_end: 4,
            ..RuleEdit::default()
        });
        round_trips(RuleEdit { kind: RuleKind::Trim, count: 3, ..RuleEdit::default() });
        round_trips(RuleEdit {
            kind: RuleKind::Insert,
            insert: InsertKind::Suffix,
            value: "_x".into(),
            ..RuleEdit::default()
        });
        round_trips(RuleEdit {
            kind: RuleKind::Insert,
            insert: InsertKind::Position,
            at: 2,
            value: "--".into(),
            ..RuleEdit::default()
        });
        round_trips(RuleEdit {
            kind: RuleKind::Number,
            number_place: NumberPlace::Suffix,
            start: 5,
            step: 2,
            pad: 4,
            ..RuleEdit::default()
        });
        round_trips(RuleEdit {
            kind: RuleKind::Number,
            number_place: NumberPlace::Prefix,
            ..RuleEdit::default()
        });
        round_trips(RuleEdit {
            kind: RuleKind::Number,
            number_place: NumberPlace::Extension,
            ..RuleEdit::default()
        });
        round_trips(RuleEdit {
            kind: RuleKind::Date,
            date_place: DatePlace::Suffix,
            date_format: DateFormat::YyyymmddHhmmss,
            target: Target::Stem,
            ..RuleEdit::default()
        });
        round_trips(RuleEdit {
            kind: RuleKind::Case,
            case: CaseStyle::Pascal,
            ..RuleEdit::default()
        });
        round_trips(RuleEdit {
            kind: RuleKind::Normalize,
            normalize: [false, true, false, false, false],
            ..RuleEdit::default()
        });
    }

    #[test]
    fn a_missing_default_card_is_added_once_and_custom_cards_are_kept() {
        let custom = RuleEdit { kind: RuleKind::Replace, value: "x".into(), ..RuleEdit::default() };
        let mut seeds = default_rules();
        seeds.retain(|edit| edit.kind != RuleKind::Trim);
        seeds.push(custom.clone());

        let edits = with_defaults(seeds);
        assert_eq!(edits.iter().filter(|edit| edit.kind == RuleKind::Trim).count(), 1);
        assert_eq!(edits.iter().filter(|edit| edit.builtin).count(), RuleKind::ALL.len());
        assert!(edits.contains(&custom));
    }

    #[test]
    fn a_blank_card_is_dropped_and_a_configured_one_is_kept() {
        let blank = RuleEdit { kind: RuleKind::Trim, ..RuleEdit::default() };
        let kept = RuleEdit { kind: RuleKind::Trim, count: 3, ..RuleEdit::default() };

        let edits = with_defaults(vec![blank, kept]);
        let trims: Vec<&RuleEdit> =
            edits.iter().filter(|edit| edit.kind == RuleKind::Trim).collect();
        assert_eq!(trims.len(), 1);
        assert_eq!(trims[0].count, 3);
        assert!(trims[0].builtin);
    }

    #[test]
    fn a_remove_prefix_from_an_old_config_reads_as_a_range() {
        let loaded = RuleEdit::from_rule(
            &Rule::InsertRemove { op: InsertRemoveOp::RemovePrefix(3), target: Target::Stem },
            true,
            false,
        );
        assert_eq!((loaded.kind, loaded.remove_start, loaded.remove_end), (RuleKind::Remove, 0, 3));
    }
}
