# annotation-delivery

## Purpose

This capability covers what the plugin does after the revdiff child process
finishes inside a review pane: it decides the review outcome from revdiff's
exit code and output (annotations exist when the exit code is 10, or the exit
code is 0 with non-blank output; any other exit code is a failure), builds and
caps the prompt text, and delivers annotations to the caller agent through the
herdr CLI when one is available. It also owns the file-retention rules for the
review output file (kept on failures and no-agent paths, removed on clean
delivery and the no-annotations case), and pane teardown via best-effort
close-on-success with a tab-close fallback. Error and no-agent paths keep the
pane open by holding on stdin until the user presses Enter; the hold
mechanics themselves belong to the mode-dispatch capability.

## Requirements

### Requirement: Review outcome is decided from exit code, output, and agent availability

The pane process SHALL classify revdiff's result into exactly one outcome:
annotations exist if and only if the exit code is 10, or the exit code is 0
and the output is non-blank; when annotations exist the outcome is `Deliver`
if a caller agent is available and `NoAgent` otherwise; exit code 0 with blank
output is `NoAnnotations`; any other exit code is `Failed(rc)` even when the
output is non-blank (src/review.rs:47-60). A caller agent is considered
available only when both `HERDR_REVIEWS_CALLER_PANE` and
`HERDR_REVIEWS_CALLER_AGENT` are non-blank in the pane environment
(src/review.rs:176-177, :223; those variables are injected action-side by
the pane-open argv, src/herdr.rs:90-97).

#### Scenario: Nonzero success code with agent delivers

- **WHEN** revdiff exits with code 10 and both caller env vars are set
- **THEN** the outcome is `Deliver`

#### Scenario: Clean exit with blank output is no-annotations

- **WHEN** revdiff exits with code 0 and the output is blank
- **THEN** the outcome is `NoAnnotations`

#### Scenario: Annotations without an agent

- **WHEN** revdiff exits with code 10 (or 0 with non-blank output) and either
  caller env var is missing or blank
- **THEN** the outcome is `NoAgent`

#### Scenario: Any other exit code fails regardless of output

- **WHEN** revdiff exits with a code other than 0 or 10, even with non-blank
  output
- **THEN** the outcome is `Failed(rc)` (src/review.rs:472-504 covers these
  decisions in tests)

### Requirement: The delivered prompt is a header plus trimmed output, capped at 64 KiB on a UTF-8 boundary

The prompt sent to the agent SHALL be the literal header
`Review annotations from revdiff:\n\n` followed by revdiff's output with
leading and trailing whitespace trimmed (src/review.rs:23, :63-65). The prompt
SHALL be capped at `MAX_PROMPT_BYTES = 65536` bytes, cutting on a UTF-8
character boundary and appending
`\n\n(output truncated; full annotations kept at <path>)` only when the text
exceeds the cap; text that already fits is returned unchanged and multi-byte
characters are never split (src/review.rs:29-31, :70-84, tests :540-565).

#### Scenario: Prompt within the cap is sent intact

- **WHEN** the header plus trimmed output is at most 65536 bytes
- **THEN** the delivered text equals it unchanged, with no truncation note

#### Scenario: Oversized prompt is cut at a character boundary

- **WHEN** the header plus trimmed output exceeds 65536 bytes
- **THEN** the text is cut at the largest boundary at or below 65536 bytes and
  the note pointing at the kept output file is appended

### Requirement: Annotations are delivered to the caller pane through the herdr CLI with defined success and failure handling

Delivery SHALL run `herdr agent prompt <caller_pane> <text>` invoked via
`HERDR_BIN_PATH` (src/herdr.rs:147-149). On delivery success the process
SHALL, in order, print the full line
`herdr-revdiff: sent review annotations to agent pane <target>` to stderr,
remove the output file (tolerating `NotFound`, logging other removal
failures as warnings), perform a best-effort pane close, and exit 0
(src/review.rs:232-237, :349-359). On delivery failure the process SHALL
report the hard error
`failed to send annotations to agent pane <target>: <e>; output kept at <path>`,
keep the output file, print the error, hold the pane, and exit 1
(src/review.rs:239-242, src/main.rs:20-29).

#### Scenario: Successful delivery tears the pane down and removes the file

- **WHEN** the herdr agent prompt command succeeds
- **THEN** the `herdr-revdiff: sent review annotations to agent pane <target>`
  line is printed to stderr, the output file is removed, a best-effort pane
  close is attempted, and the process exits 0

#### Scenario: Failed delivery keeps the file and holds the pane

- **WHEN** the herdr agent prompt command fails
- **THEN** the error message names the target, the error, and the kept output
  path; the file is left in place; the pane is held; the process exits 1

### Requirement: Non-delivery outcomes follow fixed message and retention rules

On `NoAnnotations` the process SHALL print `herdr-revdiff: no annotations` to
stderr, remove the output file, attempt a best-effort pane close, and finish
successfully (src/review.rs:245-250). On `NoAgent` it SHALL print
`herdr-revdiff: no agent in focused pane; annotations kept at <path>` to
stderr, keep the output file, and hold the pane (src/review.rs:251-258). On
`Failed(rc)` it SHALL report the error
`revdiff failed with exit code <rc>; output kept at <path>`, keep the file,
and the pane-mode error path in main SHALL print the error, hold the pane,
and exit 1 (src/review.rs:259-262). The output file is therefore retained on
`Failed`, `NoAgent`, delivery failure, a read failure with a nonzero exit
code (a read failure at exit code 0 warns, yields empty output, and the
`NoAnnotations` path then removes the file, tolerating an already-absent one),
and a killed child, and removed only on delivery success and
`NoAnnotations`. (src/review.rs:190-205, :245-262, :349-353)

#### Scenario: No annotations cleans up silently

- **WHEN** the outcome is `NoAnnotations`
- **THEN** `herdr-revdiff: no annotations` is printed to stderr, the output
  file is removed, and the pane close is attempted

#### Scenario: Unreadable output at a clean exit counts as no annotations

- **WHEN** revdiff exits 0 but its output file cannot be read
- **THEN** a `herdr-revdiff: warning: cannot read <path>: <err>` line is
  written to stderr and the run proceeds as `NoAnnotations`

#### Scenario: Failed run surfaces the code and keeps evidence

- **WHEN** the outcome is `Failed(rc)`
- **THEN** the error names the exit code and the kept output path, the file
  stays on disk, and the pane is held before exiting 1

### Requirement: Attention outcomes hold the pane, and pane close is best-effort with a tab-close fallback

The `NoAgent` outcome SHALL call `hold_pane()` after printing its message,
and delivery failures and `Failed(rc)` runs reach the same hold through the
pane-mode error path in main; `hold_pane`'s mechanics — prompt line, stdin
block, EOF behavior — belong to the mode-dispatch capability and are not
restated here (src/review.rs:256; src/main.rs:25-27). `close_pane` SHALL
first try `herdr plugin pane close <HERDR_PANE_ID>`, skipping that attempt
when the variable is unset; on failure it SHALL log the full line
`herdr-revdiff: failed to close pane <pane>: <err>` and fall through to
`herdr tab close <HERDR_TAB_ID>`; all close failures are logged and never
propagated (src/review.rs:266-280, src/herdr.rs:124-131).
Success-path closes are best-effort because plugin panes are expected to
auto-close when the pane process exits.
UNVERIFIED: this auto-close mechanism (herdr-side `RuntimeExitAction::ClosePane`)
is attested only in AGENTS.md:15; the plugin manifest declares no such key, and
the herdr-side behavior was not verified from herdr sources.

#### Scenario: Pane close falls back to tab close

- **WHEN** `herdr plugin pane close` fails for a pane with
  `HERDR_PANE_ID` set
- **THEN** the failure is logged and `herdr tab close <HERDR_TAB_ID>` is
  attempted; if that also fails, the error is logged only

### Requirement: An internally inconsistent Deliver outcome is rejected as an error

A `Deliver` outcome that reaches execution without a caller pane SHALL produce
the internal error
`internal: deliver outcome without a caller pane`; this state is unreachable
given `decide`'s rules, so it guards against future inconsistency
(src/review.rs:225-230).

#### Scenario: Deliver without a caller pane

- **WHEN** the outcome is `Deliver` but the caller pane value is absent at
  execution time
- **THEN** the internal error is reported instead of attempting delivery
