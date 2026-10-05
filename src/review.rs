//! Review orchestration.
//!
//! The binary is dual-mode. Action mode resolves the repository from the plugin
//! context and opens the `revdiff` plugin pane, then exits. Pane mode runs
//! inside that pane: it spawns revdiff as a direct child on the pane tty, reads
//! its exit code, and delivers the annotations to the agent pane that requested
//! the review.
//!
//! Pane mode keeps revdiff's output under `HERDR_PLUGIN_STATE_DIR/reviews` as
//! `<pid>-<unix millis>.out`. The pid keeps two reviews started in the same
//! millisecond from clobbering each other, and kept outputs older than
//! `REVIEW_MAX_AGE` are pruned best-effort at pane start.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::context;
use crate::git::{self, Mode};
use crate::herdr;
use crate::revdiff;

const PROMPT_HEADER: &str = "Review annotations from revdiff:\n\n";
/// Printed before blocking on stdin so an auto-closing plugin pane stays open
/// on error and no-agent paths.
const HOLD_PROMPT: &str = "herdr-revdiff: press Enter to close";
/// Kept review outputs older than this are pruned at pane start.
const REVIEW_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// Cap on the delivered agent prompt, well under ARG_MAX (1 MiB macOS / 2 MiB
/// Linux): annotations travel as a single argv element.
const MAX_PROMPT_BYTES: usize = 65536;

/// What to do with revdiff's result.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Annotations exist and an agent target is available.
    Deliver,
    /// revdiff quit cleanly with nothing to report.
    NoAnnotations,
    /// Annotations exist, but no agent to send them to.
    NoAgent,
    /// revdiff exited with an error; the output is kept.
    Failed(i32),
}

/// Decides the outcome from revdiff's exit code, output, and agent availability.
pub(crate) fn decide(rc: i32, output: &str, has_agent: bool) -> Outcome {
    let annotations = rc == 10 || (rc == 0 && !output.trim().is_empty());
    if annotations {
        if has_agent {
            Outcome::Deliver
        } else {
            Outcome::NoAgent
        }
    } else if rc == 0 {
        Outcome::NoAnnotations
    } else {
        Outcome::Failed(rc)
    }
}

/// Prompt text sent to the agent.
pub(crate) fn prompt_text(output: &str) -> String {
    format!("{PROMPT_HEADER}{}", output.trim())
}

/// Caps the delivered prompt at `max_bytes`, cutting on a UTF-8 boundary and
/// appending a note pointing at the kept output file when truncation happens.
/// Returns the text unchanged when it already fits.
fn cap_prompt(text: &str, max_bytes: usize, kept_path: &Path) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut cut = max_bytes;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let mut capped = text[..cut].to_string();
    capped.push_str(&format!(
        "\n\n(output truncated; full annotations kept at {})",
        kept_path.display()
    ));
    capped
}

/// Full argv for the direct revdiff child: binary first, mode flags, then
/// `--output <out> --exit-code-on-annotations`, and the mode's positional
/// revision last so both strict and lenient parsers accept it.
pub(crate) fn child_argv(revdiff_bin: &str, out_path: &str, mode: Mode) -> Vec<String> {
    let mut argv = vec![revdiff_bin.to_string()];
    argv.extend(mode.flags().iter().map(|f| (*f).to_string()));
    argv.push("--output".to_string());
    argv.push(out_path.to_string());
    argv.push("--exit-code-on-annotations".to_string());
    if let Some(rev) = mode.positional() {
        argv.push(rev.to_string());
    }
    argv
}

/// Action mode: resolve the repository and open the review pane, then exit.
pub(crate) fn run_action() -> Result<(), String> {
    let json = std::env::var("HERDR_PLUGIN_CONTEXT_JSON")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| {
            "HERDR_PLUGIN_CONTEXT_JSON is not set; invoke this action from herdr".to_string()
        })?;
    let ctx = context::parse(&json)?;

    let workspace_id = ctx
        .workspace_id
        .or_else(|| env_non_blank("HERDR_WORKSPACE_ID"));
    let focused_pane_id = ctx
        .focused_pane_id
        .or_else(|| env_non_blank("HERDR_PANE_ID"));
    let focused_pane_agent = ctx.focused_pane_agent;

    let (pane_id, tab_id) = herdr::plugin_pane_open(
        workspace_id.as_deref(),
        &ctx.dir,
        focused_pane_id.as_deref(),
        focused_pane_agent.as_deref(),
    )?;
    match &tab_id {
        Some(tab) => {
            eprintln!("herdr-revdiff: opened review pane {pane_id} in tab {tab}");
            // Defensive: re-assert focus best-effort against a client-side
            // rendering race. Errors are deliberately ignored; herdr already
            // applies `--focus` at open.
            let _ = herdr::tab_focus(tab);
        }
        None => eprintln!("herdr-revdiff: opened review pane {pane_id}"),
    }
    Ok(())
}

/// Pane mode: run revdiff on the pane tty and handle its result.
pub(crate) fn run_pane() -> Result<(), String> {
    herdr::require_bin()?;
    // Runtime commands may default to the plugin directory as cwd; prefer the
    // directory the action resolved and exported, falling back to the process
    // cwd when the variable is absent.
    let dir = match env_non_blank("HERDR_REVIEWS_DIR") {
        Some(d) => PathBuf::from(d),
        None => {
            std::env::current_dir().map_err(|e| format!("cannot read current directory: {e}"))?
        }
    };

    let mode = git::select_mode(&git::probe(&dir));
    println!("herdr-revdiff: reviewing {}", mode.description());

    let reports = state_dir().join("reviews");
    std::fs::create_dir_all(&reports)
        .map_err(|e| format!("failed to create state dir {}: {e}", reports.display()))?;
    prune_old_reviews(&reports, SystemTime::now());
    let out_path = reports.join(state_file_name(std::process::id(), unix_millis()?));

    let revdiff_bin = revdiff::find_revdiff()?;
    let out = out_path.to_string_lossy().into_owned();
    let argv = child_argv(&revdiff_bin, &out, mode);
    let status = Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(&dir)
        .status()
        .map_err(|e| format!("failed to run {revdiff_bin} in {}: {e}", dir.display()))?;

    let code = status.code();
    let output = match code {
        Some(rc) => read_output(rc, &out_path)?,
        // No exit code means the child was killed; `finish` reports that and
        // keeps the output file, so there is nothing useful to read here.
        None => String::new(),
    };
    let caller_pane = env_non_blank("HERDR_REVIEWS_CALLER_PANE");
    let caller_agent = env_non_blank("HERDR_REVIEWS_CALLER_AGENT");
    finish(
        code,
        &out_path,
        &output,
        caller_pane.as_deref(),
        caller_agent.as_deref(),
    )
}

/// Reads revdiff's output file. A missing or unreadable file is only harmless
/// when revdiff exited 0 (nothing to annotate); otherwise annotations are
/// expected, so the read failure is fatal and the error path keeps the file.
fn read_output(rc: i32, out_path: &Path) -> Result<String, String> {
    match std::fs::read(out_path) {
        Ok(bytes) => Ok(String::from_utf8_lossy(&bytes).into_owned()),
        Err(e) if rc == 0 => {
            eprintln!(
                "herdr-revdiff: warning: cannot read {}: {e}",
                out_path.display()
            );
            Ok(String::new())
        }
        Err(e) => Err(format!(
            "cannot read revdiff output {}: {e}",
            out_path.display()
        )),
    }
}

/// Maps revdiff's exit code to the delivery decision. Quiet paths close the
/// pane; error and no-agent paths keep the output file and block on stdin so
/// the auto-closing pane stays readable.
fn finish(
    code: Option<i32>,
    out_path: &Path,
    output: &str,
    caller_pane: Option<&str>,
    caller_agent: Option<&str>,
) -> Result<(), String> {
    let Some(rc) = code else {
        return Err(format!(
            "revdiff terminated without an exit code (killed?); output kept at {}",
            out_path.display()
        ));
    };
    let has_agent = caller_pane.is_some() && caller_agent.is_some();
    match decide(rc, output, has_agent) {
        Outcome::Deliver => {
            // `decide` returns Deliver only when both caller pane and agent are
            // present, so the pane is always Some here; avoid expect() anyway.
            let Some(target) = caller_pane else {
                return Err("internal: deliver outcome without a caller pane".to_string());
            };
            let prompt = cap_prompt(&prompt_text(output), MAX_PROMPT_BYTES, out_path);
            match herdr::agent_prompt(target, &prompt) {
                Ok(()) => {
                    eprintln!("herdr-revdiff: sent review annotations to agent pane {target}");
                    remove(out_path);
                    close_pane();
                    Ok(())
                }
                Err(e) => Err(format!(
                    "failed to send annotations to agent pane {target}: {e}; output kept at {}",
                    out_path.display()
                )),
            }
        }
        Outcome::NoAnnotations => {
            eprintln!("herdr-revdiff: no annotations");
            remove(out_path);
            close_pane();
            Ok(())
        }
        Outcome::NoAgent => {
            eprintln!(
                "herdr-revdiff: no agent in focused pane; annotations kept at {}",
                out_path.display()
            );
            hold_pane();
            Ok(())
        }
        Outcome::Failed(code) => Err(format!(
            "revdiff failed with exit code {code}; output kept at {}",
            out_path.display()
        )),
    }
}

/// Best-effort pane teardown: close the pane by id, falling back to closing its
/// tab. Failures are logged, never propagated.
fn close_pane() {
    if let Some(pane) = env_non_blank("HERDR_PANE_ID") {
        match herdr::pane_close(&pane) {
            Ok(()) => return,
            Err(e) => eprintln!("herdr-revdiff: failed to close pane {pane}: {e}"),
        }
    }
    if let Some(tab) = env_non_blank("HERDR_TAB_ID")
        && let Err(e) = herdr::tab_close(&tab)
    {
        eprintln!("herdr-revdiff: failed to close tab {tab}: {e}");
    }
}

/// Blocks the pane process on stdin so an auto-closing plugin pane stays
/// readable on error and no-agent paths. A closed stdin (EOF) returns at once,
/// so the hold never hangs a non-interactive run.
pub(crate) fn hold_pane() {
    eprintln!("{HOLD_PROMPT}");
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
}

/// State filename: `<pid>-<unix millis>.out`. The pid keeps two reviews
/// started in the same millisecond from clobbering each other.
fn state_file_name(pid: u32, millis: u128) -> String {
    format!("{pid}-{millis}.out")
}

/// Removes `*.out` files directly under `dir` whose mtime is older than
/// `REVIEW_MAX_AGE`. Best-effort: a missing dir is ignored, removal failures
/// are logged, and non-`.out` entries are never touched.
fn prune_old_reviews(dir: &Path, now: SystemTime) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("out") {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        let Ok(age) = now.duration_since(modified) else {
            continue;
        };
        if age < REVIEW_MAX_AGE {
            continue;
        }
        if let Err(e) = std::fs::remove_file(&path) {
            eprintln!(
                "herdr-revdiff: warning: cannot remove {}: {e}",
                path.display()
            );
        }
    }
}

fn state_dir() -> PathBuf {
    env_non_blank("HERDR_PLUGIN_STATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

fn env_non_blank(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn unix_millis() -> Result<u128, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .map_err(|e| format!("system clock error: {e}"))
}

/// Removes a kept output file; already gone counts as success, any other
/// failure is logged, never propagated.
fn remove(path: &Path) {
    if let Err(e) = std::fs::remove_file(path) {
        if e.kind() == std::io::ErrorKind::NotFound {
            return;
        }
        eprintln!(
            "herdr-revdiff: warning: cannot remove {}: {e}",
            path.display()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory removed when the test ends.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir = std::env::temp_dir().join(format!(
                "herdr-revdiff-review-test-{tag}-{}-{nanos}",
                std::process::id()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
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

    fn set_mtime(path: &Path, t: SystemTime) {
        let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_times(std::fs::FileTimes::new().set_modified(t))
            .unwrap();
    }

    #[test]
    fn child_argv_default_has_no_flags_or_positional() {
        assert_eq!(
            child_argv("/bin/revdiff", "/s/1.out", Mode::Default),
            vec![
                "/bin/revdiff",
                "--output",
                "/s/1.out",
                "--exit-code-on-annotations"
            ]
        );
    }

    #[test]
    fn child_argv_all_files_puts_flag_before_output() {
        assert_eq!(
            child_argv("/bin/revdiff", "/s/1.out", Mode::AllFiles),
            vec![
                "/bin/revdiff",
                "--all-files",
                "--output",
                "/s/1.out",
                "--exit-code-on-annotations"
            ]
        );
    }

    #[test]
    fn child_argv_head_prev_puts_positional_last() {
        assert_eq!(
            child_argv("/bin/revdiff", "/s/1.out", Mode::HeadPrev),
            vec![
                "/bin/revdiff",
                "--output",
                "/s/1.out",
                "--exit-code-on-annotations",
                "HEAD~1"
            ]
        );
    }

    #[test]
    fn child_argv_staged_and_untracked() {
        assert_eq!(
            child_argv("/bin/revdiff", "/s/1.out", Mode::Staged),
            vec![
                "/bin/revdiff",
                "--staged",
                "--output",
                "/s/1.out",
                "--exit-code-on-annotations"
            ]
        );
        assert_eq!(
            child_argv("/bin/revdiff", "/s/1.out", Mode::Untracked),
            vec![
                "/bin/revdiff",
                "--untracked",
                "--output",
                "/s/1.out",
                "--exit-code-on-annotations"
            ]
        );
    }

    #[test]
    fn prompt_text_has_header_and_trimmed_body() {
        assert_eq!(
            prompt_text("  ## a/b.rs:3 (+) fix\n"),
            "Review annotations from revdiff:\n\n## a/b.rs:3 (+) fix"
        );
    }

    #[test]
    fn rc10_with_agent_delivers() {
        assert_eq!(decide(10, "## f:1 (+)", true), Outcome::Deliver);
    }

    #[test]
    fn rc10_without_agent_keeps_annotations() {
        assert_eq!(decide(10, "## f:1 (+)", false), Outcome::NoAgent);
    }

    #[test]
    fn rc0_with_output_and_agent_delivers() {
        assert_eq!(decide(0, "## f (file-level)", true), Outcome::Deliver);
    }

    #[test]
    fn rc0_with_output_without_agent_keeps_annotations() {
        assert_eq!(decide(0, "## f (file-level)", false), Outcome::NoAgent);
    }

    #[test]
    fn rc0_blank_output_is_no_annotations() {
        for out in ["", "   \n\t"] {
            assert_eq!(decide(0, out, true), Outcome::NoAnnotations);
            assert_eq!(decide(0, out, false), Outcome::NoAnnotations);
        }
    }

    #[test]
    fn error_exit_codes_are_failures_even_with_output() {
        assert_eq!(decide(1, "## f:1 (+)", true), Outcome::Failed(1));
        assert_eq!(decide(2, "", false), Outcome::Failed(2));
    }

    #[test]
    fn state_file_name_has_pid_and_out_extension() {
        let name = state_file_name(4242, 1_700_000_000_000);
        assert_eq!(name, "4242-1700000000000.out");
        assert!(name.starts_with("4242-"), "{name}");
        assert!(name.ends_with(".out"), "{name}");
    }

    #[test]
    fn read_output_missing_file_rc0_is_empty() {
        let dir = TempDir::new("read-rc0");
        let missing = dir.path().join("missing.out");
        assert_eq!(read_output(0, &missing).unwrap(), "");
    }

    #[test]
    fn read_output_missing_file_rc10_is_error() {
        let dir = TempDir::new("read-rc10");
        let missing = dir.path().join("missing.out");
        let err = read_output(10, &missing).unwrap_err();
        assert!(err.contains("cannot read revdiff output"), "{err}");
        assert!(err.contains("missing.out"), "{err}");
    }

    #[test]
    fn read_output_replaces_invalid_utf8() {
        let dir = TempDir::new("read-lossy");
        let path = dir.path().join("lossy.out");
        std::fs::write(&path, [b'f', 0xff, b'f']).unwrap();
        let out = read_output(10, &path).unwrap();
        assert!(out.contains('\u{fffd}'), "{out:?}");
    }

    #[test]
    fn cap_prompt_short_text_is_unchanged() {
        let path = Path::new("/state/reviews/1.out");
        assert_eq!(cap_prompt("short", MAX_PROMPT_BYTES, path), "short");
    }

    #[test]
    fn cap_prompt_truncates_long_ascii_and_notes_path() {
        let path = Path::new("/state/reviews/1.out");
        let text = "a".repeat(MAX_PROMPT_BYTES + 100);
        let capped = cap_prompt(&text, MAX_PROMPT_BYTES, path);
        assert!(capped.starts_with(&"a".repeat(MAX_PROMPT_BYTES)));
        assert!(
            capped.contains("(output truncated; full annotations kept at /state/reviews/1.out)"),
            "{capped}"
        );
        assert!(capped.len() < text.len());
    }

    #[test]
    fn cap_prompt_does_not_split_multibyte_char() {
        let path = Path::new("/state/reviews/1.out");
        let text = "é".repeat(100);
        let capped = cap_prompt(&text, 3, path);
        assert!(capped.starts_with('é'), "{capped}");
        assert!(capped.contains("(output truncated"), "{capped}");
    }

    #[test]
    fn prune_removes_only_old_out_files() {
        let dir = TempDir::new("prune");
        let old = dir.path().join("old.out");
        let fresh = dir.path().join("fresh.out");
        let keep = dir.path().join("keep.txt");
        std::fs::write(&old, "x").unwrap();
        std::fs::write(&fresh, "x").unwrap();
        std::fs::write(&keep, "x").unwrap();
        let now = SystemTime::now();
        set_mtime(&old, now - Duration::from_secs(8 * 24 * 60 * 60));
        set_mtime(&fresh, now);

        prune_old_reviews(dir.path(), now);

        assert!(!old.exists(), "old .out should be pruned");
        assert!(fresh.exists(), "fresh .out should be kept");
        assert!(keep.exists(), "non-.out entry should be kept");
    }

    #[test]
    fn prune_missing_dir_is_noop() {
        let dir = TempDir::new("prune-missing");
        let missing = dir.path().join("does-not-exist");
        prune_old_reviews(&missing, SystemTime::now());
    }
}
