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
use std::sync::Arc;

use egui::{RichText, Ui};
use renameant_core::apply::is_staged;
use renameant_core::journal::{Journal, RunId, Session};

use crate::app::warning;
use crate::format;

/// What the user asked to undo: a whole run, or one row of it.
pub enum Undo {
    Run(Session),
    Row { run: RunId, from: PathBuf, to: PathBuf },
}

/// What the panel asks the app to do. Every variant changes the journal, so the
/// caller owns the confirmation.
pub enum Action {
    Undo(Undo),
    Remove(Session),
    ClearAll,
}

/// How many run bodies stay cached. Enough for the open runs, bounded for a
/// long history.
const MAX_CACHED_RUNS: usize = 8;

type Reversed = HashMap<RunId, HashSet<(PathBuf, PathBuf)>>;
type ReversedKey = (usize, Option<RunId>);

/// The runs the user opened, the renames each showed, and the list's order.
#[derive(Default)]
pub struct History {
    open: HashSet<RunId>,
    renames: HashMap<RunId, Result<Vec<(PathBuf, PathBuf)>, String>>,
    /// The reversal evidence, rebuilt only when the session list changes.
    reversed: Option<(ReversedKey, Arc<Reversed>)>,
    oldest_first: bool,
}

impl History {
    /// Drops every cached read after the journal changed under the list.
    pub fn forget_all(&mut self) {
        self.open.clear();
        self.renames.clear();
        self.reversed = None;
    }

    /// The rows a later undo run reversed, cached by session list. The caller
    /// gets a handle, so it can keep reading while the panel borrows the cache
    /// mutably.
    fn evidence(&mut self, sessions: &[Session], journal: &Path) -> Arc<Reversed> {
        let key = (sessions.len(), sessions.last().map(|session| session.run));
        if self.reversed.as_ref().is_none_or(|(cached, _)| cached != &key) {
            let reversed = reversed_rows(sessions, journal, self);
            self.reversed = Some((key, Arc::new(reversed)));
        }
        self.reversed.as_ref().expect("just cached").1.clone()
    }
}

/// Draws the list, and returns the action the user asked for. The caller owns
/// the confirmation: nothing is changed from here.
pub fn show(
    ui: &mut Ui,
    sessions: &[Session],
    busy: bool,
    journal: &Path,
    history: &mut History,
) -> Option<Action> {
    if sessions.is_empty() {
        ui.label("Nothing has been renamed yet.");
        return None;
    }

    let mut action = None;
    ui.horizontal(|ui| {
        let label = if history.oldest_first { "Oldest first" } else { "Newest first" };
        if ui.button(label).on_hover_text("switch the journal's order").clicked() {
            history.oldest_first = !history.oldest_first;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add_enabled(!busy, egui::Button::new("Clear all")).clicked() {
                action = Some(Action::ClearAll);
            }
        });
    });
    ui.separator();

    // shortcut: a row's reversal is read from the undo run that recorded its
    // inverse, so a second undo of an undo is not tracked. It would only mis-mark
    // a row after a redo, so upgrade when redo gets its own command.
    let reversed = history.evidence(sessions, journal);

    let mut order: Vec<&Session> = sessions.iter().collect();
    if !history.oldest_first {
        order.reverse();
    }

    egui::ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
        for session in order {
            let expanded = history.open.contains(&session.run);
            ui.horizontal(|ui| {
                let enabled = session.renames > 0 && !busy;
                if ui.add_enabled(enabled, egui::Button::new("Undo")).clicked() {
                    action = Some(Action::Undo(Undo::Run(session.clone())));
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
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(!busy, egui::Button::new("✕").small())
                        .on_hover_text("delete this run from the journal; its undo is lost")
                        .accessible_name(format!("remove run {} from the journal", session.run))
                        .clicked()
                    {
                        action = Some(Action::Remove(session.clone()));
                    }
                });
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
                ui.colored_label(warning(ui), marks.join(" · "));
            }
            if expanded
                && let Some(row) =
                    detail(ui, journal, history, session, reversed.get(&session.run), busy)
            {
                action = Some(Action::Undo(row));
            }
            ui.separator();
        }
    });
    action
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
    if !history.renames.contains_key(&run) && history.renames.len() >= MAX_CACHED_RUNS {
        let open = history.open.clone();
        history.renames.retain(|id, _| open.contains(id));
    }
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
            ui.colored_label(warning(ui), format!("the renames could not be read: {error}"));
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
                    ui.colored_label(warning(ui), "reversed");
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
        JournalRecord::Rename { run, from: from.into(), to: to.into(), id: None }
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
