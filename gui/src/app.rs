//! The window: three panels, and the apply gating.
//!
//! Rules on the left, the preview in the middle, the journal on the right. The
//! plan shown in the table is the plan that Apply runs — never a fresh one — and
//! the initial state writes nothing.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::TextureHandle;
use renameant_core::apply::{ApplyOptions, ApplyReport, Outcome};
use renameant_core::filter::Filter;
use renameant_core::journal::{self, RunId, Session};
use renameant_core::rule::RuleSet;
use renameant_core::scan::Scan;

use crate::about::About;
use crate::config;
use crate::dialogs::{self, Choice};
use crate::edit::{RuleEdit, default_rules, with_defaults};
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
/// The narrowest a modal gets, so a short report does not squeeze its text into
/// a column.
const OVERLAY_WIDTH: f32 = 520.0;
/// The About button in the footer: one line of the Rename now label, icon included.
const ABOUT_ICON_SIZE: f32 = 24.0;
/// Only used when neither XDG_CONFIG_HOME nor HOME is set.
const FALLBACK_CONFIG: &str = "renameant-rules.json";

enum Overlay {
    ConfirmUndo(Session),
    ConfirmUndoRow {
        run: RunId,
        from: PathBuf,
        to: PathBuf,
    },
    ConfirmRemove(Session),
    ConfirmClear,
    Report {
        title: String,
        report: ApplyReport,
        /// Indices of the rows the report lists, chosen when it arrived.
        listed: Vec<usize>,
        /// Rows the report does not list, counted when it arrived.
        rest: usize,
        /// Renamed, failed and skipped, counted when it arrived.
        counts: (usize, usize, usize),
    },
}

pub struct App {
    model: PlanModel,
    /// The last scan stopped at the core's entry cap, so its rows are only part
    /// of the picked tree.
    scan_truncated: bool,
    /// Directories the last scan could not read.
    scan_unreadable: Vec<PathBuf>,
    /// The folders and files to rename, in the order they were picked. Every
    /// scan is over this list, so one folder is just the one-pick case.
    added: Vec<PathBuf>,
    /// The directory typed into the field and committed with Enter.
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
    /// The list the last save attempt tried to write. A failed save is retried
    /// on the next edit, not on every frame.
    save_attempted: Vec<RuleEdit>,
    config_path: PathBuf,
    /// Saved with the rules; the logger reads it once logging lands.
    log_level: config::LogLevel,
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
    /// Which runs are open, and the renames they loaded.
    history: history::History,
    /// The faint ant painted behind the journal when it has no history.
    logo: TextureHandle,
    /// The journal panel is shown unless the user collapses it.
    journal_open: bool,
    status: String,
    /// A failed run's message. A rescan replaces `status`, so this stays until
    /// the next run.
    run_error: Option<String>,
    /// A journal read failure. It hides undo history, so it stays until a read
    /// succeeds.
    journal_error: Option<String>,
    overlay: Option<Overlay>,
    about: About,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        style::apply(&cc.egui_ctx);
        let config_path = config::default_path().unwrap_or_else(|_| PathBuf::from(FALLBACK_CONFIG));
        let (edits, status, log_level) = match config::load(&config_path) {
            Ok((edits, unsupported, log_level)) => {
                let mut status = unsupported_rules(&unsupported);
                // A later save writes only the rules this version knows. Keep
                // the original file so a newer version's rule is not lost.
                if !unsupported.is_empty() {
                    let backup = config_path.with_extension("json.bak");
                    if let Err(error) = std::fs::copy(&config_path, &backup) {
                        status.push_str(&format!("; the rules could not be backed up: {error}"));
                    }
                }
                (with_defaults(edits), status, log_level)
            }
            Err(error) => (
                default_rules(),
                format!("the saved rules could not be read: {error}"),
                config::LogLevel::default(),
            ),
        };
        let journal = journal::default_path()
            .expect("main resolved the state directory before opening the window");
        let mut app = App {
            model: PlanModel::default(),
            scan_truncated: false,
            scan_unreadable: Vec::new(),
            added: Vec::new(),
            dir_field: String::new(),
            recursive: false,
            fast: false,
            saved: edits.clone(),
            save_attempted: edits.clone(),
            edits,
            config_path,
            log_level,
            planned: (RuleSet::default(), Filter::default()),
            dirty_since: None,
            worker: Worker::spawn(),
            outstanding: 0,
            busy: false,
            browsing: false,
            stat_pending: false,
            journal,
            sessions: Vec::new(),
            history: history::History::default(),
            logo: logo::texture(&cc.egui_ctx),
            journal_open: true,
            status,
            run_error: None,
            journal_error: None,
            overlay: None,
            about: About::default(),
        };
        // Pruning and listing the journal read the whole file; keep them off
        // the first frame.
        app.dispatch(Job::LoadJournal(app.journal.clone()));
        app
    }

    /// The enabled rules, in order. An off rule is skipped here and nowhere else.
    fn rules(&self) -> RuleSet {
        RuleSet {
            rules: self
                .edits
                .iter()
                .filter(|edit| edit.is_active())
                .map(RuleEdit::to_rule)
                .collect(),
        }
    }

    /// The rules and filter a plan would be built from, for the change check.
    fn job(&self) -> (RuleSet, Filter) {
        (self.rules(), self.model.filter.clone())
    }

    fn replan(&mut self) {
        let (rules, filter) = self.job();
        self.replan_with(rules, filter);
    }

    /// Replan from a rule set and filter the caller already built, so one frame
    /// does not build the pipeline twice.
    fn replan_with(&mut self, rules: RuleSet, filter: Filter) {
        let filter_changed = self.planned.1 != filter;
        self.model.replan(&rules);
        // The stat pass follows the plan. A filter change can expose rows that
        // were never read, so they get one more pass; a pass already in flight
        // covered the old rows only.
        if filter_changed {
            self.model.meta_loaded = false;
            self.stat_pending = false;
        }
        self.planned = (rules, filter);
        self.dirty_since = None;
    }

    /// Recompute once the editing has stopped for [`DEBOUNCE`].
    fn replan_if_due(&mut self, ctx: &egui::Context) {
        let job = self.job();
        if job == self.planned {
            self.dirty_since = None;
            return;
        }
        let since = *self.dirty_since.get_or_insert_with(Instant::now);
        let waited = since.elapsed();
        if waited >= DEBOUNCE {
            let (rules, filter) = job;
            self.replan_with(rules, filter);
        } else {
            ctx.request_repaint_after(DEBOUNCE - waited);
        }
    }

    /// Writes the rule list to disk when it changed. Every frame compares, so a
    /// keystroke is one small write, never one per frame.
    fn persist(&mut self) {
        if self.edits == self.saved || self.edits == self.save_attempted {
            return;
        }
        self.save_settings();
    }

    /// Writes the rules and the log level together, so the file stays one
    /// source of truth.
    fn save_settings(&mut self) {
        self.save_attempted.clone_from(&self.edits);
        match config::save(&self.config_path, &self.edits, self.log_level) {
            Ok(()) => self.saved.clone_from(&self.edits),
            Err(error) => self.status = format!("the settings could not be saved: {error}"),
        }
    }

    fn append_status(&mut self, text: &str) {
        if !self.status.is_empty() {
            self.status.push(' ');
        }
        self.status.push_str(text);
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
            self.scan_truncated = false;
            self.scan_unreadable.clear();
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
        // A file dialog can hand over thousands of paths; a linear scan of
        // `added` per pick is quadratic.
        let mut known: HashSet<PathBuf> = self.added.iter().cloned().collect();
        for path in picked {
            // The journal replays names from another working directory, so a
            // relative pick would make undo depend on where the app was started.
            let path = match std::path::absolute(&path) {
                Ok(path) => path,
                Err(error) => {
                    self.status = format!("cannot use {}: {error}", path.display());
                    continue;
                }
            };
            // A typed path that does not exist would otherwise scan as an empty
            // selection with no explanation.
            if let Err(error) = std::fs::symlink_metadata(&path) {
                self.status = format!("cannot use {}: {error}", path.display());
                continue;
            }
            if known.insert(path.clone()) {
                self.added.push(path);
            }
        }
        if self.added.len() != before {
            self.scan();
        }
    }

    /// A folder joins the picked list at once, like a file. The dialog is the
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
        // Built from the picks as they were before the run, so a chain or a
        // cycle maps every row at once instead of chasing its own edits.
        let index: HashMap<PathBuf, usize> =
            self.added.iter().cloned().enumerate().map(|(at, path)| (path, at)).collect();
        for row in &report.rows {
            if !matches!(row.outcome, Outcome::Renamed) {
                continue;
            }
            if let Some(&at) = index.get(&row.src) {
                self.added[at] = row.dst.clone();
            }
        }
    }

    fn start_apply(&mut self) {
        self.run_error = None;
        let options = ApplyOptions { journal: self.journal.clone(), batched_sync: self.fast };
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
        self.run_error = None;
        self.status = format!("undoing run {run}…");
        let options =
            ApplyOptions { batched_sync: self.fast, ..ApplyOptions::new(self.journal.clone()) };
        self.dispatch(Job::Undo(options, run));
    }

    fn start_undo_row(&mut self, run: RunId, from: PathBuf, to: PathBuf) {
        self.run_error = None;
        self.status = format!("undoing one rename in run {run}…");
        let options =
            ApplyOptions { batched_sync: self.fast, ..ApplyOptions::new(self.journal.clone()) };
        self.dispatch(Job::UndoOne(options, run, from, to));
    }

    fn start_remove(&mut self, run: RunId) {
        self.status = format!("removing run {run} from the journal…");
        self.dispatch(Job::Forget(self.journal.clone(), run));
    }

    fn start_clear(&mut self) {
        self.status = "clearing the journal…".to_owned();
        self.dispatch(Job::Clear(self.journal.clone()));
    }

    fn poll(&mut self, ctx: &egui::Context) {
        while let Some(done) = self.worker.try_recv() {
            self.outstanding = self.outstanding.saturating_sub(1);
            self.busy = self.outstanding > 0;
            self.handle(done);
        }
        if self.busy {
            ctx.request_repaint_after(POLL);
        }
    }

    /// One result from the worker. Separate from `poll` so the failure paths can
    /// be driven from a test without a broken filesystem.
    fn handle(&mut self, done: Done) {
        match done {
            // The selection was cleared while this scan ran; its rows belong
            // to a pick the user has already dropped.
            Done::Scanned(_) if self.added.is_empty() => {
                self.status = "nothing picked".to_owned();
            }
            Done::Scanned(result) => match result {
                Ok(Scan { entries, index, unreadable, truncated }) => {
                    let count = entries.len();
                    self.scan_truncated = truncated;
                    self.scan_unreadable = unreadable;
                    self.status = if truncated {
                        format!("stopped at {}", crate::format::plural(count, "entry", "entries"))
                    } else if !self.scan_unreadable.is_empty() {
                        format!(
                            "{}; {} could not be read",
                            crate::format::plural(count, "entry", "entries"),
                            crate::format::plural(self.scan_unreadable.len(), "folder", "folders")
                        )
                    } else {
                        // The count is shown under the table; repeating it in the
                        // status line reads as a second scan result.
                        String::new()
                    };
                    self.model.set_scan(entries, index);
                    self.replan();
                }
                Err(error) => {
                    // The old rows belong to a selection that just failed
                    // to scan; applying them would rename the wrong files.
                    self.model.clear();
                    self.scan_truncated = false;
                    self.scan_unreadable.clear();
                    self.status = format!("cannot read the picked paths: {error}");
                }
            },
            Done::JournalLoaded { sessions, prune_error } => {
                match sessions {
                    Ok(sessions) => {
                        self.sessions = sessions;
                        self.journal_error = None;
                    }
                    Err(error) => {
                        self.journal_error = Some(format!(
                            "the journal could not be read: {error}; its runs cannot be \
                             shown or undone from here"
                        ));
                    }
                }
                if let Some(error) = prune_error {
                    self.append_status(&format!("the journal could not be pruned: {error}"));
                }
                let interrupted =
                    self.sessions.iter().filter(|session| session.interrupted).count();
                if interrupted > 0 {
                    self.append_status(&format!(
                        "{interrupted} interrupted {} in the journal: reverse {} from the journal",
                        if interrupted == 1 { "run" } else { "runs" },
                        if interrupted == 1 { "it" } else { "them" },
                    ));
                }
            }
            Done::Statted(meta, requested) => {
                self.stat_pending = false;
                // The pass may have been built for the plan before a filter
                // change. The paths, not their count, say whether it still
                // answers for the rows on screen; a stale pass is dropped so
                // the rows are read again instead of marked as read.
                if requested == self.model.meta_paths() {
                    self.model.set_meta(meta);
                } else {
                    self.model.meta_loaded = false;
                }
            }
            Done::Picked(picked) => {
                self.browsing = false;
                match picked {
                    Picked::Folder(Some(folder)) => {
                        self.dir_field = folder.display().to_string();
                        self.add(vec![folder]);
                    }
                    Picked::Folder(None) => {}
                    Picked::Files(paths) => self.add(paths),
                }
            }
            Done::Ran { undo_of, result, session, sessions } => {
                if let Some(session) = session {
                    self.sessions.push(session);
                } else {
                    match sessions {
                        Ok(sessions) => {
                            self.sessions = sessions;
                            self.journal_error = None;
                        }
                        Err(error) => {
                            self.journal_error = Some(format!(
                                "the journal could not be read after the run: {error}; its \
                                 undo records may not be shown"
                            ));
                        }
                    }
                }
                match result {
                    Ok(report) => {
                        self.run_error = None;
                        let run = report.run;
                        let title = match undo_of {
                            Some(undone) => format!("Undo of run {undone}"),
                            None => format!("Applied run {run}"),
                        };
                        let (renamed, failed) = (report.renamed(), report.failed());
                        let counts = (renamed, failed, report.skipped());
                        self.status = format!("run {run}: {renamed} renamed, {failed} failed");
                        self.follow_renames(&report);
                        let listed = dialogs::listed_indices(&report);
                        let rest = dialogs::unlisted_rows(&report);
                        self.overlay =
                            Some(Overlay::Report { title, report, listed, rest, counts });
                    }
                    Err(error) => {
                        self.run_error = Some(format!(
                            "the run stopped: {error}; any renames that completed stay on \
                             disk, and the journal holds their undo"
                        ));
                    }
                }
                // The names on disk have changed, so the preview is stale.
                self.scan();
            }
            Done::Forgotten { result, sessions } => match result {
                Ok(()) => {
                    // A change landed, so every cached read is stale even if
                    // the new list cannot be read back.
                    self.history.forget_all();
                    match sessions {
                        Ok(sessions) => {
                            self.sessions = sessions;
                            self.journal_error = None;
                            self.status = if self.sessions.is_empty() {
                                "journal cleared".to_owned()
                            } else {
                                "journal updated".to_owned()
                            };
                        }
                        Err(error) => {
                            self.journal_error = Some(format!(
                                "the journal was changed, but its runs could not be \
                                 re-read: {error}"
                            ));
                        }
                    }
                }
                Err(error) => {
                    self.status = format!("the journal could not be changed: {error}");
                }
            },
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
                    if style::pointer(ui.button(egui::RichText::new("📄 Add files…").heading()))
                        .accessible_name("Add files…")
                        .clicked()
                    {
                        self.pick_files();
                    }
                    if style::pointer(ui.button(egui::RichText::new("📁 Add folder…").heading()))
                        .accessible_name("Add folder…")
                        .clicked()
                    {
                        self.pick_folder();
                    }
                });
                let typed = style::field(ui, &mut self.dir_field, DIR_FIELD_WIDTH)
                    .accessible_name("Directory");
                // Enter commits the typed path now that there is no Add! button.
                if typed.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                    let dir = PathBuf::from(self.dir_field.trim());
                    if !dir.as_os_str().is_empty() {
                        self.add(vec![dir]);
                    }
                }
                if style::pointer(
                    ui.add_enabled(
                        !self.busy,
                        egui::Checkbox::new(&mut self.recursive, "Subdirectories"),
                    )
                    .on_hover_text("Also add subdirectories"),
                )
                .changed()
                {
                    self.scan();
                }
                style::pointer(
                    ui.add(egui::Checkbox::new(&mut self.fast, "Fast mode")).on_hover_text(
                        "Do not read sizes and times on import, allows for faster processing",
                    ),
                );
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
        if !self.scan_unreadable.is_empty() {
            let first = self.scan_unreadable[0].display();
            let text = match self.scan_unreadable.len() {
                1 => format!("a folder could not be read: {first}"),
                count => format!("{count} folders could not be read; first: {first}"),
            };
            ui.colored_label(style::warning(ui), text);
        }
        if self.scan_truncated {
            ui.colored_label(
                style::warning(ui),
                format!(
                    "the scan stopped at {} entries: narrow the pick or turn off Subdirectories",
                    self.model.entries.len()
                ),
            );
        }
        if !self.model.filter.kinds.dirs && !self.model.filter.kinds.files {
            ui.colored_label(
                style::warning(ui),
                "the filter keeps nothing: tick Directories or Files",
            );
        }
        if not_utf8 > 0 {
            ui.colored_label(
                style::warning(ui),
                format!(
                    "{not_utf8} not UTF-8: regex, case, normalize and the remove rules skip them"
                ),
            );
        }
        if not_nfc > 0 {
            ui.colored_label(style::warning(ui), format!("{not_nfc} not NFC"));
        }
        if let Some(error) = &self.run_error {
            ui.colored_label(style::danger(ui), error.as_str());
        }
        if let Some(error) = &self.journal_error {
            ui.colored_label(style::danger(ui), error.as_str());
        }
        if let Some(error) = &self.model.error {
            ui.colored_label(style::warning(ui), format!("warning: {error}"));
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
                    .on_hover_text("Enable regular expression matching"),
            );
            style::pointer(
                ui.checkbox(&mut self.model.filter.case_sensitive, "case sensitive")
                    .on_hover_text("Enable exact case matching"),
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
            let clear = !self.added.is_empty();
            if ui
                .add_enabled(clear, egui::Button::new("Clear Content"))
                .on_hover_text("Clear the imported content list")
                .clicked()
            {
                self.added.clear();
                self.scan();
            }
        });
    }

    fn footer(&mut self, ui: &mut egui::Ui) {
        let totals = self.model.totals;
        let ready = totals.actionable > 0
            && !self.busy
            && self.model.error.is_none()
            && !self.scan_truncated;
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    ready,
                    egui::Button::new(
                        egui::RichText::new("✏ Rename now").size(style::ACTION_TEXT_SIZE),
                    ),
                )
                .accessible_name("Rename now")
                .on_hover_text("Rename every matched row")
                .clicked()
            {
                self.start_apply();
            }
            ui.label(format!(
                "{} / {}",
                crate::format::plural(totals.files, "file", "files"),
                crate::format::plural(totals.folders, "folder", "folders")
            ));
            // The foot of the content column: the logo opens About.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Equal padding both ways, or the image button frames oblong.
                ui.scope(|ui| {
                    ui.spacing_mut().button_padding = egui::Vec2::splat(style::BUTTON_PADDING.y);
                    let icon = egui::Image::new(&self.logo)
                        .fit_to_exact_size(egui::Vec2::splat(ABOUT_ICON_SIZE));
                    if style::pointer(ui.add(egui::Button::image(icon)))
                        .on_hover_text("About renameAnt")
                        .accessible_name("About renameAnt")
                        .clicked()
                    {
                        self.about.toggle();
                    }
                });
            });
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
            // The ant stays behind the runs, which paint no background of their
            // own, so it reads through the list as well as the empty panel.
            logo::paint(ui, &self.logo);
            if let Some(action) =
                history::show(ui, &self.sessions, self.busy, &self.journal, &mut self.history)
            {
                self.overlay = Some(match action {
                    history::Action::Undo(history::Undo::Run(session)) => {
                        Overlay::ConfirmUndo(session)
                    }
                    history::Action::Undo(history::Undo::Row { run, from, to }) => {
                        Overlay::ConfirmUndoRow { run, from, to }
                    }
                    history::Action::Remove(session) => Overlay::ConfirmRemove(session),
                    history::Action::ClearAll => Overlay::ConfirmClear,
                });
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
            // The count reads as the table's own footer, so it is a panel of its
            // own: the table takes the rest without this caller measuring a line.
            let (shown, scanned) = (self.model.plan.rows.len(), self.model.entries.len());
            let text = if self.model.filter.is_inactive() {
                crate::format::plural(shown, "entry", "entries")
            } else {
                format!("{shown} of {}", crate::format::plural(scanned, "entry", "entries"))
            };
            egui::Panel::bottom("renameant-count")
                .frame(egui::Frame::new())
                .show_separator_line(false)
                .show(ui, |ui| ui.label(egui::RichText::new(text).weak()));
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
            ui.set_min_width(OVERLAY_WIDTH);
            let choice = match &overlay {
                Overlay::ConfirmUndo(session) => dialogs::confirm_undo(ui, session),
                Overlay::ConfirmUndoRow { run, from, to } => {
                    dialogs::confirm_undo_row(ui, *run, from, to)
                }
                Overlay::ConfirmRemove(session) => dialogs::confirm_remove(ui, session),
                Overlay::ConfirmClear => dialogs::confirm_clear(
                    ui,
                    self.sessions.len(),
                    self.sessions.iter().filter(|session| session.interrupted).count(),
                ),
                Overlay::Report { title, report, listed, rest, counts } => {
                    dialogs::show_report(ui, title, report, *counts, listed, *rest)
                }
            };
            match choice {
                Choice::None => {}
                Choice::UndoRun(run) => {
                    keep = false;
                    self.start_undo(run);
                }
                Choice::UndoRow { run, from, to } => {
                    keep = false;
                    self.start_undo_row(run, from, to);
                }
                Choice::Remove(run) => {
                    keep = false;
                    self.start_remove(run);
                }
                Choice::ClearAll => {
                    keep = false;
                    self.start_clear();
                }
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

/// What to say about saved rules this version could not read.
fn unsupported_rules(tags: &[String]) -> String {
    let names = format!("\"{}\"", tags.join("\", \""));
    match tags.len() {
        0 => String::new(),
        1 => format!("saved rule {names} is not supported, so it was removed"),
        _ => format!("saved rules {names} are not supported, so they were removed"),
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
        // A modal shows numbers captured from the current plan; do not replace
        // that plan until the modal is gone.
        if self.overlay.is_none() {
            self.replan_if_due(&ctx);
        }
        self.persist();

        egui::Panel::left("renameant-rules")
            .frame(egui::Frame::new())
            .resizable(true)
            .default_size(RULES_WIDTH)
            .show(ui, |ui| self.rules_panel(ui));
        let mut journal_open = self.journal_open;
        egui::Panel::right("renameant-journal")
            .frame(egui::Frame::new())
            .resizable(true)
            .default_size(JOURNAL_WIDTH)
            .show_collapsible(ui, &mut journal_open, |ui| self.journal_panel(ui));
        self.journal_open = journal_open;
        egui::CentralPanel::default()
            .frame(egui::Frame::new())
            .show(ui, |ui| self.content_panel(ui));
        self.maybe_stat();
        self.show_overlay(&ctx);
        if self.about.show(&ctx, &mut self.log_level) {
            self.save_settings();
        }
    }
}

#[cfg(test)]
mod kittest;

#[cfg(test)]
mod perf;
