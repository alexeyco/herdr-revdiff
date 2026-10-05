//! herdr plugin that reviews the current workspace with revdiff.
//!
//! The binary is dual-mode:
//! - action mode (`HERDR_PLUGIN_ACTION_ID` set) resolves the repository from
//!   the plugin context and opens a `revdiff` plugin pane (`plugin pane open
//!   --placement tab`), then exits immediately.
//! - pane mode (`HERDR_PLUGIN_ENTRYPOINT_ID` set) runs inside that pane, execs
//!   revdiff directly with the pane tty, reads its exit code, and delivers the
//!   annotations to the agent pane that requested the review.
//!
//! Plugin docs: https://herdr.dev/docs/plugins/
//! revdiff: https://github.com/umputun/revdiff

mod context;
mod git;
mod herdr;
mod revdiff;
mod review;

fn main() {
    if let Err(e) = run() {
        eprintln!("herdr-revdiff: {e}");
        // Plugin panes auto-close when this process exits, so pane-mode errors
        // must block until the user has read the message.
        if env_set("HERDR_PLUGIN_ENTRYPOINT_ID") {
            review::hold_pane();
        }
        std::process::exit(1);
    }
}

/// Which entry point the process should run.
#[derive(Debug, PartialEq, Eq)]
enum RunMode {
    /// `HERDR_PLUGIN_ENTRYPOINT_ID` is set: running inside the review pane.
    Pane,
    /// `HERDR_PLUGIN_ACTION_ID` is set: running the action.
    Action,
    /// Neither is set: invoked outside herdr.
    Outside,
}

/// Picks the mode from the entry-point environment. The entrypoint wins when
/// both are set, matching herdr's action-spawns-pane nesting.
fn run_mode() -> RunMode {
    if env_set("HERDR_PLUGIN_ENTRYPOINT_ID") {
        RunMode::Pane
    } else if env_set("HERDR_PLUGIN_ACTION_ID") {
        RunMode::Action
    } else {
        RunMode::Outside
    }
}

fn run() -> Result<(), String> {
    match run_mode() {
        RunMode::Pane => review::run_pane(),
        RunMode::Action => review::run_action(),
        RunMode::Outside => Err("invoked outside herdr plugin context".to_string()),
    }
}

fn env_set(key: &str) -> bool {
    std::env::var(key)
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::sync::Mutex;

    /// Serializes tests that mutate the mode environment.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Sets both mode variables for the duration of a test, restoring the
    /// previous values on drop.
    struct ModeEnv {
        _lock: std::sync::MutexGuard<'static, ()>,
        prev_entry: Option<OsString>,
        prev_action: Option<OsString>,
    }

    impl ModeEnv {
        fn set(entry: Option<&str>, action: Option<&str>) -> Self {
            let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let prev_entry = std::env::var_os("HERDR_PLUGIN_ENTRYPOINT_ID");
            let prev_action = std::env::var_os("HERDR_PLUGIN_ACTION_ID");
            set_var("HERDR_PLUGIN_ENTRYPOINT_ID", entry);
            set_var("HERDR_PLUGIN_ACTION_ID", action);
            Self {
                _lock: lock,
                prev_entry,
                prev_action,
            }
        }
    }

    impl Drop for ModeEnv {
        fn drop(&mut self) {
            match &self.prev_entry {
                Some(v) => unsafe { std::env::set_var("HERDR_PLUGIN_ENTRYPOINT_ID", v) },
                None => unsafe { std::env::remove_var("HERDR_PLUGIN_ENTRYPOINT_ID") },
            }
            match &self.prev_action {
                Some(v) => unsafe { std::env::set_var("HERDR_PLUGIN_ACTION_ID", v) },
                None => unsafe { std::env::remove_var("HERDR_PLUGIN_ACTION_ID") },
            }
        }
    }

    fn set_var(key: &str, value: Option<&str>) {
        match value {
            Some(v) => unsafe { std::env::set_var(key, v) },
            None => unsafe { std::env::remove_var(key) },
        }
    }

    #[test]
    fn entrypoint_only_is_pane() {
        let _env = ModeEnv::set(Some("review"), None);
        assert_eq!(run_mode(), RunMode::Pane);
    }

    #[test]
    fn action_only_is_action() {
        let _env = ModeEnv::set(None, Some("review"));
        assert_eq!(run_mode(), RunMode::Action);
    }

    #[test]
    fn both_set_prefers_pane() {
        let _env = ModeEnv::set(Some("review"), Some("review"));
        assert_eq!(run_mode(), RunMode::Pane);
    }

    #[test]
    fn neither_set_is_outside() {
        let _env = ModeEnv::set(None, None);
        assert_eq!(run_mode(), RunMode::Outside);
    }

    #[test]
    fn blank_values_count_as_unset() {
        let _env = ModeEnv::set(Some("   "), Some("\t"));
        assert_eq!(run_mode(), RunMode::Outside);
    }
}
