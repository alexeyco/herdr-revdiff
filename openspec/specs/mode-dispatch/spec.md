# mode-dispatch

## Purpose

The `herdr-revdiff` binary is a single executable serving two herdr plugin
entry points: an action and a pane. At startup it inspects the plugin
environment herdr injects (`HERDR_PLUGIN_ENTRYPOINT_ID`,
`HERDR_PLUGIN_ACTION_ID`) to decide which role to play, and treats a run
outside any herdr context as a hard error. The manifest `herdr-plugin.toml`
declares both entry points, each with the same array-valued command pointing
at the same binary. Around this dispatch the binary defines a
uniform top-level error policy: errors print with a `herdr-revdiff:` prefix
on stderr and exit 1, with pane-mode errors additionally blocking on stdin
so an auto-closing pane stays readable; the binary itself only ever exits
with 0 or 1.

## Requirements

### Requirement: Mode is dispatched purely from the plugin environment

The binary SHALL choose its run mode at startup from environment variables
only, checking `HERDR_PLUGIN_ENTRYPOINT_ID` first: when it is set the binary
runs pane mode, otherwise when `HERDR_PLUGIN_ACTION_ID` is set it runs action
mode, otherwise it fails as "invoked outside herdr plugin context". Because
the entry-point variable is checked first, pane mode wins when both are set,
matching herdr's action-spawns-pane nesting.
(src/main.rs:43-53, :55-60; test src/main.rs:134-137)

#### Scenario: Only the entry-point variable is set

- **WHEN** `HERDR_PLUGIN_ENTRYPOINT_ID` is set and `HERDR_PLUGIN_ACTION_ID` is not
- **THEN** the binary runs pane mode

#### Scenario: Both mode variables are set

- **WHEN** `HERDR_PLUGIN_ENTRYPOINT_ID` and `HERDR_PLUGIN_ACTION_ID` are both set
- **THEN** the binary runs pane mode, not action mode

#### Scenario: Neither mode variable is set

- **WHEN** the binary is invoked with neither `HERDR_PLUGIN_ENTRYPOINT_ID` nor
  `HERDR_PLUGIN_ACTION_ID` set
- **THEN** it writes `herdr-revdiff: invoked outside herdr plugin context` to
  stderr and exits 1 (tests/e2e.rs:484-489)

### Requirement: Blank or whitespace-only mode variables count as unset

The environment check SHALL treat a variable whose value is empty or
whitespace-only as unset, via trimming, so herdr passing empty strings does
not silently select a mode. (src/main.rs:63-67; test src/main.rs:145-149)

#### Scenario: Whitespace-only entry point with a set action

- **WHEN** `HERDR_PLUGIN_ENTRYPOINT_ID` is set to whitespace and
  `HERDR_PLUGIN_ACTION_ID` is set to a non-blank value
- **THEN** the binary runs action mode

#### Scenario: Both variables set to blank values

- **WHEN** both mode variables are present but blank or whitespace-only
- **THEN** the binary reports the outside-herdr error and exits 1

### Requirement: Every top-level error prints on stderr and exits 1; pane-mode errors hold the pane

When any stage of `run()` returns an error, the binary SHALL print
`herdr-revdiff: <error message>` to stderr and exit 1. Before exiting, if
`HERDR_PLUGIN_ENTRYPOINT_ID` is set (pane mode), it SHALL call `hold_pane()`,
which prints `herdr-revdiff: press Enter to close` and blocks reading one
line from stdin, so the auto-closing plugin pane stays readable; a closed
stdin (EOF) returns immediately. Action-mode errors exit without holding.
(src/main.rs:20-29; src/review.rs:24-26, 282-289)

#### Scenario: Pane-mode error is held for reading

- **WHEN** pane mode fails at any stage
- **THEN** stderr shows the `herdr-revdiff:` error and the prompt `herdr-revdiff:
press Enter to close`, the process waits for Enter, then exits 1

#### Scenario: Error with closed stdin does not hang

- **WHEN** pane mode errors and stdin is already at EOF
- **THEN** `hold_pane()` returns at once and the process exits 1

#### Scenario: Action-mode error exits immediately

- **WHEN** action mode fails
- **THEN** the error is printed on stderr and the process exits 1 without
  blocking on stdin

### Requirement: The binary exits with 0 or 1 only

Successful paths SHALL exit 0 by returning normally (no explicit exit call);
all error paths exit 1. Exit codes from the revdiff child are internal
outcomes consumed by pane mode and SHALL not become the binary's exit code.
(src/main.rs:28; src/review.rs:47-60)

#### Scenario: Success in either mode

- **WHEN** pane or action mode completes without error
- **THEN** the process exits 0

#### Scenario: revdiff child failure does not leak its code

- **WHEN** pane mode's revdiff child exits with a nonzero code
- **THEN** that code is mapped to an internal outcome, not propagated as the
  binary's exit status

### Requirement: Output convention distinguishes errors and best-effort warnings

Top-level errors SHALL use the prefix `herdr-revdiff: <msg>` on stderr and
best-effort warnings SHALL use `herdr-revdiff: warning: <msg>` on stderr
(for example unreadable review output at exit 0, and prune or removal
failures). Pane progress lines follow the same prefix; the one stdout
progress line pane mode prints (`herdr-revdiff: reviewing <description>`)
is owned by the review-scope-selection capability, not restated here.
(src/main.rs:22; src/review.rs:127-135, 194-197, 319-322)

#### Scenario: Best-effort cleanup failure warns rather than fails

- **WHEN** a prune or similar best-effort step cannot complete (for example,
  removing an old review output)
- **THEN** a `herdr-revdiff: warning: ...` line is written to stderr and the run
  continues

### Requirement: The manifest declares paired action and pane entry points for one binary

`herdr-plugin.toml` SHALL declare plugin identity `revdiff` with
`min_herdr_version = "0.9.0"` and `platforms = ["linux", "macos"]`, one action
with id `review` (`contexts = ["workspace"]`) and one pane with id `review`
(`placement = "tab"`), both declaring the identical argv-form command
`["./target/release/herdr-revdiff"]` — a JSON/TOML array, not a shell string.
The manifest `version` SHALL stay equal to the `Cargo.toml` version, enforced
by the CI version-sync job.
(herdr-plugin.toml:2-23; Cargo.toml:3; docs/development.md:59-68)
UNVERIFIED: that herdr executes array commands as argv without a shell layer
is attested only in AGENTS.md and herdr's own documentation, not by any code
in this repository.

#### Scenario: Both entry points resolve to the same binary

- **WHEN** the manifest's action and pane entries are compared
- **THEN** both declare the identical command array
  `["./target/release/herdr-revdiff"]`, and the binary's environment
  dispatch selects which role the launched process plays

#### Scenario: Manifest and Cargo versions diverge

- **WHEN** `version` in `herdr-plugin.toml` differs from the version in
  `Cargo.toml`
- **THEN** the CI version-sync check fails the build
