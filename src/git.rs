//! Read-only git probes and review-mode selection.
//!
//! Detection runs in the pane process, in the pane's working directory (the
//! repository), before revdiff starts. Every probe is read-only: the index and
//! the working tree are never mutated.

use std::path::Path;
use std::process::{Command, Output};

/// Which revdiff invocation to use for the resolved directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Bare `revdiff`: the directory is not a git work tree.
    Default,
    /// `--all-files`: a git repository with no commits yet (or a single commit
    /// with a clean tree, where `HEAD~1` does not exist).
    AllFiles,
    /// Positional `HEAD~1`: a clean tree with a parent commit, so the last
    /// commit is reviewed.
    HeadPrev,
    /// `--staged`: staged changes and nothing else.
    Staged,
    /// `--untracked`: unstaged changes and/or untracked files (also covers a
    /// mixed staged+unstaged tree).
    Untracked,
}

impl Mode {
    /// revdiff flags for this mode; never combined with a positional.
    pub(crate) fn flags(self) -> &'static [&'static str] {
        match self {
            Mode::Default | Mode::HeadPrev => &[],
            Mode::AllFiles => &["--all-files"],
            Mode::Staged => &["--staged"],
            Mode::Untracked => &["--untracked"],
        }
    }

    /// Optional positional revision, placed after every flag.
    pub(crate) fn positional(self) -> Option<&'static str> {
        match self {
            Mode::HeadPrev => Some("HEAD~1"),
            _ => None,
        }
    }

    /// Short human description used in the action log line.
    pub(crate) fn description(self) -> &'static str {
        match self {
            Mode::Default => "bare working tree",
            Mode::AllFiles => "all files",
            Mode::HeadPrev => "last commit",
            Mode::Staged => "staged changes",
            Mode::Untracked => "untracked + working tree",
        }
    }
}

/// Read-only git state used to pick a review mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct GitStatus {
    pub(crate) repo: bool,
    pub(crate) has_head: bool,
    pub(crate) has_head_prev: bool,
    pub(crate) unstaged: bool,
    pub(crate) staged: bool,
    pub(crate) untracked: bool,
}

/// Picks the review mode from a git state. Pure: every branch is unit-tested.
pub(crate) fn select_mode(s: &GitStatus) -> Mode {
    if !s.repo {
        return Mode::Default;
    }
    if !s.has_head {
        return Mode::AllFiles;
    }
    if !s.unstaged && !s.staged && !s.untracked {
        return if s.has_head_prev {
            Mode::HeadPrev
        } else {
            Mode::AllFiles
        };
    }
    if !s.unstaged && !s.untracked {
        return Mode::Staged;
    }
    Mode::Untracked
}

/// Probes `dir` read-only and returns its git state. On any unexpected git
/// failure the review stays resilient: one warning is written to stderr and a
/// default (non-repo) status is returned, which selects `Mode::Default`.
pub(crate) fn probe(dir: &Path) -> GitStatus {
    match try_probe(dir) {
        Ok(status) => status,
        Err(e) => {
            eprintln!(
                "herdr-revdiff: git probe failed ({}): {}; falling back to default mode",
                e.cmd, e.err
            );
            GitStatus::default()
        }
    }
}

/// A failed probe: the command that ran and what went wrong.
struct ProbeError {
    cmd: String,
    err: String,
}

impl ProbeError {
    fn new(cmd: String, err: impl Into<String>) -> Self {
        Self {
            cmd,
            err: err.into(),
        }
    }
}

fn try_probe(dir: &Path) -> Result<GitStatus, ProbeError> {
    if !is_repo(dir)? {
        return Ok(GitStatus::default());
    }
    if !has_rev(dir, "HEAD")? {
        return Ok(GitStatus {
            repo: true,
            ..GitStatus::default()
        });
    }
    Ok(GitStatus {
        repo: true,
        has_head: true,
        has_head_prev: has_rev(dir, "HEAD~1")?,
        unstaged: diff_present(dir, &[])?,
        staged: diff_present(dir, &["--cached"])?,
        untracked: has_untracked(dir)?,
    })
}

/// `rev-parse --is-inside-work-tree` exits 0 and prints `true` inside a work
/// tree; any other exit is the expected "not a repository" signal.
fn is_repo(dir: &Path) -> Result<bool, ProbeError> {
    let args = ["rev-parse", "--is-inside-work-tree"];
    let out = run_git(dir, &args)?;
    Ok(out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "true")
}

/// `rev-parse -q --verify <rev>`: exit 0 means the revision exists.
fn has_rev(dir: &Path, rev: &str) -> Result<bool, ProbeError> {
    let args = ["rev-parse", "-q", "--verify", rev];
    let out = run_git(dir, &args)?;
    Ok(out.status.success())
}

/// A quiet diff exits 0 with no changes and 1 with changes; anything else is
/// unexpected.
fn diff_present(dir: &Path, extra: &[&str]) -> Result<bool, ProbeError> {
    let mut args = vec!["diff"];
    args.extend_from_slice(extra);
    args.push("--quiet");
    let out = run_git(dir, &args)?;
    match out.status.code() {
        Some(0) => Ok(false),
        Some(1) => Ok(true),
        _ => Err(unexpected(&args, &out)),
    }
}

/// `ls-files --others --exclude-standard` lists untracked files on stdout.
fn has_untracked(dir: &Path) -> Result<bool, ProbeError> {
    let args = ["ls-files", "--others", "--exclude-standard"];
    let out = run_git(dir, &args)?;
    if !out.status.success() {
        return Err(unexpected(&args, &out));
    }
    Ok(!String::from_utf8_lossy(&out.stdout).trim().is_empty())
}

fn run_git(dir: &Path, args: &[&str]) -> Result<Output, ProbeError> {
    Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| ProbeError::new(command(args), e.to_string()))
}

fn unexpected(args: &[&str], out: &Output) -> ProbeError {
    let stderr = String::from_utf8_lossy(&out.stderr);
    ProbeError::new(
        command(args),
        format!("exit {}: {}", out.status, stderr.trim()),
    )
}

fn command(args: &[&str]) -> String {
    format!("git {}", args.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// A canonicalized scratch directory, removed when the test ends.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir = std::env::temp_dir().join(format!(
                "herdr-revdiff-git-test-{tag}-{}-{nanos}",
                std::process::id()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(std::fs::canonicalize(&dir).unwrap())
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Runs git in `dir` with an inline identity so no global git config is
    /// required. Panics on a non-zero exit with git's stderr.
    fn git(dir: &Path, args: &[&str]) {
        let mut full = vec!["-c", "user.name=t", "-c", "user.email=t@example.com"];
        full.extend_from_slice(args);
        let out = Command::new("git")
            .args(&full)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn init_and_commit(dir: &Path, file: &str, contents: &str) {
        git(dir, &["init", "-q"]);
        std::fs::write(dir.join(file), contents).unwrap();
        git(dir, &["add", file]);
        git(dir, &["commit", "-q", "-m", "base"]);
    }

    // Test-only helper building a full GitStatus literal; the six booleans are the
    // whole struct, and a builder would be more code than the literal.
    #[allow(clippy::too_many_arguments)]
    fn status(
        repo: bool,
        has_head: bool,
        has_head_prev: bool,
        unstaged: bool,
        staged: bool,
        untracked: bool,
    ) -> GitStatus {
        GitStatus {
            repo,
            has_head,
            has_head_prev,
            unstaged,
            staged,
            untracked,
        }
    }

    #[test]
    fn non_git_selects_default() {
        assert_eq!(select_mode(&GitStatus::default()), Mode::Default);
    }

    #[test]
    fn repo_without_commits_selects_all_files() {
        assert_eq!(
            select_mode(&status(true, false, false, false, false, false)),
            Mode::AllFiles
        );
    }

    #[test]
    fn clean_single_commit_selects_all_files() {
        assert_eq!(
            select_mode(&status(true, true, false, false, false, false)),
            Mode::AllFiles
        );
    }

    #[test]
    fn clean_multi_commit_selects_head_prev() {
        assert_eq!(
            select_mode(&status(true, true, true, false, false, false)),
            Mode::HeadPrev
        );
    }

    #[test]
    fn staged_only_selects_staged() {
        assert_eq!(
            select_mode(&status(true, true, true, false, true, false)),
            Mode::Staged
        );
    }

    #[test]
    fn untracked_only_selects_untracked() {
        assert_eq!(
            select_mode(&status(true, true, true, false, false, true)),
            Mode::Untracked
        );
    }

    #[test]
    fn mixed_staged_and_unstaged_selects_untracked() {
        assert_eq!(
            select_mode(&status(true, true, true, true, true, false)),
            Mode::Untracked
        );
    }

    #[test]
    fn git_error_fallback_status_selects_default() {
        // `probe` returns `GitStatus::default()` on any unexpected failure.
        assert_eq!(select_mode(&GitStatus::default()), Mode::Default);
    }

    #[test]
    fn modes_map_to_flags_and_positional() {
        assert_eq!(Mode::Default.flags(), &[] as &[&str]);
        assert_eq!(Mode::Default.positional(), None);
        assert_eq!(Mode::AllFiles.flags(), &["--all-files"] as &[&str]);
        assert_eq!(Mode::AllFiles.positional(), None);
        assert_eq!(Mode::HeadPrev.flags(), &[] as &[&str]);
        assert_eq!(Mode::HeadPrev.positional(), Some("HEAD~1"));
        assert_eq!(Mode::Staged.flags(), &["--staged"] as &[&str]);
        assert_eq!(Mode::Staged.positional(), None);
        assert_eq!(Mode::Untracked.flags(), &["--untracked"] as &[&str]);
        assert_eq!(Mode::Untracked.positional(), None);
    }

    #[test]
    fn probe_non_repo_dir_is_default() {
        let dir = TempDir::new("nonrepo");
        let status = probe(dir.path());
        assert_eq!(status, GitStatus::default());
        assert!(!status.repo);
        assert_eq!(select_mode(&status), Mode::Default);
    }

    #[test]
    fn probe_repo_without_commits_selects_all_files() {
        let dir = TempDir::new("nocommits");
        git(dir.path(), &["init", "-q"]);
        let status = probe(dir.path());
        assert!(status.repo);
        assert!(!status.has_head);
        assert_eq!(select_mode(&status), Mode::AllFiles);
    }

    #[test]
    fn probe_clean_multi_commit_selects_head_prev() {
        let dir = TempDir::new("clean");
        init_and_commit(dir.path(), "a.txt", "a\n");
        std::fs::write(dir.path().join("a.txt"), "b\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-q", "-m", "second"]);
        let status = probe(dir.path());
        assert_eq!(
            status,
            GitStatus {
                repo: true,
                has_head: true,
                has_head_prev: true,
                unstaged: false,
                staged: false,
                untracked: false,
            }
        );
        assert_eq!(select_mode(&status), Mode::HeadPrev);
    }

    #[test]
    fn probe_staged_only_selects_staged() {
        let dir = TempDir::new("staged");
        init_and_commit(dir.path(), "base.txt", "b\n");
        std::fs::write(dir.path().join("added.txt"), "n\n").unwrap();
        git(dir.path(), &["add", "added.txt"]);
        let status = probe(dir.path());
        assert_eq!(
            status,
            GitStatus {
                repo: true,
                has_head: true,
                has_head_prev: false,
                unstaged: false,
                staged: true,
                untracked: false,
            }
        );
        assert_eq!(select_mode(&status), Mode::Staged);
    }

    #[test]
    fn probe_untracked_only_selects_untracked() {
        let dir = TempDir::new("untracked");
        init_and_commit(dir.path(), "base.txt", "b\n");
        std::fs::write(dir.path().join("new.txt"), "n\n").unwrap();
        let status = probe(dir.path());
        assert!(status.repo && status.has_head);
        assert_eq!(
            status,
            GitStatus {
                repo: true,
                has_head: true,
                has_head_prev: false,
                unstaged: false,
                staged: false,
                untracked: true,
            }
        );
        assert_eq!(select_mode(&status), Mode::Untracked);
    }
}
