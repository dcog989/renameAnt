//! Headless UI driving, so the whole window is exercised rather than the model
//! alone. Panels, the rule presets, the apply confirmation, the run
//! report and the undo are all clicked by their AccessKit label.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

use super::{App, Overlay};
use crate::edit::{RuleEdit, RuleKind};
use crate::model::SortKey;
use renameant_core::apply::{ApplyError, ApplyReport, ApplyRow, Outcome};
use renameant_core::rule::Target;

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
        Some(Overlay::ConfirmApply) => "confirm-apply",
        Some(Overlay::ConfirmUndo(_)) => "confirm-undo",
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

    // Apply, through the confirmation. The modal's own buttons are clicked via
    // AccessKit: a pointer click at their rect is swallowed by the modal's
    // backdrop, while the AccessKit Click action reaches the button.
    harness.get_by_label("Rename The Content").click();
    harness.step();
    assert!(matches!(harness.state().overlay, Some(Overlay::ConfirmApply)));
    pump_for_labels(&mut harness, "Apply", 1);
    harness.get_by_label("Apply").click_accesskit();
    pump_until(&mut harness, "the run report", |app| {
        matches!(app.overlay, Some(Overlay::Report { .. }))
    });
    assert_eq!(names(&target), ["001.txt", "002.txt", "003.txt"]);

    pump_for_labels(&mut harness, "Close", 1);
    harness.get_by_label("Close").click_accesskit();
    pump_until(&mut harness, "the rescan after apply", |app| app.overlay.is_none() && !app.busy);

    // Undo, through its confirmation: the journal's Undo, then the modal's. The
    // two share a label, so pick the modal's by its place under the
    // confirmation's heading; the journal's Undo is above the modal.
    assert_eq!(harness.state().sessions.len(), 1);
    harness.get_by_label("Undo").click();
    harness.step();
    assert!(matches!(harness.state().overlay, Some(Overlay::ConfirmUndo(_))));
    pump_for_labels(&mut harness, "Undo", 2);
    let heading = harness.get_by_label("Undo run 1").rect();
    harness
        .get_all_by_label("Undo")
        .find(|node| node.rect().top() > heading.bottom())
        .expect("the confirmation's Undo button")
        .click_accesskit();
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
