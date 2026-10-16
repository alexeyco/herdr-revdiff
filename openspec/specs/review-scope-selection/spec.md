# review-scope-selection

## Purpose

When the plugin runs in pane mode it must decide what revdiff is about to
review before the TUI opens. This capability resolves the review directory —
preferring the directory the action exported through `HERDR_REVIEWS_DIR` and
falling back to the pane process working directory — checks that the herdr
environment is usable, then runs a sequence of strictly read-only git probes
in that directory to classify the repository state. From that classification
it selects one of five review modes (default, all-files, last-commit,
staged, untracked), announces the choice on stdout, and maps the mode to the
revdiff flags or positional revision used later by the execution path. The
probes never mutate the git index or working tree, and any unexpected probe
failure degrades silently to the bare default mode rather than aborting the
review.

## Requirements

### Requirement: Review directory resolution

Pane mode SHALL resolve the review directory by preferring a non-blank
`HERDR_REVIEWS_DIR` environment variable (value trimmed; whitespace-only or
empty counts as absent), falling back to the process current directory, and
failing with `cannot read current directory: <err>` only when that fallback
`current_dir()` call itself fails (a non-blank `HERDR_REVIEWS_DIR`, even a
bogus one, never reaches this error)
(src/review.rs:141-149, :333-338).

#### Scenario: Action-exported directory wins

- **WHEN** pane mode starts with `HERDR_REVIEWS_DIR` set to a non-blank path
- **THEN** that trimmed path is used as the review directory and the process
  cwd is not consulted

#### Scenario: Blank variable falls back to cwd

- **WHEN** pane mode starts with `HERDR_REVIEWS_DIR` unset, empty, or
  whitespace-only
- **THEN** the process current directory is used as the review directory

#### Scenario: Unreadable cwd is an error

- **WHEN** `HERDR_REVIEWS_DIR` is absent and reading the current directory
  fails
- **THEN** pane mode returns the error `cannot read current directory: <err>`

### Requirement: Herdr environment pre-check before revdiff starts

Before spawning revdiff or reporting a mode, pane mode SHALL call
`herdr::require_bin()` so a missing or blank `HERDR_BIN_PATH` is reported as
an environment error before the TUI opens (src/herdr.rs:38-42,
src/review.rs:140).

#### Scenario: Missing HERDR_BIN_PATH fails early

- **WHEN** pane mode starts without a usable `HERDR_BIN_PATH`
- **THEN** it fails before running any git probe or printing the mode line,
  with the message `herdr did not provide HERDR_BIN_PATH (plugin API v1
contract); cannot call back into herdr`

### Requirement: Read-only git probe

The plugin SHALL probe the resolved directory by running, in order,
`git rev-parse --is-inside-work-tree`, `git rev-parse -q --verify HEAD`,
`git rev-parse -q --verify HEAD~1`, `git diff --quiet`,
`git diff --cached --quiet`, and `git ls-files --others --exclude-standard`,
each with `current_dir(dir)`, and every command SHALL be read-only so the
index and working tree are never mutated (src/git.rs:3-5, :122-179, :181-187).
Exit-code meanings are fixed: `--is-inside-work-tree` other than success with
output `true` means non-repo (not an error); `rev-parse -q --verify` exit 0
means the revision exists; quiet diffs exit 0 for clean and 1 for changes;
non-empty trimmed stdout from `ls-files --others` means untracked files exist
(src/git.rs:142-179).

#### Scenario: Probe stops early for a non-repo directory

- **WHEN** `git rev-parse --is-inside-work-tree` in the directory does not
  succeed with trimmed output `true`
- **THEN** the probe returns the default (non-repo) status without running
  the remaining commands

#### Scenario: Probe stops early when HEAD does not exist

- **WHEN** the directory is a work tree but `git rev-parse -q --verify HEAD`
  exits non-zero
- **THEN** the probe returns `repo = true` with `has_head = false` and skips
  the diff and untracked commands

#### Scenario: Full probe on a repository with commits

- **WHEN** the directory is a work tree with a HEAD commit
- **THEN** `has_head_prev`, `unstaged`, `staged`, and `untracked` are each
  determined by their respective read-only commands

### Requirement: Review mode selection

From the probed git state the plugin SHALL select exactly one mode with the
pure function `select_mode`: non-repo → `Mode::Default`; repo without commits
→ `Mode::AllFiles`; clean tree (no staged, unstaged, or untracked changes)
with `HEAD~1` → `Mode::HeadPrev`; clean tree without `HEAD~1` →
`Mode::AllFiles`; staged changes only → `Mode::Staged`; anything unstaged
and/or untracked, including a mixed staged+unstaged tree → `Mode::Untracked`
(src/git.rs:71-89). Pane mode SHALL then print
`herdr-revdiff: reviewing <description>` to stdout, where the description is
one of `bare working tree`, `all files`, `last commit`, `staged changes`, or
`untracked + working tree` (src/review.rs:151-152, src/git.rs:48-56).

#### Scenario: Clean multi-commit repo reviews the last commit

- **WHEN** the probe reports a repo with HEAD, HEAD~1, and no changes of any
  kind
- **THEN** `Mode::HeadPrev` is selected and stdout shows
  `herdr-revdiff: reviewing last commit`

#### Scenario: Working changes take priority over staged ones

- **WHEN** the probe reports both staged changes and unstaged or untracked
  files
- **THEN** `Mode::Untracked` is selected

#### Scenario: Staged-only tree

- **WHEN** the probe reports staged changes and nothing unstaged or untracked
- **THEN** `Mode::Staged` is selected and stdout shows
  `herdr-revdiff: reviewing staged changes`

### Requirement: Silent fallback to the default mode

A directory that is not a git work tree SHALL silently select
`Mode::Default` with no user-facing "not a git repo" error (src/git.rs:71-73,
:122-125). Any unexpected git failure during probing SHALL print one warning
to stderr in the form `herdr-revdiff: git probe failed (<cmd>): <err>;
falling back to default mode`, return `GitStatus::default()`, and thereby
select `Mode::Default` (bare `revdiff`, no mode flags) instead of aborting
(src/git.rs:91-105, :157-169, :175-177).

#### Scenario: Unexpected diff exit code falls back

- **WHEN** `git diff --quiet` exits with a code other than 0 or 1
- **THEN** a single probe-failure warning naming the command is written to
  stderr and the selected mode is `Mode::Default`

#### Scenario: Non-repo directory reviews the bare working tree

- **WHEN** the resolved directory is not inside a git work tree
- **THEN** `Mode::Default` is selected, stdout shows
  `herdr-revdiff: reviewing bare working tree`, and no error is raised

### Requirement: Mode maps to revdiff flags or a positional, never both

Each mode SHALL expose its revdiff argument fragment statically: `Default`
and `HeadPrev` contribute no flags; `AllFiles` contributes `--all-files`;
`Staged` contributes `--staged`; `Untracked` contributes `--untracked`; and
only `HeadPrev` contributes the positional `HEAD~1`, which is placed after
all flags and never combined with any flag (src/git.rs:29-45). The execution
path consumes these fragments when building argv
(src/review.rs:86-99; argv construction itself belongs to the revdiff
execution capability).

#### Scenario: HeadPrev yields a positional with no flags

- **WHEN** `Mode::HeadPrev` is asked for its fragment
- **THEN** `flags()` is empty and `positional()` is `HEAD~1`

#### Scenario: Flag modes yield no positional

- **WHEN** any of `AllFiles`, `Staged`, or `Untracked` is asked for its
  fragment
- **THEN** `positional()` is `None` and `flags()` is the mode's single flag
