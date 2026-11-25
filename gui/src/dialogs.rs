//! The modals: the apply confirmation, the undo confirmation and a run report.
//!
//! The confirmation reads "Apply N of M" and names what will be
//! skipped, and the overwrite opt-in is called out as irrecoverable. None of
//! this writes anything; it only reports the user's choice back to the app.

use std::path::{Path, PathBuf};

use egui::Ui;
use renameant_core::apply::{ApplyReport, ApplyRow, Outcome};
use renameant_core::journal::{RunId, Session};

use crate::app::{DANGER, WARNING};
use crate::format;
use crate::history;
use crate::model::PlanModel;

/// Skipped rows named in the confirmation before it says "and N more".
const SKIP_ROWS_SHOWN: usize = 20;
/// Rows listed in a run report before it says "and N more".
const REPORT_ROWS_SHOWN: usize = 200;
const REPORT_HEIGHT: f32 = 260.0;

/// What the user chose. [`Choice::None`] leaves the modal open.
#[derive(Clone)]
pub enum Choice {
    None,
    Apply,
    UndoRun(RunId),
    UndoRow { run: RunId, from: PathBuf, to: PathBuf },
    Remove(RunId),
    ClearAll,
    Close,
}

pub fn confirm_apply(ui: &mut Ui, model: &PlanModel, overwrite: bool, fast: bool) -> Choice {
    let totals = model.totals;
    let (actionable, candidates, skipped) = (totals.actionable, totals.candidates, totals.skipped);
    ui.heading(format!("Apply {actionable} of {candidates}"));
    if overwrite {
        ui.colored_label(
            DANGER,
            "Overwrite is on: existing files are clobbered, and undo cannot bring them back.",
        );
    }
    if fast {
        ui.colored_label(
            WARNING,
            "Fast mode is on: the undo records are written in batches, so a crash mid-run can \
             leave the last renames of this run without an undo record.",
        );
    }
    if skipped > 0 {
        ui.label(format!("{skipped} rows are not applied:"));
        let mut shown = 0;
        for row in model.skipped_rows(SKIP_ROWS_SHOWN) {
            ui.monospace(format!(
                "{}: {} -> {}",
                format::status_label(row.status),
                format::file_text(&row.src),
                format::file_text(&row.dst)
            ));
            shown += 1;
        }
        let rest = skipped - shown;
        if rest > 0 {
            ui.label(format!("and {rest} more"));
        }
    }
    buttons(ui, "Apply", Choice::Apply)
}

pub fn confirm_undo(ui: &mut Ui, session: &Session) -> Choice {
    let (run, renames) = (session.run, session.renames);
    ui.heading(format!("Undo run {run}"));
    ui.label(format!(
        "{renames} completed renames, reversed newest first. A row whose file has moved is reported, not forced."
    ));
    let note = history::note(session);
    if !note.is_empty() {
        ui.colored_label(WARNING, note);
    }
    buttons(ui, "Undo", Choice::UndoRun(run))
}

/// One row's undo: the inverse of a single rename, with the rest of the run left
/// as it is.
pub fn confirm_undo_row(ui: &mut Ui, run: RunId, from: &Path, to: &Path) -> Choice {
    ui.heading(format!("Undo 1 rename in run {run}"));
    ui.monospace(format!("{} → {}", format::file_text(to), format::file_text(from)))
        .on_hover_text(format!("{} → {}", to.display(), from.display()));
    ui.label("This reverses that one rename. Every other row of the run stays as it is.");
    buttons(ui, "Undo", Choice::UndoRow { run, from: from.to_path_buf(), to: to.to_path_buf() })
}

/// Dropping one run from the journal: the files stay where the run put them.
pub fn confirm_remove(ui: &mut Ui, session: &Session) -> Choice {
    let (run, renames) = (session.run, session.renames);
    ui.heading(format!("Remove run {run} from the journal"));
    ui.label(format!(
        "Its {renames} renames stay as they are on disk, but this run can no longer be undone."
    ));
    buttons(ui, "Remove", Choice::Remove(run))
}

/// Emptying the journal: every run's undo goes, the files do not move.
pub fn confirm_clear(ui: &mut Ui, runs: usize) -> Choice {
    ui.heading("Clear the journal");
    ui.label(format!(
        "All {runs} runs are removed. Files stay as they are, and nothing in the journal can be undone afterward."
    ));
    buttons(ui, "Clear all", Choice::ClearAll)
}

pub fn show_report(ui: &mut Ui, title: &str, report: &ApplyReport, rest: usize) -> Choice {
    ui.heading(title);
    ui.label(format!(
        "{} renamed, {} failed, {} skipped",
        report.renamed(),
        report.failed(),
        report.skipped()
    ));
    if report.aborted {
        ui.colored_label(
            DANGER,
            "The run aborted: this filesystem rejected RENAME_NOREPLACE, and nothing falls back to a rename that could overwrite. The renames already done are kept.",
        );
    }
    if report.failed() > 0 {
        ui.colored_label(
            WARNING,
            "Failed rows are listed below. Nothing is rolled back: undo the run to get the old names back.",
        );
    }
    egui::ScrollArea::vertical().max_height(REPORT_HEIGHT).show(ui, |ui| {
        for row in listed_rows(report) {
            ui.monospace(format!(
                "{}: {} -> {}",
                row.outcome,
                format::file_text(&row.src),
                format::file_text(&row.dst)
            ));
        }
        if rest > 0 {
            ui.label(format!("and {rest} more"));
        }
    });
    if ui.button("Close").clicked() { Choice::Close } else { Choice::None }
}

/// Rows a report will not list. Counted once, when the report arrives, so no
/// frame walks the whole run.
pub fn unlisted_rows(report: &ApplyReport) -> usize {
    report
        .rows
        .iter()
        .filter(|row| !matches!(row.outcome, Outcome::Skipped))
        .skip(REPORT_ROWS_SHOWN)
        .count()
}

fn listed_rows(report: &ApplyReport) -> impl Iterator<Item = &ApplyRow> {
    report
        .rows
        .iter()
        .filter(|row| !matches!(row.outcome, Outcome::Skipped))
        .take(REPORT_ROWS_SHOWN)
}

fn buttons(ui: &mut Ui, accept: &str, choice: Choice) -> Choice {
    let mut chosen = Choice::None;
    ui.horizontal(|ui| {
        if ui.button(accept).clicked() {
            chosen = choice;
        }
        if ui.button("Cancel").clicked() {
            chosen = Choice::Close;
        }
    });
    chosen
}
