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
- Dev: `just run`
- Check clippy, formatting and tests: `just check`
- Test: `just test`
- Lint: `just lint`
- Format: `just format`
- Build: `just build`; AppImage: `just package`

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

- For non-trivial work, propose an approach and confirm before implementing, including architectural changes.
- Keep modifications minimal and scoped; prefer incremental improvements over rewrites.
- Use explicit types and named constants (no magic numbers).
- Return explicit error types; do not suppress exceptions.
- Follow standard repository linting and formatting configs.
- Decompose files over 400 lines if they mix concerns.
- Name things clearly; reserve comments for the non-obvious why (workarounds, edge cases).
- Never run git mutations (commit, push, reset, rebase, amend) unless explicitly instructed.
- Do not create documentation files unless explicitly requested.
- Adhere to <https://develop.kde.org/hig/>.

### Author Environment

- Ryzen 5800X3D, Radeon RX 7900 XT, Display (AW3225QF); CachyOS, KDE, KWin / Wayland, Btrfs.
- fish shell, Ghostty terminal, Fresh TUI editor, yay package manager, bun npm manager, Firefox, Zed code editor.

### Testing

- Add tests only when the logic is genuinely testable and worth guarding. No test files for trivial changes or behavior the environment cannot test reliably (e.g. UI layout/click mapping).

### Definition of Done

- Logic fully implemented.
- Existing docs updated if public interfaces changed.
- Run the corresponding `Workflow` command when `Verification` requires it.

### Commit Messages

- On completion of an update or fix, print a conventional commit message in a fenced code block.
- Add a body only when the "why" isn't obvious.
- Never hard-wrap: one physical line per paragraph, however long.

### Communication Style

- Format: short declarative sentences, one idea per line.
- Instructions: numbered steps, one action each (exact command, path, or code); no prose recap.
- Explanations: essential context only. End question answers with `TL;DR:`, one sentence; omit for status updates and completed work.
- Tone: no preamble, restating, hedging, disclaimers, emotional framing, or closing offers. Keep every fact, command, flag, path, default, caveat.
- Edge Cases: state risks explicitly; ask when requirements are ambiguous.
