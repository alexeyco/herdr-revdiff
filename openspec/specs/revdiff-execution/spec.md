# revdiff-execution

## Purpose

This capability covers how the plugin's pane process locates and runs the
revdiff binary and manages the annotation output file. Before spawning, the
pane process resolves revdiff from its live environment — an explicit
`REVDIFF_BIN` override first, then the first executable `revdiff` found on
`PATH` — and canonicalizes the result to an absolute path, because the herdr
daemon that spawns the pane may have a different `PATH` and working directory
than the action process. It then creates a collision-safe output file named
`<pid>-<unix millis>.out` under the plugin state directory, prunes kept
outputs older than seven days, spawns revdiff as a direct child on the pane
tty with fixed `--output` and `--exit-code-on-annotations` arguments, and
reads the child's exit code and output file back for downstream handling.

## Requirements

### Requirement: Resolve the revdiff binary from the pane process environment

The pane process SHALL resolve the revdiff binary immediately before spawning
it, reading the live process environment: the `REVDIFF_BIN` variable takes
precedence, trimmed of surrounding whitespace, and an empty or
whitespace-only value SHALL fall through to `PATH`; otherwise the first
`PATH` entry, scanned in order, that contains an executable file named
`revdiff` SHALL be used. Empty `PATH` entries SHALL be skipped. When neither
source yields a binary, resolution SHALL fail with `revdiff not found on
PATH; install it from https://github.com/umputun/revdiff, or set REVDIFF_BIN
to its path`. `REVDIFF_BIN` is a user-set variable inherited from the herdr
daemon environment; the action never injects it.

Evidence: src/revdiff.rs:8-12, :16-17, :21-46, :51-54; src/review.rs:160;
tests src/revdiff.rs:191-199, :201-230, :243-254, :256-265.

#### Scenario: Executable REVDIFF_BIN wins over PATH

- **WHEN** `REVDIFF_BIN` names an executable file and `PATH` also contains an
  executable `revdiff`
- **THEN** resolution succeeds with the canonicalized `REVDIFF_BIN` path

#### Scenario: Whitespace-only REVDIFF_BIN falls through to PATH

- **WHEN** `REVDIFF_BIN` is set to an empty or whitespace-only string and an
  executable `revdiff` exists on `PATH`
- **THEN** resolution returns the first executable `PATH` hit

#### Scenario: No revdiff anywhere

- **WHEN** `REVDIFF_BIN` is absent and no `PATH` entry contains an executable
  `revdiff`
- **THEN** resolution fails with the not-found message including the install
  hint `https://github.com/umputun/revdiff` and the `REVDIFF_BIN` suggestion

### Requirement: Validate and canonicalize the resolved path

A selected candidate SHALL be a regular file with at least one execute bit
(`mode & 0o111`) on unix; on non-unix platforms any existing regular file
counts as executable. A `REVDIFF_BIN` value that fails this check SHALL be a
hard error shown with the path as the user gave it: `REVDIFF_BIN (<value>) is
not an executable file`. Both a valid `REVDIFF_BIN` value and a `PATH` hit
SHALL be canonicalized to an absolute path before use, and a canonicalization
failure SHALL be a hard error: `found revdiff at <candidate> but cannot
resolve it to an absolute path: <error>`. There SHALL be no working-directory
fallback; a relative `PATH` entry canonicalizes against the process working
directory at resolve time.

Evidence: src/revdiff.rs:1-6, :34-39, :58, :61-71, :73-89; tests
src/revdiff.rs:217-230, :232-240, :276-285.

#### Scenario: Non-executable REVDIFF_BIN is a hard error

- **WHEN** `REVDIFF_BIN` names a file that exists but has no execute bit on
  unix
- **THEN** resolution fails with `REVDIFF_BIN (<value>) is not an executable
file` quoting the value as given, without falling through to `PATH`

#### Scenario: Candidate cannot be canonicalized

- **WHEN** an executable candidate is found but `canonicalize` fails
- **THEN** resolution fails with `found revdiff at <candidate> but cannot
resolve it to an absolute path: <error>` and no cwd-relative guess is used

### Requirement: Create the annotation output file under the state directory

The pane process SHALL write revdiff annotations to
`<state_dir>/reviews/<pid>-<unix millis>.out`, where `state_dir` is the
trimmed, non-blank value of `HERDR_PLUGIN_STATE_DIR` or the system temporary
directory when that variable is unset or blank, the pid prefix keeps reviews
started in the same millisecond collision-safe, and the filename is generated
after pruning. The pane process SHALL create the `reviews` directory with
`create_dir_all`, failing with `failed to create state dir <path>: <error>`
when creation fails, and SHALL fail with `system clock error: <error>` if the
system clock is before the Unix epoch when the millisecond stamp is taken.

Evidence: src/review.rs:9-12, :154-158, :291-295, :327-338, :340-345; test
src/review.rs:507-512.

#### Scenario: Output path layout

- **WHEN** `HERDR_PLUGIN_STATE_DIR` is set to `/state` and the pane process
  has pid 4242 at unix millisecond 1700000000000
- **THEN** the output file path is `/state/reviews/4242-1700000000000.out`

#### Scenario: State directory created on demand

- **WHEN** `<state_dir>/reviews` does not exist at pane start
- **THEN** the pane process creates it before spawning revdiff

#### Scenario: Missing state-dir variable uses the temp directory

- **WHEN** `HERDR_PLUGIN_STATE_DIR` is unset, empty, or whitespace-only
- **THEN** the output file lives under `reviews/` in the system temporary
  directory

### Requirement: Prune kept review outputs older than seven days at pane start

Before generating the output file name, the pane process SHALL prune top-level
`*.out` files in `<state_dir>/reviews` whose modification time is at least
`REVIEW_MAX_AGE` (7 days, `7 * 24 * 60 * 60` seconds) old. Pruning is
best-effort: an unreadable directory, entries without a `.out` extension, and
files whose mtime cannot be read or lies in the future SHALL be skipped
silently, and a failed removal SHALL log
`herdr-revdiff: warning: cannot remove <path>: <error>` without aborting the
review.

Evidence: src/review.rs:28, :157-158, :297-325; tests src/review.rs:568-592.

#### Scenario: Old output removed, fresh output kept

- **WHEN** `reviews/` contains an 8-day-old `old.out`, a current `fresh.out`,
  and a non-`.out` file, and a pane starts
- **THEN** `old.out` is removed, `fresh.out` and the non-`.out` file remain

#### Scenario: Pruning never blocks the review

- **WHEN** removal of an old `.out` file fails (e.g. permissions)
- **THEN** a warning is logged and the pane continues to spawn revdiff

### Requirement: Spawn revdiff as a direct child on the pane tty

The pane process SHALL spawn the resolved revdiff binary with
`Command::new(<bin>).args(<argv>).current_dir(<review dir>).status()`,
keeping the plugin process as revdiff's direct parent. Child stdout and
stderr SHALL NOT be piped; they SHALL inherit the pane tty. The argv SHALL be
`<bin> [mode flags…] --output <out> --exit-code-on-annotations [positional]`:
the fixed trailing flags `--output <out>` (the annotation file) and
`--exit-code-on-annotations` come after the mode flags, and the mode's
positional revision, when present, goes last so both strict and lenient
argument parsers accept it. A spawn failure SHALL be a hard error:
`failed to run <bin> in <dir>: <error>`.

Evidence: src/review.rs:5-7, :86-99, :161-167; tests src/review.rs:400-462.

#### Scenario: Fixed trailing flags and positional placement

- **WHEN** the review mode defines flags and a positional revision
- **THEN** the child argv is `<bin> <mode flags…> --output <out>
--exit-code-on-annotations <positional>` with the positional last

#### Scenario: revdiff cannot be executed

- **WHEN** spawning the resolved binary fails (e.g. it was removed between
  resolution and spawn)
- **THEN** the pane process reports `failed to run <bin> in <dir>: <error>`

### Requirement: Read the child's exit code and annotation output

The pane process SHALL take revdiff's exit code directly from the child
status with no intermediate file. When the child was killed and reports no
exit code, the output file SHALL NOT be read and the run SHALL end with
`revdiff terminated without an exit code (killed?); output kept at <path>`.
When an exit code is present, the pane process SHALL read the output file
lossily (`String::from_utf8_lossy`, replacing invalid UTF-8). A missing or
unreadable output file with exit code 0 SHALL log
`herdr-revdiff: warning: cannot read <path>: <error>` and count as empty
output; with any other exit code it SHALL be a hard error
`cannot read revdiff output <path>: <error>` and the file SHALL be kept.

Evidence: src/review.rs:169-175, :187-205, :217-222; tests
src/review.rs:515-528, :531-537.

#### Scenario: Killed child keeps the output

- **WHEN** the revdiff child is killed and has no exit code
- **THEN** the run fails with `revdiff terminated without an exit code
(killed?); output kept at <path>` and the output file is not read

#### Scenario: Invalid UTF-8 in the output file

- **WHEN** the output file contains bytes that are not valid UTF-8
- **THEN** they are replaced with U+FFFD and the read still succeeds

#### Scenario: Output file unreadable after a clean exit

- **WHEN** revdiff exits 0 but the output file cannot be read
- **THEN** a warning is logged and empty output is used, not an error
