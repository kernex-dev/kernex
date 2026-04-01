//! Declarative allow/deny permission rules for tool calls.
//!
//! Rules are evaluated in order: deny list first, then allow list.
//! An empty allow list means "allow all not explicitly denied."

/// Glob-style pattern rules controlling which tool calls are permitted.
///
/// Pattern format:
/// - `"ToolName"` — matches any call to that tool (case-insensitive).
/// - `"ToolName(glob)"` — matches the tool name and applies a wildcard glob
///   against the concatenated string values in the tool's argument JSON.
///   Example: `"Bash(git *)"` matches any Bash call whose command starts with
///   `git `.
///
/// Evaluation order:
/// 1. If any `deny` pattern matches, the call is blocked.
/// 2. If `allow` is non-empty, the call must match at least one allow pattern.
/// 3. Otherwise the call proceeds.
#[derive(Debug, Clone, Default)]
pub struct PermissionRules {
    /// Patterns that are always blocked.
    pub allow: Vec<String>,
    /// Patterns that are explicitly permitted (all others blocked when non-empty).
    pub deny: Vec<String>,
}

/// Outcome of a permission check.
pub enum PermissionOutcome {
    /// The tool call may proceed.
    Allow,
    /// The tool call is blocked. Contains a human-readable reason.
    Deny(String),
}

impl PermissionRules {
    /// Check whether a tool call is permitted.
    ///
    /// `tool_name` is the name of the tool being called.
    /// `args` is the JSON object of arguments passed to the tool.
    pub fn check(&self, tool_name: &str, args: &serde_json::Value) -> PermissionOutcome {
        let args_str = extract_args_string(args);

        // Deny list evaluated first.
        for pattern in &self.deny {
            if pattern_matches(pattern, tool_name, &args_str) {
                return PermissionOutcome::Deny(format!("denied by rule: {pattern}"));
            }
        }

        // If allow list is non-empty, the call must match at least one entry.
        if !self.allow.is_empty() {
            let allowed = self
                .allow
                .iter()
                .any(|p| pattern_matches(p, tool_name, &args_str));
            if !allowed {
                return PermissionOutcome::Deny(format!("tool '{tool_name}' not in allow list"));
            }
        }

        PermissionOutcome::Allow
    }
}

/// Concatenate all top-level string values in a JSON object into one string.
///
/// This gives the glob pattern something meaningful to match against without
/// requiring tool-specific argument extraction logic.
fn extract_args_string(args: &serde_json::Value) -> String {
    match args {
        serde_json::Value::Object(map) => map
            .values()
            .filter_map(|v| v.as_str())
            .collect::<Vec<_>>()
            .join(" "),
        serde_json::Value::String(s) => s.clone(),
        _ => String::new(),
    }
}

/// Returns true if `pattern` matches the given tool call.
///
/// Patterns with parentheses — e.g. `Bash(git *)` — match the tool name and
/// apply a wildcard glob to the args string. Patterns without parentheses
/// match on tool name alone (case-insensitive).
fn pattern_matches(pattern: &str, tool_name: &str, args_str: &str) -> bool {
    if let Some(paren_idx) = pattern.find('(') {
        if pattern.ends_with(')') {
            let pat_tool = &pattern[..paren_idx];
            let glob = &pattern[paren_idx + 1..pattern.len() - 1];
            if !pat_tool.eq_ignore_ascii_case(tool_name) {
                return false;
            }
            return glob_match(glob, args_str);
        }
    }
    // No parentheses: tool name match only.
    pattern.eq_ignore_ascii_case(tool_name)
}

/// Wildcard glob match where `*` matches any sequence of characters.
///
/// Matching is case-insensitive. `?` is not supported (not needed for the
/// current permission rule format).
fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let plen = p.len();
    let tlen = t.len();

    // dp[i][j] = pattern[0..i] matches text[0..j]
    let mut dp = vec![vec![false; tlen + 1]; plen + 1];
    dp[0][0] = true;

    // Leading stars match the empty string.
    for i in 1..=plen {
        if p[i - 1] == '*' {
            dp[i][0] = dp[i - 1][0];
        }
    }

    for i in 1..=plen {
        for j in 1..=tlen {
            if p[i - 1] == '*' {
                // Star: match zero chars (dp[i][j-1]) or one more char (dp[i-1][j]).
                dp[i][j] = dp[i][j - 1] || dp[i - 1][j];
            } else {
                dp[i][j] = dp[i - 1][j - 1] && p[i - 1].eq_ignore_ascii_case(&t[j - 1]);
            }
        }
    }

    dp[plen][tlen]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // --- glob_match ---

    #[test]
    fn test_glob_exact_match() {
        assert!(glob_match("git status", "git status"));
    }

    #[test]
    fn test_glob_star_prefix() {
        assert!(glob_match("git *", "git status"));
        assert!(glob_match("git *", "git commit -m \"fix\""));
        assert!(!glob_match("git *", "rm -rf /"));
    }

    #[test]
    fn test_glob_star_anywhere() {
        assert!(glob_match("*rm*", "sudo rm -rf /"));
        assert!(glob_match("*rm*", "rm -rf"));
        assert!(!glob_match("*rm*", "git status"));
    }

    #[test]
    fn test_glob_case_insensitive() {
        assert!(glob_match("GIT *", "git status"));
        assert!(glob_match("git *", "GIT STATUS"));
    }

    #[test]
    fn test_glob_empty_pattern_matches_empty_only() {
        assert!(glob_match("", ""));
        assert!(!glob_match("", "anything"));
    }

    #[test]
    fn test_glob_star_only_matches_anything() {
        assert!(glob_match("*", ""));
        assert!(glob_match("*", "anything at all"));
    }

    // --- pattern_matches ---

    #[test]
    fn test_pattern_tool_name_only() {
        assert!(pattern_matches("Bash", "bash", "some args"));
        assert!(pattern_matches("Write", "write", ""));
        assert!(!pattern_matches("Read", "write", ""));
    }

    #[test]
    fn test_pattern_with_glob() {
        assert!(pattern_matches("Bash(git *)", "bash", "git status"));
        assert!(!pattern_matches("Bash(git *)", "bash", "rm -rf /"));
        assert!(!pattern_matches("Bash(git *)", "read", "git status"));
    }

    // --- PermissionRules::check ---

    #[test]
    fn test_empty_rules_allows_everything() {
        let rules = PermissionRules::default();
        assert!(matches!(
            rules.check("bash", &json!({"command": "rm -rf /"})),
            PermissionOutcome::Allow
        ));
    }

    #[test]
    fn test_deny_by_tool_name() {
        let rules = PermissionRules {
            allow: vec![],
            deny: vec!["Write".to_string()],
        };
        assert!(matches!(
            rules.check("write", &json!({"path": "/etc/passwd"})),
            PermissionOutcome::Deny(_)
        ));
        assert!(matches!(
            rules.check("read", &json!({"path": "/etc/passwd"})),
            PermissionOutcome::Allow
        ));
    }

    #[test]
    fn test_deny_by_glob_pattern() {
        let rules = PermissionRules {
            allow: vec![],
            deny: vec!["Bash(rm *)".to_string()],
        };
        assert!(matches!(
            rules.check("bash", &json!({"command": "rm -rf /"})),
            PermissionOutcome::Deny(_)
        ));
        assert!(matches!(
            rules.check("bash", &json!({"command": "git status"})),
            PermissionOutcome::Allow
        ));
    }

    #[test]
    fn test_allow_list_restricts_unlisted_tools() {
        let rules = PermissionRules {
            allow: vec!["Read".to_string(), "Bash(git *)".to_string()],
            deny: vec![],
        };
        assert!(matches!(
            rules.check("read", &json!({"path": "src/lib.rs"})),
            PermissionOutcome::Allow
        ));
        assert!(matches!(
            rules.check("bash", &json!({"command": "git log"})),
            PermissionOutcome::Allow
        ));
        // Write is not in the allow list.
        assert!(matches!(
            rules.check("write", &json!({"path": "out.txt"})),
            PermissionOutcome::Deny(_)
        ));
        // Bash with non-git command is not in the allow list.
        assert!(matches!(
            rules.check("bash", &json!({"command": "curl https://example.com"})),
            PermissionOutcome::Deny(_)
        ));
    }

    #[test]
    fn test_deny_takes_precedence_over_allow() {
        let rules = PermissionRules {
            allow: vec!["Bash".to_string()],
            deny: vec!["Bash(rm *)".to_string()],
        };
        // Bash is allowed, but rm is denied — deny wins.
        assert!(matches!(
            rules.check("bash", &json!({"command": "rm file.txt"})),
            PermissionOutcome::Deny(_)
        ));
        // Other bash commands still pass.
        assert!(matches!(
            rules.check("bash", &json!({"command": "echo hello"})),
            PermissionOutcome::Allow
        ));
    }

    #[test]
    fn test_deny_reason_contains_pattern() {
        let rules = PermissionRules {
            allow: vec![],
            deny: vec!["Bash(sudo *)".to_string()],
        };
        if let PermissionOutcome::Deny(reason) =
            rules.check("bash", &json!({"command": "sudo apt-get install vim"}))
        {
            assert!(reason.contains("Bash(sudo *)"));
        } else {
            panic!("expected Deny");
        }
    }
}
