# Agent Directives

## Project Specifics

- Name: renameAnt
- Description: Bulk renamer for files and directories on Linux. An ordered rule pipeline produces a preview; renaming is dry-run by default, never overwrites a target, and is undoable from an append-only journal.
- Tech: Rust — pure-logic `core` crate, `eframe`/`egui` GUI. Shipped as a single AppImage (linuxdeploy).

### Key Files

Structure and invariants live in the code and its comments. The layout below is built — check a path before relying on it:

- `core/src/lib.rs` — rules, plan, conflicts, journal, apply; no UI deps
- `gui/src/main.rs` — eframe app
- `dist/` — `.desktop`, icon, AppImage build script

### Workflow

Run from the workspace root:

- Install: `rustup toolchain install stable`
- Dev: `cargo run -p renameant-gui`
- Test: `cargo test --workspace`
- Lint: `cargo clippy --workspace --all-targets -- -D warnings`
- Format: `cargo fmt --all`
- Build: `cargo build --release`; AppImage via the `dist/` script

### Common Patterns

- Add a rule type: add the `Rule` variant and its compiled form in `core/src/rule.rs`, handle it where rules are applied, extend conflict/validation if it can produce a new `Status`, then expose it in the GUI rule editor. `plan()` and `apply()` should need no new code path.
- Keep `core` pure: `plan()` stays syscall-free and answers existence questions from the scan's `DirIndex`. Only `apply()` touches the filesystem.
- Exercise `core` with unit tests over temp dirs, not through the GUI.
- These are examples. Follow only if they apply; otherwise state your approach before implementing.

### File System Access

- Allowed: `/home/bubba/Projects/renameAnt/` and all contained directories + files; `/tmp/*`, prefer `/tmp/opencode/`
- Read-Only: `.env*`, `.git/`, `.docs/ToDo.md`
- Disallowed: everything not listed in 'Allowed' unless user grants permission.
- Require confirmation: adding/removing dependencies (including any new crate in a `Cargo.toml`), any operation outside project root
- Do not delete files or make destructive changes without permission / confirmation.

### Verification

- Do not run test, lint, format, or type-check commands; the user builds, tests, and lints manually.
- Run them only when the user explicitly asks.

---

## General Guidelines

### Code Changes

- For non-trivial work, propose an approach and confirm before implementing.
- Keep modifications minimal and scoped; prefer incremental improvements over rewrites. Ask before architectural changes.
- Use explicit types and named constants (no magic numbers).
- Return explicit error types; do not suppress exceptions.
- Follow standard repository linting and formatting configs.
- Decompose files over 400 lines if they mix concerns.
- Use clear naming over comments; reserve comments for complex workarounds or non-obvious issues — why, not what.
- Never run git mutations (commit, push, reset, rebase, amend) unless explicitly instructed.
- Do not create documentation files unless explicitly requested.

### Author Environment

- CachyOS, KDE Plasma 6, Wayland, Btrfs.
- fish shell, Ghostty terminal, Fresh TUI editor, yay package manager, bun npm manager, Firefox, and Zed code editor.

### Testing

- Do not create test files for trivial changes, or for behavior that is not reliably unit-testable in the test environment (e.g. UI layout/click mapping). Prefer no new files; only add a test when the logic is genuinely testable and worth guarding.

### Definition of Done

- Logic fully implemented.
- Existing docs updated if public interfaces changed.
- When required by the `Verification` rules, run the corresponding `Workflow` command.

### Commit Messages

- On completion of an update or fix, print a conventional commit message in a fenced code block.
- Add a body only when the "why" isn't obvious.
- Do not wrap lines. One physical line per paragraph however long it gets. Never hard-wrap the subject or the body.

### Communication Style

- Fixes and instructions: numbered steps, one action each, exact command/path/code. No prose paragraphs or recap.
- Questions and explanations: direct prose, only the reasoning needed. No tables unless asked.
- No preamble, restating, hedging, disclaimers, meta-commentary, emotional framing, or closing offers; end when done unless a real decision is needed.
- Ask when requirements are ambiguous; put risks and edge cases, if any, in one final line.
