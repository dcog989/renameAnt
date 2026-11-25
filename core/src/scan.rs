//! Reads one directory's children — or a whole tree — and snapshots the names
//! each directory holds.
//!
//! Symlinks are recorded as symlinks and never followed inside a scan: no path
//! here is canonicalised and no symlinked directory below a picked root is
//! descended into. A picked root that is itself a symlink to a directory is
//! followed once, because that is the folder the user asked to scan.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use crate::apply::is_temp_name;
use crate::osname;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    File,
    Dir,
    Symlink,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    pub name: OsString,
    pub kind: Kind,
}

/// The name snapshot taken by the scan, keyed by the directory that holds them.
/// `plan()` answers every existence question from this, which is what keeps it
/// syscall-free.
///
/// One entry per scanned directory: a recursive walk fills one per directory it
/// visited. A directory that was never scanned has no entry, which reads as a
/// directory holding no names.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DirIndex {
    /// Write through `insert`, not directly: the folded view is built with it.
    existing: BTreeMap<PathBuf, BTreeSet<OsString>>,
    /// The same names folded for a case-insensitive comparison, built with
    /// `existing` so a plan does not rebuild them.
    folded: BTreeMap<PathBuf, BTreeSet<Vec<u8>>>,
}

impl DirIndex {
    /// The names `parent` held when the scan visited it.
    pub fn names(&self, parent: &Path) -> Option<&BTreeSet<OsString>> {
        self.existing.get(parent)
    }

    /// The names `parent` held, folded for a case-insensitive comparison.
    pub(crate) fn folded_names(&self, parent: &Path) -> Option<&BTreeSet<Vec<u8>>> {
        self.folded.get(parent)
    }

    /// Records one directory's names in both the raw and the folded view.
    pub fn insert(&mut self, parent: PathBuf, names: BTreeSet<OsString>) {
        let folded = names.iter().map(|name| osname::case_fold(name.as_bytes())).collect();
        self.folded.insert(parent.clone(), folded);
        self.existing.insert(parent, names);
    }
}

/// An entry cap, so one wrong pick cannot keep the single worker busy for
/// minutes. A scan that hits it is incomplete, and the window refuses to apply
/// it.
const MAX_SCAN_ENTRIES: usize = 250_000;

/// What a picked-path scan found.
#[derive(Debug, Default)]
pub struct Scan {
    pub entries: Vec<Entry>,
    pub index: DirIndex,
    /// Directories the walk could not read, including a picked root. One found
    /// inside a scan still has its name in the parent's index, so a target on
    /// that name conflicts.
    pub unreadable: Vec<PathBuf>,
    /// The walk stopped at the entry cap, so the plan is incomplete.
    pub truncated: bool,
}

/// Collects one directory's immediate children. Directory order is the
/// filesystem's; nothing downstream may depend on it.
///
/// An unreadable subdirectory is skipped; only `dir` itself is an error.
pub fn scan_dir(dir: &Path) -> io::Result<(Vec<Entry>, DirIndex)> {
    scan_at(dir, false)
}

/// Collects `dir` and everything below it, depth first. A symlinked directory
/// below `dir` is recorded and never descended into; `dir` itself is read even
/// when it is a link, since that is the folder the caller asked for.
pub fn scan_tree(dir: &Path) -> io::Result<(Vec<Entry>, DirIndex)> {
    scan_at(dir, true)
}

/// Scans what the user picked: a directory contributes its children — or the
/// whole tree — and a plain file contributes only itself.
///
/// A picked file still has its directory's names indexed, so a destination that
/// is already taken by a file the user never picked reads as taken rather than
/// as free. A path that is gone since it was picked is skipped: an apply has
/// every right to have moved it. A picked directory that is a symlink is
/// followed once; links below it are still never followed.
///
/// An unreadable directory below a picked root, including the root itself, is
/// skipped and listed in [`Scan::unreadable`]. An error is returned only when a
/// picked path cannot be inspected at all.
pub fn scan_paths(paths: &[PathBuf], recursive: bool) -> io::Result<Scan> {
    let mut scan = Scan::default();
    let mut picked: Vec<(PathBuf, Kind)> = Vec::new();
    let mut dirs: Vec<PathBuf> = Vec::new();

    for path in paths {
        let link = match fs::symlink_metadata(path) {
            Ok(meta) => meta,
            // A path that vanished since it was picked is not an error.
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(with_path(error, path)),
        };
        // Follow a link only for the picked root: that is the folder the user
        // asked for. Everything below is read without following links.
        if fs::metadata(path).is_ok_and(|meta| meta.is_dir()) {
            if !dirs.contains(path) {
                dirs.push(path.clone());
            }
        } else {
            let kind = if link.is_symlink() { Kind::Symlink } else { Kind::File };
            picked.push((path.clone(), kind));
        }
    }

    for dir in &dirs {
        // A recursive walk of the parent already covers a nested pick.
        if recursive && dirs.iter().any(|parent| parent != dir && dir.starts_with(parent.as_path()))
        {
            continue;
        }
        if walk(dir, recursive, &mut scan, MAX_SCAN_ENTRIES, true).is_err() {
            scan.unreadable.push(dir.clone());
        }
    }

    let parents: Vec<&Path> = {
        let mut parents: BTreeSet<&Path> = BTreeSet::new();
        for (path, _) in &picked {
            if let Some(parent) = path.parent() {
                parents.insert(parent);
            }
        }
        parents.into_iter().collect()
    };
    for parent in parents {
        // A picked folder, or an ancestor of one, already indexed this.
        if scan.index.existing.contains_key(parent) {
            continue;
        }
        // A file can be renamed without listing its directory; the missing
        // names only make the preview less certain, and `apply` still refuses
        // to overwrite. Only the names are indexed here, never a row.
        if walk(parent, false, &mut scan, MAX_SCAN_ENTRIES, false).is_err() {
            scan.unreadable.push(parent.to_path_buf());
        }
    }

    let mut seen: HashSet<PathBuf> = scan.entries.iter().map(|entry| entry.path.clone()).collect();
    for (path, kind) in picked {
        let name = path.file_name().unwrap_or(path.as_os_str()).to_os_string();
        if seen.insert(path.clone()) {
            scan.entries.push(Entry { path, name, kind });
        }
    }
    scan.unreadable.sort();
    scan.unreadable.dedup();
    Ok(scan)
}

/// An error that names the path it came from, which a bare `io::Error` does not.
fn with_path(error: io::Error, path: &Path) -> io::Error {
    io::Error::new(error.kind(), format!("{}: {error}", path.display()))
}

fn scan_at(dir: &Path, recurse: bool) -> io::Result<(Vec<Entry>, DirIndex)> {
    let mut scan = Scan::default();
    // These helpers have no way to report a cut walk, so they stay uncapped;
    // the window scans through `scan_paths`.
    walk(dir, recurse, &mut scan, usize::MAX, true)?;
    Ok((scan.entries, scan.index))
}

/// One directory: emit its children, snapshot their names, then descend into
/// the real subdirectories. A subdirectory is scanned after its own entry is
/// emitted, so every directory appears before its contents.
///
/// A picked file's directory is walked with `rows` false: its names are needed
/// for conflicts, but its other children are not rows.
///
/// An error below `dir` is returned to the caller, which records the directory
/// as unreadable and carries on with its siblings.
///
/// shortcut: one stack frame per directory and no visited set, so a
/// pathologically deep tree, or a bind mount of an ancestor inside the tree,
/// walks until the kernel rejects the path length; add an explicit worklist and
/// a device/inode cycle check if that stops being hypothetical.
fn walk(dir: &Path, recurse: bool, scan: &mut Scan, limit: usize, rows: bool) -> io::Result<()> {
    let mut names = BTreeSet::new();
    let mut subdirs: Vec<PathBuf> = Vec::new();
    let mut failed = None;

    for dirent in fs::read_dir(dir)? {
        if rows && scan.entries.len() >= limit {
            scan.truncated = true;
            break;
        }
        let dirent = match dirent {
            Ok(dirent) => dirent,
            Err(error) => {
                failed = Some(error);
                break;
            }
        };
        let name = dirent.file_name();
        let path = dir.join(&name);
        let kind = match classify(&dirent) {
            Ok(kind) => kind,
            Err(error) => {
                failed = Some(error);
                break;
            }
        };
        names.insert(name.clone());
        // A name left behind by an interrupted staged run is the app's own and
        // must not become a row; it stays in the index so a destination on it
        // still reads as taken. Its contents are not scanned either.
        let temp = is_temp_name(&name);
        if rows && !temp {
            scan.entries.push(Entry { path: path.clone(), name, kind });
        }
        if recurse && kind == Kind::Dir && !temp {
            subdirs.push(path);
        }
    }
    // Keep the names that were read, so a row against one of them still sees it.
    scan.index.insert(dir.to_path_buf(), names);
    if let Some(error) = failed {
        return Err(error);
    }

    for subdir in subdirs {
        if scan.truncated {
            break;
        }
        if walk(&subdir, recurse, scan, limit, rows).is_err() {
            scan.unreadable.push(subdir);
        }
    }
    Ok(())
}

/// `DirEntry::file_type` reports the link itself, and costs a `stat` only where
/// `readdir` said `DT_UNKNOWN` — at scan time, never per frame.
fn classify(dirent: &fs::DirEntry) -> io::Result<Kind> {
    let file_type = dirent.file_type()?;
    Ok(if file_type.is_symlink() {
        Kind::Symlink
    } else if file_type.is_dir() {
        Kind::Dir
    } else {
        Kind::File
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_records_kinds_without_following_links() {
        let temp = tempfile::tempdir().expect("temp dir");
        let root = temp.path();
        fs::write(root.join("plain.txt"), b"x").unwrap();
        fs::create_dir(root.join("nested")).unwrap();
        std::os::unix::fs::symlink(root.join("nested"), root.join("link")).unwrap();

        let (entries, index) = scan_dir(root).expect("scan");

        let kind_of =
            |name: &str| entries.iter().find(|e| e.name == name).map(|e| e.kind).expect(name);
        assert_eq!(kind_of("plain.txt"), Kind::File);
        assert_eq!(kind_of("nested"), Kind::Dir);
        assert_eq!(kind_of("link"), Kind::Symlink);
        assert_eq!(index.names(root).expect("root names").len(), 3);
        assert!(index.names(root).unwrap().contains(&OsString::from("plain.txt")));
    }

    #[test]
    fn a_picked_file_is_one_row_and_its_siblings_stay_indexed() {
        let temp = tempfile::tempdir().expect("temp dir");
        let root = temp.path();
        fs::write(root.join("a.txt"), b"x").unwrap();
        fs::write(root.join("b.txt"), b"x").unwrap();

        let scan = scan_paths(&[root.join("a.txt")], false).expect("scan");

        assert_eq!(scan.entries.len(), 1);
        assert_eq!(scan.entries[0].name, OsString::from("a.txt"));
        // b.txt is not a row, but it is still on disk, so `plan` has to see it.
        assert!(scan.index.names(root).unwrap().contains(&OsString::from("b.txt")));
    }

    #[test]
    fn a_folder_and_a_file_below_it_are_scanned_once() {
        let temp = tempfile::tempdir().expect("temp dir");
        let root = temp.path();
        let nested = root.join("nested");
        fs::create_dir(&nested).unwrap();
        fs::write(root.join("top.txt"), b"x").unwrap();
        fs::write(nested.join("mid.txt"), b"x").unwrap();

        let scan = scan_paths(&[root.to_path_buf(), nested.join("mid.txt")], true).expect("scan");

        let mut names: Vec<String> =
            scan.entries.iter().map(|entry| entry.name.to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names, vec!["mid.txt", "nested", "top.txt"]);
        assert!(scan.index.names(&nested).unwrap().contains(&OsString::from("mid.txt")));
    }

    #[test]
    fn a_folder_and_a_folder_below_it_add_one_row_each() {
        let temp = tempfile::tempdir().expect("temp dir");
        let root = temp.path();
        let nested = root.join("nested");
        fs::create_dir(&nested).unwrap();
        fs::write(root.join("top.txt"), b"x").unwrap();
        fs::write(nested.join("mid.txt"), b"x").unwrap();

        let scan = scan_paths(&[root.to_path_buf(), nested.clone()], true).expect("scan");

        let mut names: Vec<String> =
            scan.entries.iter().map(|entry| entry.name.to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names, vec!["mid.txt", "nested", "top.txt"]);
        assert!(scan.index.names(&nested).unwrap().contains(&OsString::from("mid.txt")));

        // The nested pick is covered by its parent's walk.
        let scan = scan_paths(&[nested.clone(), root.to_path_buf()], true).expect("scan");
        let mut names: Vec<String> =
            scan.entries.iter().map(|entry| entry.name.to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names, vec!["mid.txt", "nested", "top.txt"]);
    }

    #[test]
    fn a_picked_path_that_is_gone_is_skipped() {
        let temp = tempfile::tempdir().expect("temp dir");

        let scan = scan_paths(&[temp.path().join("gone.txt")], false).expect("scan");

        assert!(scan.entries.is_empty());
        assert!(scan.index.existing.is_empty());
        assert!(scan.unreadable.is_empty());
    }

    #[test]
    fn a_picked_symlinked_folder_is_scanned_once() {
        let temp = tempfile::tempdir().expect("temp dir");
        let real = temp.path().join("real");
        fs::create_dir(&real).unwrap();
        fs::write(real.join("photo.jpg"), b"x").unwrap();
        let link = temp.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let scan = scan_paths(std::slice::from_ref(&link), true).expect("scan");

        assert_eq!(scan.entries.len(), 1);
        assert_eq!(scan.entries[0].name, OsString::from("photo.jpg"));
        assert_eq!(scan.entries[0].path, link.join("photo.jpg"));
        assert!(scan.index.names(&link).unwrap().contains(&OsString::from("photo.jpg")));
    }

    #[test]
    fn a_picked_broken_link_is_one_symlink_row() {
        let temp = tempfile::tempdir().expect("temp dir");
        let broken = temp.path().join("broken");
        std::os::unix::fs::symlink(temp.path().join("missing"), &broken).unwrap();

        let scan = scan_paths(std::slice::from_ref(&broken), false).expect("scan");

        assert_eq!(scan.entries.len(), 1);
        assert_eq!(scan.entries[0].path, broken);
        assert_eq!(scan.entries[0].kind, Kind::Symlink);
    }

    #[test]
    fn an_unreadable_subdirectory_is_skipped_and_reported() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("temp dir");
        let locked = temp.path().join("locked");
        fs::create_dir(&locked).unwrap();
        fs::write(locked.join("hidden.txt"), b"x").unwrap();
        fs::write(temp.path().join("seen.txt"), b"x").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

        // A root user can still list it, and then the test proves nothing.
        if fs::read_dir(&locked).is_ok() {
            fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
            return;
        }

        let scan = scan_paths(&[temp.path().to_path_buf()], true).expect("scan");
        // A picked root that cannot be read is reported, not fatal, so other
        // picks still scan.
        let root_scan = scan_paths(std::slice::from_ref(&locked), true).expect("scan");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();

        assert_eq!(scan.unreadable, vec![locked.clone()]);
        let names: Vec<OsString> = scan.entries.iter().map(|entry| entry.name.clone()).collect();
        assert!(names.contains(&OsString::from("seen.txt")));
        assert!(names.contains(&OsString::from("locked")));
        assert!(!names.contains(&OsString::from("hidden.txt")));
        assert!(!scan.truncated);
        assert!(root_scan.entries.is_empty());
        assert_eq!(root_scan.unreadable, vec![locked]);
    }

    #[test]
    fn a_generated_temp_name_is_not_a_row_but_stays_taken() {
        let temp = tempfile::tempdir().expect("temp dir");
        let root = temp.path();
        fs::write(root.join(".renameant-tmp-1-0-0"), b"x").unwrap();
        fs::write(root.join("keep"), b"x").unwrap();

        let (entries, index) = scan_dir(root).expect("scan");

        assert!(!entries.iter().any(|entry| entry.name == ".renameant-tmp-1-0-0"));
        assert!(entries.iter().any(|entry| entry.name == "keep"));
        assert!(
            index.names(root).expect("index").contains(&OsString::from(".renameant-tmp-1-0-0")),
            "a destination on the temp name still reads as taken"
        );
    }

    #[test]
    fn the_walk_stops_at_its_entry_limit() {
        let temp = tempfile::tempdir().expect("temp dir");
        for name in ["a", "b", "c"] {
            fs::write(temp.path().join(name), b"x").unwrap();
        }

        let mut scan = Scan::default();
        walk(temp.path(), false, &mut scan, 2, true).expect("walk");

        assert!(scan.truncated);
        assert_eq!(scan.entries.len(), 2);
    }

    #[test]
    fn a_recursive_scan_indexes_every_directory_and_follows_no_link() {
        let temp = tempfile::tempdir().expect("temp dir");
        let root = temp.path();
        let nested = root.join("nested");
        let deeper = nested.join("deeper");
        fs::create_dir_all(&deeper).unwrap();
        fs::write(root.join("top.txt"), b"x").unwrap();
        fs::write(nested.join("mid.txt"), b"x").unwrap();
        fs::write(deeper.join("low.txt"), b"x").unwrap();
        // A link back to the root would walk forever if it were followed.
        std::os::unix::fs::symlink(root, root.join("loop")).unwrap();

        let (entries, index) = scan_tree(root).expect("scan");

        let relative =
            |entry: &Entry| entry.path.strip_prefix(root).unwrap().to_string_lossy().into_owned();
        let mut names: Vec<String> = entries.iter().map(relative).collect();
        let at = |name: &str| names.iter().position(|found| found == name).expect(name);
        // A directory is emitted before the children inside it, so the walk is
        // depth first and a parent's row can never follow its own child's.
        assert!(at("nested") < at("nested/mid.txt"));
        assert!(at("nested/deeper") < at("nested/deeper/low.txt"));

        names.sort();
        assert_eq!(
            names,
            vec![
                "loop",
                "nested",
                "nested/deeper",
                "nested/deeper/low.txt",
                "nested/mid.txt",
                "top.txt",
            ]
        );
        assert_eq!(index.existing.len(), 3, "the linked directory is never visited");
        assert!(index.names(&deeper).unwrap().contains(&OsString::from("low.txt")));
        assert!(index.names(&nested).unwrap().contains(&OsString::from("deeper")));
        assert!(index.names(root).unwrap().contains(&OsString::from("top.txt")));
    }
}
