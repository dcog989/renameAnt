//! The window: three panels, and the apply gating.
//!
//! Rules on the left, the preview in the middle, the journal on the right. The
//! plan shown in the table is the plan that Apply runs — never a fresh one — and
//! the initial state writes nothing.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::{Color32, TextureHandle};
use renameant_core::apply::{ApplyOptions, ApplyReport, Outcome};
use renameant_core::filter::Filter;
use renameant_core::journal::{self, Journal, RunId, Session};
use renameant_core::rule::RuleSet;

use crate::config;
use crate::dialogs::{self, Choice};
use crate::edit::{RuleEdit, default_rules};
use crate::editor;
use crate::history;
use crate::logo;
use crate::model::{PlanModel, Sort, SortKey};
use crate::style;
use crate::table;
use crate::worker::{Browse, Done, Job, Picked, Worker};

/// The preview recomputes on a debounce, not on every keystroke.
const DEBOUNCE: Duration = Duration::from_millis(100);
/// While a job runs, poll often enough that the window still feels live.
const POLL: Duration = Duration::from_millis(50);
const RULES_WIDTH: f32 = 340.0;
const JOURNAL_WIDTH: f32 = 260.0;
const DIR_FIELD_WIDTH: f32 = 440.0;
const FILTER_FIELD_WIDTH: f32 = 340.0;
/// Control height on the sources toolbar: taller than the rest, so the row the
/// user starts from reads as the first thing on the panel.
const TOP_ROW_HEIGHT: f32 = 32.0;
/// The primary action, deliberately larger than every other button.
const ACTION_TEXT_SIZE: f32 = 20.0;
/// Only used when neither XDG_STATE_HOME nor HOME is set.
const FALLBACK_JOURNAL: &str = "renameant-journal.jsonl";
/// Only used when neither XDG_CONFIG_HOME nor HOME is set.
const FALLBACK_CONFIG: &str = "renameant-rules.json";

pub(crate) const OK: Color32 = Color32::from_rgb(110, 190, 120);
pub(crate) const WARNING: Color32 = Color32::from_rgb(230, 165, 60);
pub(crate) const DANGER: Color32 = Color32::from_rgb(222, 95, 85);

enum Overlay {
    ConfirmApply,
    ConfirmUndo(Session),
    Report {
        title: String,
        report: ApplyReport,
        /// Rows the report does not list, counted when it arrived.
        rest: usize,
    },
}

pub struct App {
    model: PlanModel,
    /// The folders and files to rename, in the order they were picked. Every
    /// scan is over this list, so one folder is just the one-pick case.
    added: Vec<PathBuf>,
    /// The directory typed into the field, committed by `Add!` or filled by a
    /// browse.
    dir_field: String,
    /// Scan and rename the whole tree below every picked folder, not just its
    /// immediate children.
    recursive: bool,
    /// Fast mode: defer the size/mtime read to the first sort that needs it, and
    /// batch the journal's `fsync` during a run.
    fast: bool,
    edits: Vec<RuleEdit>,
    /// The rule list as it is on disk, so a change is written once.
    saved: Vec<RuleEdit>,
    config_path: PathBuf,
    /// The rules and filter the current plan was built from.
    planned: (RuleSet, Filter),
    dirty_since: Option<Instant>,
    worker: Worker,
    /// Jobs sent that have not come back. More than one can be queued, so a
    /// single `Done` does not mean the window is idle.
    outstanding: usize,
    /// Whether a job is out, derived from `outstanding` by `dispatch` and `poll`.
    busy: bool,
    /// A file browser is open on its own thread. At most one at a time.
    browsing: bool,
    /// A stat pass is out for the current rows.
    stat_pending: bool,
    journal: PathBuf,
    sessions: Vec<Session>,
    /// The faint ant painted behind the journal when it has no history.
    logo: TextureHandle,
    /// The journal panel is shown unless the user collapses it.
    journal_open: bool,
    overwrite: bool,
    status: String,
    overlay: Option<Overlay>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        style::apply(&cc.egui_ctx);
        let config_path = config::default_path().unwrap_or_else(|_| PathBuf::from(FALLBACK_CONFIG));
        let (edits, mut status) = match config::load(&config_path) {
            Ok(edits) if !edits.is_empty() => (edits, String::new()),
            Ok(_) => (default_rules(), String::new()),
            Err(error) => (default_rules(), format!("the saved rules could not be read: {error}")),
        };
        let journal = journal::default_path().unwrap_or_else(|_| PathBuf::from(FALLBACK_JOURNAL));
        let sessions = Journal::sessions(&journal).unwrap_or_default();
        let interrupted = sessions.iter().filter(|session| session.interrupted).count();
        if interrupted > 0 {
            if !status.is_empty() {
                status.push(' ');
            }
            status.push_str(&format!(
                "{interrupted} interrupted run(s) in the journal: reverse them from the journal"
            ));
        }
        App {
            model: PlanModel::default(),
            added: Vec::new(),
            dir_field: String::new(),
            recursive: false,
            fast: false,
            saved: edits.clone(),
            edits,
            config_path,
            planned: (RuleSet::default(), Filter::default()),
            dirty_since: None,
            worker: Worker::spawn(),
            outstanding: 0,
            busy: false,
            browsing: false,
            stat_pending: false,
            journal,
            sessions,
            logo: logo::texture(&cc.egui_ctx),
            journal_open: true,
            overwrite: false,
            status,
            overlay: None,
        }
    }

    /// The enabled rules, in order. An off rule is skipped here and nowhere else.
    fn rules(&self) -> RuleSet {
        RuleSet {
            rules: self.edits.iter().filter(|edit| edit.enabled).map(RuleEdit::to_rule).collect(),
        }
    }

    /// The rules and filter a plan would be built from, for the change check.
    fn job(&self) -> (RuleSet, Filter) {
        (self.rules(), self.model.filter.clone())
    }

    fn replan(&mut self) {
        let (rules, filter) = self.job();
        self.model.replan(&rules);
        self.planned = (rules, filter);
        self.dirty_since = None;
    }

    /// Recompute once the editing has stopped for [`DEBOUNCE`].
    fn replan_if_due(&mut self, ctx: &egui::Context) {
        if self.job() == self.planned {
            self.dirty_since = None;
            return;
        }
        let since = *self.dirty_since.get_or_insert_with(Instant::now);
        let waited = since.elapsed();
        if waited >= DEBOUNCE {
            self.replan();
        } else {
            ctx.request_repaint_after(DEBOUNCE - waited);
        }
    }

    /// Writes the rule list to disk when it changed. Every frame compares, so a
    /// keystroke is one small write, never one per frame.
    fn persist(&mut self) {
        if self.edits == self.saved {
            return;
        }
        if let Err(error) = config::save(&self.config_path, &self.edits) {
            self.status = format!("the rules could not be saved: {error}");
        }
        self.saved.clone_from(&self.edits);
    }

    /// Hands one job to the worker. Counted, so a `Done` that lands while
    /// another job is still queued does not read as idle.
    fn dispatch(&mut self, job: Job) {
        self.begin_job();
        self.worker.send(job);
    }

    fn begin_job(&mut self) {
        self.outstanding += 1;
        self.busy = true;
    }

    /// Opens the file browser on its own thread, counted like any other job so
    /// the window stays busy — and starts no stat pass — until it comes back.
    fn browse(&mut self, kind: Browse) {
        if self.browsing {
            return;
        }
        self.browsing = true;
        self.begin_job();
        self.worker.browse(kind);
    }

    fn scan(&mut self) {
        if self.added.is_empty() {
            self.model = PlanModel::default();
            self.replan();
            self.status.clear();
            return;
        }
        self.status = "scanning…".to_owned();
        self.dispatch(Job::Scan(self.added.clone(), self.recursive));
    }

    /// Takes what the file browser returned, skipping anything already listed.
    fn add(&mut self, picked: Vec<PathBuf>) {
        let before = self.added.len();
        for path in picked {
            if !self.added.contains(&path) {
                self.added.push(path);
            }
        }
        if self.added.len() != before {
            self.scan();
        }
    }

    /// A folder goes into the field, waiting for `Add!`. The dialog is the
    /// worker's own thread, so a slow portal never stalls a frame.
    fn pick_folder(&mut self) {
        self.browse(Browse::Folder);
    }

    /// A file joins the picked list at once, when the browser comes back.
    fn pick_files(&mut self) {
        self.browse(Browse::Files);
    }

    /// A picked file an apply or an undo actually moved follows the run to its
    /// new name, so the next scan still finds it. A row that failed or was
    /// skipped stays where it is: its destination was never created, and
    /// pointing the list at it would drop the file from the window.
    fn follow_renames(&mut self, report: &ApplyReport) {
        for row in &report.rows {
            if !matches!(row.outcome, Outcome::Renamed) {
                continue;
            }
            if let Some(added) = self.added.iter_mut().find(|added| **added == row.src) {
                *added = row.dst.clone();
            }
        }
    }

    fn start_apply(&mut self) {
        let options = ApplyOptions {
            journal: self.journal.clone(),
            overwrite: self.overwrite,
            batched_sync: self.fast,
        };
        self.status = "applying…".to_owned();
        self.dispatch(Job::Apply(Box::new(self.model.plan.clone()), options));
    }

    /// The size and mtime the scan does not capture, read on the worker right
    /// after it, so a large directory never holds up a frame. Once a pass
    /// lands, the metadata is kept until the paths are rescanned.
    fn maybe_stat(&mut self) {
        // A modal means an apply or undo is one click away, so do not start a
        // background pass that would race it.
        if self.stat_wanted() && !self.stat_pending && !self.busy && self.overlay.is_none() {
            self.stat_pending = true;
            self.dispatch(Job::Stat(self.model.meta_paths()));
        }
    }

    /// Whether the metadata pass still has rows to read. Fast mode leaves the
    /// whole pass to the first size or modified sort that needs it.
    fn stat_wanted(&self) -> bool {
        !self.model.meta_loaded
            && !self.model.entries.is_empty()
            && (!self.fast || self.model.sort.key.needs_meta())
    }

    fn start_undo(&mut self, run: RunId) {
        self.status = format!("undoing run {run}…");
        let options =
            ApplyOptions { batched_sync: self.fast, ..ApplyOptions::new(self.journal.clone()) };
        self.dispatch(Job::Undo(options, run));
    }

    fn poll(&mut self, ctx: &egui::Context) {
        while let Some(done) = self.worker.try_recv() {
            self.outstanding = self.outstanding.saturating_sub(1);
            self.busy = self.outstanding > 0;
            match done {
                Done::Scanned(result) => match result {
                    Ok((entries, index)) => {
                        self.status = format!("{} entries", entries.len());
                        self.model.set_scan(entries, index);
                        self.replan();
                    }
                    Err(error) => self.status = format!("cannot read the picked paths: {error}"),
                },
                Done::Statted(meta) => {
                    self.stat_pending = false;
                    self.model.set_meta(meta);
                }
                Done::Picked(picked) => {
                    self.browsing = false;
                    match picked {
                        Picked::Folder(Some(folder)) => {
                            self.dir_field = folder.display().to_string();
                        }
                        Picked::Folder(None) => {}
                        Picked::Files(paths) => self.add(paths),
                    }
                }
                Done::Ran { undo_of, result, sessions } => {
                    self.sessions = sessions;
                    match result {
                        Ok(report) => {
                            let run = report.run;
                            let title = match undo_of {
                                Some(undone) => format!("Undo of run {undone}"),
                                None => format!("Applied run {run}"),
                            };
                            let (renamed, failed) = (report.renamed(), report.failed());
                            self.status = format!("run {run}: {renamed} renamed, {failed} failed");
                            self.follow_renames(&report);
                            let rest = dialogs::unlisted_rows(&report);
                            self.overlay = Some(Overlay::Report { title, report, rest });
                        }
                        Err(error) => {
                            self.status = format!(
                                "the journal could not be written, so the run aborted: {error}"
                            );
                        }
                    }
                    // The names on disk have changed, so the preview is stale.
                    self.scan();
                }
            }
        }
        if self.busy {
            ctx.request_repaint_after(POLL);
        }
    }

    /// What to rename: a browse, a named directory to add, and the scan options.
    /// The picked paths are not listed here; their rows are the list below, and
    /// that is where one is removed from.
    fn top_row(&mut self, ui: &mut egui::Ui) {
        // The spacing is scoped to this row: the filter row and the table keep
        // the base interact size.
        ui.scope(|ui| {
            ui.spacing_mut().interact_size.y = TOP_ROW_HEIGHT;
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!self.browsing, |ui| {
                    ui.menu_button("Browse…", |ui| {
                        style::menu(ui);
                        if ui.button("Folders…").clicked() {
                            ui.close();
                            self.pick_folder();
                        }
                        if ui.button("Files…").clicked() {
                            ui.close();
                            self.pick_files();
                        }
                    });
                });
                style::field(ui, &mut self.dir_field, DIR_FIELD_WIDTH).accessible_name("Directory");
                if style::pointer(
                    ui.add_enabled(
                        !self.busy,
                        egui::Checkbox::new(&mut self.recursive, "Subdirectories"),
                    )
                    .on_hover_text(
                        "rename inside every subdirectory too; scanned on the next scan",
                    ),
                )
                .changed()
                {
                    self.scan();
                }
                style::pointer(
                    ui.add(egui::Checkbox::new(&mut self.fast, "Fast mode")).on_hover_text(
                        "read sizes and times only when you sort by them, and sync the journal \
                         once per batch instead of once per rename",
                    ),
                );
                let can_add = !self.dir_field.trim().is_empty();
                if ui.add_enabled(can_add, egui::Button::new("Add!")).clicked() {
                    let dir = PathBuf::from(self.dir_field.trim());
                    self.add(vec![dir]);
                }
                if self.busy && !self.browsing {
                    ui.spinner();
                }
            });
        });
    }

    /// Name flags and a planning error: without them the preview reads as the
    /// whole truth when it is not.
    fn notices(&self, ui: &mut egui::Ui) {
        let (not_utf8, not_nfc) = (self.model.not_utf8, self.model.not_nfc);
        if not_utf8 > 0 {
            ui.colored_label(
                WARNING,
                format!(
                    "{not_utf8} not UTF-8: regex, case, normalize and the remove rules skip them"
                ),
            );
        }
        if not_nfc > 0 {
            ui.colored_label(WARNING, format!("{not_nfc} not NFC"));
        }
        if let Some(error) = &self.model.error {
            ui.colored_label(DANGER, format!("error: {error}"));
        }
    }

    /// The filter, the way back to apply order, the row count and the way to
    /// forget everything picked.
    fn filter_row(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label("Filter:");
            style::pointer(ui.checkbox(&mut self.model.filter.kinds.dirs, "Directories"));
            style::pointer(ui.checkbox(&mut self.model.filter.kinds.files, "Files"));
            style::field(ui, &mut self.model.filter.pattern, FILTER_FIELD_WIDTH)
                .accessible_name("Filter by name");
            style::pointer(
                ui.checkbox(&mut self.model.filter.regex, "regex")
                    .on_hover_text("match the name field as a regular expression, not plain text"),
            );
            style::pointer(
                ui.checkbox(&mut self.model.filter.case_sensitive, "case sensitive")
                    .on_hover_text("match the name field's case exactly"),
            );
            ui.separator();
            // The headers are the sort control; this is the way back to the
            // order apply runs the rows in.
            let reset = self.model.sort.key != SortKey::Apply;
            if ui
                .add_enabled(reset, egui::Button::new("Reset sort"))
                .on_hover_text("Reset any view sorts to the processing order.")
                .clicked()
            {
                self.model.sort = Sort::default();
                self.model.rebuild_view();
            }
            let text = if self.model.filter.is_inactive() {
                format!("{} rows", self.model.view.len())
            } else {
                let (rows, entries) = (self.model.plan.rows.len(), self.model.entries.len());
                format!("{rows} of {entries} entries")
            };
            ui.label(egui::RichText::new(text).weak());
            let clear = !self.added.is_empty();
            if ui
                .add_enabled(clear, egui::Button::new("Clear Content"))
                .on_hover_text("forget every picked path and clear the preview")
                .clicked()
            {
                self.added.clear();
                self.scan();
            }
        });
    }

    fn footer(&mut self, ui: &mut egui::Ui) {
        let totals = self.model.totals;
        let (files, folders, actionable) = (totals.files, totals.folders, totals.actionable);
        ui.horizontal(|ui| {
            ui.label(format!("{files} files / {folders} folders to rename"));
            let ready = actionable > 0 && !self.busy && self.model.error.is_none();
            if ui
                .add_enabled(
                    ready,
                    egui::Button::new(
                        egui::RichText::new("Rename The Content").size(ACTION_TEXT_SIZE),
                    ),
                )
                .on_hover_text("nothing is written before this")
                .clicked()
            {
                self.overlay = Some(Overlay::ConfirmApply);
            }
            style::pointer(ui.checkbox(&mut self.overwrite, "Overwrite existing files"));
            if self.overwrite {
                ui.colored_label(DANGER, "irrecoverable: undo cannot restore what this clobbers");
            }
        });
        if !self.status.is_empty() {
            ui.label(self.status.as_str());
        }
    }

    fn rules_panel(&mut self, ui: &mut egui::Ui) {
        style::panel_heading(ui, "Rules");
        style::panel_body(ui.style()).show(ui, |ui| editor::show(ui, &mut self.edits));
    }

    fn journal_panel(&mut self, ui: &mut egui::Ui) {
        style::panel_heading(ui, "Journal");
        style::panel_body(ui.style()).show(ui, |ui| {
            // Fill the panel width, or the list shrinks to its content and the
            // resize handle is inert (the same defect the rules panel had).
            ui.set_min_width(ui.available_width());
            if self.sessions.is_empty() {
                logo::paint(ui, &self.logo);
            }
            if let Some(session) = history::show(ui, &self.sessions, self.busy) {
                self.overlay = Some(Overlay::ConfirmUndo(session));
            }
        });
    }

    /// The centre: the sources, the filter, the notices and the table, with the
    /// action bar along the bottom.
    fn content_panel(&mut self, ui: &mut egui::Ui) {
        style::panel_heading(ui, "Content");
        // The footer is a panel of its own, outside the body's padding, so its
        // separator line runs the full width, up to the drag bars.
        egui::Panel::bottom("renameant-footer")
            .frame(style::panel_body(ui.style()))
            .show(ui, |ui| self.footer(ui));
        style::panel_body(ui.style()).show(ui, |ui| {
            self.top_row(ui);
            self.filter_row(ui);
            self.notices(ui);
            let change = table::show(ui, &mut self.model);
            if let Some(path) = change.removed {
                // A path that came from the picked list leaves it too, so the
                // next scan does not bring the row back.
                self.added.retain(|added| added != &path);
                self.model.removed.insert(path);
                self.replan();
            }
        });
    }

    /// Hands the modal to `dialogs` and acts on what the user chose: the plan
    /// that was just displayed is the plan that gets applied.
    fn show_overlay(&mut self, ctx: &egui::Context) {
        let Some(overlay) = self.overlay.take() else {
            return;
        };
        let mut keep = true;
        let modal = egui::Modal::new(egui::Id::unique("renameant-overlay")).show(ctx, |ui| {
            let choice = match &overlay {
                Overlay::ConfirmApply => {
                    dialogs::confirm_apply(ui, &self.model, self.overwrite, self.fast)
                }
                Overlay::ConfirmUndo(session) => dialogs::confirm_undo(ui, session),
                Overlay::Report { title, report, rest } => {
                    dialogs::show_report(ui, title, report, *rest)
                }
            };
            match choice {
                Choice::None => {}
                Choice::Apply => self.start_apply(),
                Choice::Undo(run) => self.start_undo(run),
                Choice::Close => keep = false,
            }
        });
        if modal.should_close() {
            keep = false;
        }
        if keep {
            self.overlay = Some(overlay);
        }
    }
}

impl eframe::App for App {
    /// Opaque: the panels draw no fill of their own, so egui's default
    /// semi-transparent clear colour would let the desktop show through.
    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        visuals.window_fill().to_normalized_gamma_f32()
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll(&ctx);
        self.replan_if_due(&ctx);
        self.persist();

        egui::Panel::left("renameant-rules")
            .frame(style::panel_frame())
            .resizable(true)
            .default_size(RULES_WIDTH)
            .show(ui, |ui| self.rules_panel(ui));
        let mut journal_open = self.journal_open;
        egui::Panel::right("renameant-journal")
            .frame(style::panel_frame())
            .resizable(true)
            .default_size(JOURNAL_WIDTH)
            .show_collapsible(ui, &mut journal_open, |ui| self.journal_panel(ui));
        self.journal_open = journal_open;
        egui::CentralPanel::default()
            .frame(style::panel_frame())
            .show(ui, |ui| self.content_panel(ui));
        self.maybe_stat();
        self.show_overlay(&ctx);
    }
}

#[cfg(test)]
mod kittest;

#[cfg(test)]
mod perf;
