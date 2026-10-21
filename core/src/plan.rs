//! The pure planner.

use std::borrow::Borrow;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::conflict;
use crate::osname;
use crate::rule::{RuleError, RuleSet};
use crate::scan::{DirIndex, Entry};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Will be applied.
    Ok,
    /// The pipeline produced the name the entry already has.
    Unchanged,
    /// Would be applied but cannot be, so it is never applied.
    Conflict(ConflictKind),
    /// Rejected up front rather than mid-batch.
    Invalid(InvalidKind),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictKind {
    /// Two rows in the same parent want the same destination.
    DuplicateTarget,
    /// The destination is already on disk and no row is moving out of its way.
    TargetExists,
    /// Two rows whose destinations collide under case folding, which is only a
    /// real collision on a case-insensitive mount.
    CaseOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvalidKind {
    Empty,
    IllegalChar,
    NameTooLong,
    PathTooLong,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PlanRow {
    #[serde(with = "crate::bytes_serde::path")]
    pub src: PathBuf,
    #[serde(with = "crate::bytes_serde::path")]
    pub dst: PathBuf,
    pub status: Status,
    /// Apply must stage `src` under a generated name before renaming it: a
    /// rename cycle, or a case-only rename.
    pub via_temp: bool,
}

/// Rows in apply order, not scan or view order.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Plan {
    pub rows: Vec<PlanRow>,
}

impl Plan {
    /// Rows apply will act on: `N` in "Apply N of M".
    pub fn actionable(&self) -> usize {
        self.rows.iter().filter(|row| row.status == Status::Ok).count()
    }

    /// Rows that would change a name but never will.
    pub fn skipped(&self) -> usize {
        self.rows
            .iter()
            .filter(|row| matches!(row.status, Status::Conflict(_) | Status::Invalid(_)))
            .count()
    }

    /// Rows that are not no-ops: `M` in "Apply N of M".
    pub fn candidates(&self) -> usize {
        self.rows.iter().filter(|row| row.status != Status::Unchanged).count()
    }
}

/// Builds a plan from a scan and a rule set. Pure: no syscalls, and every
/// existence question is answered from `index`.
///
/// Takes `Entry` or `&Entry` items, so a caller that only has references — the
/// window's filtered view — does not have to clone them.
pub fn plan<E: Borrow<Entry>>(
    entries: &[E],
    index: &DirIndex,
    rules: &RuleSet,
) -> Result<Plan, RuleError> {
    let mut compiled = rules.compile()?;

    // `Sequence` consumes entries in name order, taken before the pipeline runs
    // and independent of filesystem walk order. In a recursive scan
    // that order is the path relative to the root — for one directory the two
    // are the same. Only `Sequence` reads the result, so a pipeline without one
    // never pays for the sort.
    let rank: Vec<usize> = if compiled.needs_rank() {
        let mut order: Vec<usize> = (0..entries.len()).collect();
        order.sort_by(|&left, &right| {
            entries[left]
                .borrow()
                .path
                .as_os_str()
                .as_bytes()
                .cmp(entries[right].borrow().path.as_os_str().as_bytes())
        });
        let mut rank = vec![0usize; entries.len()];
        for (position, &entry) in order.iter().enumerate() {
            rank[entry] = position;
        }
        rank
    } else {
        Vec::new()
    };

    // Reused across every entry: a pipeline costs one output name per entry and
    // nothing else.
    let mut current: Vec<u8> = Vec::new();
    let mut selection: Vec<u8> = Vec::new();
    let mut spare: Vec<u8> = Vec::new();
    let mut rows = Vec::with_capacity(entries.len());

    for (position, entry) in entries.iter().enumerate() {
        let entry = entry.borrow();
        current.clear();
        current.extend_from_slice(entry.name.as_bytes());
        compiled.apply(
            &mut current,
            rank.get(position).copied().unwrap_or(0),
            &mut selection,
            &mut spare,
        );

        let name = current.as_slice();
        // Rules change the basename only, so the destination keeps its own
        // entry's parent.
        let dst = parent_of(&entry.path).join(OsStr::from_bytes(name));
        let status = if name == entry.name.as_bytes() {
            Status::Unchanged
        } else if let Some(kind) = osname::validate(name, dst.as_os_str().as_bytes().len()) {
            Status::Invalid(kind)
        } else {
            Status::Ok
        };
        rows.push(PlanRow { src: entry.path.clone(), dst, status, via_temp: false });
    }

    conflict::resolve(&mut rows, index);
    Ok(Plan { rows })
}

/// The directory a row lives in. Rules only ever change the basename, so this is
/// the row's own parent, and a recursive scan puts rows in many of them; the
/// conflict checks are written per parent for exactly that reason.
pub(crate) fn parent_of(path: &Path) -> &Path {
    path.parent().unwrap_or_else(|| Path::new(""))
}

pub(crate) fn file_name_of(path: &Path) -> &OsStr {
    path.file_name().unwrap_or(path.as_os_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rule::{CaseStyle, InsertRemoveOp, Rule, SequencePlace, Target};
    use crate::scan::{Kind, scan_dir, scan_tree};
    use std::collections::BTreeSet;
    use std::ffi::OsString;
    use std::fs;

    fn plan_dir(dir: &Path, rules: Vec<Rule>) -> Plan {
        let (entries, index) = scan_dir(dir).expect("scan");
        plan(&entries, &index, &RuleSet { rules }).expect("plan")
    }

    fn plan_tree_dir(dir: &Path, rules: Vec<Rule>) -> Plan {
        let (entries, index) = scan_tree(dir).expect("scan");
        plan(&entries, &index, &RuleSet { rules }).expect("plan")
    }

    fn row_at<'a>(plan: &'a Plan, root: &Path, relative: &str) -> &'a PlanRow {
        let wanted = root.join(relative);
        plan.rows.iter().find(|row| row.src == wanted).unwrap_or_else(|| panic!("{relative}"))
    }

    fn row_for<'a>(plan: &'a Plan, name: &str) -> &'a PlanRow {
        plan.rows.iter().find(|row| row.src.file_name().unwrap() == OsStr::new(name)).expect(name)
    }

    fn status_of(plan: &Plan, name: &str) -> Status {
        row_for(plan, name).status
    }

    fn dst_of<'a>(plan: &'a Plan, name: &str) -> &'a OsStr {
        row_for(plan, name).dst.file_name().unwrap()
    }

    fn position(plan: &Plan, name: &str) -> usize {
        plan.rows
            .iter()
            .position(|row| row.src.file_name().unwrap() == OsStr::new(name))
            .expect(name)
    }

    fn write(dir: &Path, names: &[&str]) {
        for name in names {
            fs::write(dir.join(name), b"x").expect("write fixture");
        }
    }

    #[test]
    fn unchanged_rows_are_marked_not_dropped() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), &["a.txt"]);

        let plan = plan_dir(temp.path(), vec![]);

        assert_eq!(plan.rows.len(), 1);
        assert_eq!(status_of(&plan, "a.txt"), Status::Unchanged);
        assert_eq!(plan.candidates(), 0);
    }

    #[test]
    fn a_chain_is_ordered_so_the_vacated_name_is_free() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), &["a", "ax"]);

        // a -> ax, ax -> axx: `ax` is already on disk but is itself a row, so it
        // is not a conflict and must be applied first.
        let plan = plan_dir(
            temp.path(),
            vec![Rule::InsertRemove {
                op: InsertRemoveOp::AddSuffix(b"x".to_vec()),
                target: Target::Whole,
            }],
        );

        assert_eq!(status_of(&plan, "a"), Status::Ok);
        assert_eq!(dst_of(&plan, "a"), OsStr::new("ax"));
        assert!(position(&plan, "ax") < position(&plan, "a"));
    }

    #[test]
    fn a_cycle_is_ok_and_staged_as_one_temp_run() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), &["ab", "ba"]);

        // Move the last character to the front: ab -> ba and ba -> ab.
        let plan = plan_dir(
            temp.path(),
            vec![Rule::Regex {
                pattern: "^(.+)(.)$".into(),
                replacement: "${2}${1}".into(),
                target: Target::Whole,
            }],
        );

        assert_eq!(status_of(&plan, "ab"), Status::Ok);
        assert_eq!(dst_of(&plan, "ab"), OsStr::new("ba"));
        assert_eq!(plan.actionable(), 2);
        assert_eq!(plan.rows.iter().filter(|row| row.via_temp).count(), 2);
        assert!((position(&plan, "ab") as i64 - position(&plan, "ba") as i64).abs() == 1);
    }

    #[test]
    fn two_rows_wanting_one_destination_both_conflict() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), &["a.txt", "b.txt"]);

        let plan = plan_dir(
            temp.path(),
            vec![Rule::Regex {
                pattern: ".*".into(),
                replacement: "same".into(),
                target: Target::Stem,
            }],
        );

        assert_eq!(status_of(&plan, "a.txt"), Status::Conflict(ConflictKind::DuplicateTarget));
        assert_eq!(status_of(&plan, "b.txt"), Status::Conflict(ConflictKind::DuplicateTarget));
        assert_eq!(plan.actionable(), 0);
        assert_eq!(plan.skipped(), 2);
    }

    #[test]
    fn a_destination_that_exists_and_stays_put_is_a_conflict() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), &["a.txt", "b.txt"]);

        // a.txt -> b.txt, while b.txt keeps its name.
        let plan = plan_dir(
            temp.path(),
            vec![Rule::Literal {
                find: b"a".to_vec(),
                replace: b"b".to_vec(),
                target: Target::Stem,
            }],
        );

        assert_eq!(status_of(&plan, "a.txt"), Status::Conflict(ConflictKind::TargetExists));
        assert_eq!(status_of(&plan, "b.txt"), Status::Unchanged);
    }

    #[test]
    fn case_only_collisions_are_conflicts() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), &["one.txt", "two.txt"]);

        let plan = plan_dir(
            temp.path(),
            vec![
                Rule::Literal {
                    find: b"one".to_vec(),
                    replace: b"Same".to_vec(),
                    target: Target::Stem,
                },
                Rule::Literal {
                    find: b"two".to_vec(),
                    replace: b"same".to_vec(),
                    target: Target::Stem,
                },
            ],
        );

        assert_eq!(status_of(&plan, "one.txt"), Status::Conflict(ConflictKind::CaseOnly));
        assert_eq!(status_of(&plan, "two.txt"), Status::Conflict(ConflictKind::CaseOnly));
    }

    #[test]
    fn a_case_only_rename_is_ok_and_staged_through_a_temp_name() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), &["readme.md"]);

        let plan = plan_dir(
            temp.path(),
            vec![Rule::Case { style: CaseStyle::Upper, target: Target::Stem }],
        );

        assert_eq!(status_of(&plan, "readme.md"), Status::Ok);
        assert!(row_for(&plan, "readme.md").via_temp);
    }

    #[test]
    fn sequence_numbers_entries_in_name_order_not_walk_order() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), &["c.txt", "a.txt", "b.txt"]);

        let plan = plan_dir(
            temp.path(),
            vec![Rule::Sequence {
                start: 1,
                step: 1,
                pad: 2,
                place: SequencePlace::Replace,
                target: Target::Stem,
            }],
        );

        assert_eq!(dst_of(&plan, "a.txt"), OsStr::new("01.txt"));
        assert_eq!(dst_of(&plan, "b.txt"), OsStr::new("02.txt"));
        assert_eq!(dst_of(&plan, "c.txt"), OsStr::new("03.txt"));
    }

    #[test]
    fn an_overlong_name_is_invalid_before_anything_is_attempted() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), &["a.txt"]);

        let plan = plan_dir(
            temp.path(),
            vec![Rule::InsertRemove {
                op: InsertRemoveOp::AddSuffix(vec![b'x'; crate::osname::NAME_MAX]),
                target: Target::Stem,
            }],
        );

        assert_eq!(status_of(&plan, "a.txt"), Status::Invalid(InvalidKind::NameTooLong));
        assert_eq!(plan.actionable(), 0);
        assert_eq!(plan.skipped(), 1);
    }

    #[test]
    fn a_name_made_empty_is_invalid() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), &["a"]);

        let plan = plan_dir(
            temp.path(),
            vec![Rule::InsertRemove {
                op: InsertRemoveOp::RemovePrefix(usize::MAX),
                target: Target::Whole,
            }],
        );

        assert_eq!(status_of(&plan, "a"), Status::Invalid(InvalidKind::Empty));
    }

    #[test]
    fn the_same_name_in_two_directories_is_not_a_collision() {
        let temp = tempfile::tempdir().unwrap();
        for sub in ["one", "two"] {
            let dir = temp.path().join(sub);
            fs::create_dir(&dir).unwrap();
            write(&dir, &["a.txt"]);
        }

        let plan = plan_tree_dir(
            temp.path(),
            vec![Rule::InsertRemove {
                op: InsertRemoveOp::AddPrefix(b"p-".to_vec()),
                target: Target::Whole,
            }],
        );

        // Each row keeps its own directory, so both files rename and nothing is
        // a duplicate target.
        let one = row_at(&plan, temp.path(), "one/a.txt");
        let two = row_at(&plan, temp.path(), "two/a.txt");
        assert_eq!(one.status, Status::Ok);
        assert_eq!(two.status, Status::Ok);
        assert_eq!(one.dst, temp.path().join("one/p-a.txt"));
        assert_eq!(two.dst, temp.path().join("two/p-a.txt"));
        assert_eq!(plan.skipped(), 0);
    }

    #[test]
    fn a_sequence_numbers_the_whole_tree_in_path_order() {
        let temp = tempfile::tempdir().unwrap();
        let sub = temp.path().join("sub");
        fs::create_dir(&sub).unwrap();
        write(temp.path(), &["a.txt"]);
        write(&sub, &["b.txt"]);

        let plan = plan_tree_dir(
            temp.path(),
            vec![Rule::Sequence {
                start: 1,
                step: 1,
                pad: 2,
                place: SequencePlace::Replace,
                target: Target::Stem,
            }],
        );

        // One counter over the tree, ordered by path: a.txt, then `sub`, then
        // the file inside it.
        assert_eq!(dst_of(&plan, "a.txt"), OsStr::new("01.txt"));
        assert_eq!(dst_of(&plan, "sub"), OsStr::new("02"));
        assert_eq!(row_at(&plan, temp.path(), "sub/b.txt").dst, sub.join("03.txt"));
    }

    /// The only claim that needs a number rather than a check: 100k entries,
    /// `plan()` recompute under 50 ms. Pure, so it is timed over synthetic
    /// in-memory entries with no filesystem. Not run by default:
    /// `cargo test -p renameant-core --release -- --ignored --nocapture perf`
    #[test]
    #[ignore = "a measurement, not a check"]
    fn perf_plan_over_100k_entries() {
        const COUNT: usize = 100_000;
        let root = Path::new("/perf");

        let mut entries = Vec::with_capacity(COUNT);
        let mut names = BTreeSet::new();
        for index in 0..COUNT {
            let name = OsString::from(format!("IMG_{index:06}.txt"));
            names.insert(name.clone());
            entries.push(Entry { path: root.join(&name), name, kind: Kind::File });
        }
        let mut index = DirIndex::default();
        index.existing.insert(root.to_path_buf(), names);

        let pipelines: [(&str, Vec<Rule>); 6] = [
            ("empty", vec![]),
            (
                "literal",
                vec![Rule::Literal {
                    find: b"IMG_".to_vec(),
                    replace: b"".to_vec(),
                    target: Target::Stem,
                }],
            ),
            ("case", vec![Rule::Case { style: CaseStyle::Lower, target: Target::Stem }]),
            (
                "sequence",
                vec![Rule::Sequence {
                    start: 1,
                    step: 1,
                    pad: 6,
                    place: SequencePlace::Replace,
                    target: Target::Stem,
                }],
            ),
            (
                "regex",
                vec![Rule::Regex {
                    pattern: r"^IMG_(\d+)\.txt$".into(),
                    replacement: "photo-$1.jpg".into(),
                    target: Target::Whole,
                }],
            ),
            (
                "literal+case+sequence",
                vec![
                    Rule::Literal {
                        find: b"IMG_".to_vec(),
                        replace: b"".to_vec(),
                        target: Target::Stem,
                    },
                    Rule::Case { style: CaseStyle::Lower, target: Target::Stem },
                    Rule::Sequence {
                        start: 1,
                        step: 1,
                        pad: 6,
                        place: SequencePlace::Replace,
                        target: Target::Stem,
                    },
                ],
            ),
        ];

        for (label, rules) in pipelines {
            let rules = RuleSet { rules };
            let started = std::time::Instant::now();
            let plan = plan(&entries, &index, &rules).expect("plan");
            let elapsed = started.elapsed();
            println!(
                "{label}: {COUNT} entries -> {} rows, {} actionable, {elapsed:?}",
                plan.rows.len(),
                plan.actionable()
            );
        }
    }
}
