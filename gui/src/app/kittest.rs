//! Headless UI driving, so the whole window is exercised rather than the model
//! alone. Panels, the rule presets, the run report and the undo are all clicked
//! by their AccessKit label.

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

use super::{App, Overlay};
use crate::edit::{InsertKind, RuleEdit, RuleKind};
use crate::model::{FileMeta, SortKey};
use crate::worker::Done;
use renameant_core::apply::{ApplyError, ApplyReport, ApplyRow, Outcome};
use renameant_core::journal::Session;
use renameant_core::rule::Target;
use renameant_core::scan::{DirIndex, Entry, Kind};

/// `set_var` is process-global and unsafe while another thread may read the
/// environment. Only the tests that build an `App` set it.
pub(super) static ENV: Mutex<()> = Mutex::new(());

/// The app debounces replans by 100 ms of real time and runs scan, apply and
/// undo on a worker thread, so the harness pulls frames until a condition holds
/// instead of trusting a fixed frame count.
const STEP_SLEEP: Duration = Duration::from_millis(10);
const MAX_PUMP_STEPS: usize = 600;
/// The harness window, wide enough for the rules, the table and the journal.
const WINDOW_WIDTH: f32 = 1500.0;
const WINDOW_HEIGHT: f32 = 860.0;

pub(super) fn pump_until(
    harness: &mut Harness<'_, App>,
    what: &str,
    mut settled: impl FnMut(&App) -> bool,
) {
    for _ in 0..MAX_PUMP_STEPS {
        if settled(harness.state()) {
            return;
        }
        harness.step();
        std::thread::sleep(STEP_SLEEP);
    }
    let app = harness.state();
    panic!(
        "{what} did not settle in {MAX_PUMP_STEPS} frames: busy={}, status={:?}, overlay={}",
        app.busy,
        app.status,
        overlay_name(app),
    );
}

/// Pull frames until at least `at_least` nodes carry `label`. A modal is an
/// `egui::Area`, and egui keeps an area invisible on the frame it first appears
/// (to hide first-frame jitter), so its nodes reach the tree a frame later.
fn pump_for_labels(harness: &mut Harness<'_, App>, label: &str, at_least: usize) {
    for _ in 0..MAX_PUMP_STEPS {
        if harness.query_all_by_label(label).count() >= at_least {
            return;
        }
        harness.step();
        std::thread::sleep(STEP_SLEEP);
    }
    panic!("expected at least {at_least} node(s) labelled {label:?}");
}

/// The overlay in play, for diagnostics when a pump times out.
fn overlay_name(app: &App) -> &'static str {
    match &app.overlay {
        None => "none",
        Some(Overlay::ConfirmUndo(_)) => "confirm-undo",
        Some(Overlay::ConfirmUndoRow { .. }) => "confirm-undo-row",
        Some(Overlay::ConfirmRemove(_)) => "confirm-remove",
        Some(Overlay::ConfirmClear) => "confirm-clear",
        Some(Overlay::Report { .. }) => "report",
    }
}

/// Sorted names in `dir`, so the assertion does not depend on read order.
fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .expect("read the directory")
        .map(|entry| entry.expect("a directory entry").file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn the_window_scans_plans_applies_and_undoes() {
    let _guard = ENV.lock().unwrap_or_else(|poisoned| poisoned.into_inner());

    let root = tempfile::tempdir().expect("a home for the run");
    let target = root.path().join("files");
    fs::create_dir(&target).expect("the scanned directory");
    for name in ["a.txt", "b.txt", "c.txt"] {
        fs::write(target.join(name), b"x").expect("a file");
    }
    let state = tempfile::tempdir().expect("somewhere for the journal");

    // Home is where the window starts, where the journal is kept and where the
    // rules are saved, so all three are sandboxed before the app is built
    // (edition 2024 makes `set_var` unsafe).
    unsafe {
        env::set_var("HOME", root.path());
        env::set_var("XDG_STATE_HOME", state.path());
        env::set_var("XDG_CONFIG_HOME", state.path());
    }

    let mut harness = Harness::builder()
        .with_size(egui::vec2(WINDOW_WIDTH, WINDOW_HEIGHT))
        // The default accessibility check stays on: every input the window draws
        // is named, so an unnamed one fails the test.
        .allow_missing_glyphs()
        .build_eframe(|cc| App::new(cc));

    // The window starts with nothing picked, so it is pointed at the fixture
    // before the first scan runs.
    pump_until(&mut harness, "the first frame", |app| !app.busy);
    harness.state_mut().added = vec![target.clone()];
    harness.state_mut().scan();
    pump_until(&mut harness, "the fixture scan", |app| app.model.entries.len() == 3 && !app.busy);
    assert_eq!(harness.state().model.meta.len(), 3, "sizes are read without a sort asking");
    assert_eq!(harness.state().model.totals.actionable, 0, "an empty pipeline changes nothing");

    // The headers are the sort control: clicking one sorts by that column, and
    // clicking it again flips the direction.
    harness.get_by_label("New Name").click();
    pump_until(&mut harness, "the header sort", |app| app.model.sort.key == SortKey::After);
    assert!(!harness.state().model.sort.desc);
    harness.get_by_label("New Name").click();
    pump_until(&mut harness, "the direction flip", |app| app.model.sort.desc);
    // Apply runs the plan, not the view, so the sort is undone for the rest of
    // the run through the same control the window offers.
    harness.get_by_label("Reset sort").click();
    pump_until(&mut harness, "the way back", |app| app.model.sort.key == SortKey::Apply);

    // A rule that needs no typing: number the names 001, 002, 003.
    harness.state_mut().edits.push(RuleEdit {
        enabled: true,
        kind: RuleKind::Number,
        target: Target::Stem,
        ..RuleEdit::default()
    });
    pump_until(&mut harness, "the replan", |app| app.model.totals.actionable == 3);
    assert_eq!(harness.state().model.totals.candidates, 3);
    assert_eq!(harness.state().model.totals.files, 3);

    // Apply, straight from the button: there is no confirmation to click.
    harness.get_by_label("Rename now").click();
    pump_until(&mut harness, "the run report", |app| {
        matches!(app.overlay, Some(Overlay::Report { .. }))
    });
    assert_eq!(names(&target), ["001.txt", "002.txt", "003.txt"]);

    pump_for_labels(&mut harness, "Close", 1);
    harness.get_by_label("Close").click_accesskit();
    pump_until(&mut harness, "the rescan after apply", |app| app.overlay.is_none() && !app.busy);

    // Undo, through its confirmation: the journal's run button, then the
    // modal's. The two have distinct accessible names, so no position lookup.
    assert_eq!(harness.state().sessions.len(), 1);
    harness.get_by_label("undo run 1").click();
    harness.step();
    assert!(matches!(harness.state().overlay, Some(Overlay::ConfirmUndo(_))));
    pump_for_labels(&mut harness, "confirm undo run 1", 1);
    harness.get_by_label("confirm undo run 1").click_accesskit();
    pump_until(&mut harness, "the undo report", |app| {
        matches!(app.overlay, Some(Overlay::Report { .. }))
    });
    assert_eq!(names(&target), ["a.txt", "b.txt", "c.txt"]);

    // Fast mode: the scan reads no sizes, and the first size sort is what asks
    // for them. The undo's report is closed first, because a modal holds the
    // stat pass off by design.
    pump_for_labels(&mut harness, "Close", 1);
    harness.get_by_label("Close").click_accesskit();
    pump_until(&mut harness, "the undo report closing", |app| app.overlay.is_none());
    harness.state_mut().fast = true;
    harness.state_mut().scan();
    pump_until(&mut harness, "the fast scan", |app| app.model.entries.len() == 3 && !app.busy);
    assert!(harness.state().model.meta.is_empty(), "fast mode leaves the stat pass to a sort");
    // Clicked through AccessKit: the header is a plain button, and the click
    // action reaches it wherever it lands.
    harness.get_by_label("Size").click_accesskit();
    pump_until(&mut harness, "the sort's own stat pass", |app| {
        app.model.meta.len() == 3 && !app.busy
    });

    // Every rule kind's fields, so the accessibility check sees each one: an
    // input the editor adds without a name fails here.
    harness.state_mut().edits =
        RuleKind::ALL.iter().map(|kind| RuleEdit { kind: *kind, ..RuleEdit::default() }).collect();
    harness.run();
}

/// A picked path follows only a rename that happened. A failed row's
/// destination was never created, so pointing the list at it would make the
/// next scan drop the file from the window.
#[test]
fn a_picked_path_follows_only_a_rename_that_happened() {
    let _guard = ENV.lock().unwrap_or_else(|poisoned| poisoned.into_inner());

    let home = tempfile::tempdir().expect("a home for the run");
    let state = tempfile::tempdir().expect("somewhere for the journal");
    unsafe {
        env::set_var("HOME", home.path());
        env::set_var("XDG_STATE_HOME", state.path());
        env::set_var("XDG_CONFIG_HOME", state.path());
    }

    let mut harness = Harness::builder()
        .with_size(egui::vec2(WINDOW_WIDTH, WINDOW_HEIGHT))
        .allow_missing_glyphs()
        .build_eframe(|cc| App::new(cc));
    harness.step();

    let app = harness.state_mut();
    app.added = vec![PathBuf::from("/d/renamed"), PathBuf::from("/d/failed")];
    let report = ApplyReport {
        run: 1,
        at: 0,
        records: 1,
        aborted: false,
        rows: vec![
            ApplyRow {
                src: "/d/renamed".into(),
                dst: "/d/new-name".into(),
                outcome: Outcome::Renamed,
            },
            ApplyRow {
                src: "/d/failed".into(),
                dst: "/d/never-made".into(),
                outcome: Outcome::Failed(ApplyError::PermissionDenied),
            },
        ],
    };

    app.follow_renames(&report);

    assert_eq!(app.added, vec![PathBuf::from("/d/new-name"), PathBuf::from("/d/failed")]);
}

/// A scan answers for the picks it was given. Once the user clears them, that
/// answer must not put the rows back.
#[test]
fn a_scan_that_lands_after_clear_content_is_dropped() {
    let _guard = ENV.lock().unwrap_or_else(|poisoned| poisoned.into_inner());

    let home = tempfile::tempdir().expect("a home for the run");
    let state = tempfile::tempdir().expect("somewhere for the journal");
    let target = home.path().join("files");
    fs::create_dir(&target).expect("the scanned directory");
    fs::write(target.join("a.txt"), b"x").expect("a file");
    unsafe {
        env::set_var("HOME", home.path());
        env::set_var("XDG_STATE_HOME", state.path());
        env::set_var("XDG_CONFIG_HOME", state.path());
    }

    let mut harness = Harness::builder()
        .with_size(egui::vec2(WINDOW_WIDTH, WINDOW_HEIGHT))
        .allow_missing_glyphs()
        .build_eframe(|cc| App::new(cc));
    pump_until(&mut harness, "the first frame", |app| !app.busy);

    harness.state_mut().added = vec![target];
    harness.state_mut().scan();
    // Clear before the frame that would apply the answer.
    harness.state_mut().added.clear();
    harness.state_mut().scan();
    pump_until(&mut harness, "the stale scan", |app| !app.busy);

    assert!(harness.state().model.entries.is_empty());
    assert_eq!(harness.state().status, "nothing picked");
}

/// A rescan rewrites the status line. A failed run and an unreadable journal
/// have to stay visible, and a failed read must not replace the old list.
#[test]
fn a_failed_run_and_an_unreadable_journal_stay_on_screen() {
    let _guard = ENV.lock().unwrap_or_else(|poisoned| poisoned.into_inner());

    let home = tempfile::tempdir().expect("a home for the run");
    let state = tempfile::tempdir().expect("somewhere for the journal");
    unsafe {
        env::set_var("HOME", home.path());
        env::set_var("XDG_STATE_HOME", state.path());
        env::set_var("XDG_CONFIG_HOME", state.path());
    }

    let mut harness = Harness::builder()
        .with_size(egui::vec2(WINDOW_WIDTH, WINDOW_HEIGHT))
        .allow_missing_glyphs()
        .build_eframe(|cc| App::new(cc));
    pump_until(&mut harness, "the first frame", |app| !app.busy);

    let session =
        Session { run: 1, at: 0, renames: 1, overwrite: false, undo_of: None, interrupted: false };
    let app = harness.state_mut();
    app.sessions = vec![session.clone()];
    app.handle(Done::Ran {
        undo_of: None,
        result: Err(io::Error::other("disk full")),
        session: None,
        sessions: Err(io::Error::other("corrupt journal")),
    });

    assert!(app.run_error.as_deref().is_some_and(|text| text.contains("disk full")));
    assert!(app.journal_error.as_deref().is_some_and(|text| text.contains("corrupt journal")));
    assert_eq!(app.sessions.len(), 1, "an unreadable journal keeps the old list");

    app.scan();
    assert!(app.run_error.is_some(), "a rescan must not clear the failure");
    assert!(app.journal_error.is_some(), "a rescan must not clear the read failure");

    app.handle(Done::JournalLoaded { sessions: Ok(Vec::new()), prune_error: None });
    assert!(app.journal_error.is_none(), "a good read clears the journal warning");
    assert!(app.run_error.is_some(), "the run warning waits for the next run");

    let report = ApplyReport { run: 2, at: 0, records: 1, aborted: false, rows: vec![] };
    app.handle(Done::Ran {
        undo_of: None,
        result: Ok(report),
        session: Some(Session { run: 2, ..session }),
        sessions: Ok(Vec::new()),
    });
    assert!(app.run_error.is_none(), "a good run clears the last failure");
}

/// A stat pass built for the plan before a filter change must not mark the
/// newly exposed rows as read.
#[test]
fn a_stat_pass_for_the_old_plan_is_retried() {
    let _guard = ENV.lock().unwrap_or_else(|poisoned| poisoned.into_inner());

    let home = tempfile::tempdir().expect("a home for the run");
    let state = tempfile::tempdir().expect("somewhere for the journal");
    unsafe {
        env::set_var("HOME", home.path());
        env::set_var("XDG_STATE_HOME", state.path());
        env::set_var("XDG_CONFIG_HOME", state.path());
    }

    let mut harness = Harness::builder()
        .with_size(egui::vec2(WINDOW_WIDTH, WINDOW_HEIGHT))
        .allow_missing_glyphs()
        .build_eframe(|cc| App::new(cc));
    pump_until(&mut harness, "the first frame", |app| !app.busy);

    let entry = |name: &str| Entry {
        path: PathBuf::from("/x").join(name),
        name: name.into(),
        kind: Kind::File,
    };
    let mut index = DirIndex::default();
    index.insert(
        PathBuf::from("/x"),
        ["a", "b"].iter().map(|name| std::ffi::OsString::from(*name)).collect(),
    );
    let app = harness.state_mut();
    app.model.set_scan(vec![entry("a"), entry("b")], index);
    app.replan();
    assert_eq!(app.model.plan.rows.len(), 2);

    let meta = FileMeta { size: Some(1), mtime: None };
    app.handle(Done::Statted(vec![(PathBuf::from("/x/a"), meta)], 1));
    assert!(!app.model.meta_loaded, "one of the two rows was read");

    app.maybe_stat();
    assert!(app.stat_pending, "the pass is asked for again");

    app.handle(Done::Statted(
        vec![(PathBuf::from("/x/a"), meta), (PathBuf::from("/x/b"), meta)],
        2,
    ));
    assert!(app.model.meta_loaded);
}

/// A modal shows numbers captured from the plan. A pending edit must not swap
/// that plan until the modal closes.
#[test]
fn a_pending_replan_waits_for_the_modal_to_close() {
    let _guard = ENV.lock().unwrap_or_else(|poisoned| poisoned.into_inner());

    let home = tempfile::tempdir().expect("a home for the run");
    let state = tempfile::tempdir().expect("somewhere for the journal");
    unsafe {
        env::set_var("HOME", home.path());
        env::set_var("XDG_STATE_HOME", state.path());
        env::set_var("XDG_CONFIG_HOME", state.path());
    }

    let mut harness = Harness::builder()
        .with_size(egui::vec2(WINDOW_WIDTH, WINDOW_HEIGHT))
        .allow_missing_glyphs()
        .build_eframe(|cc| App::new(cc));
    pump_until(&mut harness, "the first frame", |app| !app.busy);

    let entry = Entry { path: "/x/a".into(), name: "a".into(), kind: Kind::File };
    let mut index = DirIndex::default();
    index.insert(PathBuf::from("/x"), std::iter::once(std::ffi::OsString::from("a")).collect());
    let app = harness.state_mut();
    app.model.set_scan(vec![entry], index);
    app.model.meta_loaded = true;
    app.edits = vec![RuleEdit {
        enabled: true,
        kind: RuleKind::Insert,
        insert: InsertKind::Prefix,
        value: "p-".to_owned(),
        ..RuleEdit::default()
    }];
    app.replan();
    let before = app.model.plan.clone();

    app.edits[0].value = "q-".to_owned();
    app.dirty_since = Some(Instant::now() - Duration::from_secs(1));
    app.overlay = Some(Overlay::ConfirmClear);

    harness.step();
    assert_eq!(harness.state().model.plan, before, "the modal freezes the plan");

    harness.state_mut().overlay = None;
    harness.step();
    assert_ne!(harness.state().model.plan, before, "closing the modal replans");
}
