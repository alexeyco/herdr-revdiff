# Contributing

## Setup

- Install [rustup](https://rustup.rs); the pinned toolchain comes from `rust-toolchain.toml` on first build.
- herdr >= 0.9.0 for linking and testing.
- [revdiff](https://github.com/umputun/revdiff) >= 1.12 on `PATH` for exercising the review flow.

## Checks

What CI runs, and what should pass before you push: see [docs/development.md](docs/development.md) → Checks.

Format Markdown with `make fmt`; CI enforces the formatting check.

## Conventions

- Prefer existing ready-made plugins, integrations, and tools over building custom ones; only write new code when nothing suitable already exists.
- No `unwrap`/`expect` in production paths; errors are `Result<(), String>`.
- `#[allow(...)]` only with a comment explaining why.
- Conventional, lowercase commit messages (`feat: ...`, `fix: ...`); no AI co-author trailers.
- Run the checks in [docs/development.md](docs/development.md) → Checks before pushing.

## Workflow

1. Open an issue first for behavior changes (bug report or proposal templates).
2. Keep PRs focused; the PR template has a checklist.
3. For behavior changes, run the manual smoke test in [docs/development.md](docs/development.md) → Manual smoke test.

## Releases

Maintainers tag `v<version>`; see [docs/development.md](docs/development.md) for the version sync rules and what the release workflow does.
