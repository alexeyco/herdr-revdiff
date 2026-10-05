# Development

Development happens against a locally linked plugin; no GitHub install needed.

## Dev loop

```
make build                       # produces ./target/release/herdr-revdiff
herdr plugin link .              # register the working directory as a plugin
herdr plugin action list --plugin revdiff
herdr plugin action invoke revdiff.review
herdr plugin log list --plugin revdiff
```

`plugin link` does not run build commands, so rebuild after every source change. Unlink with `herdr plugin unlink revdiff`.

The action calls back into herdr through `HERDR_BIN_PATH` (set by herdr), not through a bare `herdr` on `PATH`. That is the binary herdr launched, so callbacks work even when the plugin is linked.

To exercise the review flow you need a [revdiff](https://github.com/umputun/revdiff) >= 1.12 binary. The action looks it up via the `REVDIFF_BIN` environment override when it names an executable file, otherwise the first executable `revdiff` on `$PATH`, and runs it by absolute path.

## Checks

```
make lint        # cargo fmt --check + cargo clippy
make test        # cargo test
make build       # release build
make fmt-check   # prettier check
```

`make check` runs all of them. CI runs the same commands verbatim (it does not use make); see [.github/workflows/ci.yml](../.github/workflows/ci.yml). Prettier is pinned to `prettier@3.9.6` in the Makefile and in CI, so the local formatting check matches what CI enforces.

`cargo test --locked` runs the unit tests plus the integration suite in `tests/`; the integration tests drive the real binary with stub `herdr` and `revdiff` shell scripts, so they need no network and no herdr install.

## Manual smoke test

```
make build
herdr plugin link .
herdr plugin list                     # shows revdiff 0.1.0
herdr plugin action invoke revdiff.review
```

1. Bind `prefix+shift+r` in `~/.config/herdr/config.toml`:

   ```toml
   [[keys.command]]
   key = "prefix+shift+r"
   type = "plugin_action"
   command = "revdiff.review"
   ```

2. Happy path: focus an agent pane, review a diff in the `revdiff` tab, annotate, and quit; the annotations arrive in the agent as one prompt prefixed `Review annotations from revdiff:`.
3. Quit revdiff without annotating → nothing is sent.
4. Close the `revdiff` tab early → the review is cancelled; nothing is sent.
5. Invoke the action from a pane with no agent → the annotations are kept on disk, the path is printed in the plugin log, and the review pane waits for Enter before closing.
6. Override the binary: export `REVDIFF_BIN=/path/to/revdiff` in the session where herdr is launched (the daemon inherits it), restart herdr, then invoke the action.
7. Inspect the log: `herdr plugin log list --plugin revdiff`.

## Version sync

The version lives in two files that must match:

- `Cargo.toml`
- `herdr-plugin.toml`

Bump both in the same commit. A release is cut by pushing a tag `v<version>` (e.g. `v0.1.0`); the release workflow fails if the tag does not match both files.

CI enforces this on every PR: a `version-sync` job fails when the two `version = "..."` values diverge, so drift is caught before a release tag, not only at tag time. An `msrv` job reads `rust-version` from `Cargo.toml`, installs that exact toolchain, and runs `cargo check --locked`.

The release workflow first verifies the tag against both version files, then runs the full CI suite (via `workflow_call`) and only then creates the release and builds artifacts, so a release is gated on the same lint and test matrix as a PR. It ships binaries for four targets: `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`, and `aarch64-apple-darwin`. The macOS binaries are unsigned and not notarized. For release steps, see [CONTRIBUTING.md](../CONTRIBUTING.md).

## Notes

- herdr validates `min_herdr_version` against the running herdr binary; linking with an older herdr is refused.
- Usage and requirements are documented in the [README](../README.md); behavior details in [how-it-works.md](how-it-works.md).
