//! Locating the revdiff binary.
//!
//! The path is resolved to a canonical absolute one before revdiff reaches the
//! pane process: herdr spawns that process with its own `PATH` and working
//! directory, which may differ from the action process's. A candidate that
//! cannot be canonicalized is an error rather than a cwd-relative guess.
//!
//! `REVDIFF_BIN` is this plugin's own optional override, read from the pane
//! process environment. It is not set by herdr or by revdiff, and the action
//! does not inject it: export it where the herdr daemon is launched (the
//! daemon inherits it, and a herdr restart picks up changes) to select a
//! specific build or a revdiff outside the daemon's `PATH`.

use std::path::{Path, PathBuf};

const INSTALL_HINT: &str = "install it from https://github.com/umputun/revdiff, \
     or set REVDIFF_BIN to its path";

/// `REVDIFF_BIN` override first (validated executable, canonicalized), then the
/// first executable `revdiff` on `PATH`.
pub(crate) fn find_revdiff() -> Result<String, String> {
    let revdiff_bin = std::env::var("REVDIFF_BIN").ok();
    let path = std::env::var("PATH").unwrap_or_default();
    resolve_revdiff(revdiff_bin.as_deref(), &path)
}

/// Resolves revdiff from the raw `REVDIFF_BIN` value and `PATH`. Split out so
/// the selection matrix is testable without mutating the process environment.
fn resolve_revdiff(revdiff_bin: Option<&str>, path: &str) -> Result<String, String> {
    if let Some(v) = revdiff_bin {
        let v = v.trim();
        if !v.is_empty() {
            let candidate = Path::new(v);
            if !is_executable(candidate) {
                // Show the path as the user gave it, not its canonical form.
                return Err(format!("REVDIFF_BIN ({v}) is not an executable file"));
            }
            return canonical_absolute(candidate.to_path_buf())
                .map(|p| p.to_string_lossy().into_owned());
        }
    }
    if let Some(found) = first_in_path(path)? {
        return Ok(found.to_string_lossy().into_owned());
    }
    Err(format!("revdiff not found on PATH; {INSTALL_HINT}"))
}

/// Canonical absolute path of the first executable `revdiff` among `PATH`
/// entries, in order. Empty entries are skipped.
fn first_in_path(path: &str) -> Result<Option<PathBuf>, String> {
    let Some(candidate) = std::env::split_paths(path)
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(|dir| dir.join("revdiff"))
        .find(|candidate| is_executable(candidate))
    else {
        return Ok(None);
    };
    canonical_absolute(candidate).map(Some)
}

/// Canonicalizes a candidate to an absolute path. There is deliberately no
/// working-directory fallback: the action and pane processes can have different
/// cwds, so guessing would break the pane-safety contract.
fn canonical_absolute(candidate: PathBuf) -> Result<PathBuf, String> {
    std::fs::canonicalize(&candidate).map_err(|e| {
        format!(
            "found revdiff at {} but cannot resolve it to an absolute path: {e}",
            candidate.display()
        )
    })
}

fn is_executable(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Serializes tests that set `REVDIFF_BIN`, and restores the previous
    /// value on drop so other tests see the original environment.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct RevdiffBinEnv {
        _lock: std::sync::MutexGuard<'static, ()>,
        prev: Option<std::ffi::OsString>,
    }

    impl RevdiffBinEnv {
        fn set(value: &Path) -> Self {
            let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let prev = std::env::var_os("REVDIFF_BIN");
            unsafe { std::env::set_var("REVDIFF_BIN", value) };
            Self { _lock: lock, prev }
        }
    }

    impl Drop for RevdiffBinEnv {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => unsafe { std::env::set_var("REVDIFF_BIN", v) },
                None => unsafe { std::env::remove_var("REVDIFF_BIN") },
            }
        }
    }

    /// Serializes tests that change the process working directory.
    static CWD_LOCK: Mutex<()> = Mutex::new(());

    /// Sets the process cwd for the duration of a test and restores it on drop.
    struct CwdGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        prev: PathBuf,
    }

    impl CwdGuard {
        fn set(dir: &Path) -> Self {
            let lock = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let prev = std::env::current_dir().unwrap();
            std::env::set_current_dir(dir).unwrap();
            Self { _lock: lock, prev }
        }
    }

    impl Drop for CwdGuard {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.prev);
        }
    }

    /// A canonicalized scratch directory, removed when the test ends, so
    /// comparisons survive symlinked temp roots (e.g. `/var` → `/private/var`
    /// on macOS).
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir = std::env::temp_dir().join(format!(
                "herdr-revdiff-test-{tag}-{}-{nanos}",
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

    fn write_revdiff(dir: &Path, executable: bool) {
        let path = dir.join("revdiff");
        std::fs::write(&path, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = if executable { 0o755 } else { 0o644 };
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        }
        #[cfg(not(unix))]
        let _ = executable;
    }

    #[test]
    fn empty_path_has_no_revdiff() {
        assert!(first_in_path("").unwrap().is_none());
        assert!(
            first_in_path(":/nonexistent-dir-for-herdr-test:")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn first_executable_entry_wins() {
        let first = TempDir::new("first");
        let second = TempDir::new("second");
        write_revdiff(first.path(), true);
        write_revdiff(second.path(), true);
        let path = std::env::join_paths([first.path(), second.path()])
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(
            first_in_path(&path).unwrap(),
            Some(first.path().join("revdiff"))
        );
    }

    #[test]
    fn non_executable_entry_is_skipped_and_result_is_absolute() {
        let first = TempDir::new("nonexec");
        let second = TempDir::new("exec");
        write_revdiff(first.path(), false);
        write_revdiff(second.path(), true);
        let path = std::env::join_paths([first.path(), second.path()])
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let found = first_in_path(&path).unwrap().unwrap();
        assert_eq!(found, second.path().join("revdiff"));
        assert!(found.is_absolute());
    }

    #[test]
    fn relative_path_entry_resolves_to_absolute() {
        let dir = TempDir::new("relpath");
        write_revdiff(dir.path(), true);
        let _cwd = CwdGuard::set(dir.path());
        let found = first_in_path(".").unwrap().unwrap();
        assert!(found.is_absolute());
        assert_eq!(found, dir.path().join("revdiff"));
    }

    #[test]
    fn whitespace_revdiff_bin_falls_back_to_path() {
        let dir = TempDir::new("blank-bin");
        write_revdiff(dir.path(), true);
        let path = std::env::join_paths([dir.path()])
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(
            resolve_revdiff(Some("   "), &path).unwrap(),
            dir.path().join("revdiff").to_string_lossy()
        );
    }

    #[test]
    fn missing_path_entry_errors_with_install_hint() {
        // Empty string covers PATH unset (find_revdiff normalizes it to "");
        // the ":" entry has no non-empty components either.
        for path in ["", ":"] {
            let err = resolve_revdiff(None, path).unwrap_err();
            assert!(err.contains("revdiff not found"), "{err}");
            assert!(err.contains("https://github.com/umputun/revdiff"), "{err}");
        }
    }

    #[test]
    fn executable_revdiff_bin_is_canonicalized() {
        let dir = TempDir::new("bin-exec");
        write_revdiff(dir.path(), true);
        let bin = dir.path().join("revdiff");
        let _env = RevdiffBinEnv::set(&bin);
        assert_eq!(find_revdiff().unwrap(), bin.to_string_lossy());
    }

    #[test]
    fn non_executable_revdiff_bin_errors_with_path() {
        let dir = TempDir::new("bin-nonexec");
        write_revdiff(dir.path(), false);
        let bin = dir.path().join("revdiff");
        let _env = RevdiffBinEnv::set(&bin);
        let err = find_revdiff().unwrap_err();
        assert!(err.contains(bin.to_string_lossy().as_ref()), "{err}");
        assert!(err.contains("not an executable file"), "{err}");
    }
}
