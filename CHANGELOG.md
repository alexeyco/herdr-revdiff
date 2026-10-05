# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.1.0 - 2026-10-05

### Added

- `revdiff.review` action that opens revdiff in a new herdr tab rooted at the repository, straight into the TUI (the pane process execs revdiff directly, with no shell layer and no echoed command).
- Review mode picked from read-only git probes: `HEAD~1` (last commit) for a clean tree with a parent commit, `--staged` for staged-only changes, `--untracked` for unstaged or untracked changes, and `--all-files` for a repository with no commits (or a single commit and a clean tree). Outside a git work tree, revdiff runs bare. The probes never write the git index.
- Annotations delivered to the agent in the focused pane as a single prompt when revdiff exits with annotations; with no agent, or on a revdiff or delivery failure, the annotations are kept on disk and the path is printed, and the pane holds until Enter.
- Review state kept as a single `<pid>-<millis>.out` file per review under `HERDR_PLUGIN_STATE_DIR/reviews/`, removed on delivery or a clean no-annotation exit, and pruned after 7 days.
- Delivered prompts capped at 64 KiB on a UTF-8 boundary; when truncated, the full annotations stay in the kept file and the prompt says so.
- revdiff resolved from `REVDIFF_BIN` (canonicalized before exec; a non-executable value is a hard error) or the first executable `revdiff` on `$PATH`, with an install hint when neither is found.
- Release binaries for Linux (`x86_64`, `aarch64`) and macOS (`x86_64`, `aarch64`), built by a release pipeline that runs the full CI suite before publishing. The macOS binaries are unsigned.
- Documentation: README, the behavioral spec in `docs/how-it-works.md`, a development guide, a contributing guide, and a security policy.
