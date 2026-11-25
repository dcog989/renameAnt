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
