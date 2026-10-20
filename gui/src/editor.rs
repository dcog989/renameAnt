//! The rule pipeline editor: an ordered list of cards, reordered by dragging a
//! card's handle.
//!
//! The edit shape lives in [`crate::edit`]; this is its widgets.

use egui::{Color32, DragValue, Frame, RichText, Stroke, Ui};

use renameant_core::rule::{CaseStyle, Target};

use crate::edit::{InsertKind, NORMALIZE_OPS, RuleEdit, RuleKind, case_label, target_label};

/// The add-rule row drawn under the list; the list leaves it this much room.
const ADD_MENU_HEIGHT: f32 = 34.0;
/// Never shrink the list below this, however short the panel is.
const MIN_LIST_HEIGHT: f32 = 120.0;
const FIELD_WIDTH: f32 = 150.0;
const ICON_SIZE: f32 = 18.0;
/// Upper bounds keep a drag from producing a name that cannot exist anyway.
const MAX_COUNT: u64 = 10_000;
const MAX_STEP: u64 = 1_000_000;
const MAX_PAD: u64 = 32;
/// An off rule is dimmed, not hidden: it keeps its place and its order.
const OFF: Color32 = Color32::from_gray(110);

/// One glyph per rule type, all present in the bundled fonts, so no icon font is
/// needed.
fn icon(kind: RuleKind) -> &'static str {
    match kind {
        RuleKind::Regex => "🔍",
        RuleKind::Literal => "✂",
        RuleKind::Case => "🔤",
        RuleKind::InsertRemove => "➕",
        RuleKind::Sequence => "🔢",
        RuleKind::Normalize => "✨",
    }
}

pub fn show(ui: &mut Ui, edits: &mut Vec<RuleEdit>) {
    let mut move_to: Option<(usize, usize)> = None;
    let mut remove: Option<usize> = None;
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
                // The drop zone is the card: it is the frame, and it highlights
                // while a card is dragged over it.
                let frame = card_frame(ui, edit.enabled);
                let (_, dropped) = ui.dnd_drop_zone::<usize, _>(frame, |ui| {
                    card(ui, index, edit, &mut remove);
                });
                if let Some(from) = dropped
                    && *from != index
                {
                    move_to = Some((*from, index));
                }
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

/// The card frame, dimmed at the border when the rule is off.
fn card_frame(ui: &Ui, enabled: bool) -> Frame {
    let frame = crate::style::group_frame(ui.style());
    if enabled { frame } else { frame.stroke(Stroke::new(1.0, OFF)) }
}

fn card(ui: &mut Ui, index: usize, edit: &mut RuleEdit, remove: &mut Option<usize>) {
    // The frame sizes to its content, so pin the content to the column width to
    // make the card fill it.
    ui.set_min_width(ui.available_width());
    if !edit.enabled {
        ui.style_mut().visuals.override_text_color = Some(OFF);
    }
    ui.horizontal_wrapped(|ui| {
        ui.label(format!("{}.", index + 1));
        ui.add(egui::Label::new(RichText::new("☰")).sense(egui::Sense::drag()))
            .on_hover_text("drag to reorder")
            .dnd_set_drag_payload(index);
        ui.label(RichText::new(icon(edit.kind)).size(ICON_SIZE));
        ui.label(edit.kind.label());
        crate::style::pointer(ui.checkbox(&mut edit.enabled, "on"));
        crate::style::pointer(
            egui::ComboBox::from_id_salt(("target", index))
                .selected_text(target_label(edit.target))
                .show_ui(ui, |ui| {
                    for target in [Target::Stem, Target::Ext, Target::Whole] {
                        ui.selectable_value(&mut edit.target, target, target_label(target));
                    }
                })
                .response,
        )
        .accessible_name(name(index, "target"));
        if ui.button("remove").clicked() {
            *remove = Some(index);
        }
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
        if ui.button("Clear").clicked() {
            edits.clear();
        }
        ui.label(format!("{} rules, applied in order", edits.len()));
    });
}

/// The accessible name of a field in rule `index`, which the pipeline shows as
/// `index + 1`: what a screen reader reaches the field by, and what the
/// harness's accessibility check asks for.
fn name(index: usize, field: &str) -> String {
    format!("rule {} {field}", index + 1)
}

fn fields(ui: &mut Ui, index: usize, edit: &mut RuleEdit) {
    match edit.kind {
        RuleKind::Regex => {
            ui.horizontal_wrapped(|ui| {
                ui.label("pattern");
                crate::style::field(ui, &mut edit.value, FIELD_WIDTH)
                    .accessible_name(name(index, "pattern"));
                ui.label("replace");
                crate::style::field(ui, &mut edit.replacement, FIELD_WIDTH)
                    .accessible_name(name(index, "replace"));
            });
            ui.label(RichText::new("substitute a group as ${1}").weak());
        }
        RuleKind::Literal => {
            ui.horizontal_wrapped(|ui| {
                ui.label("find");
                crate::style::field(ui, &mut edit.value, FIELD_WIDTH)
                    .accessible_name(name(index, "find"));
                ui.label("replace");
                crate::style::field(ui, &mut edit.replacement, FIELD_WIDTH)
                    .accessible_name(name(index, "replace"));
            });
            ui.label(
                RichText::new("byte for byte, so this one also reaches a name that is not UTF-8")
                    .weak(),
            );
        }
        RuleKind::Case => {
            ui.horizontal_wrapped(|ui| {
                ui.label("style");
                crate::style::pointer(
                    egui::ComboBox::from_id_salt(("case", index))
                        .selected_text(case_label(edit.case))
                        .show_ui(ui, |ui| {
                            for style in [
                                CaseStyle::Lower,
                                CaseStyle::Upper,
                                CaseStyle::Title,
                                CaseStyle::Sentence,
                            ] {
                                ui.selectable_value(&mut edit.case, style, case_label(style));
                            }
                        })
                        .response,
                )
                .accessible_name(name(index, "style"));
            });
        }
        RuleKind::InsertRemove => {
            ui.horizontal_wrapped(|ui| {
                crate::style::pointer(
                    egui::ComboBox::from_id_salt(("insert", index))
                        .selected_text(edit.insert.label())
                        .show_ui(ui, |ui| {
                            for op in InsertKind::ALL {
                                ui.selectable_value(&mut edit.insert, op, op.label());
                            }
                        })
                        .response,
                )
                .accessible_name(name(index, "operation"));
                if edit.insert.is_byte() {
                    crate::style::field(ui, &mut edit.value, FIELD_WIDTH)
                        .accessible_name(name(index, "text"));
                } else {
                    ui.add(DragValue::new(&mut edit.count).range(0..=MAX_COUNT))
                        .accessible_name(name(index, "characters"));
                    ui.label("characters");
                }
            });
            if !edit.insert.is_byte() {
                ui.label(RichText::new("counted in characters, so UTF-8 names only").weak());
            }
        }
        RuleKind::Sequence => {
            ui.horizontal_wrapped(|ui| {
                ui.label("start");
                ui.add(DragValue::new(&mut edit.start)).accessible_name(name(index, "start"));
                ui.label("step");
                ui.add(DragValue::new(&mut edit.step).range(1..=MAX_STEP))
                    .accessible_name(name(index, "step"));
                ui.label("digits");
                ui.add(DragValue::new(&mut edit.pad).range(1..=MAX_PAD))
                    .accessible_name(name(index, "digits"));
            });
            ui.label(RichText::new("numbered in name order, not in this table's order").weak());
        }
        RuleKind::Normalize => {
            for (at, (_, label)) in NORMALIZE_OPS.iter().enumerate() {
                crate::style::pointer(ui.checkbox(&mut edit.normalize[at], *label));
            }
        }
    }
}
