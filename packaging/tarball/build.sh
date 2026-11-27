#!/usr/bin/env bash
# Staging tree and tarball for a manual install without a package manager.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

for tool in rsvg-convert zstd; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "error: $tool not installed (see README Requirements)" >&2
        exit 1
    fi
done

version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
name="renameant-$version-x86_64-linux"
staging="$root/target/tarball/$name"
outdir="$root/target/tarball"

cargo build --release --locked

rm -rf "$staging"
mkdir -p "$staging"

install -m755 target/release/renameant-gui "$staging/renameant-gui"
install -m644 dist/renameant.desktop "$staging/renameant.desktop"
install -m644 assets/logo.svg "$staging/renameant.svg"
rsvg-convert -w 256 -h 256 assets/logo.svg -o "$staging/renameant.png"
install -m644 LICENSE "$staging/LICENSE"
install -m755 packaging/tarball/install.sh "$staging/install.sh"

tar --zstd -C "$outdir" -cf "$outdir/$name.tar.zst" "$name"

echo "tarball written to $outdir/$name.tar.zst"
