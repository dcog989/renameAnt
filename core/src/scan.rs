//! Reads one directory's children — or a whole tree — and snapshots the names
//! each directory holds.
//!
//! Symlinks are recorded as symlinks and never followed: no path here is
//! canonicalised and no symlinked directory is descended into.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

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
    pub existing: BTreeMap<PathBuf, BTreeSet<OsString>>,
}

impl DirIndex {
    /// The names `parent` held when the scan visited it.
    pub fn names(&self, parent: &Path) -> Option<&BTreeSet<OsString>> {
        self.existing.get(parent)
    }
}

/// Collects one directory's immediate children. Directory order is the
/// filesystem's; nothing downstream may depend on it.
pub fn scan_dir(dir: &Path) -> io::Result<(Vec<Entry>, DirIndex)> {
    scan_at(dir, false)
}

/// Collects `dir` and everything below it, depth first. A symlinked directory is
/// recorded and never descended into.
pub fn scan_tree(dir: &Path) -> io::Result<(Vec<Entry>, DirIndex)> {
    scan_at(dir, true)
}

/// Scans what the user picked: a directory contributes its children — or the
/// whole tree — and a plain file contributes only itself.
///
/// A picked file still has its directory's names indexed, so a destination that
/// is already taken by a file the user never picked reads as taken rather than
/// as free. A path that is gone since it was picked is skipped: an apply has
/// every right to have moved it.
pub fn scan_paths(paths: &[PathBuf], recursive: bool) -> io::Result<(Vec<Entry>, DirIndex)> {
    let mut entries: Vec<Entry> = Vec::new();
    let mut index = DirIndex::default();
    let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
    let mut picked: Vec<(PathBuf, Kind)> = Vec::new();

    for path in paths {
        let Ok(meta) = fs::symlink_metadata(path) else { continue };
        if meta.is_dir() {
            let scanned = if recursive { scan_tree(path) } else { scan_dir(path) };
            let (found, found_index) = scanned.map_err(|error| with_path(error, path))?;
            merge_index(&mut index, found_index);
            for entry in found {
                if seen.insert(entry.path.clone()) {
                    entries.push(entry);
                }
            }
        } else {
            let kind = if meta.is_symlink() { Kind::Symlink } else { Kind::File };
            picked.push((path.clone(), kind));
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
        let (_, parent_index) = scan_dir(parent).map_err(|error| with_path(error, parent))?;
        merge_index(&mut index, parent_index);
    }
    for (path, kind) in picked {
        let name = path.file_name().unwrap_or(path.as_os_str()).to_os_string();
        if seen.insert(path.clone()) {
            entries.push(Entry { path, name, kind });
        }
    }
    Ok((entries, index))
}

/// Folds one scan's names into another's: two picked paths can share a
/// directory, and each side only knows the names it saw.
fn merge_index(into: &mut DirIndex, from: DirIndex) {
    for (dir, names) in from.existing {
        into.existing.entry(dir).or_default().extend(names);
    }
}

/// An error that names the path it came from, which a bare `io::Error` does not.
fn with_path(error: io::Error, path: &Path) -> io::Error {
    io::Error::new(error.kind(), format!("{}: {error}", path.display()))
}

fn scan_at(dir: &Path, recurse: bool) -> io::Result<(Vec<Entry>, DirIndex)> {
    let mut entries = Vec::new();
    let index = walk(dir, recurse, &mut entries)?;
    Ok((entries, index))
}

/// One directory: emit its children, snapshot their names, then descend into
/// the real subdirectories. A subdirectory is scanned after its own entry is
/// emitted, so every directory appears before its contents.
///
/// shortcut: one stack frame per directory and no visited set, so a
/// pathologically deep tree, or a bind mount of an ancestor inside the tree,
/// walks until the kernel rejects the path length; add an explicit worklist and
/// a device/inode cycle check if that stops being hypothetical.
fn walk(dir: &Path, recurse: bool, entries: &mut Vec<Entry>) -> io::Result<DirIndex> {
    let mut names = BTreeSet::new();
    let mut subdirs: Vec<PathBuf> = Vec::new();
    let mut index = DirIndex::default();

    for dirent in fs::read_dir(dir)? {
        let dirent = dirent?;
        let name = dirent.file_name();
        let path = dir.join(&name);
        let kind = classify(&dirent)?;
        names.insert(name.clone());
        entries.push(Entry { path: path.clone(), name, kind });
        if recurse && kind == Kind::Dir {
            subdirs.push(path);
        }
    }
    index.existing.insert(dir.to_path_buf(), names);

    for subdir in subdirs {
        let found = walk(&subdir, recurse, entries)?;
        index.existing.extend(found.existing);
    }
    Ok(index)
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

        let (entries, index) = scan_paths(&[root.join("a.txt")], false).expect("scan");

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, OsString::from("a.txt"));
        // b.txt is not a row, but it is still on disk, so `plan` has to see it.
        assert!(index.names(root).unwrap().contains(&OsString::from("b.txt")));
    }

    #[test]
    fn a_folder_and_a_file_below_it_are_scanned_once() {
        let temp = tempfile::tempdir().expect("temp dir");
        let root = temp.path();
        let nested = root.join("nested");
        fs::create_dir(&nested).unwrap();
        fs::write(root.join("top.txt"), b"x").unwrap();
        fs::write(nested.join("mid.txt"), b"x").unwrap();

        let (entries, index) =
            scan_paths(&[root.to_path_buf(), nested.join("mid.txt")], true).expect("scan");

        let mut names: Vec<String> =
            entries.iter().map(|entry| entry.name.to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names, vec!["mid.txt", "nested", "top.txt"]);
        assert!(index.names(&nested).unwrap().contains(&OsString::from("mid.txt")));
    }

    #[test]
    fn a_picked_path_that_is_gone_is_skipped() {
        let temp = tempfile::tempdir().expect("temp dir");

        let (entries, index) = scan_paths(&[temp.path().join("gone.txt")], false).expect("scan");

        assert!(entries.is_empty());
        assert!(index.existing.is_empty());
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
