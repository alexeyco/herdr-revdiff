# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.1.0 - 2026-10-05

### Added

- `revdiff.review` action that resolves the repository directory from the herdr plugin context (`worktree.checkout_path` → `workspace_cwd` → `focused_pane_cwd`) and opens a `revdiff` plugin pane (`herdr plugin pane open --placement tab`) rooted there. The pane process runs revdiff as a direct child on the pane tty (argv exec, no shell), so the tab opens straight into the revdiff TUI. The review mode is selected automatically from the git state: bare working tree outside a git work tree, `--all-files` with no commits (or a single commit), `HEAD~1` for a clean tree with a parent commit, `--staged` for staged-only changes, and `--untracked` for unstaged or untracked changes.
- Pane lifecycle: the action exports the resolved directory as `HERDR_REVIEWS_DIR`, so pane mode reviews that directory and runs revdiff with it as the working directory regardless of the runtime cwd. The pane process reads revdiff's exit code directly from the child process (no rc file, no start sentinel, no poll loop) and closes the pane on quiet success, falling back to closing the tab.
- Agent delivery: annotations are sent to the agent pane that launched the action, captured at action time via `HERDR_REVIEWS_CALLER_PANE` / `HERDR_REVIEWS_CALLER_AGENT`, with `herdr agent prompt`, agent-agnostic across every agent kind herdr supports.
- Edge behavior: no annotations on exit 0 with empty output; output file kept and error logged on revdiff failure; the pane prints the message and waits for Enter before auto-closing when no agent caller is present or when revdiff or delivery fails; review cancelled silently when the tab is closed early.
- revdiff lookup: `REVDIFF_BIN` when it names an executable file is canonicalized to an absolute path and used (a non-executable value is a hard error), otherwise the first executable `revdiff` on `$PATH`, resolved to an absolute path; neither found is an actionable error. Errors show the path as given. State is kept under `HERDR_PLUGIN_STATE_DIR/reviews/` as a single `<pid>-<millis>.out` file, so two reviews started in the same millisecond no longer clobber each other.
- Automatic pruning: kept review output older than 7 days is removed best-effort at pane start (removal failures are logged as warnings, never fatal).
- Integration test suite in `tests/`, driving the real binary with stub `herdr` and `revdiff` shell scripts (no network, no herdr install).
- Documentation: README, how it works, development guide, contributing guide.
- `SECURITY.md` with the vulnerability reporting process and trust model.
- CI: fmt, clippy, and tests on Linux and macOS; the release workflow publishes binaries for `v*` tags for `x86_64`/`aarch64` Linux and `aarch64`/`x86_64` macOS, running the full CI suite (via `workflow_call`) before artifacts are built.

### Changed

- Delivered agent prompts are capped at 64 KiB on a UTF-8 boundary; truncation appends a note pointing at the kept output file.
- Release profile uses LTO, `strip`, `codegen-units = 1`, and `panic = "abort"` for smaller binaries.
- Prettier is pinned to `3.9.6` in the Makefile and CI.

### Fixed

- Unreadable review output no longer silently drops annotations: a warning is logged on a clean exit, and any other exit code is a hard error that keeps the file.
- Non-UTF-8 review output is decoded lossily instead of aborting.
- No more `expect` in production paths.
