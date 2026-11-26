//! Two-phase execution, syscalls and journaling.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use rustix::fs::{CWD, RenameFlags, renameat_with};
use rustix::io::Errno;

use crate::journal::{
    BATCHED_SYNC_EVERY, FileId, Journal, JournalRecord, RecordedRename, RunId, RunOutcome, now,
};
use crate::plan::{Plan, Status};

/// Generated names to try before giving up on staging a cycle.
const TEMP_ATTEMPTS: u32 = 32;
const TEMP_PREFIX: &str = ".renameant-tmp";

#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    #[error("the destination already exists")]
    TargetExists,
    #[error("permission denied")]
    PermissionDenied,
    #[error("the filesystem is read-only")]
    ReadOnlyFs,
    #[error("the name is too long")]
    NameTooLong,
    #[error("renameAnt needs absolute paths, so undo does not depend on the working directory")]
    RelativePath,
    #[error("the file is not the one the run renamed")]
    FileChanged,
    #[error(
        "this filesystem rejected RENAME_NOREPLACE, and renameAnt never falls back to a rename that could overwrite"
    )]
    NotSupported,
    #[error("could not be put back; the file is at {}", .0.display())]
    Staged(PathBuf),
    #[error(transparent)]
    Io(#[from] io::Error),
}

impl From<Errno> for ApplyError {
    fn from(errno: Errno) -> Self {
        if errno == Errno::EXIST {
            return ApplyError::TargetExists;
        }
        if errno == Errno::ACCESS || errno == Errno::PERM {
            return ApplyError::PermissionDenied;
        }
        if errno == Errno::ROFS {
            return ApplyError::ReadOnlyFs;
        }
        if errno == Errno::NAMETOOLONG {
            return ApplyError::NameTooLong;
        }
        // No racy check-then-rename fallback.
        if errno == Errno::INVAL
            || errno == Errno::NOSYS
            || errno == Errno::NOTSUP
            || errno == Errno::OPNOTSUPP
        {
            return ApplyError::NotSupported;
        }
        ApplyError::Io(errno.into())
    }
}

#[derive(Debug)]
pub enum Outcome {
    Renamed,
    /// Never attempted: a `Conflict` or `Invalid` row, a row the run
    /// gave up before reaching, or one that became a no-op.
    Skipped,
    Failed(ApplyError),
}

impl fmt::Display for Outcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Outcome::Renamed => formatter.write_str("renamed"),
            Outcome::Skipped => formatter.write_str("skipped"),
            Outcome::Failed(error) => write!(formatter, "failed: {error}"),
        }
    }
}

#[derive(Debug)]
pub struct ApplyRow {
    pub src: PathBuf,
    pub dst: PathBuf,
    pub outcome: Outcome,
}

#[derive(Debug)]
pub struct ApplyReport {
    pub run: RunId,
    /// When the run's `end` record was written.
    pub at: u64,
    /// Rename records the run wrote, staged halves included. This is what the
    /// journal's session count will show.
    pub records: usize,
    pub rows: Vec<ApplyRow>,
    /// The run stopped early on `NotSupported`. Completed renames are
    /// kept; there is no rollback.
    pub aborted: bool,
}

impl ApplyReport {
    pub fn renamed(&self) -> usize {
        self.rows.iter().filter(|row| matches!(row.outcome, Outcome::Renamed)).count()
    }

    pub fn failed(&self) -> usize {
        self.rows.iter().filter(|row| matches!(row.outcome, Outcome::Failed(_))).count()
    }

    pub fn skipped(&self) -> usize {
        self.rows.iter().filter(|row| matches!(row.outcome, Outcome::Skipped)).count()
    }

    pub fn outcome(&self) -> RunOutcome {
        if self.renamed() == 0 {
            RunOutcome::Aborted
        } else if self.failed() == 0 {
            RunOutcome::Completed
        } else {
            RunOutcome::Partial
        }
    }
}

#[derive(Clone, Debug)]
pub struct ApplyOptions {
    pub journal: PathBuf,
    /// Fast mode: batch the journal's `fsync` instead of writing one per
    /// rename. The renames themselves are unaffected; what a crash can lose is
    /// the last batch of undo records. A staged rename — a cycle or a
    /// case-only rename — is synced on its own, because it is the one step a
    /// crash could otherwise leave no record of.
    pub batched_sync: bool,
}

impl ApplyOptions {
    pub fn new(journal: PathBuf) -> Self {
        ApplyOptions { journal, batched_sync: false }
    }

    /// Records between two journal `fsync`s.
    fn sync_every(&self) -> usize {
        match self.batched_sync {
            true => BATCHED_SYNC_EVERY,
            false => 1,
        }
    }
}

/// Executes `plan` in apply order.
///
/// Every write goes through `renameat2(RENAME_NOREPLACE)`, each completed
/// rename is `fsync`ed before the next begins (unless
/// [`ApplyOptions::batched_sync`] batches those, see
/// [`crate::journal::BATCHED_SYNC_EVERY`]), and an `io::Error` return means the
/// journal failed and the run aborted. Any other failure is per row: partial
/// application is a normal outcome and the completed renames stay undoable.
pub fn apply(plan: &Plan, options: &ApplyOptions) -> io::Result<ApplyReport> {
    let mut journal = Journal::open_syncing(options.journal.clone(), options.sync_every())?;
    let run = Journal::next_run_id(&options.journal)?;
    journal.append(&JournalRecord::Begin {
        run,
        at: now(),
        planned: plan.actionable(),
        overwrite: false,
        undo_of: None,
    })?;
    // The run's own mark is durable before the first rename even when the
    // rename records themselves are batched.
    journal.sync()?;

    let mut rows: Vec<ApplyRow> = plan
        .rows
        .iter()
        .map(|row| ApplyRow {
            src: row.src.clone(),
            dst: row.dst.clone(),
            outcome: Outcome::Skipped,
        })
        .collect();

    let mut aborted = false;
    let mut at = 0;
    while at < plan.rows.len() && !aborted {
        if plan.rows[at].status != Status::Ok {
            at += 1;
            continue;
        }

        if !plan.rows[at].via_temp {
            let outcome = rename(&plan.rows[at].src, &plan.rows[at].dst, &mut journal, run)?;
            if let Err(ApplyError::NotSupported) = &outcome {
                aborted = true;
            }
            rows[at].outcome = as_outcome(outcome);
            at += 1;
            continue;
        }

        // A run of staged rows: a cycle, or a case-only rename. Every source is
        // moved aside first, so every destination is free and the staged names
        // can land in reverse — which also keeps two cycles that happen to sit
        // next to each other from being mistaken for one.
        let end = at
            + plan.rows[at..]
                .iter()
                .take_while(|row| row.status == Status::Ok && row.via_temp)
                .count();
        let mut staged: Vec<(usize, PathBuf)> = Vec::with_capacity(end - at);
        let mut stage_failed = false;
        for (offset, row) in plan.rows[at..end].iter().enumerate() {
            let index = at + offset;
            let outcome = match stage(&row.src, run, index, &mut journal) {
                Ok(outcome) => outcome,
                Err(error) => {
                    // The journal itself failed. Put every peer staged so far
                    // back with raw renames: a file must not stay hidden under a
                    // generated name because no record could be written. The
                    // row that just failed has already been put back by `stage`.
                    let stranded = restore_staged(plan, &staged);
                    return Err(match stranded {
                        0 => error,
                        count => io::Error::new(
                            error.kind(),
                            format!(
                                "{error} ({count} staged names could not be put back; undo the interrupted run)"
                            ),
                        ),
                    });
                }
            };
            match outcome {
                Err(error) => {
                    if matches!(error, ApplyError::NotSupported) {
                        aborted = true;
                    }
                    rows[index].outcome = Outcome::Failed(error);
                    stage_failed = true;
                    break;
                }
                Ok(temp) => staged.push((index, temp)),
            }
        }
        if stage_failed {
            // One member failing leaves the others with no destination: landing
            // them would leave files under generated names. Put every staged
            // name back and skip the run.
            for (index, temp) in staged.iter().rev() {
                let outcome = rename(temp, &plan.rows[*index].src, &mut journal, run)?;
                rows[*index].outcome = match outcome {
                    Ok(()) => Outcome::Skipped,
                    Err(_) => Outcome::Failed(ApplyError::Staged(temp.clone())),
                };
            }
            at = end;
            continue;
        }
        for (index, temp) in staged.iter().rev() {
            let outcome = rename(temp, &plan.rows[*index].dst, &mut journal, run)?;
            rows[*index].outcome = match outcome {
                Ok(()) => Outcome::Renamed,
                Err(error) => {
                    if matches!(error, ApplyError::NotSupported) {
                        aborted = true;
                    }
                    // A landing can fail after the source was moved aside. Put
                    // the name back when it is still free; otherwise the report
                    // must name the temp path, or the file looks lost.
                    match rename(temp, &plan.rows[*index].src, &mut journal, run)? {
                        Ok(()) => Outcome::Skipped,
                        Err(_) => Outcome::Failed(ApplyError::Staged(temp.clone())),
                    }
                }
            };
        }
        at = end;
    }

    let report = ApplyReport { run, at: now(), records: journal.renames_written(), rows, aborted };
    journal.append(&JournalRecord::End { run, at: report.at, outcome: report.outcome() })?;
    journal.sync()?;
    Ok(report)
}

/// Reverse-applies a run's completed renames, newest first, appending the
/// inverse records to the same journal so redo is just an undo of the undo.
///
/// A row a later undo run already reversed is skipped, so a second undo writes
/// nothing. Undo is not transactional: a row whose file is no longer where the
/// journal expects it fails and the rest continue. It never overwrites.
pub fn undo(options: &ApplyOptions, target: RunId) -> io::Result<ApplyReport> {
    // Lock before reading: the lock is held for the whole undo, so a concurrent
    // apply cannot claim this undo's run id.
    let journal = Journal::open_syncing(options.journal.clone(), options.sync_every())?;
    let mut completed = Journal::renames_with_id(&options.journal, target)?;
    completed.reverse();
    // A row a later undo run already reversed is not reversed again: undo is
    // idempotent even when the panel misses the mark.
    let reversed = reversed_by_later_runs(&options.journal, target)?;
    completed.retain(|record| !reversed.contains(&(record.from.clone(), record.to.clone())));
    reverse(options, journal, target, completed)
}

/// The renames of `target` that a later run with `undo_of` recorded the
/// inverse of, as `(from, to)` of the target's own records.
fn reversed_by_later_runs(path: &Path, target: RunId) -> io::Result<HashSet<(PathBuf, PathBuf)>> {
    let mut reversed = HashSet::new();
    for session in Journal::sessions(path)? {
        if session.undo_of != Some(target) {
            continue;
        }
        for (from, to) in Journal::renames(path, session.run)? {
            reversed.insert((to, from));
        }
    }
    Ok(reversed)
}

/// Reverses one recorded rename of `target`, leaving the rest of the run alone.
///
/// The pair must be one the run recorded, so a stale row can never move a file
/// the run never renamed, and it must not be half of a staged pair, which is
/// only reversible with the whole run. The undo is a run of its own and can be
/// undone.
pub fn undo_one(
    options: &ApplyOptions,
    target: RunId,
    from: &Path,
    to: &Path,
) -> io::Result<ApplyReport> {
    if is_staged(target, from) || is_staged(target, to) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a staged rename must be reversed with its whole run",
        ));
    }
    let journal = Journal::open_syncing(options.journal.clone(), options.sync_every())?;
    let recorded = Journal::renames_with_id(&options.journal, target)?
        .into_iter()
        .find(|record| record.from.as_path() == from && record.to.as_path() == to);
    let Some(record) = recorded else {
        let message = format!("run {target} did not rename {} to {}", from.display(), to.display());
        return Err(io::Error::new(io::ErrorKind::InvalidInput, message));
    };
    reverse(options, journal, target, vec![record])
}

/// Appends a run that reverses `completed` in order. The caller holds the lock
/// on the journal in `journal`.
fn reverse(
    options: &ApplyOptions,
    mut journal: Journal,
    target: RunId,
    completed: Vec<RecordedRename>,
) -> io::Result<ApplyReport> {
    if completed.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("run {target} has no renames left to reverse"),
        ));
    }
    let run = Journal::next_run_id(&options.journal)?;
    journal.append(&JournalRecord::Begin {
        run,
        at: now(),
        planned: completed.len(),
        overwrite: false,
        undo_of: Some(target),
    })?;
    journal.sync()?;

    let mut rows = Vec::with_capacity(completed.len());
    for record in &completed {
        let outcome = match record.id {
            Some(id) if FileId::of(&record.to) != Some(id) => Err(ApplyError::FileChanged),
            _ => rename(&record.to, &record.from, &mut journal, run)?,
        };
        rows.push(ApplyRow {
            src: record.to.clone(),
            dst: record.from.clone(),
            outcome: as_outcome(outcome),
        });
    }

    let report =
        ApplyReport { run, at: now(), records: journal.renames_written(), rows, aborted: false };
    journal.append(&JournalRecord::End { run, at: report.at, outcome: report.outcome() })?;
    journal.sync()?;
    Ok(report)
}

/// Whether `name` has the exact shape [`stage`] generates, for any run.
pub(crate) fn is_temp_name(name: &OsStr) -> bool {
    let Some(rest) = name.as_encoded_bytes().strip_prefix(TEMP_PREFIX.as_bytes()) else {
        return false;
    };
    let Some(rest) = rest.strip_prefix(b"-") else { return false };
    let mut parts = rest.split(|byte| *byte == b'-');
    matches!(
        (parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(_run), Some(at), Some(attempt), None) if digits(at) && digits(attempt)
    )
}

/// Whether `path` carries the name [`stage`] generated for `run`. Such a row is
/// one half of a rename that must be reversed as a whole, never alone.
pub fn is_staged(run: RunId, path: &Path) -> bool {
    let Some(name) = path.file_name() else { return false };
    let Some(rest) = name.as_encoded_bytes().strip_prefix(TEMP_PREFIX.as_bytes()) else {
        return false;
    };
    let Some(rest) = rest.strip_prefix(b"-") else { return false };
    let Some(rest) = rest.strip_prefix(run.to_string().as_bytes()) else { return false };
    let Some(rest) = rest.strip_prefix(b"-") else { return false };
    let mut parts = rest.split(|byte| *byte == b'-');
    matches!(
        (parts.next(), parts.next(), parts.next()),
        (Some(at), Some(attempt), None) if digits(at) && digits(attempt)
    )
}

fn digits(part: &[u8]) -> bool {
    !part.is_empty() && part.iter().all(u8::is_ascii_digit)
}

/// The result of one rename.
///
/// The outer error means the journal could not be written, which aborts the run.
/// The inner error is this row's own failure, which does not.
type RenameResult = io::Result<Result<(), ApplyError>>;

/// The result of staging one name aside: `Ok` names the generated path.
type StageResult = io::Result<Result<PathBuf, ApplyError>>;

fn rename(src: &Path, dst: &Path, journal: &mut Journal, run: RunId) -> RenameResult {
    // A journal entry is later replayed from another working directory.
    if !src.is_absolute() || !dst.is_absolute() {
        return Ok(Err(ApplyError::RelativePath));
    }

    let result = renameat_with(CWD, src, CWD, dst, RenameFlags::NOREPLACE);

    match result {
        Err(errno) => Ok(Err(ApplyError::from(errno))),
        Ok(()) => {
            let id = FileId::of(dst);
            if let Err(error) = journal.append(&JournalRecord::Rename {
                run,
                from: src.to_path_buf(),
                to: dst.to_path_buf(),
                id,
            }) {
                // No record, no rename: put the file back. `NOREPLACE` means a
                // name that came back in the meantime is left alone.
                let _ = renameat_with(CWD, dst, CWD, src, RenameFlags::NOREPLACE);
                return Err(error);
            }
            Ok(Ok(()))
        }
    }
}

/// Moves `src` aside under a generated name in its own directory.
///
/// Staging always uses `RENAME_NOREPLACE`: the generated name is renameAnt's
/// own and must not clobber anyone else's file.
///
/// The aside rename is the one step a crash cannot describe from the other
/// side — the file is at neither `src` nor its destination — so its record is
/// forced to disk before moving on, and a record that cannot be written puts
/// the name back. Every other rename leaves the file on a name the user chose.
fn stage(src: &Path, run: RunId, at: usize, journal: &mut Journal) -> StageResult {
    let parent = src.parent().unwrap_or_else(|| Path::new("."));
    for attempt in 0..TEMP_ATTEMPTS {
        let temp = parent.join(format!("{TEMP_PREFIX}-{run}-{at}-{attempt}"));
        let outcome = match rename(src, &temp, journal, run) {
            Ok(outcome) => outcome,
            Err(error) => return Err(unstage(&temp, src, error)),
        };
        if let Err(error) = journal.sync() {
            return Err(unstage(&temp, src, error));
        }
        match outcome {
            Ok(()) => return Ok(Ok(temp)),
            Err(ApplyError::TargetExists) => continue,
            Err(error) => return Ok(Err(error)),
        }
    }
    Ok(Err(ApplyError::TargetExists))
}

/// Puts a staged name back after its record could not be made durable, so the
/// file is never left under a generated name with nothing pointing at it. Best
/// effort, and never a clobber: the caller aborts on the journal error either
/// way, and a name that came back in the meantime is left as it is.
fn unstage(temp: &Path, src: &Path, error: io::Error) -> io::Error {
    let _ = renameat_with(CWD, temp, CWD, src, RenameFlags::NOREPLACE);
    error
}

/// Puts every peer staged before a journal failure back under its source name.
/// Raw renames only: the journal is the thing that failed, so no record can be
/// written. Returns how many could not be put back.
fn restore_staged(plan: &Plan, staged: &[(usize, PathBuf)]) -> usize {
    staged
        .iter()
        .rev()
        .filter(|(index, temp)| {
            renameat_with(
                CWD,
                temp.as_path(),
                CWD,
                plan.rows[*index].src.as_path(),
                RenameFlags::NOREPLACE,
            )
            .is_err()
        })
        .count()
}

fn as_outcome(result: Result<(), ApplyError>) -> Outcome {
    match result {
        Ok(()) => Outcome::Renamed,
        Err(error) => Outcome::Failed(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::plan;
    use crate::rule::{InsertRemoveOp, Rule, RuleSet, Target};
    use crate::scan::{scan_dir, scan_tree};
    use std::fs;

    struct Fixture {
        _temp: tempfile::TempDir,
        dir: PathBuf,
        options: ApplyOptions,
    }

    fn fixture(names: &[&str]) -> Fixture {
        let temp = tempfile::tempdir().expect("temp dir");
        let dir = temp.path().join("work");
        fs::create_dir(&dir).expect("work dir");
        for name in names {
            fs::write(dir.join(name), b"x").expect("write fixture");
        }
        let options = ApplyOptions::new(temp.path().join("state").join("journal.jsonl"));
        Fixture { _temp: temp, dir, options }
    }

    impl Fixture {
        fn plan(&self, rules: Vec<Rule>) -> Plan {
            let (entries, index) = scan_dir(&self.dir).expect("scan");
            plan(&entries, &index, &RuleSet { rules }).expect("plan")
        }

        fn plan_tree(&self, rules: Vec<Rule>) -> Plan {
            let (entries, index) = scan_tree(&self.dir).expect("scan");
            plan(&entries, &index, &RuleSet { rules }).expect("plan")
        }

        fn exists(&self, name: &str) -> bool {
            self.dir.join(name).exists()
        }
    }

    fn prefix(text: &str) -> Vec<Rule> {
        vec![Rule::InsertRemove {
            op: InsertRemoveOp::AddPrefix(text.as_bytes().to_vec()),
            target: Target::Whole,
        }]
    }

    /// Swaps the last two bytes of each name, so `ab` and `ba` form a cycle.
    fn swap() -> Vec<Rule> {
        vec![Rule::Regex {
            pattern: "^(.+)(.)$".into(),
            replacement: "${2}${1}".into(),
            target: Target::Whole,
            path: false,
        }]
    }

    #[test]
    fn apply_renames_and_journals_every_completed_row() {
        let fixture = fixture(&["a", "b"]);
        let report = apply(&fixture.plan(prefix("p-")), &fixture.options).expect("apply");

        assert_eq!(report.renamed(), 2);
        assert_eq!(report.failed(), 0);
        assert_eq!(report.outcome(), RunOutcome::Completed);
        assert!(fixture.exists("p-a"));
        assert!(fixture.exists("p-b"));
        assert!(!fixture.exists("a"));

        let sessions = Journal::sessions(&fixture.options.journal).expect("sessions");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].renames, 2);
        assert!(!sessions[0].interrupted);
    }

    #[test]
    fn a_batched_run_journals_the_same_renames_as_a_durable_one() {
        let renames = |batched_sync: bool| {
            let fixture = fixture(&["a", "b", "c"]);
            let options =
                ApplyOptions { batched_sync, ..ApplyOptions::new(fixture.options.journal.clone()) };
            let report = apply(&fixture.plan(prefix("p-")), &options).expect("apply");

            assert_eq!(report.renamed(), 3);
            assert!(fixture.exists("p-a"));
            let renames = Journal::renames(&options.journal, report.run).expect("renames");
            assert_eq!(renames.len(), 3, "the batch is written, not just held in memory");
            assert!(!Journal::sessions(&options.journal).expect("sessions")[0].interrupted);
            // Two temp dirs, so compare the names, not the roots.
            renames
                .into_iter()
                .map(|(from, to)| {
                    (from.file_name().unwrap().to_owned(), to.file_name().unwrap().to_owned())
                })
                .collect::<Vec<_>>()
        };

        // Fast mode batches the `fsync`; it must not change what the journal
        // says was renamed, only when it reaches the disk.
        assert_eq!(renames(false), renames(true));
    }

    #[test]
    fn nothing_is_written_before_apply_runs() {
        let fixture = fixture(&["a"]);
        let _plan = fixture.plan(prefix("p-"));

        assert!(fixture.exists("a"));
        assert!(!fixture.exists("p-a"));
        assert!(!fixture.options.journal.exists());
    }

    #[test]
    fn conflict_and_invalid_rows_are_never_applied() {
        let fixture = fixture(&["a.txt", "b.txt"]);
        // a.txt -> b.txt while b.txt keeps its name: a target-exists conflict.
        let rules = vec![Rule::Literal {
            find: b"a".to_vec(),
            replace: b"b".to_vec(),
            target: Target::Stem,
        }];

        let plan = fixture.plan(rules);
        assert_eq!(plan.actionable(), 0);
        let report = apply(&plan, &fixture.options).expect("apply");

        assert_eq!(report.renamed(), 0);
        assert_eq!(report.skipped(), 2);
        assert_eq!(report.outcome(), RunOutcome::Aborted, "nothing was renamed");
        assert!(fixture.exists("a.txt"));
        assert!(fixture.exists("b.txt"));
    }

    #[test]
    fn a_cycle_goes_through_a_temp_name_and_leaves_none_behind() {
        let fixture = fixture(&["ab", "ba"]);
        let rules = swap();

        let report = apply(&fixture.plan(rules), &fixture.options).expect("apply");

        assert_eq!(report.renamed(), 2);
        assert!(fixture.exists("ba"));
        assert!(fixture.exists("ab"));
        let leftovers: Vec<_> = fs::read_dir(&fixture.dir)
            .expect("read dir")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(TEMP_PREFIX))
            .collect();
        assert!(leftovers.is_empty(), "the staged name must be consumed");
    }

    #[test]
    fn a_staging_record_that_cannot_be_written_puts_the_name_back() {
        // `/dev/full` fails every write with ENOSPC, the one way a test can
        // make the journal fail. It is only written here, never read.
        let full = Path::new("/dev/full");
        if !full.exists() {
            eprintln!("/dev/full is absent, so the failing journal cannot be built");
            return;
        }
        let fixture = fixture(&["a"]);
        let mut journal =
            Journal::open_syncing(full.to_path_buf(), 1).expect("open /dev/full for writing");

        let result = stage(&fixture.dir.join("a"), 1, 0, &mut journal);

        assert!(result.is_err(), "a record that cannot be written aborts the run");
        assert!(fixture.exists("a"), "the name is back where it started");
        let leftovers: Vec<_> = fs::read_dir(&fixture.dir)
            .expect("read dir")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(TEMP_PREFIX))
            .collect();
        assert!(leftovers.is_empty(), "the file is not left under a generated name");
    }

    #[test]
    fn two_neighbouring_cycles_are_staged_and_landed_independently() {
        // Both cycles end up in one contiguous staged run, which must not be
        // mistaken for a single four-name cycle.
        let fixture = fixture(&["ab", "ba", "cd", "dc"]);
        let rules = swap();

        let report = apply(&fixture.plan(rules), &fixture.options).expect("apply");

        assert_eq!(report.renamed(), 4);
        assert_eq!(report.failed(), 0);
        for name in ["ab", "ba", "cd", "dc"] {
            assert!(fixture.exists(name), "{name} must be back");
        }
    }

    #[test]
    fn a_recursive_run_moves_a_directory_after_the_files_inside_it() {
        let fixture = fixture(&["top.txt"]);
        fs::create_dir(fixture.dir.join("d")).expect("nested dir");
        fs::write(fixture.dir.join("d").join("f.txt"), b"x").expect("nested file");

        let first = apply(&fixture.plan_tree(prefix("p-")), &fixture.options).expect("apply");

        assert_eq!(first.failed(), 0);
        assert!(fixture.exists("p-top.txt"));
        assert!(fixture.exists("p-d/p-f.txt"));
        assert!(!fixture.exists("d"));
        assert!(!fixture.exists("p-d/f.txt"));

        // Undo restores the directory before the file inside it, so the child's
        // path is valid again.
        let undone = undo(&fixture.options, first.run).expect("undo");
        assert_eq!(undone.failed(), 0);
        assert!(fixture.exists("top.txt"));
        assert!(fixture.exists("d/f.txt"));
    }

    #[test]
    fn a_case_only_rename_is_staged_so_a_case_insensitive_mount_would_work() {
        let fixture = fixture(&["readme.md"]);
        let report = apply(
            &fixture.plan(vec![Rule::Case {
                style: crate::rule::CaseStyle::Upper,
                target: Target::Stem,
            }]),
            &fixture.options,
        )
        .expect("apply");

        assert_eq!(report.renamed(), 1);
        assert!(fixture.exists("README.md"));
        assert!(!fixture.exists("readme.md"));
    }

    #[test]
    fn undo_restores_the_previous_names_in_reverse() {
        let fixture = fixture(&["a", "b"]);
        let first = apply(&fixture.plan(prefix("p-")), &fixture.options).expect("apply");
        assert!(fixture.exists("p-a"));

        let undone = undo(&fixture.options, first.run).expect("undo");

        assert_eq!(undone.renamed(), 2);
        assert_eq!(undone.run, first.run + 1);
        assert!(fixture.exists("a"));
        assert!(fixture.exists("b"));
        assert!(!fixture.exists("p-a"));

        // The undo is itself a run and can be undone, which is redo (§9).
        let redone = undo(&fixture.options, undone.run).expect("redo");
        assert_eq!(redone.renamed(), 2);
        assert!(fixture.exists("p-a"));
    }

    #[test]
    fn undo_does_not_reverse_a_run_twice() {
        let fixture = fixture(&["a", "b"]);
        let first = apply(&fixture.plan(prefix("p-")), &fixture.options).expect("apply");
        undo(&fixture.options, first.run).expect("the first undo");

        let refused = undo(&fixture.options, first.run);

        assert!(refused.is_err(), "nothing is left to reverse");
        assert_eq!(
            Journal::sessions(&fixture.options.journal).expect("sessions").len(),
            2,
            "the second undo writes no run"
        );
        assert!(fixture.exists("a"));
        assert!(fixture.exists("b"));
    }

    #[test]
    fn undo_skips_rows_a_later_undo_already_reversed() {
        let fixture = fixture(&["a", "b"]);
        let first = apply(&fixture.plan(prefix("p-")), &fixture.options).expect("apply");
        let row = Journal::renames(&fixture.options.journal, first.run)
            .expect("renames")
            .into_iter()
            .find(|(from, _)| from == &fixture.dir.join("a"))
            .expect("the row for a");
        undo_one(&fixture.options, first.run, &row.0, &row.1).expect("undo one");

        let undone = undo(&fixture.options, first.run).expect("the rest");

        assert_eq!(undone.renamed(), 1, "only b was left to reverse");
        assert_eq!(undone.failed(), 0, "a reversed row is not a failure");
        assert!(fixture.exists("a"));
        assert!(fixture.exists("b"));
    }

    #[test]
    fn undo_reverses_a_cycle_too() {
        let fixture = fixture(&["ab", "ba"]);
        let rules = swap();
        let first = apply(&fixture.plan(rules), &fixture.options).expect("apply");

        let undone = undo(&fixture.options, first.run).expect("undo");

        assert_eq!(undone.renamed(), 4, "two staged aside, two landed");
        assert!(fixture.exists("ab"));
        assert!(fixture.exists("ba"));
    }

    #[test]
    fn undo_reports_rows_that_are_no_longer_where_the_journal_expects_them() {
        let fixture = fixture(&["a"]);
        let first = apply(&fixture.plan(prefix("p-")), &fixture.options).expect("apply");
        fs::remove_file(fixture.dir.join("p-a")).expect("remove the renamed file");

        let undone = undo(&fixture.options, first.run).expect("undo");

        assert_eq!(undone.renamed(), 0);
        assert_eq!(undone.failed(), 1);
        assert!(!fixture.exists("a"));
    }

    #[test]
    fn undo_one_reverses_only_that_row() {
        let fixture = fixture(&["a", "b"]);
        let first = apply(&fixture.plan(prefix("p-")), &fixture.options).expect("apply");
        let row = Journal::renames(&fixture.options.journal, first.run)
            .expect("renames")
            .into_iter()
            .find(|(from, _)| from == &fixture.dir.join("a"))
            .expect("the row for a");

        let undone = undo_one(&fixture.options, first.run, &row.0, &row.1).expect("undo one");

        assert_eq!(undone.renamed(), 1);
        assert!(fixture.exists("a"), "the chosen row is back");
        assert!(fixture.exists("p-b"), "the other row is untouched");
        assert!(!fixture.exists("p-a"));
        // The undo is a run of its own, naming the run it partly reversed.
        let sessions = Journal::sessions(&fixture.options.journal).expect("sessions");
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[1].undo_of, Some(first.run));
        assert_eq!(sessions[0].renames, 2, "the original run keeps both records");
    }

    #[test]
    fn undo_one_refuses_a_pair_the_run_never_recorded() {
        let fixture = fixture(&["a"]);
        let first = apply(&fixture.plan(prefix("p-")), &fixture.options).expect("apply");

        let refused =
            undo_one(&fixture.options, first.run, Path::new("/tmp/x"), Path::new("/tmp/y"));

        assert!(refused.is_err());
        assert_eq!(
            Journal::sessions(&fixture.options.journal).expect("sessions").len(),
            1,
            "a refused undo writes no run"
        );
        assert!(fixture.exists("p-a"));
    }

    #[test]
    fn a_staged_name_is_recognized_so_it_is_never_undone_alone() {
        assert!(is_staged(1, Path::new("/tmp/.renameant-tmp-1-0-0")));
        assert!(!is_staged(2, Path::new("/tmp/.renameant-tmp-1-0-0")), "another run's name");
        assert!(!is_staged(1, Path::new("/tmp/.renameant-tmp-old")), "not the generated shape");
        assert!(!is_staged(1, Path::new("/tmp/a")));
    }

    #[test]
    fn a_rename_whose_record_cannot_be_written_puts_the_file_back() {
        let full = Path::new("/dev/full");
        if !full.exists() {
            eprintln!("/dev/full is absent, so the failing journal cannot be built");
            return;
        }
        let fixture = fixture(&["a"]);
        let mut journal =
            Journal::open_syncing(full.to_path_buf(), 1).expect("open /dev/full for writing");

        let result = rename(&fixture.dir.join("a"), &fixture.dir.join("b"), &mut journal, 1);

        assert!(result.is_err(), "a record that cannot be written aborts the run");
        assert!(fixture.exists("a"), "the name is back where it started");
        assert!(!fixture.exists("b"), "the rename was reversed");
    }

    #[test]
    fn undo_refuses_a_destination_that_is_now_a_different_file() {
        let fixture = fixture(&["a"]);
        let first = apply(&fixture.plan(prefix("p-")), &fixture.options).expect("apply");
        let renamed = fixture.dir.join("p-a");
        // Keep the original inode alive under another name, then put a new file
        // at the journal's destination.
        fs::rename(&renamed, fixture.dir.join("old")).expect("move the original");
        fs::write(&renamed, b"new").expect("replacement");

        let undone = undo(&fixture.options, first.run).expect("undo");

        assert_eq!(undone.failed(), 1);
        assert_eq!(undone.renamed(), 0);
        assert!(fixture.exists("p-a"), "the replacement stays where it is");
        assert_eq!(fs::read(fixture.dir.join("p-a")).expect("read"), b"new");
        assert!(!fixture.exists("a"), "nothing was moved to the old name");
    }

    #[test]
    fn a_failed_stage_puts_the_staged_peers_back() {
        let fixture = fixture(&["ab", "ba"]);
        let rules = swap();
        let plan = fixture.plan(rules);
        // The plan is built, then one source disappears before apply runs.
        fs::remove_file(fixture.dir.join("ba")).expect("remove the second source");

        let report = apply(&plan, &fixture.options).expect("apply");

        assert_eq!(report.renamed(), 0);
        assert_eq!(report.failed(), 1);
        assert!(fixture.exists("ab"), "the staged peer is back at its source");
        let leftovers: Vec<_> = fs::read_dir(&fixture.dir)
            .expect("read dir")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(TEMP_PREFIX))
            .collect();
        assert!(leftovers.is_empty(), "no file is left under a generated name");
    }

    #[test]
    fn undo_one_refuses_half_of_a_staged_pair() {
        let fixture = fixture(&["ab", "ba"]);
        let first = apply(&fixture.plan(swap()), &fixture.options).expect("apply");
        let landed = Journal::renames(&fixture.options.journal, first.run)
            .expect("renames")
            .into_iter()
            .find(|(from, _)| is_staged(first.run, from))
            .expect("a landed half");

        let refused = undo_one(&fixture.options, first.run, &landed.0, &landed.1);

        assert!(refused.is_err());
        assert_eq!(
            Journal::sessions(&fixture.options.journal).expect("sessions").len(),
            1,
            "a refused undo writes no run"
        );
        assert!(fixture.exists("ab"));
        assert!(fixture.exists("ba"));
    }

    #[test]
    fn undo_of_a_run_with_no_renames_is_refused() {
        let fixture = fixture(&["a"]);

        let refused = undo(&fixture.options, 9);

        assert!(refused.is_err());
        assert!(Journal::sessions(&fixture.options.journal).expect("sessions").is_empty());
    }

    #[test]
    fn undo_puts_back_a_name_staged_before_a_crash() {
        let fixture = fixture(&["ab", "ba"]);
        let src = fixture.dir.join("ab");
        let temp = fixture.dir.join(format!("{TEMP_PREFIX}-1-0-0"));
        fs::rename(&src, &temp).expect("stage aside");
        let id = FileId::of(&temp);
        let mut journal = Journal::open(fixture.options.journal.clone()).expect("open");
        journal
            .append(&JournalRecord::Begin {
                run: 1,
                at: 0,
                planned: 2,
                overwrite: false,
                undo_of: None,
            })
            .expect("begin");
        journal
            .append(&JournalRecord::Rename { run: 1, from: src.clone(), to: temp, id })
            .expect("record");
        drop(journal);

        let undone = undo(&fixture.options, 1).expect("undo");

        assert_eq!(undone.renamed(), 1);
        assert!(fixture.exists("ab"), "the staged name is back at its source");
    }

    #[test]
    fn a_failed_landing_puts_the_file_back_at_its_source() {
        let fixture = fixture(&["a"]);
        let blocked = fixture.dir.join("blocked");
        fs::create_dir(&blocked).expect("destination directory");
        // A hand-built plan: the destination exists, so landing must fail after
        // the source has been staged aside.
        let plan = Plan {
            rows: vec![crate::plan::PlanRow {
                src: fixture.dir.join("a"),
                dst: blocked.clone(),
                status: Status::Ok,
                via_temp: true,
            }],
        };

        let report = apply(&plan, &fixture.options).expect("apply");

        assert_eq!(report.renamed(), 0);
        assert_eq!(report.failed(), 0);
        assert_eq!(report.skipped(), 1);
        assert!(fixture.exists("a"), "the file is back at its source");
        assert!(blocked.is_dir(), "the destination is untouched");
        let leftovers: Vec<_> = fs::read_dir(&fixture.dir)
            .expect("read dir")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(TEMP_PREFIX))
            .collect();
        assert!(leftovers.is_empty(), "no file is left under a generated name");
    }

    #[test]
    fn a_journal_failure_puts_every_staged_peer_back() {
        let fixture = fixture(&["ab", "ba"]);
        let plan = fixture.plan(swap());
        let mut journal = Journal::open(fixture.options.journal.clone()).expect("open");
        let mut staged = Vec::new();
        for (index, row) in plan.rows.iter().enumerate() {
            let temp = stage(&row.src, 1, index, &mut journal).expect("stage").expect("free name");
            staged.push((index, temp));
        }

        let stranded = restore_staged(&plan, &staged);

        assert_eq!(stranded, 0);
        assert!(fixture.exists("ab"));
        assert!(fixture.exists("ba"));
        let leftovers: Vec<_> = fs::read_dir(&fixture.dir)
            .expect("read dir")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(TEMP_PREFIX))
            .collect();
        assert!(leftovers.is_empty(), "no file is left under a generated name");
    }

    #[test]
    fn an_interrupted_run_has_a_begin_with_no_end() {
        let fixture = fixture(&["a"]);
        let mut journal = Journal::open(fixture.options.journal.clone()).expect("open");
        journal
            .append(&JournalRecord::Begin {
                run: 1,
                at: 0,
                planned: 1,
                overwrite: false,
                undo_of: None,
            })
            .expect("append");
        drop(journal);

        let sessions = Journal::sessions(&fixture.options.journal).expect("sessions");
        assert!(sessions[0].interrupted);
    }
}
