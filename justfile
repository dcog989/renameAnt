# Verify the repo: clippy, formatting check and tests.
default: check

# Lint, format check and tests.
check: lint format-check test

# Clippy, warnings as errors.
lint:
    cargo clippy --locked --workspace --all-targets -- -D warnings

# Rewrite formatting.
format:
    cargo fmt --all

# Fail if anything needs formatting.
format-check:
    cargo fmt --all -- --check

# Test suite.
test:
    cargo test --locked --workspace

# Release build of the workspace.
build:
    cargo build --release --locked

# Install the release build for the current user: binary, desktop entry and icon.
install: build
    install -Dm755 target/release/renameant-gui "$HOME/.local/bin/renameant-gui"
    install -Dm644 dist/renameant.desktop "$HOME/.local/share/applications/renameant.desktop"
    rsvg-convert -w 256 -h 256 assets/logo.svg -o "$HOME/.local/share/icons/hicolor/256x256/apps/renameant.png"

# Debug build and run of the GUI.
run:
    cargo run -p renameant-gui

# AppImage into dist/out.
package:
    bash ./dist/build-appimage.sh

# Remove build artifacts.
clean:
    cargo clean

# Refresh dependencies to the latest allowed versions.
update:
    cargo update

# Bump the version from conventional commits, write the changelog and tag it.
release:
    cog bump --auto

# Verify a commit message file, as the commit-msg hook passes it.
commit-check file *args:
    cog verify --file {{file}} {{args}}
