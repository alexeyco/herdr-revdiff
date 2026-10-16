# review-launch

## Purpose

The review-launch capability is the action mode of the herdr-revdiff binary:
when herdr invokes the plugin's `review` action (signaled by
`HERDR_PLUGIN_ACTION_ID`), the process reads the herdr plugin context from
`HERDR_PLUGIN_CONTEXT_JSON`, resolves the repository directory to review, and
opens a plugin-managed review pane through the herdr CLI (`plugin pane open
--plugin revdiff --entrypoint review --placement tab`). It passes the resolved
directory to the pane both as `--cwd` and as an injected
`HERDR_REVIEWS_DIR` environment variable, forwards optional workspace and
caller identity, reports success or failure on stderr, and exits. The action
does not run revdiff itself — that happens in the pane (a separate
capability).

## Requirements

### Requirement: Action mode reads and validates the herdr plugin context

The action SHALL obtain the plugin context from the environment variable
`HERDR_PLUGIN_CONTEXT_JSON`, and SHALL fail with a clear error when it is
missing, blank, or not valid JSON. (`src/review.rs:103-109`,
`src/context.rs:20-21`)

#### Scenario: Context variable absent or blank

- **WHEN** the action runs and `HERDR_PLUGIN_CONTEXT_JSON` is unset or
  contains only whitespace
- **THEN** the action fails with the message "HERDR_PLUGIN_CONTEXT_JSON is not
  set; invoke this action from herdr" (`src/review.rs:103-108`)

#### Scenario: Context is invalid JSON

- **WHEN** `HERDR_PLUGIN_CONTEXT_JSON` contains JSON that cannot be parsed
- **THEN** the action fails with a message beginning "invalid plugin context
  JSON: " followed by the parser error (`src/context.rs:20-21`)

### Requirement: The review directory is resolved along a fixed fallback chain

The action SHALL pick the first present, non-blank string among
`worktree.checkout_path`, `workspace_cwd`, and `focused_pane_cwd` in the
context JSON; a non-string or whitespace-only value counts as absent; when all
three are absent the action SHALL fail. (`src/context.rs:36-52`,
`src/context.rs:22-24`, `src/context.rs:55-65`)

#### Scenario: Worktree checkout path wins

- **WHEN** the context JSON has a non-blank `worktree.checkout_path`
- **THEN** the action uses that value as the review directory, regardless of
  `workspace_cwd` or `focused_pane_cwd` (`src/context.rs:37-40`)

#### Scenario: Fallback chain and blank handling

- **WHEN** `worktree.checkout_path` is absent, non-string, or whitespace-only,
  and a later candidate in the chain (`workspace_cwd`, then
  `focused_pane_cwd`) is a non-blank string
- **THEN** the action uses the first such later candidate
  (`src/context.rs:41-50`, `src/context.rs:62-65`)

#### Scenario: No directory anywhere in the context

- **WHEN** none of the three candidates yields a non-blank string
- **THEN** the action fails with "no directory in plugin context; invoke this
  action inside a workspace" (`src/context.rs:22-24`)

### Requirement: Caller identity fields are read from context with defined env fallbacks

The action SHALL read `workspace_id`, `focused_pane_id`, and
`focused_pane_agent` from the context JSON (blank or non-string counts as
absent); `workspace_id` and `focused_pane_id` SHALL fall back to the
environment variables `HERDR_WORKSPACE_ID` and `HERDR_PANE_ID` respectively,
and `focused_pane_agent` SHALL have no environment fallback.
(`src/context.rs:25-30`, `src/review.rs:111-117`)

#### Scenario: Pane id taken from the environment

- **WHEN** the context JSON has no non-blank `focused_pane_id` and
  `HERDR_PANE_ID` is set to a non-blank value
- **THEN** the action uses the `HERDR_PANE_ID` value as the caller pane
  identifier (`src/review.rs:114-116`)

#### Scenario: Agent without context entry is simply absent

- **WHEN** the context JSON has no non-blank `focused_pane_agent`
- **THEN** the action treats the caller agent as absent even if any
  environment variable names an agent, and omits the corresponding `--env`
  argument from the pane-open call (`src/review.rs:117`, `src/herdr.rs:94-97`)

### Requirement: The review pane is opened through the herdr CLI with a fixed, ordered argv

The action SHALL open the pane by invoking the herdr binary at
`HERDR_BIN_PATH` — never a bare `herdr` on `$PATH` — with the argument vector
`plugin pane open --plugin revdiff --entrypoint review --placement tab`, then
a conditional `--workspace <id>`, then `--cwd <dir>`, then always
`--env HERDR_REVIEWS_DIR=<dir>`, then conditional
`--env HERDR_REVIEWS_CALLER_PANE=<pane>` and
`--env HERDR_REVIEWS_CALLER_AGENT=<agent>` (only when those values are
present), then `--focus`. (`src/herdr.rs:60-100`, `src/herdr.rs:28-36`;
ordering locked by tests `src/herdr.rs:197-277`)

#### Scenario: Resolved directory is passed twice

- **WHEN** the action opens the pane for a resolved directory
- **THEN** the argv contains both `--cwd <dir>` and
  `--env HERDR_REVIEWS_DIR=<dir>`, because runtime commands may default the
  pane's cwd to the plugin directory (`src/herdr.rs:84-89`)

#### Scenario: The pane-open argv always carries its own placement

- **WHEN** the action builds the `plugin pane open` argv
- **THEN** it always includes `--placement tab` as the argument the action
  actually passes, while the manifest's `placement = "tab"` is a separate
  declaration the code comment notes can drift from the argv
  (`src/herdr.rs:74-78`, `herdr-plugin.toml:22`)

#### Scenario: HERDR_BIN_PATH is missing or blank

- **WHEN** any herdr CLI call is attempted and `HERDR_BIN_PATH` is unset or
  whitespace-only
- **THEN** the action fails with "herdr did not provide HERDR_BIN_PATH (plugin
  API v1 contract); cannot call back into herdr" (`src/herdr.rs:28-36`)

### Requirement: The pane-open reply must yield a non-blank pane id

The action SHALL parse the stdout of `plugin pane open` as JSON and require a
non-blank `result.plugin_pane.pane.pane_id`; `result.plugin_pane.pane.tab_id`
SHALL be optional, with a blank value treated as absent; parse or lookup
failures SHALL include a raw reply snippet truncated to 200 characters.
(`src/herdr.rs:104-121`, `src/herdr.rs:152-158`)

#### Scenario: Reply without a usable pane id

- **WHEN** the reply is not valid JSON, or lacks a non-blank
  `result.plugin_pane.pane.pane_id`
- **THEN** the action fails with an error naming the problem and quoting the
  raw reply snippet (`src/herdr.rs:106-114`)

#### Scenario: Overlay-style reply without a tab

- **WHEN** the reply carries a valid `pane_id` and an absent or blank
  `tab_id`
- **THEN** the action proceeds with tab_id treated as absent
  (`src/herdr.rs:115-120`)

### Requirement: Success is reported on stderr and the action exits without holding

On success the action SHALL log to stderr and exit 0; when a `tab_id` is
present the log SHALL read "opened review pane {pane_id} in tab {tab}" and be
followed by a best-effort `herdr tab focus <tab>` whose errors are ignored;
without a `tab_id` the log SHALL read "opened review pane {pane_id}" and no
extra focus call SHALL be made. (`src/review.rs:125-135`,
`src/herdr.rs:138-144`, `src/review.rs:119-124`) Action-mode failures follow
the mode-dispatch capability's top-level error policy (printed with the
`herdr-revdiff:` prefix, exit 1, no pane hold); the review-launch-specific
fact is the message texts listed here.

#### Scenario: Pane opens with a tab

- **WHEN** `plugin pane open` returns a pane id and a tab id
- **THEN** stderr shows "herdr-revdiff: opened review pane {pane_id} in tab
  {tab}" and a `tab focus {tab}` call runs best-effort against a rendering
  race; its failure does not affect the exit code, because herdr already
  applied `--focus` at open (`src/review.rs:125-132`)

#### Scenario: A CLI call exits nonzero

- **WHEN** a herdr CLI invocation returns a nonzero status
- **THEN** the review-launch-specific error is "{bin} {args} failed
  ({status}): {stderr snippet}" (`src/herdr.rs:16-25`), which propagates as
  an action-mode failure under the mode-dispatch capability's error policy

#### Scenario: The herdr binary cannot be spawned

- **WHEN** spawning the binary at `HERDR_BIN_PATH` fails
- **THEN** the action fails with "failed to run {bin} {args}: {io error}" and
  exits 1 (`src/herdr.rs:10-13`)
