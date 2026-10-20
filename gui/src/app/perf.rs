//! The frame budget: per-frame UI work is O(visible rows), never the whole
//! set, so the window fits in one 60 fps period (16.6 ms) with 100k rows on
//! screen. Timed over a synthetic in-memory scan, so the measurement needs no
//! files; the figure is the harness's whole frame — the window plus its own
//! bookkeeping — so it bounds the app's frame from above. Two plan sizes, 100×
//! apart, show the cost follows the visible rows and not the plan. Headless, so
//! this is a frame's CPU side: the GPU present is the real window's, and the
//! sixty frames a second are only as good as this number leaves room for.
//!
//! A measurement, not a check:
//! `cargo test -p renameant-gui --release -- --ignored --nocapture perf`

use std::collections::BTreeSet;
use std::env;
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use renameant_core::scan::{DirIndex, Entry, Kind};

use super::App;
use super::kittest::{ENV, pump_until};

use crate::edit::RuleEdit;

/// One 60 fps period, what the whole frame has to fit in.
const FRAME_BUDGET: Duration = Duration::from_millis(16);
/// The plan sizes to compare, 100× apart.
const SIZES: [usize; 2] = [1_000, 100_000];
const WARMUP_FRAMES: usize = 20;
const TIMED_FRAMES: usize = 100;
/// Wheel steps in the scroll sweep, and the lines each one carries.
const SCROLL_NOTCHES: usize = 20;
const SCROLL_LINES_PER_NOTCH: f32 = 3.0;
/// A point over the table: the rules panel is 340 pt wide, and the table starts
/// below the top and filter rows.
const TABLE_POINT: egui::Pos2 = egui::pos2(740.0, 400.0);
/// The first row's name, at the tail of its Path cell, gone from the tree once
/// the table has scrolled.
const FIRST_NAME: &str = "IMG_000000.txt";

#[test]
#[ignore = "a measurement, not a check"]
fn perf_frames_over_100k_rows() {
    let _guard = ENV.lock().unwrap_or_else(|poisoned| poisoned.into_inner());

    // Home is where the window starts, where the journal is kept and where the
    // rules are saved, so all three are sandboxed before the app is built
    // (edition 2024 makes `set_var` unsafe).
    let home = tempfile::tempdir().expect("a home for the run");
    let state = tempfile::tempdir().expect("somewhere for the journal");
    unsafe {
        env::set_var("HOME", home.path());
        env::set_var("XDG_STATE_HOME", state.path());
        env::set_var("XDG_CONFIG_HOME", state.path());
    }

    let mut harness = Harness::builder()
        .with_size(egui::vec2(1100.0, 720.0))
        .allow_missing_glyphs()
        .build_eframe(|cc| App::new(cc));

    // Nothing is picked at startup, so nothing lands on the model after the
    // synthetic scan replaces what the window holds.
    pump_until(&mut harness, "the first frame", |app| !app.busy);

    let mut worst = Duration::ZERO;
    for count in SIZES {
        let (entries, index) = synthetic(count);
        let app = harness.state_mut();
        app.model.set_scan(entries, index);
        // A pipeline that renames, so the preview shows an After that differs.
        app.edits = vec![RuleEdit { value: "p-".to_owned(), ..RuleEdit::default() }];
        app.replan();
        assert_eq!(app.model.view.len(), count, "the whole scan is in the view");

        for _ in 0..WARMUP_FRAMES {
            harness.step();
        }
        let idle = frame_time(&mut harness, TIMED_FRAMES);
        // The second round starts where the first left off, which is still a
        // scroll; the assert is what keeps a stuck table from being timed.
        let scrolling = scroll(&mut harness);
        assert_eq!(
            harness.query_all_by_label(FIRST_NAME).count(),
            0,
            "the table did not scroll: the frame times are not a scroll"
        );
        println!("{count} rows: idle {idle:?}/frame, scrolling {scrolling:?}/frame");
        worst = worst.max(idle).max(scrolling);
    }
    println!("worst frame {worst:?}, one 60 fps period is {FRAME_BUDGET:?}");
}

/// One directory of `count` files as the scan would have produced it: pure and
/// in-memory, so nothing here touches a filesystem.
fn synthetic(count: usize) -> (Vec<Entry>, DirIndex) {
    let root = PathBuf::from("/perf");
    let mut names = BTreeSet::new();
    let mut entries = Vec::with_capacity(count);
    for index in 0..count {
        let name = OsString::from(format!("IMG_{index:06}.txt"));
        names.insert(name.clone());
        entries.push(Entry { path: root.join(&name), name, kind: Kind::File });
    }
    let mut index = DirIndex::default();
    index.existing.insert(root.clone(), names);
    (entries, index)
}

/// The mean frame over `frames` steps.
fn frame_time(harness: &mut Harness<'_, App>, frames: usize) -> Duration {
    let started = Instant::now();
    for _ in 0..frames {
        harness.step();
    }
    started.elapsed() / frames as u32
}

/// Scrolls the table a notch per frame and returns the worst frame.
fn scroll(harness: &mut Harness<'_, App>) -> Duration {
    harness.hover_at(TABLE_POINT);
    harness.step();
    let mut worst = Duration::ZERO;
    for _ in 0..SCROLL_NOTCHES {
        harness.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: egui::vec2(0.0, -SCROLL_LINES_PER_NOTCH),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
            modifiers: egui::Modifiers::NONE,
        });
        let started = Instant::now();
        harness.step();
        worst = worst.max(started.elapsed());
    }
    worst
}
