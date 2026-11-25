//! Conflict detection and apply ordering.
//!
//! Every check here is pure: existence comes from the `DirIndex` snapshot, and
//! apply re-checks each write against the real filesystem with
//! `RENAME_NOREPLACE`.
//!
//! The checks walk sorted arrays instead of hash tables. At 100k rows the cost
//! is memory latency, not comparisons: a sorted walk reads the rows and their
//! names in order, where a key hashed once per row per check jumps around a
//! table.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use crate::osname;
use crate::plan::{ConflictKind, PlanRow, Status, file_name_of, parent_of};
use crate::scan::DirIndex;

/// Marks every `Ok` row that cannot be applied, then puts the survivors in
/// apply order.
pub(crate) fn resolve(rows: &mut [PlanRow], index: &DirIndex) {
    // The checks borrow the names out of `rows`, so which rows survive is kept
    // beside them and written back in one pass at the end.
    let mut status: Vec<Status> = rows.iter().map(|row| row.status).collect();
    let mut dirs = Directories::new(index);

    let mut targets = Targets::of(rows, &status, &mut dirs);
    mark_collisions(&mut status, &mut targets);

    // Only a row that is still applied can vacate a name for another row, so
    // the sources are collected once the collision marks have landed. A row
    // that `mark_target_exists` marks is settled afterwards by
    // `propagate_blocked`, which spreads that mark up the chain.
    let sources = sources_of(rows, &status, &targets.by_name);
    mark_target_exists(&mut status, &targets, &sources, &dirs);
    let blocker = blockers(rows, &status, &targets, &sources);
    propagate_blocked(&mut status, &blocker);

    mark_case_only_renames(rows, &status);
    let order = apply_order(rows, &status, &blocker);

    for (row, status) in rows.iter_mut().zip(&status) {
        row.status = *status;
    }
    reorder(rows, &order);
}

/// A row's name in the form the checks work on: which directory it is in, the
/// name itself, and the row it belongs to.
///
/// A destination and a source use the same shape, because a destination that is
/// another row's source is the one relation apply order is built on.
#[derive(Clone, Copy)]
struct Destination<'a> {
    dir: u32,
    name: &'a OsStr,
    row: usize,
}

impl Destination<'_> {
    /// Directory first, then name bytes. A directory's names come out of the
    /// scan and off the filesystem in the same byte order, so sorting both this
    /// way makes the two sides of a merge walk together.
    fn compare(left: &Self, right: &Self) -> Ordering {
        (left.dir, left.name.as_bytes()).cmp(&(right.dir, right.name.as_bytes()))
    }

    /// The same, with name bytes folded to ASCII lower case: the order that puts
    /// names differing only by case side by side.
    fn compare_folded(left: &Self, right: &Self) -> Ordering {
        left.dir.cmp(&right.dir).then_with(|| folded_order(left.name, right.name))
    }
}

/// Byte order ignoring ASCII case.
fn folded_order(left: &OsStr, right: &OsStr) -> Ordering {
    let (left, right) = (left.as_bytes(), right.as_bytes());
    for (left, right) in left.iter().zip(right) {
        let order = left.to_ascii_lowercase().cmp(&right.to_ascii_lowercase());
        if order != Ordering::Equal {
            return order;
        }
    }
    left.len().cmp(&right.len())
}

/// The destinations of the rows that can still be applied, in the two orders the
/// checks need.
struct Targets<'a> {
    /// By directory and name: duplicate targets, and the names the scan found.
    by_name: Vec<Destination<'a>>,
    /// By directory and folded name: case-only collisions.
    by_fold: Vec<Destination<'a>>,
}

impl<'a> Targets<'a> {
    fn of(rows: &'a [PlanRow], status: &[Status], dirs: &mut Directories<'_>) -> Self {
        let mut by_name: Vec<Destination<'a>> = rows
            .iter()
            .enumerate()
            .filter(|(at, _)| status[*at] == Status::Ok)
            .map(|(row, plan_row)| Destination {
                dir: dirs.id(parent_of(&plan_row.dst)),
                name: file_name_of(&plan_row.dst),
                row,
            })
            .collect();
        by_name.sort_unstable_by(Destination::compare);
        let mut by_fold = by_name.clone();
        by_fold.sort_unstable_by(Destination::compare_folded);
        Targets { by_name, by_fold }
    }
}

/// The rows' names as they are on disk, in the same order as the destinations.
///
/// Built from the destinations rather than from the rows again: a rule only ever
/// changes a basename, so a row's source sits in the same directory as its
/// destination and needs no second lookup.
fn sources_of<'a>(
    rows: &'a [PlanRow],
    status: &[Status],
    targets: &[Destination<'a>],
) -> Vec<Destination<'a>> {
    let mut sources: Vec<Destination<'a>> = targets
        .iter()
        .filter(|target| status[target.row] == Status::Ok)
        .map(|target| Destination {
            dir: target.dir,
            name: file_name_of(&rows[target.row].src),
            row: target.row,
        })
        .collect();
    sources.sort_unstable_by(Destination::compare);
    sources
}

/// Two rows aiming at the same name, and two destinations that differ
/// only by ASCII case. The second is only a real collision on a
/// case-insensitive mount, but it is never applied either way.
///
/// Both are found in one walk: names that are equal, and names that are equal
/// with case folded, occupy the same place in the folded order.
fn mark_collisions(status: &mut [Status], targets: &mut Targets<'_>) {
    let by_fold = &mut targets.by_fold;
    let mut at = 0;
    while at < by_fold.len() {
        let mut end = at + 1;
        while end < by_fold.len()
            && Destination::compare_folded(&by_fold[end], &by_fold[at]) == Ordering::Equal
        {
            end += 1;
        }

        let group = &mut by_fold[at..end];
        if group.len() > 1 {
            // Exact names first, so the case-only check below sees one row per
            // name and not the copies of a duplicated destination.
            group.sort_unstable_by(Destination::compare);
            let mut same_name: Vec<usize> = Vec::new();
            let mut left = 0;
            while left < group.len() {
                let mut right = left + 1;
                while right < group.len() && group[right].name == group[left].name {
                    right += 1;
                }
                if right - left > 1 {
                    for target in &group[left..right] {
                        status[target.row] = Status::Conflict(ConflictKind::DuplicateTarget);
                    }
                } else if status[group[left].row] == Status::Ok {
                    same_name.push(group[left].row);
                }
                left = right;
            }
            if same_name.len() > 1 {
                for row in same_name {
                    status[row] = Status::Conflict(ConflictKind::CaseOnly);
                }
            }
        }
        at = end;
    }
}

/// A destination already on disk. It is not a conflict when another row is
/// moving *out of* that name, because apply order runs that row first, and a
/// name is only "already there" in its own directory.
///
/// The scan's names for a directory and that directory's destinations are both
/// sorted, so they are walked together rather than looked up one by one.
fn mark_target_exists(
    status: &mut [Status],
    targets: &Targets<'_>,
    sources: &[Destination<'_>],
    dirs: &Directories<'_>,
) {
    let mut source = 0;
    let mut at = 0;
    while at < targets.by_name.len() {
        let dir = targets.by_name[at].dir;
        let end = at + targets.by_name[at..].iter().take_while(|it| it.dir == dir).count();
        let mut names = dirs.names(dir).map(|names| names.iter());
        let mut present = names.as_mut().and_then(Iterator::next);

        for target in &targets.by_name[at..end] {
            while matches!(present, Some(name) if name.as_os_str() < target.name) {
                present = names.as_mut().and_then(Iterator::next);
            }
            while matches!(sources.get(source), Some(source) if Destination::compare(source, target) == Ordering::Less)
            {
                source += 1;
            }
            let on_disk = matches!(present, Some(name) if name.as_os_str() == target.name);
            let vacated = matches!(sources.get(source), Some(source) if Destination::compare(source, target) == Ordering::Equal);
            if status[target.row] == Status::Ok && on_disk && !vacated {
                status[target.row] = Status::Conflict(ConflictKind::TargetExists);
            }
        }
        at = end;
    }
}

/// For every row, the row holding its destination: the row apply has to run
/// first, and the reason a destination that is already on disk is not a
/// conflict. Whether that holder is itself applied is settled afterwards, by
/// [`propagate_blocked`].
fn blockers(
    rows: &[PlanRow],
    status: &[Status],
    targets: &Targets<'_>,
    sources: &[Destination<'_>],
) -> Vec<Option<usize>> {
    let mut blocker: Vec<Option<usize>> = vec![None; rows.len()];
    let mut source = 0;
    for target in &targets.by_name {
        if status[target.row] != Status::Ok {
            continue;
        }
        while matches!(sources.get(source), Some(source) if Destination::compare(source, target) == Ordering::Less)
        {
            source += 1;
        }
        let Some(found) = sources.get(source) else { break };
        if Destination::compare(found, target) == Ordering::Equal && found.row != target.row {
            blocker[target.row] = Some(found.row);
        }
    }
    blocker
}

/// A row can only vacate its name if it is applied. When a row will not be
/// applied, every row whose destination it holds will not be either, and the
/// same holds for the row waiting on that one: the mark spreads up the chain.
///
/// A cycle never reaches here: every member holds another member's destination
/// and is staged through a temp name, so none of them is ever marked.
fn propagate_blocked(status: &mut [Status], blocker: &[Option<usize>]) {
    // Only a row another row waits on can spread anything, so a plan with no
    // chains builds nothing.
    let mut waiting: HashMap<usize, Vec<usize>> = HashMap::new();
    for (row, blocked_by) in blocker.iter().enumerate() {
        if let Some(blocked_by) = blocked_by {
            waiting.entry(*blocked_by).or_default().push(row);
        }
    }

    let mut queue: Vec<usize> =
        waiting.keys().copied().filter(|at| status[*at] != Status::Ok).collect();
    while let Some(blocked_by) = queue.pop() {
        for &row in waiting.get(&blocked_by).into_iter().flatten() {
            if status[row] == Status::Ok {
                status[row] = Status::Conflict(ConflictKind::TargetExists);
                queue.push(row);
            }
        }
    }
}

/// A rename that differs from its source only by case takes the temp-name path:
/// the mount may be case-insensitive, where a direct
/// `RENAME_NOREPLACE` reports the destination as already existing.
fn mark_case_only_renames(rows: &mut [PlanRow], status: &[Status]) {
    for (at, row) in rows.iter_mut().enumerate() {
        if status[at] == Status::Ok
            && osname::is_case_only(
                file_name_of(&row.src).as_bytes(),
                file_name_of(&row.dst).as_bytes(),
            )
        {
            row.via_temp = true;
        }
    }
}

/// Returns row indices in apply order and marks cycle members `via_temp`.
///
/// A row whose destination is another row's source waits for that row, so a
/// chain `A->B`, `B->C` applies `B->C` first and neither reports a conflict. A
/// genuine cycle has no such order, so its rows are marked `via_temp` and
/// emitted as one contiguous run for apply's two-phase temp rename.
///
/// Both kinds of order stay inside one directory, so a recursive scan adds one
/// rule: a deeper row goes first. That way a directory is renamed only after
/// everything under it, and a child's path is still valid when its own row runs.
fn apply_order(rows: &mut [PlanRow], status: &[Status], blocker: &[Option<usize>]) -> Vec<usize> {
    let mut by_depth: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (at, row) in rows.iter().enumerate() {
        if status[at] == Status::Ok {
            by_depth.entry(depth_of(parent_of(&row.src))).or_default().push(at);
        }
    }

    let mut emitted = vec![false; rows.len()];
    let mut order: Vec<usize> = Vec::with_capacity(rows.len());
    for group in by_depth.values().rev() {
        order.extend(order_within_depth(rows, group, blocker, &mut emitted));
    }

    // Rows apply will not touch keep their scan order.
    for (at, status) in status.iter().enumerate() {
        if *status != Status::Ok {
            order.push(at);
        }
    }
    order
}

/// How far down a path sits. Only compared, so its zero point does not matter.
fn depth_of(path: &Path) -> usize {
    path.components().count()
}

/// Chains, cycles and independent rows within one directory: the whole of the
/// single-directory ordering, restricted to rows that share a depth.
fn order_within_depth(
    rows: &mut [PlanRow],
    group: &[usize],
    blocker: &[Option<usize>],
    emitted: &mut [bool],
) -> Vec<usize> {
    // `dependency[row]` is the row that vacates `row`'s destination. It always
    // shares this row's directory, so it can never leave this group.
    let mut dependency: HashMap<usize, usize> = HashMap::new();
    for &at in group {
        if let Some(blocker) = blocker[at]
            && blocker != at
        {
            dependency.insert(at, blocker);
        }
    }

    let mut order: Vec<usize> = Vec::with_capacity(group.len());

    loop {
        loop {
            let mut progressed = false;
            for &at in group {
                if emitted[at] {
                    continue;
                }
                if matches!(dependency.get(&at), Some(blocker) if !emitted[*blocker]) {
                    continue;
                }
                order.push(at);
                emitted[at] = true;
                progressed = true;
            }
            if !progressed {
                break;
            }
        }

        // Every remaining row waits on another remaining row, so walking the
        // dependencies from any of them must revisit one: that is the cycle.
        let Some(&start) = group.iter().find(|at| !emitted[**at]) else {
            break;
        };
        let mut seen: HashMap<usize, usize> = HashMap::new();
        let mut path: Vec<usize> = Vec::new();
        let mut current = start;
        loop {
            if let Some(&looped_at) = seen.get(&current) {
                for &member in &path[looped_at..] {
                    rows[member].via_temp = true;
                    order.push(member);
                    emitted[member] = true;
                }
                break;
            }
            seen.insert(current, path.len());
            path.push(current);
            current = dependency[&current];
        }
    }
    order
}

/// Puts `rows` into `order`, where `order[at]` is the row that belongs at `at`.
///
/// `order` is a permutation, so each of its cycles is walked and rotated by
/// swapping from one position of that cycle: no row is ever cloned.
fn reorder(rows: &mut [PlanRow], order: &[usize]) {
    let mut placed = vec![false; rows.len()];
    let mut cycle: Vec<usize> = Vec::new();
    for start in 0..rows.len() {
        if placed[start] {
            continue;
        }
        cycle.clear();
        let mut at = start;
        while !placed[at] {
            placed[at] = true;
            cycle.push(at);
            at = order[at];
        }
        // Each position of the cycle takes the row from the next one, which
        // swapping against the head does one position at a time.
        for &other in cycle[1..].iter().rev() {
            rows.swap(cycle[0], other);
        }
    }
}

/// The directories the rows live in, each with the names the scan found in it,
/// handed out as a small id.
///
/// Every check asks whether a name is taken *within one directory* and never by
/// the name alone, and each row's parent is asked for by every pass:
/// interning them once means the checks hash a `u32` and look a snapshot up by
/// id instead of hashing and rebuilding paths per row.
struct Directories<'a> {
    index: &'a DirIndex,
    ids: HashMap<PathBuf, u32>,
    snapshots: Vec<Option<&'a BTreeSet<OsString>>>,
    /// The directory looked up last. A scan hands the rows over a directory at a
    /// time, so this answers almost every row without hashing its path.
    last: Option<(PathBuf, u32)>,
}

impl<'a> Directories<'a> {
    fn new(index: &'a DirIndex) -> Self {
        Directories { index, ids: HashMap::new(), snapshots: Vec::new(), last: None }
    }

    /// This directory's id, interning it on first sight. Ids are handed out in
    /// insertion order, so one is also an index into `snapshots`.
    fn id(&mut self, dir: &Path) -> u32 {
        if let Some((last, id)) = &self.last
            && last.as_path() == dir
        {
            return *id;
        }
        let id = match self.ids.get(dir) {
            Some(&id) => id,
            None => {
                let id = self.snapshots.len() as u32;
                self.ids.insert(dir.to_path_buf(), id);
                self.snapshots.push(self.index.names(dir));
                id
            }
        };
        self.last = Some((dir.to_path_buf(), id));
        id
    }

    fn names(&self, id: u32) -> Option<&BTreeSet<OsString>> {
        self.snapshots[id as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::plan;
    use crate::rule::{InsertRemoveOp, Rule, RuleSet, Target};
    use crate::scan::scan_dir;
    use std::path::Path;

    fn plan_dir(dir: &Path, rules: Vec<Rule>) -> Vec<PlanRow> {
        let (entries, index) = scan_dir(dir).expect("scan");
        plan(&entries, &index, &RuleSet { rules }).expect("plan").rows
    }

    fn plan_tree_dir(dir: &Path, rules: Vec<Rule>) -> Vec<PlanRow> {
        let (entries, index) = crate::scan::scan_tree(dir).expect("scan");
        plan(&entries, &index, &RuleSet { rules }).expect("plan").rows
    }

    fn write(dir: &Path, names: &[&str]) {
        for name in names {
            std::fs::write(dir.join(name), b"x").expect("write fixture");
        }
    }

    fn order_of(rows: &[PlanRow]) -> Vec<String> {
        rows.iter().map(|row| file_name_of(&row.src).to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn a_chain_orders_the_blocked_row_last() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), &["a", "ax"]);

        let rows = plan_dir(
            temp.path(),
            vec![Rule::InsertRemove {
                op: InsertRemoveOp::AddSuffix(b"x".to_vec()),
                target: Target::Whole,
            }],
        );

        // a -> ax needs `ax` gone, and ax -> axx is what vacates it.
        assert_eq!(order_of(&rows), vec!["ax".to_string(), "a".to_string()]);
    }

    #[test]
    fn a_cycle_becomes_one_contiguous_temp_run() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), &["ab", "ba"]);

        let rows = plan_dir(
            temp.path(),
            vec![Rule::Regex {
                pattern: "^(.+)(.)$".into(),
                replacement: "${2}${1}".into(),
                target: Target::Whole,
                path: false,
            }],
        );

        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row.via_temp), "every cycle member is staged");
    }

    #[test]
    fn independent_rows_are_not_staged_through_a_temp_name() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), &["a", "b"]);

        let rows = plan_dir(
            temp.path(),
            vec![Rule::InsertRemove {
                op: InsertRemoveOp::AddPrefix(b"p-".to_vec()),
                target: Target::Whole,
            }],
        );

        assert!(rows.iter().all(|row| !row.via_temp));
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn rows_apply_never_touches_are_kept_in_scan_order() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), &["a", "z"]);

        let (entries, index) = scan_dir(temp.path()).expect("scan");
        let rows = plan(&entries, &index, &RuleSet { rules: vec![] }).expect("plan").rows;

        assert!(rows.iter().all(|row| row.status == Status::Unchanged));
        assert_eq!(order_of(&rows).len(), entries.len());
    }

    #[test]
    fn a_directory_is_ordered_after_the_rows_inside_it() {
        let temp = tempfile::tempdir().unwrap();
        let nested = temp.path().join("d");
        std::fs::create_dir(&nested).unwrap();
        write(&nested, &["f"]);

        let rows = plan_tree_dir(
            temp.path(),
            vec![Rule::InsertRemove {
                op: InsertRemoveOp::AddPrefix(b"p-".to_vec()),
                target: Target::Whole,
            }],
        );

        let at = |relative: &str| {
            rows.iter()
                .position(|row| row.src.strip_prefix(temp.path()).unwrap() == Path::new(relative))
                .unwrap_or_else(|| panic!("{relative}"))
        };
        assert!(at("d/f") < at("d"), "the child is renamed while its path still resolves");
    }

    #[test]
    fn two_neighbouring_cycles_stay_separate_staged_runs() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), &["ab", "ba", "cd", "dc"]);

        let rows = plan_dir(
            temp.path(),
            vec![Rule::Regex {
                pattern: "^(.+)(.)$".into(),
                replacement: "${2}${1}".into(),
                target: Target::Whole,
                path: false,
            }],
        );

        assert_eq!(rows.len(), 4);
        assert!(rows.iter().all(|row| row.via_temp));
        // Emitted as cycles, not as one four-name chain: each row is the other
        // member's immediate neighbour.
        let at = |name: &str| {
            rows.iter()
                .position(|row| file_name_of(&row.src) == OsStr::new(name))
                .unwrap_or_else(|| panic!("{name}"))
        };
        assert_eq!((at("ab") as i64 - at("ba") as i64).abs(), 1);
        assert_eq!((at("cd") as i64 - at("dc") as i64).abs(), 1);
    }

    #[test]
    fn a_permutation_is_applied_without_losing_or_duplicating_a_row() {
        let names = |order: &[usize]| {
            let mut rows: Vec<PlanRow> = (0..order.len())
                .map(|at| PlanRow {
                    src: PathBuf::from(format!("/r/{at}")),
                    dst: PathBuf::from(format!("/r/d{at}")),
                    status: Status::Ok,
                    via_temp: false,
                })
                .collect();
            reorder(&mut rows, order);
            order_of(&rows)
        };

        assert_eq!(names(&[0, 1, 2]), vec!["0", "1", "2"]);
        assert_eq!(names(&[1, 0]), vec!["1", "0"]);
        assert_eq!(names(&[2, 0, 1]), vec!["2", "0", "1"]);
        assert_eq!(names(&[2, 3, 0, 1]), vec!["2", "3", "0", "1"]);
    }

    #[test]
    fn a_duplicated_name_does_not_make_a_case_only_collision() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), &["a.txt", "b.txt", "c.txt"]);

        // Two rows reach `Same.txt` and a third reaches `same.txt`: the first
        // two are duplicate targets, and the third is left alone.
        let rows = plan_dir(
            temp.path(),
            vec![
                Rule::Literal {
                    find: b"a".to_vec(),
                    replace: b"Same".to_vec(),
                    target: Target::Stem,
                },
                Rule::Literal {
                    find: b"b".to_vec(),
                    replace: b"Same".to_vec(),
                    target: Target::Stem,
                },
                Rule::Literal {
                    find: b"c".to_vec(),
                    replace: b"same".to_vec(),
                    target: Target::Stem,
                },
            ],
        );

        let status_of = |name: &str| {
            rows.iter().find(|row| file_name_of(&row.src) == OsStr::new(name)).unwrap().status
        };
        assert_eq!(status_of("a.txt"), Status::Conflict(ConflictKind::DuplicateTarget));
        assert_eq!(status_of("b.txt"), Status::Conflict(ConflictKind::DuplicateTarget));
        assert_eq!(status_of("c.txt"), Status::Ok);
    }

    #[test]
    fn a_row_waiting_on_a_conflicted_vacator_conflicts_too() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), &["a", "b", "c", "ax"]);

        // a -> ax, b -> a, c -> b. `ax` is on disk and stays, so a cannot
        // vacate its own name; nothing else moves out of the way either, so all
        // three conflicts, not just the first. `apply` would otherwise try
        // b -> a against a file that never moved.
        let rows = plan_dir(
            temp.path(),
            vec![
                Rule::Regex {
                    pattern: "^a$".into(),
                    replacement: "ax".into(),
                    target: Target::Whole,
                    path: false,
                },
                Rule::Regex {
                    pattern: "^b$".into(),
                    replacement: "a".into(),
                    target: Target::Whole,
                    path: false,
                },
                Rule::Regex {
                    pattern: "^c$".into(),
                    replacement: "b".into(),
                    target: Target::Whole,
                    path: false,
                },
            ],
        );

        let status_of = |name: &str| {
            rows.iter().find(|row| file_name_of(&row.src) == OsStr::new(name)).unwrap().status
        };
        assert_eq!(status_of("a"), Status::Conflict(ConflictKind::TargetExists));
        assert_eq!(status_of("b"), Status::Conflict(ConflictKind::TargetExists));
        assert_eq!(status_of("c"), Status::Conflict(ConflictKind::TargetExists));
        assert_eq!(status_of("ax"), Status::Unchanged);
    }
}
