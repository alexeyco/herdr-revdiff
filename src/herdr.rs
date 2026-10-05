//! Minimal client over the herdr CLI (`HERDR_BIN_PATH`).

use std::process::Command;

use serde_json::Value;

/// Runs the herdr binary with `args`, returning stdout on success.
fn exec(args: &[&str]) -> Result<String, String> {
    let bin = herdr_bin()?;
    let output = Command::new(&bin)
        .args(args)
        .output()
        .map_err(|e| format!("failed to run {} {}: {e}", bin, args.join(" ")))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!(
            "{} {} failed ({}): {}",
            bin,
            args.join(" "),
            output.status,
            snippet(&stderr)
        ))
    }
}

fn herdr_bin() -> Result<String, String> {
    match std::env::var("HERDR_BIN_PATH") {
        Ok(v) if !v.trim().is_empty() => Ok(v),
        _ => Err(
            "herdr did not provide HERDR_BIN_PATH (plugin API v1 contract); cannot call back into herdr"
                .to_string(),
        ),
    }
}

/// Fails early when `HERDR_BIN_PATH` is missing. Pane mode calls this before
/// starting revdiff so a bad environment is reported before the TUI opens.
pub(crate) fn require_bin() -> Result<(), String> {
    herdr_bin().map(|_| ())
}

/// Opens the `revdiff` plugin pane, returning `(pane_id, tab_id)`.
pub(crate) fn plugin_pane_open(
    workspace_id: Option<&str>,
    cwd: &str,
    caller_pane: Option<&str>,
    caller_agent: Option<&str>,
) -> Result<(String, Option<String>), String> {
    let args = pane_open_args(workspace_id, cwd, caller_pane, caller_agent);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = exec(&refs)?;
    parse_pane_open(&out)
}

/// `herdr plugin pane open` argv: fixed plugin/entrypoint/placement, then the
/// conditional `--workspace`, `--cwd`, the `HERDR_REVIEWS_DIR` export, the
/// optional caller `--env` pairs, `--focus`.
pub(crate) fn pane_open_args(
    workspace_id: Option<&str>,
    cwd: &str,
    caller_pane: Option<&str>,
    caller_agent: Option<&str>,
) -> Vec<String> {
    let mut args = vec![
        "plugin".to_string(),
        "pane".to_string(),
        "open".to_string(),
        "--plugin".to_string(),
        "revdiff".to_string(),
        "--entrypoint".to_string(),
        "review".to_string(),
        // herdr-plugin.toml also declares placement = "tab": that manifest value
        // is documentation, this arg is what the action actually passes, so the
        // two can drift.
        "--placement".to_string(),
        "tab".to_string(),
    ];
    if let Some(ws) = workspace_id {
        args.push("--workspace".to_string());
        args.push(ws.to_string());
    }
    args.push("--cwd".to_string());
    args.push(cwd.to_string());
    // Runtime commands default to the plugin directory as cwd; export the
    // resolved directory so pane mode does not have to trust its own cwd.
    args.push("--env".to_string());
    args.push(format!("HERDR_REVIEWS_DIR={cwd}"));
    if let Some(pane) = caller_pane {
        args.push("--env".to_string());
        args.push(format!("HERDR_REVIEWS_CALLER_PANE={pane}"));
    }
    if let Some(agent) = caller_agent {
        args.push("--env".to_string());
        args.push(format!("HERDR_REVIEWS_CALLER_AGENT={agent}"));
    }
    args.push("--focus".to_string());
    args
}

/// Parses the `plugin pane open` reply: `result.plugin_pane.pane.pane_id` is
/// required, `...tab_id` is optional (overlay panes may carry no tab).
fn parse_pane_open(json: &str) -> Result<(String, Option<String>), String> {
    let raw = snippet(json);
    let v: Value = serde_json::from_str(json)
        .map_err(|e| format!("invalid plugin pane open JSON: {e}; raw: {raw}"))?;
    let pane_id = v
        .pointer("/result/plugin_pane/pane/pane_id")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            format!("missing result.plugin_pane.pane.pane_id in plugin pane open JSON; raw: {raw}")
        })?;
    let tab_id = v
        .pointer("/result/plugin_pane/pane/tab_id")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string);
    Ok((pane_id.to_string(), tab_id))
}

/// Closes a plugin pane by id.
pub(crate) fn pane_close(pane_id: &str) -> Result<(), String> {
    exec(&["plugin", "pane", "close", pane_id]).map(|_| ())
}

/// Closes a tab by id.
pub(crate) fn tab_close(tab_id: &str) -> Result<(), String> {
    exec(&["tab", "close", tab_id]).map(|_| ())
}

/// `herdr tab focus` argv.
pub(crate) fn tab_focus_args(tab_id: &str) -> Vec<String> {
    vec!["tab".to_string(), "focus".to_string(), tab_id.to_string()]
}

/// Focuses a tab by id. Used as a best-effort re-assert after opening the
/// review pane; herdr already applies `--focus` at open.
pub(crate) fn tab_focus(tab_id: &str) -> Result<String, String> {
    let args = tab_focus_args(tab_id);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    exec(&refs)
}

/// Submits `text` to the agent hosted in `target_pane`.
pub(crate) fn agent_prompt(target_pane: &str, text: &str) -> Result<(), String> {
    exec(&["agent", "prompt", target_pane, text]).map(|_| ())
}

/// Trims and truncates a snippet for error messages.
fn snippet(s: &str) -> &str {
    let s = s.trim();
    match s.char_indices().nth(200) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PANE_OPEN: &str = concat!(
        r#"{"ok":true,"result":{"plugin_pane":{"plugin_id":"revdiff","entrypoint":"review","#,
        r#""pane":{"pane_id":"pane-1","tab_id":"tab-1","workspace_id":"ws-1","cwd":"/repo"}}}}"#
    );

    #[test]
    fn parses_pane_open_success() {
        let (pane, tab) = parse_pane_open(PANE_OPEN).unwrap();
        assert_eq!(pane, "pane-1");
        assert_eq!(tab.as_deref(), Some("tab-1"));
    }

    #[test]
    fn pane_open_missing_pane_id_is_an_error() {
        let json = r#"{"result":{"plugin_pane":{"pane":{"tab_id":"tab-1"}}}}"#;
        let err = parse_pane_open(json).unwrap_err();
        assert!(err.contains("result.plugin_pane.pane.pane_id"), "{err}");
    }

    #[test]
    fn pane_open_without_tab_id_is_ok() {
        let json = r#"{"result":{"plugin_pane":{"pane":{"pane_id":"pane-1"}}}}"#;
        assert_eq!(parse_pane_open(json).unwrap(), ("pane-1".to_string(), None));
    }

    #[test]
    fn pane_open_invalid_json_reports_raw() {
        let err = parse_pane_open("{not json").unwrap_err();
        assert!(err.contains("invalid plugin pane open JSON"), "{err}");
        assert!(err.contains("{not json"), "{err}");
    }

    #[test]
    fn pane_open_args_full_flag_order() {
        let args = pane_open_args(Some("ws-1"), "/repo", Some("pane-9"), Some("claude"));
        let got: Vec<&str> = args.iter().map(String::as_str).collect();
        assert_eq!(
            got,
            [
                "plugin",
                "pane",
                "open",
                "--plugin",
                "revdiff",
                "--entrypoint",
                "review",
                "--placement",
                "tab",
                "--workspace",
                "ws-1",
                "--cwd",
                "/repo",
                "--env",
                "HERDR_REVIEWS_DIR=/repo",
                "--env",
                "HERDR_REVIEWS_CALLER_PANE=pane-9",
                "--env",
                "HERDR_REVIEWS_CALLER_AGENT=claude",
                "--focus",
            ]
        );
    }

    #[test]
    fn pane_open_args_omit_workspace_and_callers() {
        let args = pane_open_args(None, "/repo", None, None);
        let got: Vec<&str> = args.iter().map(String::as_str).collect();
        assert_eq!(
            got,
            [
                "plugin",
                "pane",
                "open",
                "--plugin",
                "revdiff",
                "--entrypoint",
                "review",
                "--placement",
                "tab",
                "--cwd",
                "/repo",
                "--env",
                "HERDR_REVIEWS_DIR=/repo",
                "--focus",
            ]
        );
    }

    #[test]
    fn pane_open_args_include_only_the_present_env() {
        let args = pane_open_args(None, "/repo", Some("pane-9"), None);
        let got: Vec<&str> = args.iter().map(String::as_str).collect();
        assert_eq!(
            got,
            [
                "plugin",
                "pane",
                "open",
                "--plugin",
                "revdiff",
                "--entrypoint",
                "review",
                "--placement",
                "tab",
                "--cwd",
                "/repo",
                "--env",
                "HERDR_REVIEWS_DIR=/repo",
                "--env",
                "HERDR_REVIEWS_CALLER_PANE=pane-9",
                "--focus",
            ]
        );
    }

    #[test]
    fn tab_focus_args_has_command_and_id() {
        let args = tab_focus_args("tab-1");
        let got: Vec<&str> = args.iter().map(String::as_str).collect();
        assert_eq!(got, ["tab", "focus", "tab-1"]);
    }

    #[test]
    fn snippet_truncates_long_output() {
        let long = "x".repeat(500);
        assert_eq!(snippet(&long).len(), 200);
    }
}
