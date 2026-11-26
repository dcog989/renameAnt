//! The rule pipeline editor: one card per rule, switched on and off by its
//! icon and reordered by the arrows beside it.
//!
//! The edit shape lives in [`crate::edit`]; this is its widgets.

use egui::{DragValue, RichText, Ui};

use renameant_core::rule::Target;

use crate::edit::{
    CASE_STYLES, DATE_FORMATS, InsertKind, NORMALIZE_OPS, Place, RuleEdit, RuleKind, case_label,
    date_format_label, default_rules, target_label,
};

/// The add-rule row drawn under the list; the list leaves it this much room.
const ADD_MENU_HEIGHT: f32 = 34.0;
/// Never shrink the list below this, however short the panel is.
const MIN_LIST_HEIGHT: f32 = 120.0;
const FIELD_WIDTH: f32 = 120.0;
/// Upper bounds keep a drag from producing a name that cannot exist anyway.
const MAX_COUNT: u64 = 10_000;
const MAX_STEP: u64 = 1_000_000;
const MAX_PAD: u64 = 32;

/// One glyph per rule type, all present in the bundled fonts, so no icon font is
/// needed.
fn icon(kind: RuleKind) -> &'static str {
    match kind {
        RuleKind::Replace => "🔁",
        RuleKind::Remove => "🗑",
        RuleKind::Trim => "✂",
        RuleKind::Insert => "➕",
        RuleKind::Number => "🔢",
        RuleKind::Date => "📅",
        RuleKind::Case => "🔤",
        RuleKind::Normalize => "✨",
    }
}

pub fn show(ui: &mut Ui, edits: &mut Vec<RuleEdit>) {
    let mut move_to: Option<(usize, usize)> = None;
    let mut remove: Option<usize> = None;
    let count = edits.len();
    // Fill the panel: a fixed cap hid every rule past the third (ToDo), and a
    // scroll area that shrank to its cards made the resize handle inert.
    let list_height = (ui.available_height() - ADD_MENU_HEIGHT).max(MIN_LIST_HEIGHT);
    egui::ScrollArea::vertical().max_height(list_height).auto_shrink([false, true]).show(
        ui,
        |ui| {
            if edits.is_empty() {
                ui.label(
                    RichText::new(
                        "No rules yet, so every row is unchanged: pick Add rule to start.",
                    )
                    .weak(),
                );
            }
            for (index, edit) in edits.iter_mut().enumerate() {
                crate::style::card_frame(ui, edit.enabled).show(ui, |ui| {
                    card(ui, index, count, edit, &mut remove, &mut move_to);
                });
            }
        },
    );
    if let Some((from, to)) = move_to {
        let rule = edits.remove(from);
        edits.insert(to, rule);
    }
    if let Some(at) = remove {
        edits.remove(at);
    }
    add_menu(ui, edits);
}

fn card(
    ui: &mut Ui,
    index: usize,
    count: usize,
    edit: &mut RuleEdit,
    remove: &mut Option<usize>,
    move_to: &mut Option<(usize, usize)>,
) {
    // The frame sizes to its content, so pin the content to the column width to
    // make the card fill it.
    ui.set_min_width(ui.available_width());
    let color = crate::style::card_text(ui, edit.enabled);
    ui.horizontal(|ui| {
        let toggle_text = if edit.enabled { "turn this rule off" } else { "turn this rule on" };
        // The icon and the name both toggle the rule; the checkbox sits between
        // them, like every other checkbox before its label. The colour is
        // scoped to each, so the row's other buttons keep the base text colour.
        let icon = ui
            .scope(|ui| {
                ui.style_mut().visuals.widgets.inactive.fg_stroke.color = color;
                ui.add(
                    egui::Button::new(
                        RichText::new(icon(edit.kind)).size(crate::style::RULE_ICON_SIZE),
                    )
                    .frame(false),
                )
            })
            .inner
            .on_hover_text(toggle_text)
            .accessible_name(name(index, "on or off"));
        if icon.clicked() {
            edit.enabled = !edit.enabled;
        }
        crate::style::pointer(ui.add(egui::Checkbox::without_text(&mut edit.enabled)))
            .accessible_name(name(index, "enabled"));
        let title = ui
            .scope(|ui| {
                ui.style_mut().visuals.widgets.inactive.fg_stroke.color = color;
                ui.add(
                    egui::Button::new(
                        RichText::new(edit.kind.label()).size(crate::style::RULE_TITLE_SIZE),
                    )
                    .frame(false),
                )
            })
            .inner
            .on_hover_text(toggle_text);
        if title.clicked() {
            edit.enabled = !edit.enabled;
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if !edit.builtin
                && crate::style::remove(ui, &name(index, "Remove"))
                    .on_hover_text("Remove this rule")
                    .clicked()
            {
                *remove = Some(index);
            }
            if ui
                .add_enabled(
                    index + 1 < count,
                    egui::Button::new(RichText::new("▼").monospace()).frame(false),
                )
                .on_hover_text("Move down")
                .accessible_name(name(index, "Move down"))
                .clicked()
            {
                *move_to = Some((index, index + 1));
            }
            if ui
                .add_enabled(
                    index > 0,
                    egui::Button::new(RichText::new("▲").monospace()).frame(false),
                )
                .on_hover_text("Move up")
                .accessible_name(name(index, "Move up"))
                .clicked()
            {
                *move_to = Some((index, index - 1));
            }
        });
    });
    fields(ui, index, edit);
}

/// The "Add rule" menu, one entry per rule type, plus the count.
fn add_menu(ui: &mut Ui, edits: &mut Vec<RuleEdit>) {
    ui.horizontal_wrapped(|ui| {
        ui.menu_button("Add rule", |ui| {
            crate::style::menu(ui);
            for kind in RuleKind::ALL {
                if ui.button(kind.label()).clicked() {
                    ui.close();
                    edits.push(RuleEdit { kind, ..RuleEdit::default() });
                }
            }
        });
        if ui.button("Reset").clicked() {
            *edits = default_rules();
        }
        ui.label(format!(
            "{}, applied in order",
            crate::format::plural(edits.len(), "rule", "rules")
        ));
    });
}

/// The accessible name of a field in rule `index`, which the pipeline shows as
/// `index + 1`: what a screen reader reaches the field by, and what the
/// harness's accessibility check asks for.
fn name(index: usize, field: &str) -> String {
    format!("rule {} {field}", index + 1)
}

/// A field label that never wraps inside itself: in a wrapping row it has to
/// move to the next line as a whole, or the row above overlaps it.
fn label(ui: &mut Ui, text: &str) {
    ui.add(egui::Label::new(text).wrap_mode(egui::TextWrapMode::Extend));
}

fn fields(ui: &mut Ui, index: usize, edit: &mut RuleEdit) {
    // An off rule reads as dimmed and its fields are inert until it is back on.
    ui.add_enabled_ui(edit.enabled, |ui| {
        let path_regex = edit.kind == RuleKind::Replace && edit.regex && edit.full_path;
        ui.horizontal_wrapped(|ui| {
            if !matches!(edit.kind, RuleKind::Number | RuleKind::Date) && !path_regex {
                target_combo(ui, index, edit);
            }
            match edit.kind {
                RuleKind::Replace => {
                    crate::style::pointer(ui.checkbox(&mut edit.regex, "regex"));
                    if edit.regex {
                        crate::style::pointer(ui.checkbox(&mut edit.full_path, "full path"));
                    }
                    label(
                        ui,
                        if edit.regex && edit.full_path {
                            "path pattern"
                        } else if edit.regex {
                            "pattern"
                        } else {
                            "find"
                        },
                    );
                    crate::style::field(ui, &mut edit.value, FIELD_WIDTH)
                        .accessible_name(name(index, "find"));
                    label(ui, "replace");
                    crate::style::field(ui, &mut edit.replacement, FIELD_WIDTH)
                        .accessible_name(name(index, "replace"));
                }
                RuleKind::Remove => {
                    label(ui, "from");
                    ui.add(DragValue::new(&mut edit.remove_start).range(0..=MAX_COUNT))
                        .accessible_name(name(index, "from character"));
                    label(ui, "to");
                    ui.add(DragValue::new(&mut edit.remove_end).range(0..=MAX_COUNT))
                        .accessible_name(name(index, "to character"));
                }
                RuleKind::Trim => {
                    ui.add(DragValue::new(&mut edit.count).range(0..=MAX_COUNT))
                        .accessible_name(name(index, "characters"));
                    label(ui, "characters from the end");
                }
                RuleKind::Insert => {
                    crate::style::combo(
                        ui,
                        ("insert", index),
                        &mut edit.insert,
                        InsertKind::ALL,
                        InsertKind::label,
                    )
                    .accessible_name(name(index, "where"));
                    crate::style::field(ui, &mut edit.value, FIELD_WIDTH)
                        .accessible_name(name(index, "text"));
                    if edit.insert == InsertKind::Position {
                        label(ui, "at");
                        ui.add(DragValue::new(&mut edit.at).range(0..=MAX_COUNT))
                            .accessible_name(name(index, "position"));
                    }
                }
                RuleKind::Number => {
                    place_combo(ui, index, edit);
                    label(ui, "start");
                    ui.add(DragValue::new(&mut edit.start)).accessible_name(name(index, "start"));
                    label(ui, "step");
                    ui.add(DragValue::new(&mut edit.step).range(1..=MAX_STEP))
                        .accessible_name(name(index, "step"));
                    label(ui, "digits");
                    ui.add(DragValue::new(&mut edit.pad).range(1..=MAX_PAD))
                        .accessible_name(name(index, "digits"));
                }
                RuleKind::Date => {
                    place_combo(ui, index, edit);
                    crate::style::combo(
                        ui,
                        ("date format", index),
                        &mut edit.date_format,
                        DATE_FORMATS,
                        date_format_label,
                    )
                    .accessible_name(name(index, "format"));
                }
                RuleKind::Case => {
                    crate::style::combo(
                        ui,
                        ("case", index),
                        &mut edit.case,
                        CASE_STYLES,
                        case_label,
                    )
                    .accessible_name(name(index, "style"));
                }
                RuleKind::Normalize => {
                    for (at, (_, label)) in NORMALIZE_OPS.iter().enumerate() {
                        crate::style::pointer(ui.checkbox(&mut edit.normalize[at], *label));
                    }
                }
            }
        });
        hint(ui, edit);
    });
}

/// The where-combo shared by every rule that generates text.
fn place_combo(ui: &mut Ui, index: usize, edit: &mut RuleEdit) {
    crate::style::combo(ui, ("place", index), &mut edit.place, Place::ALL, Place::label)
        .accessible_name(name(index, "where"));
}

fn target_combo(ui: &mut Ui, index: usize, edit: &mut RuleEdit) {
    crate::style::combo(
        ui,
        ("target", index),
        &mut edit.target,
        [Target::Stem, Target::Ext, Target::Whole],
        target_label,
    )
    .accessible_name(name(index, "target"));
}

/// The one thing a rule type does that the fields cannot show on their own.
fn hint(ui: &mut Ui, edit: &RuleEdit) {
    let text = match edit.kind {
        RuleKind::Replace if edit.regex && edit.full_path => {
            "the pattern sees the path as listed; its result must stay in the same folder"
        }
        RuleKind::Replace if edit.regex => "a group is $1",
        RuleKind::Replace => "byte for byte, so this one also reaches a non-UTF-8 name",
        RuleKind::Remove => "characters, so UTF-8 names only; the `to` character is kept",
        RuleKind::Trim => "counted in characters, so UTF-8 names only",
        RuleKind::Insert if edit.insert == InsertKind::Position => {
            "the position counts characters, so UTF-8 names only"
        }
        RuleKind::Number => "source path order; filtering or removing a row renumbers the rest",
        RuleKind::Date => "today's date, the same for every row in this preview",
        _ => return,
    };
    ui.label(text);
}
