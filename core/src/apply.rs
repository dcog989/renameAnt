//! Two-phase execution, syscalls and journaling.

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
        if self.failed() == 0 {
            RunOutcome::Completed
        } else if self.renamed() == 0 {
            RunOutcome::Aborted
        } else {
            RunOutcome::Partial
        }
    }
}

#[derive(Clone, Debug)]
pub struct ApplyOptions {
    pub journal: PathBuf,
    /// The irrecoverable overwrite opt-in. The session is marked so in
    /// the journal, and undo cannot restore a file it destroyed.
    pub overwrite: bool,
    /// Fast mode: batch the journal's `fsync` instead of writing one per
    /// rename. The renames themselves are unaffected; what a crash can lose is
    /// the last batch of undo records. A staged rename — a cycle or a
    /// case-only rename — is synced on its own, because it is the one step a
    /// crash could otherwise leave no record of.
    pub batched_sync: bool,
}

impl ApplyOptions {
    pub fn new(journal: PathBuf) -> Self {
        ApplyOptions { journal, overwrite: false, batched_sync: false }
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
/// Every write goes through `renameat2(RENAME_NOREPLACE)` unless the overwrite
/// opt-in is set, each completed rename is `fsync`ed before the next begins
/// (unless [`ApplyOptions::batched_sync`] batches those, see
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
        overwrite: options.overwrite,
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
            let outcome = rename(
                &plan.rows[at].src,
                &plan.rows[at].dst,
                options.overwrite,
                &mut journal,
                run,
            )?;
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
            match stage(&row.src, run, index, &mut journal)? {
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
                let outcome =
                    rename(temp, &plan.rows[*index].src, false, &mut journal, run)?;
                rows[*index].outcome = match outcome {
                    Ok(()) => Outcome::Skipped,
                    Err(error) => Outcome::Failed(error),
                };
            }
            at = end;
            continue;
        }
        for (index, temp) in staged.iter().rev() {
            let outcome =
                rename(temp, &plan.rows[*index].dst, options.overwrite, &mut journal, run)?;
            if let Err(ApplyError::NotSupported) = &outcome {
                aborted = true;
            }
            rows[*index].outcome = as_outcome(outcome);
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
/// Undo is not transactional: a row whose file is no longer where the journal
/// expects it fails and the rest continue. It never overwrites, and it cannot
/// restore a file the overwrite opt-in destroyed.
pub fn undo(options: &ApplyOptions, target: RunId) -> io::Result<ApplyReport> {
    // Lock before reading: the lock is held for the whole undo, so a concurrent
    // apply cannot claim this undo's run id.
    let journal = Journal::open_syncing(options.journal.clone(), options.sync_every())?;
    let mut completed = Journal::renames_with_id(&options.journal, target)?;
    completed.reverse();
    reverse(options, journal, target, completed)
}

/// Reverses one recorded rename of `target`, leaving the rest of the run alone.
///
/// The pair must be one the run recorded, so a stale row can never move a file
/// the run never renamed. The undo is a run of its own and can be undone.
pub fn undo_one(
    options: &ApplyOptions,
    target: RunId,
    from: &Path,
    to: &Path,
) -> io::Result<ApplyReport> {
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
            _ => rename(&record.to, &record.from, false, &mut journal, run)?,
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

/// Whether `path` carries a name [`stage`] generated. Such a row is one half of
/// a rename that must be reversed as a whole, never alone.
pub fn is_staged(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name.as_encoded_bytes().starts_with(TEMP_PREFIX.as_bytes()))
}

/// The result of one rename.
///
/// The outer error means the journal could not be written, which aborts the run.
/// The inner error is this row's own failure, which does not.
type RenameResult = io::Result<Result<(), ApplyError>>;

/// The result of staging one name aside: `Ok` names the generated path.
type StageResult = io::Result<Result<PathBuf, ApplyError>>;

fn rename(
    src: &Path,
    dst: &Path,
    overwrite: bool,
    journal: &mut Journal,
    run: RunId,
) -> RenameResult {
    // A journal entry is later replayed from another working directory.
    if !src.is_absolute() || !dst.is_absolute() {
        return Ok(Err(ApplyError::RelativePath));
    }

    let result = if overwrite {
        rustix::fs::rename(src, dst)
    } else {
        renameat_with(CWD, src, CWD, dst, RenameFlags::NOREPLACE)
    };

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
                // No record, no rename: put the file back when nothing was
                // clobbered. The overwrite opt-in already made restore
                // impossible, so it keeps the file at its new name.
                if !overwrite {
                    let _ = renameat_with(CWD, dst, CWD, src, RenameFlags::NOREPLACE);
                }
                return Err(error);
            }
            Ok(Ok(()))
        }
    }
}

/// Moves `src` aside under a generated name in its own directory.
///
/// Staging always uses `RENAME_NOREPLACE` even with the overwrite opt-in: the
/// generated name is renameAnt's own and must not clobber anyone else's file.
///
/// The aside rename is the one step a crash cannot describe from the other
/// side — the file is at neither `src` nor its destination — so its record is
/// forced to disk before moving on, and a record that cannot be written puts
/// the name back. Every other rename leaves the file on a name the user chose.
fn stage(src: &Path, run: RunId, at: usize, journal: &mut Journal) -> StageResult {
    let parent = src.parent().unwrap_or_else(|| Path::new("."));
    for attempt in 0..TEMP_ATTEMPTS {
        let temp = parent.join(format!("{TEMP_PREFIX}-{run}-{at}-{attempt}"));
        let outcome = match rename(src, &temp, false, journal, run) {
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
        assert!(fixture.exists("a.txt"));
        assert!(fixture.exists("b.txt"));
    }

    #[test]
    fn a_cycle_goes_through_a_temp_name_and_leaves_none_behind() {
        let fixture = fixture(&["ab", "ba"]);
        let rules = vec![Rule::Regex {
            pattern: "^(.+)(.)$".into(),
            replacement: "${2}${1}".into(),
            target: Target::Whole,
            path: false,
        }];

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
        let rules = vec![Rule::Regex {
            pattern: "^(.+)(.)$".into(),
            replacement: "${2}${1}".into(),
            target: Target::Whole,
            path: false,
        }];

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
    fn undo_reverses_a_cycle_too() {
        let fixture = fixture(&["ab", "ba"]);
        let rules = vec![Rule::Regex {
            pattern: "^(.+)(.)$".into(),
            replacement: "${2}${1}".into(),
            target: Target::Whole,
            path: false,
        }];
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
        assert!(is_staged(Path::new("/tmp/.renameant-tmp-1-0-0")));
        assert!(!is_staged(Path::new("/tmp/a")));
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

        let result =
            rename(&fixture.dir.join("a"), &fixture.dir.join("b"), false, &mut journal, 1);

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
        let rules = vec![Rule::Regex {
            pattern: "^(.+)(.)$".into(),
            replacement: "${2}${1}".into(),
            target: Target::Whole,
            path: false,
        }];
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
    fn the_overwrite_opt_in_is_recorded_so_the_session_is_irrecoverable() {
        let fixture = fixture(&["a"]);
        let mut options = fixture.options.clone();
        options.overwrite = true;

        apply(&fixture.plan(prefix("p-")), &options).expect("apply");

        let sessions = Journal::sessions(&fixture.options.journal).expect("sessions");
        assert!(sessions[0].overwrite);
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
