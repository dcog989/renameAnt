# renameAnt

Linux file / directory bulk renamer GUI. Provides fast and safe renaming with multiple rules, preview and undo.

<!-- screenshots -->

## Features

- **Rules**: default and custom rules
- **Preview**: before/after per row; conflicts, taken names and invalid names are reported before anything is written.
- **Safety**: dry run until confirmed, no overwrite unless opted in, cycles and case-only renames staged through a temp name.
- **Undo**: append-only journal; reverse any run or one row of it, partial runs included. Runs older than 60 days are pruned at startup; the newest run is always kept.
- **Files**: folders or single files, optional subdirectories, kind and name filters, column sort, non-UTF-8 names kept as bytes.
- **Speed**: fast mode defers size and modified reads and batches journal syncs.

### Default Rules

- replace (regex toggle, optional full-path regex)
- remove (n chars from position x to y)
- trim (n chars from the end)
- insert (prefix, suffix, position)
- number (prefix, suffix, length)
- date (prefix, suffix, format (yyyymmdd, ddmmyyyy, mmddyyyy, yyyymmdd-hhmmss))
- change case (upper, lower, title, kebab, camel, pascal, snake, etc.)
- normalize

## Install

[Download the latest release](https://github.com/Bubba/renameAnt/releases/latest):

| Platform | Format   | Install                                                   |
| -------- | -------- | --------------------------------------------------------- |
| Linux    | AppImage | `chmod +x renameAnt-*.AppImage && ./renameAnt-*.AppImage` |

> [!NOTE]
> The AppImage adds itself to the application menu, installs its icon, and repoints the entry when you move or rename it.

## GUI

- **Add Folders / Files…** picks what to rename; each picked path is listed and can be removed. A folder contributes its children, a file only itself.
- **Subdirectories** scans the whole tree instead of one directory's children. Renames run bottom-up, so a child still has a valid path when its parent is renamed.
- **Filter** includes only the kinds you tick — directories, files (a symlink counts as a file) — and optionally a name pattern, plain text or regex. The pattern ignores case unless **case sensitive** is ticked. Filtered-out names stay taken.
- Click a header — Path, New Name, Size or Modified — to sort by that column, and click it again to reverse. **apply order** puts the rows back to the order Apply runs them in.
- **Skip** leaves a row untouched and out of the apply count.
- **Fast mode** skips reading sizes and modification times until you sort by Size or Modified, and batches the journal's sync instead of writing one per rename. A crash mid-run can then leave the last renames of that run without an undo record; the renames themselves are unaffected.
- **Apply** performs the previewed plan; undo lives in the same panel.

## Technical

### Build

Requires Rust 1.99+ and `rsvg-convert`.

```sh
just check          # clippy, formatting check, tests
just build          # target/release/renameant-gui
just package        # prints dist/out/renameAnt-<version>-x86_64.AppImage
just run            # Debug build and run of the GUI.
just update         # Refresh dependencies to the latest allowed versions.
```

## License

[GNU GPL v3](LICENSE)
