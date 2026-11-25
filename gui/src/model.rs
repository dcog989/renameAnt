//! What the window holds: the plan, and the sort view over it.
//!
//! `plan()` itself is `core`'s; everything here is a shell around it, so the
//! numbers a frame needs — totals, the name flags — are computed once when the
//! plan or the scan changes, never per frame.

use std::collections::{HashMap, HashSet};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use renameant_core::filter::Filter;
use renameant_core::osname;
use renameant_core::plan::{self, Plan, PlanRow, Status};
use renameant_core::rule::RuleSet;
use renameant_core::scan::{DirIndex, Entry, Kind};

use crate::format::{self, is_utf8};

/// Missing metadata, or a size that says nothing (a directory's), sorts last,
/// never first.
fn size_key(meta: Option<&FileMeta>) -> (bool, u64) {
    meta.and_then(|meta| meta.size).map_or((true, 0), |size| (false, size))
}

fn time_key(meta: Option<&FileMeta>) -> (bool, Option<SystemTime>) {
    meta.map_or((true, None), |meta| (meta.mtime.is_none(), meta.mtime))
}

/// The whole path's bytes, for the orders that compare full paths.
fn path_bytes(path: &Path) -> &[u8] {
    path.as_os_str().as_bytes()
}

/// Which order the table shows the rows in. The plan itself is never reordered;
/// this is a view over it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortKey {
    /// The order `apply` uses.
    #[default]
    Apply,
    /// By source path, as the Path column shows it.
    Path,
    /// By the status label the row shows.
    Status,
    After,
    /// By size on disk.
    Size,
    /// By modification time.
    Mtime,
}

impl SortKey {
    /// Whether this order needs the size/mtime the scan does not capture.
    pub fn needs_meta(self) -> bool {
        matches!(self, SortKey::Size | SortKey::Mtime)
    }
}

/// Size and mtime, read right after the scan. It is the link's own metadata,
/// never the target's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileMeta {
    /// `None` for a directory, whose own size is meaningless (btrfs reports it
    /// as zero), so the column stays blank.
    pub size: Option<u64>,
    /// `None` when the filesystem will not report a modification time.
    pub mtime: Option<SystemTime>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sort {
    pub key: SortKey,
    pub desc: bool,
}

/// Plan totals, so no frame has to walk the whole plan to label a button.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    pub actionable: usize,
    pub candidates: usize,
    pub skipped: usize,
    /// The actionable rows, split by what they rename.
    pub files: usize,
    pub folders: usize,
}

#[derive(Default)]
pub struct PlanModel {
    pub entries: Vec<Entry>,
    pub index: Option<DirIndex>,
    pub plan: Plan,
    pub error: Option<String>,
    pub view: Vec<usize>,
    pub sort: Sort,
    pub totals: Totals,
    pub not_utf8: usize,
    pub not_nfc: usize,
    /// Size and mtime by source path, empty until the scan's stat pass lands.
    pub meta: HashMap<PathBuf, FileMeta>,
    /// A stat pass has come back for the entries of the current scan.
    pub meta_loaded: bool,
    /// Rows the user removed from the list, by source path. A removed entry is
    /// not planned at all and is not drawn; it keeps blocking its own name.
    pub removed: HashSet<PathBuf>,
    /// The entries the pipeline runs on. Empty by default, so it keeps
    /// everything. A filtered-out entry is not planned and is not shown, like a
    /// removed one, but its name stays on disk.
    pub filter: Filter,
}

impl PlanModel {
    /// Replaces the scan. The name flags are counted here, once. The metadata
    /// belongs to the old paths, so it is dropped with them.
    pub fn set_scan(&mut self, entries: Vec<Entry>, index: DirIndex) {
        self.meta.clear();
        self.meta_loaded = false;
        // A removal is for a path, not for one scan: it survives a rescan of
        // the same tree and is dropped once the path is gone.
        let present: HashSet<&Path> =
            entries.iter().map(|entry| entry.path.as_path()).collect();
        self.removed.retain(|path| present.contains(path.as_path()));
        self.not_utf8 =
            entries.iter().filter(|entry| !is_utf8(entry.name.as_os_str().as_bytes())).count();
        self.not_nfc = entries
            .iter()
            .filter(|entry| !osname::is_nfc(entry.name.as_os_str().as_bytes()))
            .count();
        self.entries = entries;
        self.index = Some(index);
    }

    /// Drops the scan after it failed. The filter stays; it is the user's
    /// choice, not part of the scan.
    pub fn clear(&mut self) {
        let filter = std::mem::take(&mut self.filter);
        *self = PlanModel { filter, ..PlanModel::default() };
    }

    /// Recomputes the plan for `rules`. Pure: `plan()` answers existence from
    /// the scan's index.
    pub fn replan(&mut self, rules: &RuleSet) {
        match self.build_plan(rules) {
            Ok(plan) => {
                self.error = None;
                self.plan = plan;
            }
            Err(error) => {
                // Keep the last good plan and its totals: `error` disables
                // Apply, so a typo mid-edit does not blank the preview the
                // user was reading.
                self.error = Some(error);
                return;
            }
        }
        // The footer splits the rows that will be renamed by what they are. The
        // plan carries no `Kind`, so it is read back off the scan's entries.
        let ok: HashSet<&Path> = self
            .plan
            .rows
            .iter()
            .filter(|row| row.status == Status::Ok)
            .map(|row| row.src.as_path())
            .collect();
        let mut files = 0;
        let mut folders = 0;
        for entry in &self.entries {
            if ok.contains(entry.path.as_path()) {
                match entry.kind {
                    Kind::Dir => folders += 1,
                    Kind::File | Kind::Symlink => files += 1,
                }
            }
        }
        self.totals = Totals {
            actionable: self.plan.actionable(),
            candidates: self.plan.candidates(),
            skipped: self.plan.skipped(),
            files,
            folders,
        };
        self.rebuild_view();
    }

    /// The plan `replan` stores: the scan's entries minus the removed and the
    /// filtered-out, through `plan()`.
    fn build_plan(&self, rules: &RuleSet) -> Result<Plan, String> {
        let Some(index) = &self.index else {
            return Ok(Plan::default());
        };
        let filter = self.filter.compile().map_err(|error| error.to_string())?;
        // A filtered-out entry, like a removed one, is not planned at all, so it
        // stops moving out of a destination's way: a name it would have vacated
        // now conflicts exactly as it does on disk.
        //
        // The kept rows are borrowed, not cloned: this runs on every replan.
        let built = if self.removed.is_empty() && self.filter.is_inactive() {
            plan::plan(&self.entries, index, rules)
        } else {
            let filtered: Vec<&Entry> = self
                .entries
                .iter()
                .filter(|entry| !self.removed.contains(&entry.path) && filter.matches(entry))
                .collect();
            plan::plan(&filtered, index, rules)
        };
        built.map_err(|error| error.to_string())
    }

    /// Re-sorts the view. Rebuilds are on demand, never per frame.
    pub fn rebuild_view(&mut self) {
        let mut view: Vec<usize> = (0..self.plan.rows.len()).collect();
        let rows = &self.plan.rows;
        match self.sort.key {
            // Plan order is apply order.
            SortKey::Apply => {}
            SortKey::Path => view.sort_by(|left, right| {
                path_bytes(&rows[*left].src).cmp(path_bytes(&rows[*right].src))
            }),
            SortKey::Status => view.sort_by(|left, right| {
                format::status_label(rows[*left].status)
                    .cmp(format::status_label(rows[*right].status))
            }),
            SortKey::After => view.sort_by(|left, right| {
                path_bytes(&rows[*left].dst).cmp(path_bytes(&rows[*right].dst))
            }),
            // Until the stat pass lands every key ties, which a stable sort keeps
            // in apply order. A row that could not be read sorts last.
            SortKey::Size => view.sort_by(|left, right| {
                size_key(self.meta.get(&rows[*left].src))
                    .cmp(&size_key(self.meta.get(&rows[*right].src)))
            }),
            SortKey::Mtime => view.sort_by(|left, right| {
                time_key(self.meta.get(&rows[*left].src))
                    .cmp(&time_key(self.meta.get(&rows[*right].src)))
            }),
        }
        if self.sort.desc {
            view.reverse();
        }
        self.view = view;
    }

    /// The paths to stat: the rows the plan shows. A filtered-out entry is not
    /// shown, so it is read only when a filter change puts it back in the plan.
    pub fn meta_paths(&self) -> Vec<PathBuf> {
        self.plan.rows.iter().map(|row| row.src.clone()).collect()
    }

    /// Records a stat pass. A row it could not read is simply absent, so it
    /// sorts last rather than reading as zero bytes.
    pub fn set_meta(&mut self, meta: Vec<(PathBuf, FileMeta)>) {
        self.meta = meta.into_iter().collect();
        self.meta_loaded = true;
        self.rebuild_view();
    }

    /// The rows that will never be applied, capped for display.
    pub fn skipped_rows(&self, limit: usize) -> impl Iterator<Item = &PlanRow> {
        self.plan
            .rows
            .iter()
            .filter(|row| matches!(row.status, Status::Conflict(_) | Status::Invalid(_)))
            .take(limit)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use renameant_core::plan::ConflictKind;
    use renameant_core::rule::{InsertRemoveOp, Rule, RuleSet, Target};

    use super::*;

    /// The status of the row whose source is `src`.
    fn status_of(model: &PlanModel, src: &str) -> Status {
        model
            .plan
            .rows
            .iter()
            .find(|row| row.src.as_path() == Path::new(src))
            .map(|row| row.status)
            .expect(src)
    }

    #[test]
    fn the_view_is_a_sort_over_the_plan_not_a_reorder_of_it() {
        let row = |src: &str, dst: &str| PlanRow {
            src: PathBuf::from(src),
            dst: PathBuf::from(dst),
            status: Status::Ok,
            via_temp: false,
        };
        let mut model = PlanModel {
            plan: Plan { rows: vec![row("/d/b", "/d/1"), row("/d/a", "/d/2")] },
            ..PlanModel::default()
        };

        model.rebuild_view();
        assert_eq!(model.view, vec![0, 1]);

        model.sort = Sort { key: SortKey::Path, desc: false };
        model.rebuild_view();
        assert_eq!(model.view, vec![1, 0]);

        model.sort = Sort { key: SortKey::After, desc: false };
        model.rebuild_view();
        assert_eq!(model.view, vec![0, 1]);
        assert_eq!(crate::format::file_text(&model.plan.rows[model.view[0]].src), "b");

        model.sort = Sort { key: SortKey::Path, desc: true };
        model.rebuild_view();
        assert_eq!(model.view, vec![0, 1]);
    }

    #[test]
    fn a_status_sort_groups_rows_by_the_label_they_show() {
        let row = |src: &str, status: Status| PlanRow {
            src: PathBuf::from(src),
            dst: PathBuf::from(src),
            status,
            via_temp: false,
        };
        let mut model = PlanModel {
            plan: Plan {
                rows: vec![
                    row("/d/a", Status::Ok),
                    row("/d/b", Status::Conflict(ConflictKind::TargetExists)),
                ],
            },
            ..PlanModel::default()
        };

        model.sort = Sort { key: SortKey::Status, desc: false };
        model.rebuild_view();
        // "match" sorts before "name is taken".
        assert_eq!(model.view, vec![0, 1]);
    }

    #[test]
    fn a_size_or_mtime_sort_puts_an_unreadable_row_last() {
        let row = |src: &str| PlanRow {
            src: PathBuf::from(src),
            dst: PathBuf::from(src),
            status: Status::Ok,
            via_temp: false,
        };
        let mut model = PlanModel {
            plan: Plan { rows: vec![row("/d/a"), row("/d/b"), row("/d/c"), row("/d/dir")] },
            ..PlanModel::default()
        };
        let meta = |size: u64, secs: u64| FileMeta {
            size: Some(size),
            mtime: Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs)),
        };
        // /d/b could not be read, so it sorts last even though /d/a is smallest.
        // /d/dir has no size (a directory's says nothing), so it sorts last too.
        model.set_meta(vec![
            (PathBuf::from("/d/c"), meta(300, 30)),
            (PathBuf::from("/d/a"), meta(100, 10)),
            (PathBuf::from("/d/dir"), FileMeta { size: None, mtime: None }),
        ]);

        model.sort = Sort { key: SortKey::Size, desc: false };
        model.rebuild_view();
        assert_eq!(model.view, vec![0, 2, 1, 3]);

        model.sort = Sort { key: SortKey::Mtime, desc: false };
        model.rebuild_view();
        assert_eq!(model.view, vec![0, 2, 1, 3]);
    }

    #[test]
    fn a_removed_path_survives_a_rescan_and_goes_when_it_is_gone() {
        let entry = |name: &str| Entry {
            path: PathBuf::from("/d").join(name),
            name: std::ffi::OsString::from(name),
            kind: renameant_core::scan::Kind::File,
        };
        let mut index = DirIndex::default();
        index.insert(PathBuf::from("/d"), std::collections::BTreeSet::new());
        let mut model = PlanModel::default();
        model.set_scan(vec![entry("a")], index.clone());
        model.removed.insert(PathBuf::from("/d/a"));

        model.set_scan(vec![entry("a")], index.clone());
        assert!(model.removed.contains(&PathBuf::from("/d/a")), "the removal survives");

        model.set_scan(vec![entry("b")], index);
        assert!(model.removed.is_empty(), "a path outside the new scan is forgotten");
    }

    #[test]
    fn a_filtered_out_entry_is_not_planned_and_still_blocks_its_name() {
        let entry = |name: &str| Entry {
            path: PathBuf::from("/d").join(name),
            name: std::ffi::OsString::from(name),
            kind: renameant_core::scan::Kind::File,
        };
        let mut index = DirIndex::default();
        let names: std::collections::BTreeSet<std::ffi::OsString> =
            ["a", "ab"].iter().map(|name| std::ffi::OsString::from(*name)).collect();
        index.insert(PathBuf::from("/d"), names);
        let mut model = PlanModel {
            entries: vec![entry("a"), entry("ab")],
            index: Some(index),
            // Only `a` is planned, so only `a` vacates its name.
            filter: Filter { pattern: "^a$".into(), regex: true, ..Filter::default() },
            ..PlanModel::default()
        };
        // a -> ab and ab -> abb: `ab` would vacate the name `a` wants.
        let rules = RuleSet {
            rules: vec![Rule::InsertRemove {
                op: InsertRemoveOp::AddSuffix(b"b".to_vec()),
                target: Target::Whole,
            }],
        };

        model.replan(&rules);

        // `ab` is left out of the plan entirely, so it stays where it is and
        // `a` cannot take its name.
        assert_eq!(status_of(&model, "/d/a"), Status::Conflict(ConflictKind::TargetExists));
        assert!(model.plan.rows.iter().all(|row| row.src.as_path() != Path::new("/d/ab")));
        // Only rows the plan shows are statted.
        assert_eq!(model.meta_paths(), vec![PathBuf::from("/d/a")]);
    }

    #[test]
    fn a_broken_plan_keeps_the_last_preview() {
        let entry = |name: &str| Entry {
            path: PathBuf::from("/d").join(name),
            name: std::ffi::OsString::from(name),
            kind: renameant_core::scan::Kind::File,
        };
        let mut index = DirIndex::default();
        index.insert(
            PathBuf::from("/d"),
            std::iter::once(std::ffi::OsString::from("a")).collect(),
        );
        let mut model = PlanModel {
            entries: vec![entry("a")],
            index: Some(index),
            ..PlanModel::default()
        };
        let rules = RuleSet {
            rules: vec![Rule::InsertRemove {
                op: InsertRemoveOp::AddSuffix(b"b".to_vec()),
                target: Target::Whole,
            }],
        };
        model.replan(&rules);
        let before = model.plan.clone();
        assert_eq!(before.rows.len(), 1);

        model.filter = Filter { pattern: "(".into(), regex: true, ..Filter::default() };
        model.replan(&rules);

        assert!(model.error.is_some());
        assert_eq!(model.plan, before, "the last good preview stays");
    }
}
