#!/usr/bin/env bash
# Builds the renameAnt AppImage. Output lands in dist/out/.
#
#   dist/build-appimage.sh
#
# Prerequisites, none of which the repo vendors:
#   rsvg-convert                       # librsvg, to render the icon
# Network access is needed once to fetch linuxdeploy; it fetches appimagetool itself.
# If the linuxdeploy AppImage cannot mount (no FUSE), append --appimage-extract-and-run.
#
# The binary links the host's glibc, so the AppImage runs only on glibc at least as
# new as the build host's. GL/EGL and the graphics drivers are never bundled: they
# come from the host. What is bundled beyond ldd's closure is CLIENT_LIBS below.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST="$ROOT/dist"
OUT="$DIST/out"
APPDIR="$OUT/AppDir"
DESKTOP="$APPDIR/usr/share/applications/renameant.desktop"
ICON="$APPDIR/usr/share/icons/hicolor/512x512/apps/renameant.png"
BUILD="$ROOT/target/release"
ARCH=x86_64
export ARCH

# The Wayland/X11 client libraries are bundled. winit dlopens them
# (wayland-dlopen, x11-dl), so they never appear in ldd output and linuxdeploy
# would skip them; they are copied by hand below. libX11/libwayland-client sit on
# the AppImage excludelist, so bundling them is deliberate — revisit if a
# host turns up where the bundled copy breaks instead of helps.
CLIENT_LIBS=(
    libwayland-client.so.0
    libwayland-cursor.so.0
    libwayland-egl.so.1
    libxkbcommon.so.0
    libxkbcommon-x11.so.0
    libX11.so.6
    libX11-xcb.so.1
    libxcb.so.1
    libXcursor.so.1
    libXrandr.so.2
    libXi.so.6
    libXext.so.6
    libXrender.so.1
    libXau.so.6
    libXdmcp.so.6
)

die() { echo "build-appimage: $*" >&2; exit 1; }

for tool in cargo rsvg-convert; do
    command -v "$tool" >/dev/null || die "missing $tool (see the header of this script)"
done

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)"
[ -n "$VERSION" ] || die "could not read version from Cargo.toml"
export VERSION

echo "== build"
cargo build --release --locked --manifest-path "$ROOT/Cargo.toml" --target-dir "$ROOT/target" \
    -p renameant-gui

echo "== stage AppDir"
rm -rf "$APPDIR"
install -Dm755 "$BUILD/renameant-gui" "$APPDIR/usr/bin/renameant-gui"
install -Dm644 "$DIST/renameant.desktop" "$DESKTOP"
mkdir -p "$(dirname "$ICON")" "$APPDIR/usr/share/icons/hicolor/256x256/apps"
rsvg-convert -w 512 -h 512 "$ROOT/assets/logo.svg" -o "$ICON"
rsvg-convert -w 256 -h 256 "$ROOT/assets/logo.svg" -o "$APPDIR/usr/share/icons/hicolor/256x256/apps/renameant.png"

echo "== bundle dlopen'd client libraries"
mkdir -p "$APPDIR/usr/lib"
for lib in "${CLIENT_LIBS[@]}"; do
    # `awk … exit` closes the pipe, so ldconfig dies of SIGPIPE; swallow that
    # rather than let pipefail + set -e abort the build on the first library.
    src="$(ldconfig -p 2>/dev/null | awk -v l="$lib" '$1 == l && $2 ~ /x86-64/ { print $NF; exit }' || true)"
    if [ -n "$src" ]; then
        cp -L "$src" "$APPDIR/usr/lib/$lib"
    else
        echo "   warning: $lib not found on this host, not bundled" >&2
    fi
done

echo "== fetch linuxdeploy"
# A numbered release, never `continuous`: that tag is rebuilt in place, so a
# digest pinned against it would go stale within days. GitHub publishes each
# asset's sha256, and the digest below is the x86_64 asset the script checks
# out; move the tag and the digest together when updating.
LINUXDEPLOY_TAG="1-alpha-20251107-1"
LINUXDEPLOY_SHA256="c20cd71e3a4e3b80c3483cef793cda3f4e990aca14014d23c544ca3ce1270b4d"
LINUXDEPLOY="$OUT/linuxdeploy-$ARCH.AppImage"
if [ ! -x "$LINUXDEPLOY" ]; then
    curl -fL --retry 3 -o "$LINUXDEPLOY" \
        "https://github.com/linuxdeploy/linuxdeploy/releases/download/$LINUXDEPLOY_TAG/linuxdeploy-$ARCH.AppImage"
    chmod +x "$LINUXDEPLOY"
fi
# Checked on every build, not only on download: a cached file that is executable
# is otherwise trusted forever, and this binary runs arbitrary code here.
echo "$LINUXDEPLOY_SHA256  $LINUXDEPLOY" | sha256sum -c - >/dev/null ||
    die "linuxdeploy $LINUXDEPLOY_TAG digest mismatch for $ARCH: delete $LINUXDEPLOY and retry"

echo "== deploy and package"
# A version bump leaves the previous release beside the new one.
rm -f "$OUT"/renameAnt-*.AppImage
# linuxdeploy writes the AppImage to the current directory, so run it in OUT.
(
    cd "$OUT"
    "$LINUXDEPLOY" --appdir "$APPDIR" \
        --desktop-file "$DESKTOP" \
        --icon-file "$ICON" \
        --custom-apprun "$DIST/AppRun" \
        --output appimage
)

echo "== done"
find "$OUT" -maxdepth 1 -name '*.AppImage' ! -name 'linuxdeploy*' -printf "$OUT/%f\n"
