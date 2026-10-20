//! Scan, stat, apply and undo on a worker thread.
//!
//! Planning stays on the UI thread: it is pure, syscall-free and bounded to the
//! debounce interval. What lands here is what touches the disk.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use renameant_core::apply::{self, ApplyOptions, ApplyReport};
use renameant_core::journal::{Journal, RunId, Session};
use renameant_core::plan::Plan;
use renameant_core::scan::{self, DirIndex, Entry};

use crate::model::FileMeta;

pub enum Job {
    /// The picked folders and files, and whether to walk everything below the
    /// folders.
    Scan(Vec<PathBuf>, bool),
    /// Size and mtime of each path, for the preview's Size and Modified
    /// columns. This is the only `stat`, and it never runs per frame.
    Stat(Vec<PathBuf>),
    Apply(Box<Plan>, ApplyOptions),
    Undo(ApplyOptions, RunId),
}

/// What the file browser is asked for. It opens on a thread of its own, so the
/// portal round trip never holds a frame.
#[derive(Clone, Copy)]
pub enum Browse {
    /// One folder, for the directory field.
    Folder,
    /// Any number of files, which join the picked list.
    Files,
}

/// What the browser came back with. A dismissed folder dialog is `Folder(None)`;
/// a dismissed file dialog is an empty list.
pub enum Picked {
    Folder(Option<PathBuf>),
    Files(Vec<PathBuf>),
}

pub enum Done {
    Scanned(io::Result<(Vec<Entry>, DirIndex)>),
    /// A stat pass. Paths it could not read are left out.
    Statted(Vec<(PathBuf, FileMeta)>),
    /// A file browser that closed.
    Picked(Picked),
    /// An apply or an undo. `undo_of` names the run that was reversed.
    Ran {
        undo_of: Option<RunId>,
        result: io::Result<ApplyReport>,
        /// The journal as it stands after the run, so the history does not need
        /// a second round trip.
        sessions: Vec<Session>,
    },
}

pub struct Worker {
    jobs: Sender<Job>,
    done: Receiver<Done>,
    /// Held so the browser's own thread can post a `Done` too.
    done_tx: Sender<Done>,
}

impl Worker {
    pub fn spawn() -> Worker {
        let (jobs, job_rx) = mpsc::channel::<Job>();
        let (done_tx, done) = mpsc::channel::<Done>();
        thread::spawn({
            let done_tx = done_tx.clone();
            move || serve(job_rx, done_tx)
        });
        Worker { jobs, done, done_tx }
    }

    pub fn send(&self, job: Job) {
        let _ = self.jobs.send(job);
    }

    /// shortcut: rfd's portal backend is thread-safe on Linux, which is all this
    /// ships for; a GTK or macOS backend would require the main thread.
    pub fn browse(&self, kind: Browse) {
        let done = self.done_tx.clone();
        thread::spawn(move || {
            let picked = match kind {
                Browse::Folder => Picked::Folder(
                    rfd::FileDialog::new().set_title("Choose a folder").pick_folder(),
                ),
                Browse::Files => Picked::Files(
                    rfd::FileDialog::new().set_title("Add files").pick_files().unwrap_or_default(),
                ),
            };
            let _ = done.send(Done::Picked(picked));
        });
    }

    pub fn try_recv(&self) -> Option<Done> {
        self.done.try_recv().ok()
    }
}

fn serve(jobs: Receiver<Job>, done: Sender<Done>) {
    while let Ok(job) = jobs.recv() {
        let message = match job {
            Job::Scan(paths, recursive) => Done::Scanned(scan::scan_paths(&paths, recursive)),
            Job::Stat(paths) => {
                let meta = paths
                    .iter()
                    .filter_map(|path| file_meta(path).map(|meta| (path.clone(), meta)))
                    .collect();
                Done::Statted(meta)
            }
            Job::Apply(plan, options) => {
                let result = apply::apply(&plan, &options);
                Done::Ran { undo_of: None, sessions: sessions_of(&options), result }
            }
            Job::Undo(options, run) => {
                let result = apply::undo(&options, run);
                Done::Ran { undo_of: Some(run), sessions: sessions_of(&options), result }
            }
        };
        if done.send(message).is_err() {
            return;
        }
    }
}

/// The link's own metadata, never the target's. `None` when the entry
/// cannot be read, which the preview sorts last.
fn file_meta(path: &Path) -> Option<FileMeta> {
    let meta = fs::symlink_metadata(path).ok()?;
    let size = (!meta.is_dir()).then_some(meta.len());
    Some(FileMeta { size, mtime: meta.modified().ok() })
}

/// A journal that cannot be read is an empty history, not a lost run.
fn sessions_of(options: &ApplyOptions) -> Vec<Session> {
    Journal::sessions(&options.journal).unwrap_or_default()
}
