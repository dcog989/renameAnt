//! The journal panel: past runs, newest first, each one reversible.
//!
//! Undo appends a run of its own, so a reversed run and the run that reversed it
//! both stay in the list: the first is marked reversed, the second tagged with
//! the run it undid. An undo can reverse one row, so a run's mark is row-aware:
//! a row is reversed when a later run records its inverse, and the run says
//! "reversed" only when every row is.
//!
//! A run opens to show what it renamed, read from the journal the first time it
//! is opened and kept: a run's renames never change once written.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use egui::{RichText, Ui};
use renameant_core::apply::is_staged;
use renameant_core::journal::{Journal, RunId, Session};

use crate::app::WARNING;
use crate::format;

/// What the user asked to undo: a whole run, or one row of it.
pub enum Undo {
    Run(Session),
    Row { run: RunId, from: PathBuf, to: PathBuf },
}

/// The runs the user opened, and the renames each showed.
#[derive(Default)]
pub struct History {
    open: HashSet<RunId>,
    renames: HashMap<RunId, Result<Vec<(PathBuf, PathBuf)>, String>>,
}

/// Draws the list, and returns the run or row the user asked to undo. The
/// caller owns the confirmation: nothing is reversed from here.
pub fn show(
    ui: &mut Ui,
    sessions: &[Session],
    busy: bool,
    journal: &Path,
    history: &mut History,
) -> Option<Undo> {
    if sessions.is_empty() {
        ui.label("Nothing has been renamed yet.");
        return None;
    }

    // shortcut: a row's reversal is read from the undo run that recorded its
    // inverse, so a second undo of an undo is not tracked. It would only mis-mark
    // a row after a redo, so upgrade when redo gets its own command.
    let reversed = reversed_rows(sessions, journal, history);

    let mut undo = None;
    egui::ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
        for session in sessions.iter().rev() {
            let expanded = history.open.contains(&session.run);
            ui.horizontal(|ui| {
                let enabled = session.renames > 0 && !busy;
                if ui.add_enabled(enabled, egui::Button::new("Undo")).clicked() {
                    undo = Some(Undo::Run(session.clone()));
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
                ui.monospace(format::local_text(session.at));
                ui.label(format!("{} renames", session.renames));
            });
            let mut marks: Vec<String> = Vec::new();
            if let Some(evidence) = reversed.get(&session.run)
                && let Ok(rows) = rows(journal, history, session.run)
            {
                let all = !rows.is_empty()
                    && rows.iter().all(|(from, to)| is_reversed(evidence, from, to));
                marks.push(if all { "reversed" } else { "partly reversed" }.to_owned());
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
            if expanded
                && let Some(row) =
                    detail(ui, journal, history, session, reversed.get(&session.run), busy)
            {
                undo = Some(row);
            }
            ui.separator();
        }
    });
    undo
}

/// The rows a later undo run reversed, per target run.
fn reversed_rows(
    sessions: &[Session],
    journal: &Path,
    history: &mut History,
) -> HashMap<RunId, HashSet<(PathBuf, PathBuf)>> {
    let mut reversed: HashMap<RunId, HashSet<(PathBuf, PathBuf)>> = HashMap::new();
    for session in sessions {
        let Some(target) = session.undo_of else { continue };
        let Ok(rows) = rows(journal, history, session.run) else { continue };
        let evidence = reversed.entry(target).or_default();
        for (from, to) in rows {
            evidence.insert((to.clone(), from.clone()));
        }
    }
    reversed
}

/// A run's renames, read once, on first use.
fn rows<'a>(
    journal: &Path,
    history: &'a mut History,
    run: RunId,
) -> &'a Result<Vec<(PathBuf, PathBuf)>, String> {
    // shortcut: read on the UI thread; only the oldest run in a huge journal can
    // stall a frame, so move this to the worker if that ever shows.
    history
        .renames
        .entry(run)
        .or_insert_with(|| Journal::renames(journal, run).map_err(|error| error.to_string()))
}

/// Whether `from → to` was reversed by a later undo run.
fn is_reversed(evidence: &HashSet<(PathBuf, PathBuf)>, from: &Path, to: &Path) -> bool {
    evidence.contains(&(from.to_path_buf(), to.to_path_buf()))
}

/// The renames of an open run, grouped by directory, each reversible alone. A
/// staged name is one half of a swap, so it is never reversed alone.
fn detail(
    ui: &mut Ui,
    journal: &Path,
    history: &mut History,
    session: &Session,
    evidence: Option<&HashSet<(PathBuf, PathBuf)>>,
    busy: bool,
) -> Option<Undo> {
    let rows = match rows(journal, history, session.run) {
        Ok(rows) => rows.as_slice(),
        Err(error) => {
            ui.colored_label(WARNING, format!("the renames could not be read: {error}"));
            return None;
        }
    };
    if rows.is_empty() {
        ui.label("no renames recorded");
        return None;
    }

    let mut undo = None;
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
            let reversed = evidence.is_some_and(|evidence| is_reversed(evidence, from, to));
            ui.horizontal(|ui| {
                let staged = is_staged(from) || is_staged(to);
                let enabled = !busy && !reversed && !staged;
                let mut response = ui
                    .add_enabled(enabled, egui::Button::new("Undo"))
                    .on_hover_text("reverse this one rename")
                    .accessible_name(format!("undo {}", format::file_text(to)));
                response = if busy {
                    response.on_disabled_hover_text("a run is in progress")
                } else if reversed {
                    response.on_disabled_hover_text("this rename was already reversed")
                } else if staged {
                    response.on_disabled_hover_text("part of a swapped pair: undo the whole run")
                } else {
                    response
                };
                if response.clicked() {
                    undo = Some(Undo::Row { run: session.run, from: from.clone(), to: to.clone() });
                }
                ui.monospace(format!("{} → {}", format::file_text(from), format::file_text(to)))
                    .on_hover_text(format!("{} → {}", from.display(), to.display()));
                if reversed {
                    ui.colored_label(WARNING, "reversed");
                }
            });
        }
    });
    undo
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

#[cfg(test)]
mod tests {
    use super::*;
    use renameant_core::journal::{JournalRecord, RunOutcome};

    fn begin(run: RunId, undo_of: Option<RunId>) -> JournalRecord {
        JournalRecord::Begin { run, at: 0, planned: 1, overwrite: false, undo_of }
    }

    fn rename(run: RunId, from: &str, to: &str) -> JournalRecord {
        JournalRecord::Rename { run, from: from.into(), to: to.into() }
    }

    fn end(run: RunId) -> JournalRecord {
        JournalRecord::End { run, at: 0, outcome: RunOutcome::Completed }
    }

    #[test]
    fn a_row_is_reversed_only_where_an_undo_run_recorded_its_inverse() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("journal.jsonl");
        let mut journal = Journal::open(path.clone()).expect("open");
        journal.append(&begin(1, None)).expect("append");
        journal.append(&rename(1, "/d/a", "/d/p-a")).expect("append");
        journal.append(&rename(1, "/d/b", "/d/p-b")).expect("append");
        journal.append(&end(1)).expect("append");
        // A later run reverses only a's rename.
        journal.append(&begin(2, Some(1))).expect("append");
        journal.append(&rename(2, "/d/p-a", "/d/a")).expect("append");
        journal.append(&end(2)).expect("append");
        drop(journal);

        let sessions = Journal::sessions(&path).expect("sessions");
        let mut history = History::default();
        let reversed = reversed_rows(&sessions, &path, &mut history);

        let evidence = reversed.get(&1).expect("run 1 has an undo");
        assert!(is_reversed(evidence, Path::new("/d/a"), Path::new("/d/p-a")));
        assert!(!is_reversed(evidence, Path::new("/d/b"), Path::new("/d/p-b")));
    }
}
