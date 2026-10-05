# AGENTS.md

See README.md for what this plugin does; docs/development.md for the dev workflow. Only agent-relevant gotchas live here.

## Gotchas

- The version appears in `Cargo.toml` and `herdr-plugin.toml`; keep both in sync (release tags verify both, then run the full CI suite via `workflow_call` before artifacts).
- Build the release binary before `herdr plugin link` testing; a linked plugin does not rebuild itself.
- Call back into herdr through `HERDR_BIN_PATH`, never a bare `herdr`.
- `min_herdr_version` is checked against the running herdr binary; an older local herdr refuses to link.
- `REVDIFF_BIN` (user-set, inherited from the herdr daemon env; not injected by the action) is canonicalized to an absolute path when it names an executable file (a non-executable value is a hard error); errors show the path as given. Without it the action resolves the first executable `revdiff` on `$PATH` to an absolute path, and fails loudly with an install hint when none is found or a candidate cannot be canonicalized.
- The binary is dual-mode: `HERDR_PLUGIN_ACTION_ID` set = action mode (opens the pane, exits), `HERDR_PLUGIN_ENTRYPOINT_ID` set = pane mode (runs revdiff on the pane tty). Same binary, dispatched in `main`.
- The manifest declares both `[[actions]]` (keybinding target, id `review`) and `[[panes]]` (id `review`, `placement = "tab"`, `command = ["./target/release/herdr-revdiff"]`). Plugin pane commands are argv-exec, not send-text, so there is no shell layer and no echoed command.
- Runtime commands may start in the plugin directory, so the action exports `HERDR_REVIEWS_DIR=<repo>` to the pane; pane mode prefers it over its own cwd and runs revdiff with `.current_dir(dir)`. `HERDR_REVIEWS_*` is not a protected `HERDR_*` key.
- Plugin panes auto-close when the pane process exits (`RuntimeExitAction::ClosePane`, no respawn). Success paths still call `plugin pane close` (best-effort); error and no-agent paths print their message then block on stdin via `hold_pane` before returning, otherwise the message vanishes.
- Review output is written under `HERDR_PLUGIN_STATE_DIR/reviews/` as a single `<pid>-<millis>.out` (pid keeps same-millisecond reviews collision-safe); pane start prunes kept `.out` files older than 7 days, best-effort with logged warnings. It is kept when delivery is skipped (no agent caller, or revdiff/delivery failed) and removed on delivery or a clean no-annotation exit. There is no `.rc` or `.started` file, and the pane process reads revdiff's exit code directly from the child.
- The delivered agent prompt is capped at 64 KiB (UTF-8 boundary) with a `(output truncated; full annotations kept at <path>)` note; unreadable output warns and counts as no annotations when the child exited 0, but is a hard error with any other exit code (file kept).
- herdr CLI calls (`tab`, `pane`, `agent`) have no timeout in plugin API v1, so a hung herdr daemon blocks the action.
