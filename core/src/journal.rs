//! Append-only JSONL journal.
//!
//! One record per *completed* rename, `fsync`ed before the next begins — unless
//! fast mode batches the `fsync`, which trades the last batch of undo records
//! against a crash for one write per rename instead of one disk round trip. Names
//! are stored as raw bytes because a JSON string cannot hold them, and paths
//! are absolute so undo does not depend on the working directory.
//!
//! An open [`Journal`] holds an exclusive `flock` for its lifetime, so one run at
//! a time claims a run id and appends it.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use rustix::fs::{FlockOperation, flock, fsync};
use rustix::io::Errno;
use serde::{Deserialize, Serialize};

use crate::bytes_serde;

/// Journal file name under `$XDG_STATE_HOME/renameant/`.
const JOURNAL_FILE: &str = "journal.jsonl";
const STATE_DIR: &str = "renameant";
/// Bytes read per backward step when locating a run's renames.
const TAIL_CHUNK: usize = 64 * 1024;
/// Runs older than this are dropped at startup.
const RETENTION_DAYS: u64 = 60;
const SECONDS_PER_DAY: u64 = 24 * 60 * 60;

pub type RunId = u64;

/// The file a rename moved, as the kernel identifies it. Undo uses it to
/// refuse a path that now holds a different file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileId {
    pub dev: u64,
    pub ino: u64,
}

impl FileId {
    pub(crate) fn of(path: &Path) -> Option<FileId> {
        let meta = std::fs::symlink_metadata(path).ok()?;
        Some(FileId { dev: meta.dev(), ino: meta.ino() })
    }
}

/// One completed rename, with the identity of the file it put at `to`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedRename {
    pub from: PathBuf,
    pub to: PathBuf,
    /// `None` for a record written by a version that did not store one.
    pub id: Option<FileId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcome {
    /// Every planned row was renamed.
    Completed,
    /// Some rows were renamed and some failed. Partial application is normal
    /// and stays undoable.
    Partial,
    /// Nothing was renamed.
    Aborted,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "record", rename_all = "snake_case")]
pub enum JournalRecord {
    /// Written before the first rename. A `begin` with no matching `end` is an
    /// interrupted run, surfaced as such and never resumed.
    Begin {
        run: RunId,
        at: u64,
        planned: usize,
        /// The overwrite opt-in was enabled, so this session is irrecoverable.
        overwrite: bool,
        /// Set when this run is an undo of another run.
        undo_of: Option<RunId>,
    },
    /// One completed rename.
    Rename {
        run: RunId,
        #[serde(with = "bytes_serde::path")]
        from: PathBuf,
        #[serde(with = "bytes_serde::path")]
        to: PathBuf,
        /// The file the rename put at `to`, so undo can tell it from a
        /// replacement. Missing in records written before this field existed.
        #[serde(default)]
        id: Option<FileId>,
    },
    End {
        run: RunId,
        at: u64,
        outcome: RunOutcome,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    pub run: RunId,
    pub at: u64,
    /// Completed rename records, which is exactly what undo can reverse. A
    /// staged cycle writes two records per file, so a two-file cycle counts
    /// four.
    pub renames: usize,
    pub overwrite: bool,
    pub undo_of: Option<RunId>,
    /// No `end` record: the run was interrupted.
    pub interrupted: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RecordKind {
    Begin,
    Rename,
    End,
}

/// The metadata of a record, without its paths. Deserializing this skips the
/// base64 `from`/`to` a full record carries, which is where its cost is, so the
/// whole journal can be scanned for run metadata without decoding a single path.
#[derive(Deserialize)]
struct RecordMeta {
    record: RecordKind,
    run: RunId,
    #[serde(default)]
    at: u64,
    #[serde(default)]
    overwrite: bool,
    #[serde(default)]
    undo_of: Option<RunId>,
}

/// The journal file and its lock, held while it is read and rewritten.
struct LockedJournal {
    /// Read handle; the fallback lock is held on it when no lock file exists.
    file: File,
    /// The lock file beside the journal, if one could be created.
    _lock: Option<File>,
}

/// `$XDG_STATE_HOME/renameant/journal.jsonl`, default
/// `~/.local/state/renameant/journal.jsonl`.
pub fn default_path() -> io::Result<PathBuf> {
    let base = match std::env::var_os("XDG_STATE_HOME") {
        Some(value) if !value.is_empty() && Path::new(&value).is_absolute() => PathBuf::from(value),
        _ => {
            let home = std::env::var_os("HOME").ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "neither XDG_STATE_HOME nor HOME is set, so the journal has no location",
                )
            })?;
            let home = PathBuf::from(home);
            if !home.is_absolute() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "HOME is not an absolute path, so the journal has no stable location",
                ));
            }
            home.join(".local").join("state")
        }
    };
    Ok(base.join(STATE_DIR).join(JOURNAL_FILE))
}

/// Locks a file beside the journal. The journal itself is replaced by prune
/// and remove, so a lock on it would move to an unlinked inode; this one does
/// not. `None` means the directory does not allow a lock file, and the caller
/// must lock the journal file itself.
fn lock_file(path: &Path, operation: FlockOperation) -> io::Result<Option<File>> {
    let lock_path = path.with_extension("lock");
    let lock = match OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(&lock_path)
    {
        Ok(lock) => lock,
        Err(_) => return Ok(None),
    };
    match flock(&lock, operation) {
        Ok(()) => Ok(Some(lock)),
        Err(Errno::WOULDBLOCK) => Err(io::Error::from(io::ErrorKind::WouldBlock)),
        Err(errno) => Err(errno.into()),
    }
}

/// A process-wide lock, so a second window cannot overwrite the first one's
/// rules. `None` means another instance already holds it.
pub fn lock_instance(path: &Path) -> io::Result<Option<File>> {
    if let Some(parent) = path.parent() {
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(parent)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(path)?;
    match flock(&file, FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => Ok(Some(file)),
        Err(Errno::WOULDBLOCK) => Ok(None),
        Err(errno) => Err(errno.into()),
    }
}

pub(crate) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

pub struct Journal {
    file: File,
    /// The lock beside the journal. `None` when the journal file itself had to
    /// be locked; the lock is then held by `file`.
    _lock: Option<File>,
    /// Bytes in the file, so a failed append can be truncated away.
    len: u64,
    /// Records written, and how many of them are on disk.
    written: usize,
    /// Rename records written by this run, staged halves included.
    renames: usize,
    synced: usize,
    /// Records between two `fsync`s. One is the durable default; fast mode
    /// passes [`BATCHED_SYNC_EVERY`].
    sync_every: usize,
}

/// Records written between two `fsync`s in fast mode. A crash can lose at most
/// this many renames' undo records, and the renames themselves are never at
/// stake: measured on a btrfs/nvme, a per-record `fsync` costs ~785 us against
/// ~8 us batched.
pub const BATCHED_SYNC_EVERY: usize = 256;

impl Journal {
    /// Opens the journal with one `fsync` per record: every completed rename is
    /// on disk before the next begins.
    pub fn open(path: PathBuf) -> io::Result<Journal> {
        Journal::open_syncing(path, 1)
    }

    /// Opens the journal, `fsync`ing every `sync_every` records. A run calls
    /// [`Journal::sync`] at its end either way, so only the batch a crash lands
    /// in is ever at risk.
    pub fn open_syncing(path: PathBuf, sync_every: usize) -> io::Result<Journal> {
        if let Some(parent) = path.parent() {
            std::fs::DirBuilder::new().recursive(true).mode(0o700).create(parent)?;
        }
        // One writer per journal: hold an exclusive lock for the whole run so
        // two processes cannot read the same next run id and both append it.
        // The lock file stays put when the journal is rewritten. Released when
        // this `Journal` drops.
        let lock = lock_file(&path, FlockOperation::LockExclusive)?;
        let mut file =
            OpenOptions::new().create(true).read(true).append(true).mode(0o600).open(&path)?;
        if lock.is_none() {
            flock(&file, FlockOperation::LockExclusive)?;
        }
        // The journal's own name must survive a crash too, or the first run's
        // records can vanish with the file that held them. One directory sync
        // per run, not per record.
        if let Some(parent) = path.parent() {
            fsync(&File::open(parent)?)?;
        }
        // A crash or a short write can leave half a record at the end. Drop it
        // before anything appends to it; otherwise the next record would turn
        // the truncated tail into a corrupt line in the middle.
        truncate_partial_tail(&mut file)?;
        let len = file.metadata()?.len();
        Ok(Journal {
            file,
            _lock: lock,
            len,
            written: 0,
            renames: 0,
            synced: 0,
            sync_every: sync_every.max(1),
        })
    }

    /// Appends one record, `fsync`ing it when it closes a batch. A failure here
    /// aborts the run, and a partial write is truncated back so the journal
    /// stays readable.
    pub fn append(&mut self, record: &JournalRecord) -> io::Result<()> {
        let mut line = serde_json::to_string(record)?;
        line.push('\n');
        if let Err(error) = self.file.write_all(line.as_bytes()) {
            let _ = self.file.set_len(self.len);
            return Err(error);
        }
        if let Err(error) = self.file.flush() {
            let _ = self.file.set_len(self.len);
            return Err(error);
        }
        self.len += line.len() as u64;
        self.written += 1;
        if matches!(record, JournalRecord::Rename { .. }) {
            self.renames += 1;
        }
        if self.written - self.synced >= self.sync_every {
            self.sync()?;
        }
        Ok(())
    }

    /// Puts every record written since the last `fsync` on disk. A no-op when
    /// they are already there.
    pub fn sync(&mut self) -> io::Result<()> {
        if self.written == self.synced {
            return Ok(());
        }
        fsync(&self.file)?;
        self.synced = self.written;
        Ok(())
    }

    /// Rename records written so far, staged halves included.
    pub(crate) fn renames_written(&self) -> usize {
        self.renames
    }

    /// Reads the journal line by line, handing each complete line to `on_line`.
    /// A missing file is an empty journal. A truncated final line is what a crash
    /// mid-append leaves behind, so it ends the read without failing and is
    /// never parsed; every reader applies the same rule.
    fn for_each_line(
        path: &Path,
        mut on_line: impl FnMut(&str) -> io::Result<()>,
    ) -> io::Result<()> {
        let file = match File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };

        let mut reader = BufReader::new(file);
        for_each_complete_line(&mut reader, |line| {
            let text = std::str::from_utf8(line).map_err(invalid_line)?;
            on_line(text)?;
            Ok(false)
        })
    }

    /// Next free run id: one past the last `begin` in the journal. Runs are
    /// appended in order, so the last `begin` carries the highest id; only the
    /// tail is read, never the whole journal. The startup prune reads every line,
    /// so a corrupt older run is still reported there.
    pub fn next_run_id(path: &Path) -> io::Result<RunId> {
        let mut file = match File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(1),
            Err(error) => return Err(error),
        };
        let mut last = 0;
        for_each_line_backward(&mut file, |line| {
            let meta = parse_meta(std::str::from_utf8(line).map_err(invalid_line)?)?;
            if meta.record == RecordKind::Begin {
                last = meta.run;
                return Ok(true);
            }
            Ok(false)
        })?;
        Ok(last + 1)
    }

    /// Reads the journal. A missing file is an empty journal. A truncated final
    /// line is what a crash mid-append leaves behind, so it ends the read; any
    /// other unparseable line is an error rather than a silent drop.
    #[cfg(test)]
    pub(crate) fn read(path: &Path) -> io::Result<Vec<JournalRecord>> {
        let mut records: Vec<JournalRecord> = Vec::new();
        Journal::for_each_line(path, |line| {
            let record = serde_json::from_str(line).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("journal line {}: {error}", records.len() + 1),
                )
            })?;
            records.push(record);
            Ok(())
        })?;
        Ok(records)
    }

    /// Past runs, oldest first. Only the run metadata is deserialized, so listing
    /// the history never decodes a path.
    pub fn sessions(path: &Path) -> io::Result<Vec<Session>> {
        let mut sessions: Vec<Session> = Vec::new();
        let mut at_run: HashMap<RunId, usize> = HashMap::new();

        Journal::for_each_line(path, |line| {
            let meta = parse_meta(line)?;
            match meta.record {
                RecordKind::Begin => {
                    at_run.insert(meta.run, sessions.len());
                    sessions.push(Session {
                        run: meta.run,
                        at: meta.at,
                        renames: 0,
                        overwrite: meta.overwrite,
                        undo_of: meta.undo_of,
                        interrupted: true,
                    });
                }
                RecordKind::Rename => {
                    if let Some(&index) = at_run.get(&meta.run) {
                        sessions[index].renames += 1;
                    }
                }
                RecordKind::End => {
                    if let Some(&index) = at_run.get(&meta.run) {
                        sessions[index].interrupted = false;
                    }
                }
            }
            Ok(())
        })?;
        Ok(sessions)
    }

    /// Drops runs older than the retention window, keeping the newest run. Only
    /// an oldest prefix goes, so the highest run id stays in the file and
    /// [`Journal::next_run_id`] can never reuse one.
    ///
    /// Reads the journal line by line and copies the kept suffix through a
    /// temporary file, so it never holds the whole journal in memory. A crash
    /// leaves the old journal intact, and a corrupt line refuses the prune
    /// rather than drop what it cannot read. The truncated final line a crash
    /// leaves is dropped. Returns the runs removed.
    pub fn prune(path: &Path) -> io::Result<usize> {
        let mut locked = match Journal::lock_journal(path) {
            Ok(Some(locked)) => locked,
            Ok(None) => return Ok(0),
            // Another instance is mid-run. Leave its journal alone; the next
            // start prunes.
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(0),
            Err(error) => return Err(error),
        };

        // Run ids rise in append order, so the newest run is the highest id and
        // its `begin` is the earliest line that must survive.
        let mut begins: Vec<(RunId, u64, u64)> = Vec::new();
        let mut offset = 0u64;
        {
            let mut reader = BufReader::new(&mut locked.file);
            for_each_complete_line(&mut reader, |line| {
                let meta = parse_meta(std::str::from_utf8(line).map_err(invalid_line)?)?;
                if meta.record == RecordKind::Begin {
                    begins.push((meta.run, meta.at, offset));
                }
                offset += line.len() as u64 + 1;
                Ok(false)
            })?;
        }
        let Some(newest_run) = begins.iter().map(|(run, _, _)| *run).max() else {
            return Ok(0);
        };
        let cutoff = now().saturating_sub(RETENTION_DAYS * SECONDS_PER_DAY);
        let Some(first_kept) = begins
            .iter()
            .filter(|(run, at, _)| *run == newest_run || *at >= cutoff)
            .map(|(_, _, offset)| *offset)
            .min()
        else {
            return Ok(0);
        };
        if first_kept == 0 {
            return Ok(0);
        }
        let dropped = begins.iter().filter(|(_, _, offset)| *offset < first_kept).count();

        locked.file.seek(SeekFrom::Start(first_kept))?;
        let mut reader = BufReader::new(&mut locked.file);
        Journal::rewrite_temp(path, |out| {
            for_each_complete_line(&mut reader, |line| {
                out.write_all(line)?;
                out.write_all(b"\n")?;
                Ok(false)
            })?;
            Ok(true)
        })?;
        Ok(dropped)
    }

    /// Removes `runs` and every record of them, leaving the other runs undoable.
    /// The files they renamed are untouched.
    ///
    /// Only whole lines go, and no surviving record can name a newer run, so a
    /// run id is reused only when the removed run was the newest — which nothing
    /// pointed at. Returns the runs removed.
    pub fn remove_runs(path: &Path, runs: &[RunId]) -> io::Result<usize> {
        if runs.is_empty() {
            return Ok(0);
        }
        let Some(mut locked) = Journal::lock_journal(path)? else { return Ok(0) };
        let mut removed = 0usize;
        let mut changed = false;
        Journal::rewrite_temp(path, |out| {
            let mut reader = BufReader::new(&mut locked.file);
            for_each_complete_line(&mut reader, |line| {
                let meta = parse_meta(std::str::from_utf8(line).map_err(invalid_line)?)?;
                if runs.contains(&meta.run) {
                    changed = true;
                    if meta.record == RecordKind::Begin {
                        removed += 1;
                    }
                } else {
                    out.write_all(line)?;
                    out.write_all(b"\n")?;
                }
                Ok(false)
            })?;
            Ok(changed)
        })?;
        Ok(if changed { removed } else { 0 })
    }

    /// Empties the journal, keeping the file so its permissions stay. Every
    /// run's undo is lost; the files they renamed are untouched.
    pub fn clear(path: &Path) -> io::Result<()> {
        // Lock before opening: a rewrite may replace the journal, so an fd
        // opened first could point at the old inode.
        let lock = lock_file(path, FlockOperation::NonBlockingLockExclusive)?;
        let file = match OpenOptions::new().write(true).open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        if lock.is_none() {
            match flock(&file, FlockOperation::NonBlockingLockExclusive) {
                Err(Errno::WOULDBLOCK) => {
                    return Err(io::Error::from(io::ErrorKind::WouldBlock));
                }
                result => result?,
            }
        }
        file.set_len(0)?;
        fsync(&file)?;
        Ok(())
    }

    /// Opens the journal and takes its non-blocking lock, returning `None` when
    /// the journal does not exist. The returned value holds the lock for as long
    /// as it lives.
    fn lock_journal(path: &Path) -> io::Result<Option<LockedJournal>> {
        let lock = lock_file(path, FlockOperation::NonBlockingLockExclusive)?;
        let file = match OpenOptions::new().read(true).open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        // Nothing may append between the read and the rename that replaces the
        // file.
        if lock.is_none() {
            match flock(&file, FlockOperation::NonBlockingLockExclusive) {
                Err(Errno::WOULDBLOCK) => {
                    return Err(io::Error::from(io::ErrorKind::WouldBlock));
                }
                result => result?,
            }
        }
        Ok(Some(LockedJournal { file, _lock: lock }))
    }

    /// Writes a temporary journal and renames it over `path`, so a crash leaves
    /// the old one intact. `write` returns whether to commit; `false` removes
    /// the temporary file. The original file's mode is kept.
    fn rewrite_temp(
        path: &Path,
        write: impl FnOnce(&mut File) -> io::Result<bool>,
    ) -> io::Result<bool> {
        let temp = path.with_extension("jsonl.tmp");
        // Removing a stale temp first also unlinks a planted symlink; creating
        // with `create_new` then never follows one.
        let _ = std::fs::remove_file(&temp);
        let mut out = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&temp)?;
        let result = match write(&mut out) {
            Ok(false) => Ok(false),
            Ok(true) => {
                out.flush()?;
                let mode = std::fs::metadata(path)?.permissions();
                out.set_permissions(mode)?;
                fsync(&out)?;
                Ok(true)
            }
            Err(error) => Err(error),
        };
        drop(out);
        match result {
            Ok(true) => {
                if let Err(error) = std::fs::rename(&temp, path) {
                    let _ = std::fs::remove_file(&temp);
                    return Err(error);
                }
                Ok(true)
            }
            Ok(false) => {
                let _ = std::fs::remove_file(&temp);
                Ok(false)
            }
            Err(error) => {
                let _ = std::fs::remove_file(&temp);
                Err(error)
            }
        }
    }

    /// The completed renames of one run, in the order they happened.
    ///
    /// A run's `begin` precedes its `rename` records in append order, so the file
    /// is read backward and the scan stops at that `begin`: a recent run costs one
    /// tail read, not the whole journal. `rename` lines for other runs are skipped
    /// after parsing only their metadata, so no unrelated path is ever decoded.
    pub fn renames(path: &Path, run: RunId) -> io::Result<Vec<(PathBuf, PathBuf)>> {
        Ok(Journal::renames_with_id(path, run)?
            .into_iter()
            .map(|record| (record.from, record.to))
            .collect())
    }

    /// The same as [`Journal::renames`], with the file identity each record
    /// stored, so undo can refuse a destination that was replaced.
    pub fn renames_with_id(path: &Path, run: RunId) -> io::Result<Vec<RecordedRename>> {
        let mut file = match File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };

        let mut renames: Vec<RecordedRename> = Vec::new();
        for_each_line_backward(&mut file, |line| Journal::tail_line(line, run, &mut renames))?;
        renames.reverse();
        Ok(renames)
    }

    /// Handles one line read backward. Returns `true` once `run`'s `begin` is
    /// reached, which means every earlier line belongs to another run.
    fn tail_line(line: &[u8], run: RunId, out: &mut Vec<RecordedRename>) -> io::Result<bool> {
        let text = std::str::from_utf8(line).map_err(invalid_line)?;
        let meta = parse_meta(text)?;
        if meta.run != run {
            return Ok(false);
        }
        match meta.record {
            RecordKind::Begin => return Ok(true),
            RecordKind::Rename => {
                if let JournalRecord::Rename { from, to, id, .. } =
                    serde_json::from_str::<JournalRecord>(text).map_err(invalid_line)?
                {
                    out.push(RecordedRename { from, to, id });
                }
            }
            RecordKind::End => {}
        }
        Ok(false)
    }
}

/// Truncates a journal that does not end in a newline back to its last complete
/// line, so a short write or a crash cannot leave a fragment that the next
/// append turns into a corrupt middle line.
fn truncate_partial_tail(file: &mut File) -> io::Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Ok(());
    }
    let len = metadata.len();
    if len == 0 {
        return Ok(());
    }

    let mut last = [0u8; 1];
    file.seek(SeekFrom::Start(len - 1))?;
    file.read_exact(&mut last)?;
    if last[0] == b'\n' {
        return Ok(());
    }

    let mut buffer = vec![0u8; TAIL_CHUNK];
    let mut end = len;
    while end > 0 {
        let start = end.saturating_sub(TAIL_CHUNK as u64);
        let size = (end - start) as usize;
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(&mut buffer[..size])?;
        if let Some(at) = buffer[..size].iter().rposition(|&byte| byte == b'\n') {
            file.set_len(start + at as u64 + 1)?;
            return Ok(());
        }
        end = start;
    }
    file.set_len(0)?;
    Ok(())
}

/// Calls `on_line` for each line that ends in a newline, from the start. A
/// truncated final line is skipped. Returns early when `on_line` returns `true`.
fn for_each_complete_line(
    reader: &mut impl BufRead,
    mut on_line: impl FnMut(&[u8]) -> io::Result<bool>,
) -> io::Result<()> {
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            return Ok(());
        }
        if line.last() != Some(&b'\n') {
            return Ok(());
        }
        if on_line(&line[..line.len() - 1])? {
            return Ok(());
        }
    }
}

/// Calls `on_line` for each complete line, last to first. A truncated final
/// line is skipped. Returns early when `on_line` returns `true`.
fn for_each_line_backward(
    file: &mut File,
    mut on_line: impl FnMut(&[u8]) -> io::Result<bool>,
) -> io::Result<()> {
    let end = file.metadata()?.len();
    let trailing_partial = trailing_partial(file, end)?;

    // Bytes of the leading, not-yet-complete line carried over from the later
    // chunk in the previous step.
    let mut pending: Vec<u8> = Vec::new();
    let mut first_line = true;
    let mut pos = end;
    'outer: loop {
        let chunk_start = pos.saturating_sub(TAIL_CHUNK as u64);
        let mut buffer = vec![0u8; (pos - chunk_start) as usize];
        file.seek(SeekFrom::Start(chunk_start))?;
        file.read_exact(&mut buffer)?;
        buffer.extend_from_slice(&pending);

        let mut line_end = buffer.len();
        while line_end > 0 {
            let newline = buffer[..line_end].iter().rposition(|&byte| byte == b'\n');
            let line_start = newline.map_or(0, |at| at + 1);
            if newline.is_none() && chunk_start > 0 {
                // The line starts before this chunk: carry the fragment into
                // the next, earlier read.
                pending = buffer[line_start..line_end].to_vec();
                break;
            }
            let line = &buffer[line_start..line_end];
            if !line.is_empty() {
                let skip = first_line && trailing_partial;
                first_line = false;
                if !skip && on_line(line)? {
                    break 'outer;
                }
            }
            line_end = newline.unwrap_or(0);
        }

        if chunk_start == 0 {
            break;
        }
        pos = chunk_start;
    }
    Ok(())
}

/// Whether the file ends in the half-line a crash leaves. Seeks the reader to
/// the last byte; the caller seeks where it needs next.
fn trailing_partial(file: &mut File, end: u64) -> io::Result<bool> {
    if end == 0 {
        return Ok(false);
    }
    file.seek(SeekFrom::Start(end - 1))?;
    let mut last = [0u8; 1];
    file.read_exact(&mut last)?;
    Ok(last[0] != b'\n')
}

/// A line that is present but not a record.
fn invalid_line(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("journal: {error}"))
}

/// Parses a record's metadata, which carries no path.
fn parse_meta(line: &str) -> io::Result<RecordMeta> {
    serde_json::from_str(line).map_err(invalid_line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    fn temp_journal() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("state").join("journal.jsonl");
        (temp, path)
    }

    fn rename(run: RunId, from: PathBuf, to: PathBuf) -> JournalRecord {
        JournalRecord::Rename { run, from, to, id: None }
    }

    fn begin(journal: &mut Journal, run: RunId, at: u64) {
        journal
            .append(&JournalRecord::Begin { run, at, planned: 1, overwrite: false, undo_of: None })
            .expect("append");
    }

    fn end(journal: &mut Journal, run: RunId) {
        journal
            .append(&JournalRecord::End { run, at: 0, outcome: RunOutcome::Completed })
            .expect("append");
    }

    #[test]
    fn records_round_trip_through_raw_bytes() {
        let (_temp, path) = temp_journal();
        let non_utf8 = PathBuf::from(OsString::from_vec(b"/tmp/\xff".to_vec()));

        let mut journal = Journal::open(path.clone()).expect("open");
        journal.append(&rename(1, non_utf8.clone(), PathBuf::from("/tmp/b"))).expect("append");
        journal
            .append(&JournalRecord::Begin {
                run: 1,
                at: 10,
                planned: 1,
                overwrite: false,
                undo_of: None,
            })
            .expect("append");
        drop(journal);

        let records = Journal::read(&path).expect("read");
        // A name that is not UTF-8 survives, which a JSON string could not do.
        assert_eq!(records.len(), 2);
        assert_eq!(records[0], rename(1, non_utf8, PathBuf::from("/tmp/b")));
    }

    #[test]
    fn a_truncated_final_line_ends_the_read_instead_of_failing() {
        let (_temp, path) = temp_journal();
        let mut journal = Journal::open(path.clone()).expect("open");
        journal
            .append(&rename(1, PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")))
            .expect("append");
        drop(journal);

        let mut file = OpenOptions::new().append(true).open(&path).expect("reopen");
        file.write_all(b"{\"record\":\"ren").expect("append half a line");
        drop(file);

        assert_eq!(Journal::read(&path).expect("read").len(), 1);
    }

    #[test]
    fn a_partial_line_is_dropped_before_the_next_append() {
        let (_temp, path) = temp_journal();
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        let mut begin_line = serde_json::to_string(&JournalRecord::Begin {
            run: 1,
            at: 0,
            planned: 1,
            overwrite: false,
            undo_of: None,
        })
        .expect("json");
        begin_line.push('\n');
        let mut bytes = begin_line.into_bytes();
        bytes.extend_from_slice(b"{\"record\":\"rename\",\"run\":1,\"from\":\"a");
        std::fs::write(&path, bytes).expect("write");

        let mut journal = Journal::open(path.clone()).expect("open");
        begin(&mut journal, 2, 0);
        drop(journal);

        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.ends_with('\n'), "the repaired journal ends with a newline");
        assert!(!text.contains("\"record\":\"rename\""), "the partial line is gone");
        let sessions = Journal::sessions(&path).expect("sessions");
        assert_eq!(sessions.iter().map(|session| session.run).collect::<Vec<_>>(), vec![1, 2]);
        assert_eq!(Journal::next_run_id(&path).expect("next"), 3);
    }

    #[test]
    fn a_complete_record_without_a_newline_is_not_read() {
        let (_temp, path) = temp_journal();
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        let begin = serde_json::to_string(&JournalRecord::Begin {
            run: 1,
            at: 0,
            planned: 1,
            overwrite: false,
            undo_of: None,
        })
        .expect("json");
        let rename_line =
            serde_json::to_string(&rename(1, PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")))
                .expect("json");
        std::fs::write(&path, format!("{begin}\n{rename_line}")).expect("write");

        // No trailing newline means the record is incomplete; every reader
        // skips it rather than trusting a partly written line.
        assert_eq!(Journal::read(&path).expect("read").len(), 1);
        let sessions = Journal::sessions(&path).expect("sessions");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].renames, 0);
        assert!(Journal::renames(&path, 1).expect("renames").is_empty());
        assert_eq!(Journal::next_run_id(&path).expect("next"), 2);
    }

    #[test]
    fn a_corrupt_line_in_the_middle_is_an_error() {
        let (_temp, path) = temp_journal();
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(&path, b"nonsense\n").expect("write");

        assert!(Journal::read(&path).is_err());
    }

    #[test]
    fn a_begin_without_an_end_is_an_interrupted_session() {
        let (_temp, path) = temp_journal();
        let mut journal = Journal::open(path.clone()).expect("open");
        journal
            .append(&JournalRecord::Begin {
                run: 1,
                at: 10,
                planned: 3,
                overwrite: false,
                undo_of: None,
            })
            .expect("append");
        journal
            .append(&rename(1, PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")))
            .expect("append");
        journal
            .append(&JournalRecord::Begin {
                run: 2,
                at: 20,
                planned: 1,
                overwrite: true,
                undo_of: None,
            })
            .expect("append");
        journal
            .append(&JournalRecord::End { run: 2, at: 21, outcome: RunOutcome::Completed })
            .expect("append");

        let sessions = Journal::sessions(&path).expect("sessions");
        assert_eq!(sessions.len(), 2);
        assert!(sessions[0].interrupted);
        assert_eq!(sessions[0].renames, 1);
        assert!(!sessions[1].interrupted);
        assert!(sessions[1].overwrite);
        assert_eq!(Journal::next_run_id(&path).expect("next"), 3);
        assert_eq!(Journal::renames(&path, 1).expect("renames").len(), 1);
    }

    #[test]
    fn a_missing_journal_reads_as_empty() {
        let (_temp, path) = temp_journal();
        assert!(Journal::read(&path).expect("read").is_empty());
        assert_eq!(Journal::next_run_id(&path).expect("next"), 1);
    }

    #[test]
    fn the_journal_holds_an_exclusive_lock() {
        let (_temp, path) = temp_journal();
        let held = Journal::open(path.clone()).expect("open");
        let other = File::open(path.with_extension("lock")).expect("second handle");
        assert!(
            flock(&other, FlockOperation::NonBlockingLockExclusive).is_err(),
            "a held journal must refuse a second lock"
        );
        drop(held);
        assert!(
            flock(&other, FlockOperation::NonBlockingLockExclusive).is_ok(),
            "the lock is released with the Journal"
        );
    }

    #[test]
    fn the_instance_lock_refuses_a_second_holder() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("state").join("instance.lock");
        let held = lock_instance(&path).expect("lock").expect("the first holder");
        assert!(
            lock_instance(&path).expect("second lock").is_none(),
            "a second instance is refused"
        );
        drop(held);
        assert!(lock_instance(&path).expect("third lock").is_some(), "the lock is released");
    }

    #[test]
    fn renames_finds_a_run_older_than_one_tail_chunk() {
        let (_temp, path) = temp_journal();
        let mut journal = Journal::open(path.clone()).expect("open");
        journal
            .append(&JournalRecord::Begin {
                run: 1,
                at: 0,
                planned: 2,
                overwrite: false,
                undo_of: None,
            })
            .expect("append");
        journal
            .append(&rename(1, PathBuf::from("/tmp/a1"), PathBuf::from("/tmp/b1")))
            .expect("append");
        journal
            .append(&rename(1, PathBuf::from("/tmp/a2"), PathBuf::from("/tmp/b2")))
            .expect("append");
        journal
            .append(&JournalRecord::End { run: 1, at: 0, outcome: RunOutcome::Completed })
            .expect("append");
        // More than one `TAIL_CHUNK` of later runs, so run 1 is out of the tail.
        let long = PathBuf::from(format!("/tmp/{}", "x".repeat(200)));
        for run in 2..=2000 {
            journal.append(&rename(run, long.clone(), long.clone())).expect("append");
        }
        drop(journal);

        assert_eq!(
            Journal::renames(&path, 1).expect("renames"),
            vec![
                (PathBuf::from("/tmp/a1"), PathBuf::from("/tmp/b1")),
                (PathBuf::from("/tmp/a2"), PathBuf::from("/tmp/b2")),
            ]
        );
        assert_eq!(Journal::renames(&path, 2000).expect("renames").len(), 1);
        // Only run 1 wrote a `begin`; the later records belong to runs the test
        // never started.
        assert_eq!(Journal::next_run_id(&path).expect("next"), 2);
    }

    #[test]
    fn renames_ignores_a_truncated_final_line() {
        let (_temp, path) = temp_journal();
        let mut journal = Journal::open(path.clone()).expect("open");
        journal
            .append(&JournalRecord::Begin {
                run: 1,
                at: 0,
                planned: 1,
                overwrite: false,
                undo_of: None,
            })
            .expect("append");
        journal
            .append(&rename(1, PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")))
            .expect("append");
        drop(journal);

        let mut file = OpenOptions::new().append(true).open(&path).expect("reopen");
        file.write_all(b"{\"record\":\"rename\",\"run\":1,\"from\":\"a").expect("half a line");
        drop(file);

        assert_eq!(Journal::renames(&path, 1).expect("renames").len(), 1);
    }

    #[test]
    fn prune_drops_old_runs_and_keeps_the_ids_rising() {
        let (_temp, path) = temp_journal();
        let mut journal = Journal::open(path.clone()).expect("open");
        begin(&mut journal, 1, now() - 61 * SECONDS_PER_DAY);
        journal
            .append(&rename(1, PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")))
            .expect("append");
        end(&mut journal, 1);
        begin(&mut journal, 2, now() - SECONDS_PER_DAY);
        journal
            .append(&rename(2, PathBuf::from("/tmp/c"), PathBuf::from("/tmp/d")))
            .expect("append");
        end(&mut journal, 2);
        drop(journal);

        assert_eq!(Journal::prune(&path).expect("prune"), 1);

        let sessions = Journal::sessions(&path).expect("sessions");
        assert_eq!(sessions.iter().map(|session| session.run).collect::<Vec<_>>(), vec![2]);
        // The dropped run was the oldest, so the kept high-water mark is
        // unchanged and the next id cannot repeat one.
        assert_eq!(Journal::next_run_id(&path).expect("next"), 3);
        assert_eq!(Journal::renames(&path, 2).expect("renames").len(), 1);
    }

    #[test]
    fn prune_keeps_the_journals_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let (_temp, path) = temp_journal();
        let mut journal = Journal::open(path.clone()).expect("open");
        begin(&mut journal, 1, now() - 61 * SECONDS_PER_DAY);
        end(&mut journal, 1);
        begin(&mut journal, 2, now());
        end(&mut journal, 2);
        drop(journal);

        let mode =
            |path: &Path| std::fs::metadata(path).expect("metadata").permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600, "a new journal is private");
        // A rewrite must keep whatever the user set, not recreate the default.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).expect("chmod");
        assert_eq!(Journal::prune(&path).expect("prune"), 1);
        assert_eq!(mode(&path), 0o640);
    }

    #[test]
    fn prune_keeps_the_newest_run_even_when_it_is_ancient() {
        let (_temp, path) = temp_journal();
        let mut journal = Journal::open(path.clone()).expect("open");
        let old = now() - 90 * SECONDS_PER_DAY;
        begin(&mut journal, 1, old);
        end(&mut journal, 1);
        begin(&mut journal, 2, old);
        journal
            .append(&rename(2, PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")))
            .expect("append");
        end(&mut journal, 2);
        drop(journal);

        assert_eq!(Journal::prune(&path).expect("prune"), 1);

        let sessions = Journal::sessions(&path).expect("sessions");
        assert_eq!(sessions.iter().map(|session| session.run).collect::<Vec<_>>(), vec![2]);
        assert_eq!(Journal::next_run_id(&path).expect("next"), 3);
        assert_eq!(Journal::renames(&path, 2).expect("renames").len(), 1);
    }

    #[test]
    fn prune_leaves_a_journal_within_retention_untouched() {
        let (_temp, path) = temp_journal();
        let mut journal = Journal::open(path.clone()).expect("open");
        begin(&mut journal, 1, now() - SECONDS_PER_DAY);
        end(&mut journal, 1);
        begin(&mut journal, 2, now());
        end(&mut journal, 2);
        drop(journal);
        let before = std::fs::read(&path).expect("read");

        assert_eq!(Journal::prune(&path).expect("prune"), 0);
        assert_eq!(std::fs::read(&path).expect("read"), before);
    }

    #[test]
    fn prune_refuses_a_corrupt_line_and_leaves_the_file_alone() {
        let (_temp, path) = temp_journal();
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        let corrupt = b"nonsense\n";
        std::fs::write(&path, corrupt).expect("write");

        assert!(Journal::prune(&path).is_err());
        assert_eq!(std::fs::read(&path).expect("read"), corrupt.as_slice());
    }

    #[test]
    fn prune_drops_the_truncated_final_line() {
        let (_temp, path) = temp_journal();
        let mut journal = Journal::open(path.clone()).expect("open");
        begin(&mut journal, 1, now() - 61 * SECONDS_PER_DAY);
        end(&mut journal, 1);
        begin(&mut journal, 2, now());
        end(&mut journal, 2);
        drop(journal);
        let mut file = OpenOptions::new().append(true).open(&path).expect("reopen");
        file.write_all(b"{\"record\":\"ren").expect("append half a line");
        drop(file);

        assert_eq!(Journal::prune(&path).expect("prune"), 1);
        assert!(std::fs::read(&path).expect("read").ends_with(b"\n"));
        assert_eq!(Journal::sessions(&path).expect("sessions").len(), 1);
    }

    #[test]
    fn removing_a_run_keeps_the_others_and_the_ids_rising() {
        let (_temp, path) = temp_journal();
        let mut journal = Journal::open(path.clone()).expect("open");
        for run in 1..=3 {
            begin(&mut journal, run, 0);
            journal
                .append(&rename(run, PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")))
                .expect("append");
            end(&mut journal, run);
        }
        drop(journal);

        assert_eq!(Journal::remove_runs(&path, &[2]).expect("remove"), 1);

        let sessions = Journal::sessions(&path).expect("sessions");
        assert_eq!(sessions.iter().map(|session| session.run).collect::<Vec<_>>(), vec![1, 3]);
        assert_eq!(Journal::next_run_id(&path).expect("next"), 4);
        assert_eq!(Journal::renames(&path, 3).expect("renames").len(), 1);
    }

    #[test]
    fn removing_the_newest_run_lets_its_id_come_back() {
        let (_temp, path) = temp_journal();
        let mut journal = Journal::open(path.clone()).expect("open");
        begin(&mut journal, 1, 0);
        end(&mut journal, 1);
        begin(&mut journal, 2, 0);
        end(&mut journal, 2);
        drop(journal);

        assert_eq!(Journal::remove_runs(&path, &[2]).expect("remove"), 1);

        // Only the newest can come back: no surviving record can name it.
        assert_eq!(Journal::next_run_id(&path).expect("next"), 2);
        assert_eq!(Journal::sessions(&path).expect("sessions").len(), 1);
    }

    #[test]
    fn removing_refuses_a_corrupt_line_and_leaves_the_file_alone() {
        let (_temp, path) = temp_journal();
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        let corrupt = b"nonsense\n";
        std::fs::write(&path, corrupt).expect("write");

        assert!(Journal::remove_runs(&path, &[1]).is_err());
        assert_eq!(std::fs::read(&path).expect("read"), corrupt.as_slice());
    }

    #[test]
    fn clearing_empties_the_journal_and_restarts_the_ids() {
        let (_temp, path) = temp_journal();
        let mut journal = Journal::open(path.clone()).expect("open");
        begin(&mut journal, 1, 0);
        end(&mut journal, 1);
        drop(journal);

        Journal::clear(&path).expect("clear");

        assert!(Journal::sessions(&path).expect("sessions").is_empty());
        assert_eq!(Journal::next_run_id(&path).expect("next"), 1);
        assert!(path.exists(), "the file stays, with its permissions");
    }
}
