//! End-to-end tests for the dual-mode plugin binary.
//!
//! Each test spawns the real binary (`CARGO_BIN_EXE_herdr-revdiff`) with a
//! hermetic environment, a stub `herdr` callback, and a stub `revdiff` child.
//! The stubs append their argv to `$STUB_LOG`; assertions grep that log.
//! Everything runs in a unique temp directory and needs no network or global
//! git config.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

/// Stub `herdr`: logs every call, answers `plugin pane open` with a canned JSON
/// reply, always exits 0.
const HERDR_STUB: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$STUB_LOG"
if [ "$1" = "plugin" ] && [ "$2" = "pane" ] && [ "$3" = "open" ]; then
  printf '%s\n' '{"result":{"plugin_pane":{"pane":{"pane_id":"pane-stub","tab_id":"tab-stub"}}}}'
fi
exit 0
"#;

/// Stub `revdiff`: logs `REVDIFF <argv>`, finds the `--output` path and (unless
/// `REV_EMPTY=1`) writes annotations to it, then exits `$REV_RC` (default 0).
/// `REV_BIG=1` writes ~100000 bytes instead.
const REVDIFF_STUB: &str = r#"#!/bin/sh
printf 'REVDIFF %s\n' "$*" >> "$STUB_LOG"
out=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "--output" ]; then
    out="$arg"
  fi
  prev="$arg"
done
if [ "${REV_EMPTY:-0}" = "1" ]; then
  exit "${REV_RC:-0}"
fi
if [ "${REV_BIG:-0}" = "1" ]; then
  head -c 100000 /dev/zero | tr '\0' 'a' > "$out"
  exit "${REV_RC:-0}"
fi
printf '## src/main.rs:1 (+)\nbody line\n' > "$out"
exit "${REV_RC:-0}"
"#;

/// A unique scratch directory removed (best-effort) when the test ends.
struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "herdr-revdiff-e2e-{tag}-{}-{n}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    /// Writes the `herdr` and `revdiff` stubs and returns `(herdr, revdiff,
    /// stub_log)` paths.
    fn stubs(&self) -> (PathBuf, PathBuf, PathBuf) {
        let herdr = self.dir.join("herdr");
        let revdiff = self.dir.join("revdiff");
        write_executable(&herdr, HERDR_STUB);
        write_executable(&revdiff, REVDIFF_STUB);
        (herdr, revdiff, self.dir.join("stub.log"))
    }

    /// Creates a subdirectory and returns its path.
    fn subdir(&self, name: &str) -> PathBuf {
        let p = self.dir.join(name);
        fs::create_dir_all(&p).unwrap();
        p
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn write_executable(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// Spawns the plugin binary with a cleared environment plus PATH/HOME/git
/// isolation and the caller-supplied env pairs. `stdin` is null so the hold
/// paths return at once instead of hanging.
fn run_plugin(envs: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_herdr-revdiff"));
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_clear();
    cmd.env("PATH", std::env::var("PATH").unwrap_or_default());
    cmd.env("HOME", std::env::temp_dir());
    cmd.env("GIT_CONFIG_GLOBAL", "/dev/null");
    cmd.env("GIT_CONFIG_SYSTEM", "/dev/null");
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    for (key, value) in envs {
        cmd.env(key, value);
    }
    cmd.output().unwrap()
}

/// Runs git in `dir` with global config disabled and an inline identity where
/// needed. Panics with git's stderr on failure.
fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn commit(dir: &Path, message: &str) {
    git(
        dir,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "commit",
            "-q",
            "-m",
            message,
        ],
    );
}

/// Initializes a repository with two commits and a clean tree, so the probe
/// selects `HEAD~1`.
fn clean_multi_commit_repo(dir: &Path) {
    git(dir, &["init", "-q"]);
    fs::write(dir.join("a.txt"), "a\n").unwrap();
    git(dir, &["add", "a.txt"]);
    commit(dir, "base");
    fs::write(dir.join("a.txt"), "b\n").unwrap();
    git(dir, &["add", "a.txt"]);
    commit(dir, "second");
}

fn read_log(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// `*.out` files directly under `dir`.
fn out_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("out"))
        .collect()
}

/// The `REVDIFF ...` log line, if any.
fn revdiff_line(log: &str) -> &str {
    log.lines()
        .find(|l| l.starts_with("REVDIFF "))
        .unwrap_or("")
}

/// Pane-mode env pairs shared by the happy-path tests: entrypoint, repo, state,
/// pane/tab ids, both stubs and the stub log.
fn pane_env<'a>(
    repo: &'a Path,
    state: &'a Path,
    herdr: &'a Path,
    revdiff: &'a Path,
    log: &'a Path,
) -> Vec<(&'a str, &'a str)> {
    vec![
        ("HERDR_PLUGIN_ENTRYPOINT_ID", "review"),
        ("HERDR_REVIEWS_DIR", repo.to_str().unwrap()),
        ("HERDR_PLUGIN_STATE_DIR", state.to_str().unwrap()),
        ("HERDR_PANE_ID", "pane-7"),
        ("HERDR_TAB_ID", "tab-7"),
        ("HERDR_BIN_PATH", herdr.to_str().unwrap()),
        ("REVDIFF_BIN", revdiff.to_str().unwrap()),
        ("STUB_LOG", log.to_str().unwrap()),
    ]
}

#[test]
fn pane_rc10_delivers_annotations_and_closes_pane() {
    let fx = Fixture::new("rc10-deliver");
    let repo = fx.subdir("repo");
    clean_multi_commit_repo(&repo);
    let (herdr, revdiff, log) = fx.stubs();
    let state = fx.subdir("state");

    let mut env = pane_env(&repo, &state, &herdr, &revdiff, &log);
    env.extend([
        ("HERDR_REVIEWS_CALLER_PANE", "caller-1"),
        ("HERDR_REVIEWS_CALLER_AGENT", "claude"),
        ("REV_RC", "10"),
    ]);
    let out = run_plugin(&env);

    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("herdr-revdiff: reviewing last commit"),
        "stdout: {}",
        stdout(&out)
    );
    let logged = read_log(&log);
    assert!(
        logged.contains("agent prompt caller-1 Review annotations from revdiff:"),
        "{logged}"
    );
    assert!(logged.contains("plugin pane close pane-7"), "{logged}");
    assert!(revdiff_line(&logged).contains("HEAD~1"), "{logged}");
    assert!(out_files(&state.join("reviews")).is_empty());
}

#[test]
fn pane_rc0_with_output_delivers_annotations() {
    let fx = Fixture::new("rc0-deliver");
    let repo = fx.subdir("repo");
    clean_multi_commit_repo(&repo);
    let (herdr, revdiff, log) = fx.stubs();
    let state = fx.subdir("state");

    let mut env = pane_env(&repo, &state, &herdr, &revdiff, &log);
    env.extend([
        ("HERDR_REVIEWS_CALLER_PANE", "caller-1"),
        ("HERDR_REVIEWS_CALLER_AGENT", "claude"),
    ]);
    let out = run_plugin(&env);

    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let logged = read_log(&log);
    assert!(
        logged.contains("agent prompt caller-1 Review annotations from revdiff:"),
        "{logged}"
    );
    assert!(out_files(&state.join("reviews")).is_empty());
}

#[test]
fn pane_rc0_empty_is_no_annotations() {
    let fx = Fixture::new("rc0-empty");
    let repo = fx.subdir("repo");
    clean_multi_commit_repo(&repo);
    let (herdr, revdiff, log) = fx.stubs();
    let state = fx.subdir("state");

    let mut env = pane_env(&repo, &state, &herdr, &revdiff, &log);
    env.extend([
        ("HERDR_REVIEWS_CALLER_PANE", "caller-1"),
        ("HERDR_REVIEWS_CALLER_AGENT", "claude"),
        ("REV_EMPTY", "1"),
    ]);
    let out = run_plugin(&env);

    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let logged = read_log(&log);
    assert!(!logged.contains("agent prompt"), "{logged}");
    assert!(logged.contains("plugin pane close pane-7"), "{logged}");
    assert!(out_files(&state.join("reviews")).is_empty());
    assert!(
        stderr(&out).contains("warning: cannot read"),
        "stderr: {}",
        stderr(&out)
    );
}

#[test]
fn pane_rc1_keeps_output_and_fails() {
    let fx = Fixture::new("rc1-fail");
    let repo = fx.subdir("repo");
    clean_multi_commit_repo(&repo);
    let (herdr, revdiff, log) = fx.stubs();
    let state = fx.subdir("state");

    let mut env = pane_env(&repo, &state, &herdr, &revdiff, &log);
    env.extend([
        ("HERDR_REVIEWS_CALLER_PANE", "caller-1"),
        ("HERDR_REVIEWS_CALLER_AGENT", "claude"),
        ("REV_RC", "1"),
    ]);
    let out = run_plugin(&env);

    assert_eq!(out.status.code(), Some(1), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("revdiff failed with exit code 1"),
        "stderr: {}",
        stderr(&out)
    );
    let logged = read_log(&log);
    assert!(!logged.contains("agent prompt"), "{logged}");
    assert_eq!(out_files(&state.join("reviews")).len(), 1);
}

#[test]
fn pane_rc10_without_caller_keeps_output() {
    let fx = Fixture::new("rc10-noagent");
    let repo = fx.subdir("repo");
    clean_multi_commit_repo(&repo);
    let (herdr, revdiff, log) = fx.stubs();
    let state = fx.subdir("state");

    let mut env = pane_env(&repo, &state, &herdr, &revdiff, &log);
    env.push(("REV_RC", "10"));
    let out = run_plugin(&env);

    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("no agent in focused pane; annotations kept at"),
        "stderr: {}",
        stderr(&out)
    );
    let logged = read_log(&log);
    assert!(!logged.contains("agent prompt"), "{logged}");
    assert_eq!(out_files(&state.join("reviews")).len(), 1);
}

#[test]
fn pane_rc10_missing_output_is_error() {
    let fx = Fixture::new("rc10-missing");
    let repo = fx.subdir("repo");
    clean_multi_commit_repo(&repo);
    let (herdr, revdiff, log) = fx.stubs();
    let state = fx.subdir("state");

    let mut env = pane_env(&repo, &state, &herdr, &revdiff, &log);
    env.extend([
        ("HERDR_REVIEWS_CALLER_PANE", "caller-1"),
        ("HERDR_REVIEWS_CALLER_AGENT", "claude"),
        ("REV_EMPTY", "1"),
        ("REV_RC", "10"),
    ]);
    let out = run_plugin(&env);

    assert_ne!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("cannot read revdiff output"),
        "stderr: {}",
        stderr(&out)
    );
    let logged = read_log(&log);
    assert!(!logged.contains("agent prompt"), "{logged}");
}

#[test]
fn pane_untracked_repo_passes_untracked() {
    let fx = Fixture::new("untracked");
    let repo = fx.subdir("repo");
    git(&repo, &["init", "-q"]);
    fs::write(repo.join("base.txt"), "b\n").unwrap();
    git(&repo, &["add", "base.txt"]);
    commit(&repo, "base");
    fs::write(repo.join("new.txt"), "n\n").unwrap();
    let (herdr, revdiff, log) = fx.stubs();
    let state = fx.subdir("state");

    let mut env = pane_env(&repo, &state, &herdr, &revdiff, &log);
    env.push(("REV_EMPTY", "1"));
    let out = run_plugin(&env);

    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let logged = read_log(&log);
    assert!(revdiff_line(&logged).contains("--untracked"), "{logged}");
}

#[test]
fn pane_rc10_big_output_is_capped() {
    let fx = Fixture::new("big-cap");
    let repo = fx.subdir("repo");
    clean_multi_commit_repo(&repo);
    let (herdr, revdiff, log) = fx.stubs();
    let state = fx.subdir("state");

    let mut env = pane_env(&repo, &state, &herdr, &revdiff, &log);
    env.extend([
        ("HERDR_REVIEWS_CALLER_PANE", "caller-1"),
        ("HERDR_REVIEWS_CALLER_AGENT", "claude"),
        ("REV_RC", "10"),
        ("REV_BIG", "1"),
    ]);
    let out = run_plugin(&env);

    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let logged = read_log(&log);
    assert!(
        logged.len() < 100_000,
        "delivered log grew to {}",
        logged.len()
    );
    let note = format!(
        "(output truncated; full annotations kept at {}/",
        state.join("reviews").display()
    );
    assert!(
        logged.contains(&note),
        "missing truncation note in {logged}"
    );
    assert!(logged.contains(".out)"), "{logged}");
    assert!(out_files(&state.join("reviews")).is_empty());
}

#[test]
fn action_opens_pane_with_env_and_focus() {
    let fx = Fixture::new("action");
    let repo = fx.subdir("repo");
    let (herdr, _revdiff, log) = fx.stubs();
    let ctx = format!(
        r#"{{"workspace_id":"ws-1","focused_pane_id":"p1","focused_pane_agent":"claude","worktree":{{"checkout_path":"{}"}}}}"#,
        repo.display()
    );

    let out = run_plugin(&[
        ("HERDR_PLUGIN_ACTION_ID", "review"),
        ("HERDR_PLUGIN_CONTEXT_JSON", &ctx),
        ("HERDR_BIN_PATH", herdr.to_str().unwrap()),
        ("STUB_LOG", log.to_str().unwrap()),
    ]);

    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("opened review pane pane-stub in tab tab-stub"),
        "stderr: {}",
        stderr(&out)
    );
    let logged = read_log(&log);
    let open = logged
        .lines()
        .find(|l| l.starts_with("plugin pane open"))
        .unwrap_or("");
    assert!(open.contains("--workspace ws-1"), "{open}");
    assert!(
        open.contains(&format!("--cwd {}", repo.display())),
        "{open}"
    );
    assert!(
        open.contains(&format!("--env HERDR_REVIEWS_DIR={}", repo.display())),
        "{open}"
    );
    assert!(
        open.contains("--env HERDR_REVIEWS_CALLER_PANE=p1"),
        "{open}"
    );
    assert!(
        open.contains("--env HERDR_REVIEWS_CALLER_AGENT=claude"),
        "{open}"
    );
    assert!(open.ends_with("--focus"), "{open}");
    assert!(logged.contains("tab focus tab-stub"), "{logged}");
}

#[test]
fn outside_herdr_errors() {
    let out = run_plugin(&[]);
    assert_eq!(out.status.code(), Some(1), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("invoked outside herdr plugin context"),
        "stderr: {}",
        stderr(&out)
    );
}
