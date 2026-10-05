# How it works

The `revdiff.review` action is a small Rust program (sources under `src/`) that runs in two modes.

- **Action mode** runs when herdr invokes the `revdiff.review` action. herdr sets `HERDR_PLUGIN_ACTION_ID` and `HERDR_PLUGIN_CONTEXT_JSON`; the binary resolves a repository directory from that JSON and opens a `revdiff` plugin pane (`herdr plugin pane open --placement tab`), then exits immediately.
- **Pane mode** runs inside the opened pane. herdr sets `HERDR_PLUGIN_ENTRYPOINT_ID`, spawns `command = ["./target/release/herdr-revdiff"]` from the manifest as a process attached to the pane pty, with the pane's tty and `--cwd` set to the repository. The binary probes git read-only to pick a review mode and execs revdiff as a direct child on that tty; when you quit revdiff it reads the child's exit code and delivers the annotations to the agent pane that requested the review.

Plugin pane commands are argv-exec, not send-text: the manifest `command` array is spawned directly, so there is no shell layer, no echoed command line, and the revdiff TUI appears the instant the pane opens. Plugin panes also auto-close when the pane process exits, so the error and no-agent paths block on Enter before exiting.

## Dispatch

`main` picks the mode from the environment, in this order:

| Condition                        | Mode                                                   |
| -------------------------------- | ------------------------------------------------------ |
| `HERDR_PLUGIN_ENTRYPOINT_ID` set | Pane mode                                              |
| `HERDR_PLUGIN_ACTION_ID` set     | Action mode                                            |
| neither                          | Error: `invoked outside herdr plugin context` (exit 1) |

For both variables, a blank or whitespace-only value counts as unset.

## Context

Environment variables set by herdr (see the [plugin docs](https://herdr.dev/docs/plugins/)):

| Variable                                                  | Used for                                                                                                       |
| --------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------- |
| `HERDR_PLUGIN_ACTION_ID`                                  | Dispatch: action mode                                                                                          |
| `HERDR_PLUGIN_ENTRYPOINT_ID`                              | Dispatch: pane mode                                                                                            |
| `HERDR_PLUGIN_CONTEXT_JSON`                               | Action mode: source of the repository directory and the caller pane/agent; must be present                     |
| `HERDR_BIN_PATH`                                          | Every callback (`plugin pane open`, `plugin pane close`, `tab close`, `tab focus`, `agent prompt`)             |
| `HERDR_PLUGIN_STATE_DIR`                                  | Base directory for review state (`reviews/`)                                                                   |
| `HERDR_WORKSPACE_ID`, `HERDR_PANE_ID`                     | Action mode: fallbacks for `workspace_id` / `focused_pane_id` when the context JSON omits them                 |
| `HERDR_REVIEWS_DIR`                                       | Pane mode: repository directory to review, injected by the action via `--env` (preferred over the process cwd) |
| `HERDR_REVIEWS_CALLER_PANE`, `HERDR_REVIEWS_CALLER_AGENT` | Pane mode: delivery target, injected by the action via `--env`                                                 |
| `HERDR_TAB_ID`, `HERDR_PANE_ID`                           | Pane mode: pane teardown (close pane, then tab)                                                                |
| `HERDR_PLUGIN_ID`, `HERDR_PLUGIN_ROOT`                    | Set by herdr but unused by this plugin                                                                         |

Fields read from the context JSON in action mode (all optional; absent when null or blank):

| Field                    | Meaning                                                      |
| ------------------------ | ------------------------------------------------------------ |
| `worktree.checkout_path` | Checkout directory of the worktree the workspace is based on |
| `workspace_cwd`          | Workspace working directory (may be a subdirectory)          |
| `focused_pane_cwd`       | Working directory of the focused pane                        |
| `workspace_id`           | Workspace id, used to open the pane in the right workspace   |
| `focused_pane_id`        | Pane id that receives the annotations on delivery            |
| `focused_pane_agent`     | Agent kind in the focused pane, when any; delivery needs it  |

The caller pane and agent are captured at action time and passed to the pane as `HERDR_REVIEWS_CALLER_PANE` / `HERDR_REVIEWS_CALLER_AGENT`; pane mode never reads the context JSON itself.

## Directory resolution

```
worktree.checkout_path > workspace_cwd > focused_pane_cwd > error
```

`worktree.checkout_path` wins because `workspace_cwd` may be a subdirectory the user has `cd`-ed into; running revdiff there would review only that subdirectory instead of the repository. A field counts only if it is a present, non-blank string; a non-string or whitespace-only value counts as absent and the chain falls through to the next candidate.

If no field yields a directory, the action exits 1 with `no directory in plugin context`. This happens when the action is invoked outside a workspace context.

## Mode selection

In pane mode, before starting revdiff, the plugin runs a set of **read-only** git probes in the pane's working directory (the repository; `git` via `Command::new("git").current_dir(dir)`). The index and the working tree are never mutated: no `git add`, no staging, nothing that writes.

| Probe            | Command in `dir`                           | Read as                            |
| ---------------- | ------------------------------------------ | ---------------------------------- |
| Repository       | `git rev-parse --is-inside-work-tree`      | exit 0 and stdout `true`           |
| HEAD exists      | `git rev-parse -q --verify HEAD`           | exit 0                             |
| `HEAD~1` exists  | `git rev-parse -q --verify HEAD~1`         | exit 0                             |
| Unstaged changes | `git diff --quiet`                         | exit 1 means present, 0 means none |
| Staged changes   | `git diff --cached --quiet`                | exit 1 means present, 0 means none |
| Untracked files  | `git ls-files --others --exclude-standard` | non-empty stdout                   |

The pure `select_mode` maps that state to one of five revdiff invocations:

| Situation                                 | Mode        | revdiff arguments   |
| ----------------------------------------- | ----------- | ------------------- |
| Not a git work tree                       | `Default`   | (bare `revdiff`)    |
| Git repo with no commits yet              | `AllFiles`  | `--all-files`       |
| Clean tree with a parent commit           | `HeadPrev`  | positional `HEAD~1` |
| Staged changes, nothing else              | `Staged`    | `--staged`          |
| Unstaged and/or untracked changes present | `Untracked` | `--untracked`       |

The first probe matters: outside a git work tree, only the bare `revdiff` is meaningful, so it gets `Default`. In a repository with no commits there is no `HEAD` to diff against, so `AllFiles` reviews every file. A clean tree with a parent commit reviews just the last commit (`HEAD~1`); with a single commit there is no parent, so the plugin falls back to `AllFiles`. A tree with staged changes and nothing else uses `--staged`. Anything remaining — unstaged changes and/or untracked files — uses `--untracked`; this arm also catches a mixed staged+unstaged tree.

Rationale: this mirrors the pattern the official [`umputun/revdiff`](https://github.com/umputun/revdiff) integrations use — choose the diff source from the repository state, never write to the index. Some third-party integrations (for example `pi-revdiff`) run `git add -A` to force a full staged diff; that is deliberately rejected here because it mutates the user's index and can stage unintended files.

Caveats:

- **Mixed staged + unstaged** — when both are present the mode is `Untracked`, so the working tree and untracked files are reviewed but the staged-only portion is not; this matches the official Pi integration.
- **Non-git directory** — the bare `revdiff` runs with no diff source; the limitation is that git-backed review is unavailable.
- **Single commit** — a clean tree with only one commit selects `--all-files`, since `HEAD~1` does not exist.

On any unexpected git failure (a quiet diff exiting with a code other than 0 or 1, a spawn error, and so on) the plugin writes one warning to stderr — `herdr-revdiff: git probe failed (<cmd>): <err>; falling back to default mode` — and selects `Default`, so the review always stays resilient.

## Launch and lifecycle

### Action mode

1. Parse the context JSON, resolve the repository directory and the caller pane/agent (see [Context](#context)).
2. Open the review pane:

   ```
   herdr plugin pane open --plugin revdiff --entrypoint review --placement tab \
     [--workspace <workspace_id>] --cwd <dir> --env HERDR_REVIEWS_DIR=<dir> \
     [--env HERDR_REVIEWS_CALLER_PANE=<focused_pane_id>] \
     [--env HERDR_REVIEWS_CALLER_AGENT=<focused_pane_agent>] --focus
   ```

   `--workspace <workspace_id>` is added only when the context (or `HERDR_WORKSPACE_ID`) carries one. `--cwd <dir>` roots the pane at the repository resolved above, and `--env HERDR_REVIEWS_DIR=<dir>` exports the same directory so pane mode does not have to trust its own cwd. Each caller `--env` pair is added only for the caller values that are present.

3. Log the resulting pane and tab id to stderr (visible via `herdr plugin log list`) and exit 0. When the reply carries a tab id, the action then re-asserts focus with a best-effort `herdr tab focus <tab_id>` (errors ignored) — purely defensive against a client-side focus race, since herdr already applies `--focus` at open. The action does no tab management, no polling, and no mode detection: all of that happens in the pane process.

### Pane mode

The pane command is `["./target/release/herdr-revdiff"]`, spawned by herdr as a process attached to the pane pty. Its working directory is the `--cwd` passed to `pane open` (the repository). Because herdr runtime commands may default to the plugin directory as cwd, pane mode prefers `HERDR_REVIEWS_DIR` (exported by the action) over its own process cwd, and launches revdiff with that directory as the child's working directory.

1. Print `herdr-revdiff: reviewing <mode description>` to stdout. The line is immediately covered by the revdiff TUI; it is visible only when revdiff fails to start.
2. Compute the state path: `HERDR_PLUGIN_STATE_DIR/reviews/<pid>-<unix millis>.out` (the system temp directory is used when the variable is unset). The pid prefix keeps two reviews started in the same millisecond from clobbering each other. Only this `.out` file exists — there is no rc file and no start sentinel.
3. Prune kept review output: `*.out` files directly under `reviews/` older than 7 days are removed, best-effort. A removal failure is logged as a warning (`herdr-revdiff: warning: cannot remove <path>: <err>`) and never fatal; non-`.out` entries are never touched. Pruning runs again on every pane start, so a kept file survives at most 7 days beyond the next review.
4. Find the revdiff binary (see [Binary and state](#binary-and-state)).
5. Spawn revdiff as a **direct child**, argv-exec with the pane's tty inherited (no shell, so nothing is echoed and the TUI is instant):

   ```
   revdiff <mode-flags> --output <HERDR_PLUGIN_STATE_DIR>/reviews/<pid>-<unix millis>.out --exit-code-on-annotations <mode-positional>
   ```

   The selected mode's flags come first, then `--output` and `--exit-code-on-annotations`; the optional positional revision is always last, after every argument, so both strict and lenient parsers accept it. Concretely, the child is one of

   ```
   revdiff --output <out> --exit-code-on-annotations
   revdiff --all-files --output <out> --exit-code-on-annotations
   revdiff --output <out> --exit-code-on-annotations HEAD~1
   revdiff --staged --output <out> --exit-code-on-annotations
   revdiff --untracked --output <out> --exit-code-on-annotations
   ```

   revdiff writes inline annotations to `<pid>-<unix millis>.out` in the form `## <path>:<LINE> (<+|->)` followed by the annotation text. The child's working directory is set to the resolved `<dir>` (`HERDR_REVIEWS_DIR`).

6. Wait for the child and read its exit code directly from the process status (`status.code()`). There is no poll loop, no rc file, and no pane-liveness probe.

7. Act on the exit code (see [Exit-code mapping](#exit-code-mapping)). Plugin panes auto-close when the pane process exits, so on the quiet-success paths — annotations delivered, or exit 0 with empty output — the plugin still closes the pane explicitly (best-effort; the process exit is the backup):

   ```
   herdr plugin pane close <HERDR_PANE_ID>
   ```

   with a best-effort fallback to `herdr tab close <HERDR_TAB_ID>` when the pane id is missing or the close fails. A close failure is logged, never fatal. On the error and no-agent paths the plugin instead prints its message and then blocks on a stdin read (`herdr-revdiff: press Enter to close`) until you press Enter, so the message stays readable before the pane auto-closes.

If you close the tab yourself while revdiff is open, the pane process is killed: nothing is delivered, and whatever revdiff had already flushed to the output file stays on disk (pruned after 7 days on a later pane start). This is the intended cancel path.

## Exit-code mapping

revdiff is launched with `--exit-code-on-annotations`, so its exit code decides what happens next:

| revdiff exit     | Output file   | Outcome                                                               |
| ---------------- | ------------- | --------------------------------------------------------------------- |
| `0`              | empty         | No annotations; nothing is sent, file removed, pane closed, exit 0.   |
| `0`              | non-empty     | Annotations delivered to the caller agent pane.                       |
| `10`             | (annotations) | Annotations delivered to the caller agent pane.                       |
| `1` or any other | kept on disk  | revdiff failed; error logged, message held until Enter, exit 1.       |
| no code (signal) | kept on disk  | Treated as a failure; error logged, message held until Enter, exit 1. |

Any exit code other than `0` or `10` is treated as a revdiff error.

Read semantics: once the child exits, the plugin reads the output file. Non-UTF-8 bytes are handled lossily (`String::from_utf8_lossy`), so invalid sequences become U+FFFD instead of aborting. A read failure is only harmless when revdiff exited 0: the plugin logs `herdr-revdiff: warning: cannot read <path>: <err>` and treats the review as having no annotations (exit 0, file removed). With any other exit code, a read failure is fatal — `cannot read revdiff output <path>: <err>` — the file is kept, and the pane holds until Enter (exit 1).

## Delivery

Annotations are delivered only when the review produced them (exit `10`, or exit `0` with non-empty output) and the pane was opened with both `HERDR_REVIEWS_CALLER_PANE` and `HERDR_REVIEWS_CALLER_AGENT`. Delivery calls:

```
herdr agent prompt <HERDR_REVIEWS_CALLER_PANE> <text>
```

with the annotation text prefixed by `Review annotations from revdiff:`. The plugin is agent-agnostic: it does not inspect the agent kind, so any agent herdr supports receives the prompt.

The delivered prompt is capped at 64 KiB (65536 bytes), well under `ARG_MAX` since the text travels as a single argv element. The cut is made on a UTF-8 boundary, and when it truncates the text the plugin appends `\n\n(output truncated; full annotations kept at <path>)`, so the full review stays available on disk. This only matters for very large annotation sets; the cap and the kept file are independent of delivery success.

Fallbacks and edge behavior:

- **No agent caller value** — the output file is kept on disk, `herdr-revdiff: no agent in focused pane; annotations kept at <path>` is logged, and the pane blocks on `herdr-revdiff: press Enter to close` until you press Enter, so the path stays readable before the pane auto-closes.
- **revdiff error (exit 1 or other, or a signal)** — the output file is kept, the error is logged, and the pane holds until Enter.
- **Exit 0 with empty output** — treated as "no annotations"; the file is removed and the pane closed.
- **Delivery call fails** — the error is logged, the output file is kept, and the pane holds until Enter.
- **Tab closed early** — the pane process is killed; nothing is sent, and whatever revdiff already flushed to the output file stays on disk (pruned after 7 days on a later pane start).

The `<pid>-<unix millis>.out` file is removed once the annotations are delivered or when revdiff exits 0 with nothing to report; it is kept when delivery is skipped (no agent), when revdiff fails, when the output cannot be read with a non-zero exit code, or when the pane dies without an exit code.

## Binary and state

- revdiff lookup order: `REVDIFF_BIN` when set to a non-blank, executable file (canonicalized to an absolute path before exec; a non-executable value is a hard error), then the first executable `revdiff` on `$PATH`, also canonicalized to an absolute path since the pane process may have a different `$PATH` than the action process. A candidate that cannot be canonicalized is a loud error — `found revdiff at <path> but cannot resolve it to an absolute path: <err>` — never a cwd-relative guess. When neither yields a binary, the plugin fails with an install hint pointing at <https://github.com/umputun/revdiff> and `REVDIFF_BIN`. Errors show the path as you gave it, not its canonical form.
- `REVDIFF_BIN` is this plugin's own optional override — not a herdr variable and not a revdiff one. Pane mode reads it from its own environment, which is the herdr daemon's environment plus the `HERDR_*` vars herdr injects plus the `--env` pairs the action passes (`HERDR_REVIEWS_DIR`, `HERDR_REVIEWS_CALLER_PANE`, `HERDR_REVIEWS_CALLER_AGENT`) — `REVDIFF_BIN` is not among them. So it reaches the pane only if you export it in the session where herdr is launched: the daemon inherits it, and a herdr restart picks up a change. Use it when revdiff is not on the daemon's `$PATH`, or to point at a specific build:

  ```
  export REVDIFF_BIN=/path/to/revdiff
  ```

- State lives under `HERDR_PLUGIN_STATE_DIR/reviews/`: a single `<pid>-<unix millis>.out` file per review (the pid makes the name collision-safe across concurrent reviews). There is no rc file and no start sentinel; revdiff's exit code is read directly from the child process. `HERDR_PLUGIN_STATE_DIR` is expected to be an absolute path and is used as given; when it is unset, the system temp directory is used. Kept files older than 7 days are pruned at pane start (best-effort, failures logged as warnings).
- Every herdr callback uses `HERDR_BIN_PATH`, never a bare `herdr`.

## JSON parsing

The context is parsed by [`serde_json`](https://crates.io/crates/serde_json) into a generic `Value`. The plugin then takes the first present, non-blank string along the resolution chain (`worktree.checkout_path` → `workspace_cwd` → `focused_pane_cwd`). Standard JSON escaping, including `\uXXXX` with surrogate pairs, survives, so non-ASCII repository paths are preserved.

## Troubleshooting

- **`no directory in plugin context`** — the action ran without workspace context. Invoke it from inside a herdr workspace (e.g. via the keybinding), not from a bare shell.
- **`invoked outside herdr plugin context`** — the binary was run directly, without `HERDR_PLUGIN_ACTION_ID` or `HERDR_PLUGIN_ENTRYPOINT_ID`. Invoke it through herdr.
- **`revdiff not found`** — install [revdiff](https://github.com/umputun/revdiff) >= 1.12 on `PATH`, or point `REVDIFF_BIN` at the binary.
- **Empty review** — the reviewed diff follows the current git state (see [Mode selection](#mode-selection)); when that diff is empty, revdiff exits 0 with no annotations, the plugin logs `no annotations`, and nothing is delivered to the agent. The pane prints `herdr-revdiff: reviewing <description>` before opening the TUI. A mixed staged+unstaged tree is reviewed as `untracked + working tree`, so the staged-only portion is not shown.
- **Annotations not delivered** — the caller pane had no agent. The review output is kept under `HERDR_PLUGIN_STATE_DIR/reviews/`, and the pane waits for Enter before auto-closing. Focus an agent pane and re-run the action.
- **revdiff fails to start** — the pane prints `herdr-revdiff: reviewing <description>` and any error from the plugin (for example `revdiff not found`, or `failed to run <bin> in <dir>: <err>` when the child cannot be spawned or its working directory is invalid), then waits for Enter before auto-closing. Fix the cause and re-run the action; check the action side with `herdr plugin log list --plugin revdiff`.
- **`cannot read revdiff output <path>: <err>`** — revdiff exited with an error and its output file could not be read (for example a permissions problem). The file is kept; fix the permissions or the state directory. When revdiff exited 0, an unreadable file is only a warning and the review is treated as having no annotations.
- **`herdr did not provide HERDR_BIN_PATH (plugin API v1 contract)`** — the process was not given the herdr binary path it needs for callbacks. Run the plugin through herdr rather than directly, and check the herdr install.
- **The action appears hung** — herdr CLI calls (`plugin pane`, `tab`, `agent`) have no timeout by design in plugin API v1, so a hung herdr daemon blocks the action; check the daemon first.
- Action logs: `herdr plugin log list --plugin revdiff`.
