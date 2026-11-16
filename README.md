# renameAnt

Bulk renamer for files and directories on Linux. An ordered rule pipeline produces a before/after preview. Nothing is written until you apply, existing targets are never overwritten, and every run is undoable from an append-only journal.

## Install

[Download the AppImage](https://github.com/Bubba/renameAnt/releases/latest/), then:

1. Put it on your PATH:

```sh
chmod +x renameAnt-*.AppImage
mkdir -p ~/.local/bin
mv renameAnt-*.AppImage ~/.local/bin/renameant.AppImage
```

2. Add its icon and menu entry:

```sh
APP=~/.local/bin/renameant.AppImage
mkdir -p ~/.local/share/icons ~/.local/share/applications
"$APP" --appimage-extract usr/share/icons/hicolor/512x512/apps/renameant.png
mv squashfs-root/usr/share/icons/hicolor/512x512/apps/renameant.png ~/.local/share/icons/
rm -rf squashfs-root
cat > ~/.local/share/applications/renameant.desktop <<EOF
[Desktop Entry]
Type=Application
Name=renameAnt
Comment=Bulk file renamer
Exec=$HOME/.local/bin/renameant.AppImage
Icon=$HOME/.local/share/icons/renameant.png
Categories=Utility;FileTools;
Terminal=false
EOF
```

The window opens when you run `renameant.AppImage` or pick renameAnt from the launcher.

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
just run                               # GUI from source
just check                             # clippy, formatting check, tests
just build                             # target/release/renameant-gui
just package                           # prints dist/out/renameAnt-<version>-x86_64.AppImage
```

## License

[GNU GPL v3](LICENSE)
