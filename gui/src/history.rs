//! The journal panel: past runs, newest first, each one reversible.
//!
//! Undo appends a run of its own, so a reversed run and the run that reversed it
//! both stay in the list: the first is marked reversed, the second tagged with
//! the run it undid.
//!
//! A run opens to show what it renamed, read from the journal the first time it
//! is opened and kept: a run's renames never change once written.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use egui::{RichText, Ui};
use renameant_core::journal::{Journal, RunId, Session};

use crate::app::WARNING;
use crate::format;

/// The runs the user opened, and the renames each showed.
#[derive(Default)]
pub struct History {
    open: HashSet<RunId>,
    renames: HashMap<RunId, Result<Vec<(PathBuf, PathBuf)>, String>>,
}

/// Draws the list, and returns the session the user asked to undo. The caller
/// owns the confirmation: nothing is reversed from here.
pub fn show(
    ui: &mut Ui,
    sessions: &[Session],
    busy: bool,
    journal: &Path,
    history: &mut History,
) -> Option<Session> {
    if sessions.is_empty() {
        ui.label("Nothing has been renamed yet.");
        return None;
    }

    // A run is reversed once a later run names it as its undo.
    let reversed: HashSet<RunId> = sessions.iter().filter_map(|session| session.undo_of).collect();

    let mut undoing = None;
    egui::ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
        for session in sessions.iter().rev() {
            let expanded = history.open.contains(&session.run);
            ui.horizontal(|ui| {
                let enabled = session.renames > 0 && !busy;
                if ui.add_enabled(enabled, egui::Button::new("Undo")).clicked() {
                    undoing = Some(session.clone());
                }
                let toggle = ui
                    .add(
                        egui::Button::new(
                            RichText::new(format!(
                                "{} Rename run {}",
                                if expanded { "▾" } else { "▸" },
                                session.run
                            ))
                            .strong(),
                        )
                        .frame(false),
                    )
                    .on_hover_text("show what this run renamed")
                    .accessible_name(format!("renames of run {}", session.run));
                if toggle.clicked() {
                    if expanded {
                        history.open.remove(&session.run);
                    } else {
                        history.open.insert(session.run);
                    }
                }
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
            if expanded {
                detail(ui, journal, history, session);
            }
            ui.separator();
        }
    });
    undoing
}

/// The renames of an open run, grouped by directory. Read once, on first open.
fn detail(ui: &mut Ui, journal: &Path, history: &mut History, session: &Session) {
    // shortcut: read on the UI thread; only the oldest run in a huge journal can
    // stall a frame, so move this to the worker if that ever shows.
    let rows = history.renames.entry(session.run).or_insert_with(|| {
        Journal::renames(journal, session.run).map_err(|error| error.to_string())
    });
    let rows = match rows {
        Ok(rows) => rows.as_slice(),
        Err(error) => {
            ui.colored_label(WARNING, format!("the renames could not be read: {error}"));
            return;
        }
    };
    if rows.is_empty() {
        ui.label("no renames recorded");
        return;
    }

    ui.indent(("renames", session.run), |ui| {
        let mut dir: Option<&Path> = None;
        for (from, to) in rows {
            if from.parent() != dir {
                dir = from.parent();
                if let Some(dir) = dir {
                    let text = dir.display().to_string();
                    ui.add(egui::Label::new(RichText::new(text.as_str()).monospace().weak()))
                        .on_hover_text(text);
                }
            }
            ui.monospace(format!("{} → {}", format::file_text(from), format::file_text(to)))
                .on_hover_text(format!("{} → {}", from.display(), to.display()));
        }
    });
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
