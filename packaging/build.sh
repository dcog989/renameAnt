#!/usr/bin/env bash
# Builds the release AppImage, .deb and .rpm. Artifacts land in dist/out and
# target/.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

for tool in cargo-deb cargo-generate-rpm rsvg-convert; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "error: $tool not installed (see README Requirements)" >&2
        exit 1
    fi
done

# The deb and rpm asset lists point at this rendered icon.
mkdir -p target/packaging
rsvg-convert -w 256 -h 256 assets/logo.svg -o target/packaging/renameant-256.png

cargo build --release --locked
cargo deb --no-build -p renameant-gui
cargo generate-rpm -p gui
bash dist/build-appimage.sh
bash packaging/tarball/build.sh

echo "artifacts:"
find dist/out target/debian target/generate-rpm target/tarball -maxdepth 1 \
    \( -name '*.AppImage' -o -name '*.deb' -o -name '*.rpm' -o -name '*.tar.zst' \) \
    ! -name 'linuxdeploy*' 2>/dev/null
