//! The journal panel: past runs, newest first, each one reversible.
//!
//! Undo appends a run of its own, so a reversed run and the run that reversed it
//! both stay in the list: the first is marked reversed, the second tagged with
//! the run it undid.

use std::collections::HashSet;

use egui::Ui;
use renameant_core::journal::{RunId, Session};

use crate::app::WARNING;
use crate::format;

/// Tall enough for a few runs before the list scrolls.
const HISTORY_HEIGHT: f32 = 200.0;

/// Draws the list, and returns the session the user asked to undo. The caller
/// owns the confirmation: nothing is reversed from here.
pub fn show(ui: &mut Ui, sessions: &[Session], busy: bool) -> Option<Session> {
    if sessions.is_empty() {
        ui.label("Nothing has been renamed yet.");
        return None;
    }

    // A run is reversed once a later run names it as its undo.
    let reversed: HashSet<RunId> = sessions.iter().filter_map(|session| session.undo_of).collect();

    let mut undoing = None;
    egui::ScrollArea::vertical().max_height(HISTORY_HEIGHT).show(ui, |ui| {
        for session in sessions.iter().rev() {
            ui.horizontal(|ui| {
                let enabled = session.renames > 0 && !busy;
                if ui.add_enabled(enabled, egui::Button::new("Undo")).clicked() {
                    undoing = Some(session.clone());
                }
                ui.strong(format!("Rename run {}", session.run));
                ui.monospace(format::utc_text(session.at));
                ui.label(format!("{} renames", session.renames));
            });
            let mut marks: Vec<String> = Vec::new();
            if reversed.contains(&session.run) {
                marks.push("reversed".to_owned());
            }
            if let Some(undone) = session.undo_of {
                marks.push(format!("undo of run {undone}"));
            }
            let note = note(session);
            if !note.is_empty() {
                marks.push(note);
            }
            if !marks.is_empty() {
                ui.colored_label(WARNING, marks.join(" · "));
            }
            ui.separator();
        }
    });
    undoing
}

/// What a run's entry has to warn about: an interrupted run is surfaced rather
/// than resumed, and an overwrite run is irrecoverable.
pub fn note(session: &Session) -> String {
    let mut notes: Vec<&str> = Vec::new();
    if session.interrupted {
        notes.push("interrupted: only the renames it completed can be reversed");
    }
    if session.overwrite {
        notes.push("used overwrite: what it clobbered cannot come back");
    }
    notes.join("; ")
}
