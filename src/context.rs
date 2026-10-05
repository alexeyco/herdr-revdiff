//! Parsing the herdr plugin context JSON.

use serde_json::Value;

/// Values read from `HERDR_PLUGIN_CONTEXT_JSON` that the review action needs.
#[derive(Debug)]
pub(crate) struct Context {
    /// Review directory: `worktree.checkout_path` → `workspace_cwd` → `focused_pane_cwd`.
    pub(crate) dir: String,
    /// `workspace_id`, when a non-blank string.
    pub(crate) workspace_id: Option<String>,
    /// `focused_pane_id`, when a non-blank string.
    pub(crate) focused_pane_id: Option<String>,
    /// `focused_pane_agent`, when a non-blank string.
    pub(crate) focused_pane_agent: Option<String>,
}

/// Parses the plugin context and extracts the fields the review action uses.
pub(crate) fn parse(json: &str) -> Result<Context, String> {
    let v: Value =
        serde_json::from_str(json).map_err(|e| format!("invalid plugin context JSON: {e}"))?;
    let dir = dir_from_value(&v).ok_or_else(|| {
        "no directory in plugin context; invoke this action inside a workspace".to_string()
    })?;
    Ok(Context {
        dir,
        workspace_id: opt_string(&v, "workspace_id"),
        focused_pane_id: opt_string(&v, "focused_pane_id"),
        focused_pane_agent: opt_string(&v, "focused_pane_agent"),
    })
}

/// Picks the first present, non-blank string along the resolution chain:
/// `worktree.checkout_path` → `workspace_cwd` → `focused_pane_cwd`.
/// A non-string or whitespace-only value counts as absent.
fn dir_from_value(v: &Value) -> Option<String> {
    v.get("worktree")
        .and_then(|w| w.get("checkout_path"))
        .and_then(Value::as_str)
        .and_then(non_blank)
        .or_else(|| {
            v.get("workspace_cwd")
                .and_then(Value::as_str)
                .and_then(non_blank)
        })
        .or_else(|| {
            v.get("focused_pane_cwd")
                .and_then(Value::as_str)
                .and_then(non_blank)
        })
        .map(str::to_string)
}

/// Top-level field as a non-blank string; non-string or blank counts as absent.
fn opt_string(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .and_then(non_blank)
        .map(str::to_string)
}

fn non_blank(s: &str) -> Option<&str> {
    let s = s.trim();
    if s.is_empty() { None } else { Some(s) }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CTX: &str = concat!(
        r#"{"workspace_id":"ws-1","workspace_label":"demo","tab_id":"t1","#,
        r#""worktree":{"repo_key":"github.com/u/demo","repo_name":"demo","#,
        r#""repo_root":"/home/u/demo","checkout_path":"/home/u/demo","is_linked_worktree":false},"#,
        r#""workspace_cwd":"/home/u/demo/src","focused_pane_id":"p1","#,
        r#""focused_pane_agent":"claude","focused_pane_cwd":"/home/u/demo/src/tests"}"#
    );

    #[test]
    fn full_context_prefers_checkout_path() {
        let ctx = parse(CTX).unwrap();
        assert_eq!(ctx.dir, "/home/u/demo");
        assert_eq!(ctx.workspace_id.as_deref(), Some("ws-1"));
        assert_eq!(ctx.focused_pane_id.as_deref(), Some("p1"));
        assert_eq!(ctx.focused_pane_agent.as_deref(), Some("claude"));
    }

    #[test]
    fn falls_back_to_workspace_cwd() {
        let json = r#"{"worktree":{},"workspace_cwd":"/home/u/demo/src"}"#;
        assert_eq!(parse(json).unwrap().dir, "/home/u/demo/src");
    }

    #[test]
    fn falls_back_to_focused_pane_cwd() {
        let json = r#"{"workspace_cwd":"  ","focused_pane_cwd":"/home/u/demo/src"}"#;
        assert_eq!(parse(json).unwrap().dir, "/home/u/demo/src");
    }

    #[test]
    fn non_string_dir_is_absent() {
        for json in [r#"{"workspace_cwd":42}"#, r#"{"workspace_cwd":null}"#] {
            assert!(parse(json).unwrap_err().contains("no directory"));
        }
    }

    #[test]
    fn empty_string_value_falls_through_to_next_candidate() {
        let json = r#"{"worktree":{"checkout_path":"  "},"workspace_cwd":"/home/u/demo"}"#;
        assert_eq!(parse(json).unwrap().dir, "/home/u/demo");
    }

    #[test]
    fn all_blank_values_are_an_error() {
        for json in [
            r#"{"workspace_cwd":""}"#,
            r#"{"worktree":{"checkout_path":" "},"workspace_cwd":"","focused_pane_cwd":"\t"}"#,
        ] {
            assert!(parse(json).unwrap_err().contains("no directory"));
        }
    }

    #[test]
    fn invalid_json_is_an_error() {
        assert!(parse("{not json").unwrap_err().contains("invalid"));
    }

    #[test]
    fn optional_fields_missing_are_none() {
        let ctx = parse(r#"{"workspace_cwd":"/repo"}"#).unwrap();
        assert!(ctx.workspace_id.is_none());
        assert!(ctx.focused_pane_id.is_none());
        assert!(ctx.focused_pane_agent.is_none());
    }

    #[test]
    fn optional_fields_blank_or_non_string_are_none() {
        let json = r#"{"workspace_cwd":"/repo","workspace_id":" ","focused_pane_id":7,"focused_pane_agent":null}"#;
        let ctx = parse(json).unwrap();
        assert!(ctx.workspace_id.is_none());
        assert!(ctx.focused_pane_id.is_none());
        assert!(ctx.focused_pane_agent.is_none());
    }

    #[test]
    fn unescapes_windows_and_unicode_paths() {
        let json = r#"{"worktree":{"checkout_path":"C:\\Users\\caf\u00e9"}}"#;
        assert_eq!(parse(json).unwrap().dir, r"C:\Users\café");
    }
}
