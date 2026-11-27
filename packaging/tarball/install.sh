#!/usr/bin/env sh
# Manual system install from an unpacked release tarball.
set -eu

here="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"

if [ "$(id -u)" -ne 0 ]; then
    echo "error: run with sudo" >&2
    exit 1
fi

install -Dm755 "$here/renameant-gui" /usr/bin/renameant-gui
install -Dm644 "$here/renameant.desktop" /usr/share/applications/renameant.desktop
install -Dm644 "$here/renameant.svg" /usr/share/icons/hicolor/scalable/apps/renameant.svg
install -Dm644 "$here/renameant.png" /usr/share/icons/hicolor/256x256/apps/renameant.png

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database /usr/share/applications
fi
