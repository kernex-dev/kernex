//! Declarative allow/deny permission rules for tool calls.
//!
//! Rules are evaluated in order: deny list first, then allow list.
//! An empty allow list means "allow all not explicitly denied."
//!
//! A rule sees one value per tool call, its *subject*: the command of a
//! `bash` call, the path of a file tool, the URL of `web_fetch`, and the
//! serialized arguments of any other tool. Other arguments (the content of a
//! `write`, a search pattern) never reach a rule, so they cannot make one
//! match or miss.
//!
//! An allow rule for `bash` is a convenience and not a boundary: a shell
//! command has too many spellings for a pattern to pin down. The OS sandbox
//! is what confines a command. To keep the convenience from being trivially
//! bypassed, a `Bash(...)` allow rule never matches a command that contains
//! a shell control operator.

/// Glob-style pattern rules controlling which tool calls are permitted.
///
/// Pattern format:
/// - `"ToolName"` — matches any call to that tool (case-insensitive).
/// - `"ToolName(glob)"` — matches the tool name and applies a wildcard glob
///   to the call's subject (see the module docs).
///   Example: `"Bash(git *)"` matches any Bash call whose command starts with
///   `git `.
///
/// Evaluation order:
/// 1. If any `deny` pattern matches, the call is blocked.
/// 2. If `allow` is non-empty, the call must match at least one allow pattern.
/// 3. Otherwise the call proceeds.
#[derive(Debug, Clone, Default)]
pub struct PermissionRules {
    /// Patterns that are explicitly permitted (all others blocked when non-empty).
    pub allow: Vec<String>,
    /// Patterns that are always blocked.
    pub deny: Vec<String>,
}

/// Outcome of a permission check.
pub enum PermissionOutcome {
    /// The tool call may proceed.
    Allow,
    /// The tool call is blocked. Contains a human-readable reason.
    Deny(String),
}

/// Upper bound on the byte length of a subject. 64 KiB is far above any
/// legitimate command, path or URL, and keeps the DP matrix in
/// [`glob_match`] from becoming a DoS vector. A longer subject is denied:
/// truncating it would let padding hide what a rule should see.
const MAX_SUBJECT_LEN: usize = 64 * 1024;

/// What a shell reads as "and then run something else", or as a redirection.
const SHELL_CONTROL: &[&str] = &[";", "&", "|", "`", "$(", "\n", "\r", "<", ">"];

impl PermissionRules {
    /// Check whether a tool call is permitted.
    ///
    /// `tool_name` is the name of the tool being called.
    /// `args` is the JSON object of arguments passed to the tool. The rules
    /// see the call's subject as [`PermissionRules::subject`] extracts it.
    pub fn check(&self, tool_name: &str, args: &serde_json::Value) -> PermissionOutcome {
        self.check_subject(tool_name, &Self::subject(tool_name, args))
    }

    /// The value the rules match for a call to `tool_name`: `command` for
    /// `bash`, `file_path` for `read`, `write` and `edit`, `path` for `grep`
    /// and `glob`, `url` for `web_fetch`, and the arguments serialized as
    /// JSON for every other tool. A missing argument gives an empty subject.
    pub fn subject(tool_name: &str, args: &serde_json::Value) -> String {
        let named = match tool_name.to_ascii_lowercase().as_str() {
            "bash" => Some("command"),
            "read" | "write" | "edit" => Some("file_path"),
            "grep" | "glob" => Some("path"),
            "web_fetch" => Some("url"),
            _ => None,
        };
        match named {
            Some(key) => args
                .get(key)
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string(),
            None => args.to_string(),
        }
    }

    /// Check a call whose subject the caller already has, for example a file
    /// path it has resolved against its workspace.
    pub fn check_subject(&self, tool_name: &str, subject: &str) -> PermissionOutcome {
        self.check_subjects(tool_name, &[subject])
    }

    /// Whether a deny rule matches, leaving the allow list aside. For a
    /// host that filters what a call returns: a search result is a file
    /// that was read, whatever tool found it.
    pub fn denies(&self, tool_name: &str, subject: &str) -> bool {
        subject.len() > MAX_SUBJECT_LEN
            || self
                .deny
                .iter()
                .any(|pattern| pattern_matches(pattern, tool_name, subject))
    }

    /// Check a call that has more than one spelling of its subject, such as
    /// a directory with and without its trailing slash. A deny rule that
    /// matches any spelling blocks the call; with an allow list, one
    /// spelling has to match an allow rule.
    pub fn check_subjects(&self, tool_name: &str, subjects: &[&str]) -> PermissionOutcome {
        if self.allow.is_empty() && self.deny.is_empty() {
            return PermissionOutcome::Allow;
        }
        if subjects.iter().any(|s| s.len() > MAX_SUBJECT_LEN) {
            return PermissionOutcome::Deny(format!(
                "arguments of '{tool_name}' exceed the {MAX_SUBJECT_LEN}-byte limit for permission rules"
            ));
        }

        // Deny list evaluated first.
        for pattern in &self.deny {
            if subjects
                .iter()
                .any(|subject| pattern_matches(pattern, tool_name, subject))
            {
                return PermissionOutcome::Deny(format!("denied by rule: {pattern}"));
            }
        }

        // If allow list is non-empty, the call must match at least one entry.
        if !self.allow.is_empty() {
            let allowed = self.allow.iter().any(|p| {
                subjects
                    .iter()
                    .any(|subject| allow_pattern_matches(p, tool_name, subject))
            });
            if !allowed {
                return PermissionOutcome::Deny(format!("tool '{tool_name}' not in allow list"));
            }
        }

        PermissionOutcome::Allow
    }
}

/// [`pattern_matches`] for an allow rule. A `Bash(...)` rule with a glob
/// does not match a command that chains, pipes, substitutes or redirects:
/// `Bash(git *)` would otherwise allow `git status; curl ...`.
fn allow_pattern_matches(pattern: &str, tool_name: &str, subject: &str) -> bool {
    // `Bash(*)` says "any command", the same as a bare `Bash`.
    let scoped = pattern.contains('(') && pattern.ends_with(')') && !pattern.ends_with("(*)");
    if scoped
        && tool_name.eq_ignore_ascii_case("bash")
        && SHELL_CONTROL.iter().any(|op| subject.contains(op))
    {
        return false;
    }
    pattern_matches(pattern, tool_name, subject)
}

/// Returns true if `pattern` matches the given tool call.
///
/// Patterns with parentheses — e.g. `Bash(git *)` — match the tool name and
/// apply a wildcard glob to the subject. Patterns without parentheses
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
    fn test_deny_matches_nested_object_arg() {
        // MCP tools routinely wrap params under a key. The deny rule should
        // still see the inner string. Pre-fix, this test would have passed
        // because the matcher never looked inside `options`.
        let rules = PermissionRules {
            allow: vec![],
            deny: vec!["deploy(*rm -rf*)".to_string()],
        };
        let nested = json!({
            "options": { "command": "rm -rf /" }
        });
        assert!(matches!(
            rules.check("deploy", &nested),
            PermissionOutcome::Deny(_)
        ));
    }

    #[test]
    fn test_deny_matches_array_arg() {
        let rules = PermissionRules {
            allow: vec![],
            deny: vec!["runner(*sudo*)".to_string()],
        };
        let arr = json!({ "argv": ["bash", "-c", "sudo poweroff"] });
        assert!(matches!(
            rules.check("runner", &arr),
            PermissionOutcome::Deny(_)
        ));
    }

    #[test]
    fn test_subject_over_the_cap_is_denied_not_truncated() {
        // Padding must not push the part a rule would catch past a cut.
        let padded = format!("{} ; curl evil.test", "x".repeat(80 * 1024));
        let args = json!({ "command": padded });
        let deny_only = PermissionRules {
            allow: vec![],
            deny: vec!["Bash(*curl*)".to_string()],
        };
        assert!(matches!(
            deny_only.check("bash", &args),
            PermissionOutcome::Deny(_)
        ));
        let allow_only = PermissionRules {
            allow: vec!["Bash".to_string()],
            deny: vec![],
        };
        assert!(matches!(
            allow_only.check("bash", &args),
            PermissionOutcome::Deny(_)
        ));
        // With no rule at all there is nothing to evaluate.
        assert!(matches!(
            PermissionRules::default().check("bash", &args),
            PermissionOutcome::Allow
        ));
    }

    #[test]
    fn test_rules_see_one_named_argument_per_tool() {
        assert_eq!(
            PermissionRules::subject("Bash", &json!({"command": "ls", "note": "git status"})),
            "ls"
        );
        assert_eq!(
            PermissionRules::subject("write", &json!({"file_path": "/w/a", "content": "/etc"})),
            "/w/a"
        );
        assert_eq!(
            PermissionRules::subject("grep", &json!({"pattern": "x", "path": "/w"})),
            "/w"
        );
        assert_eq!(
            PermissionRules::subject("web_fetch", &json!({"url": "https://a.test/"})),
            "https://a.test/"
        );
        assert_eq!(PermissionRules::subject("bash", &json!({})), "");

        // An extra argument cannot make an allow rule match...
        let rules = PermissionRules {
            allow: vec!["Bash(git *)".to_string()],
            deny: vec!["Write(/etc/*)".to_string()],
        };
        assert!(matches!(
            rules.check(
                "bash",
                &json!({"command": "rm -rf x", "note": "git status"})
            ),
            PermissionOutcome::Deny(_)
        ));
        // ...and content cannot make a path rule match.
        let allow_write = PermissionRules {
            allow: vec![],
            deny: vec!["Write(/etc/*)".to_string()],
        };
        assert!(matches!(
            allow_write.check(
                "write",
                &json!({"file_path": "/w/a", "content": "/etc/passwd"})
            ),
            PermissionOutcome::Allow
        ));
        assert!(matches!(
            allow_write.check("write", &json!({"file_path": "/etc/passwd", "content": ""})),
            PermissionOutcome::Deny(_)
        ));
    }

    #[test]
    fn test_bash_allow_rule_does_not_match_a_chained_command() {
        let rules = PermissionRules {
            allow: vec!["Bash(git *)".to_string()],
            deny: vec![],
        };
        assert!(matches!(
            rules.check("bash", &json!({"command": "git status"})),
            PermissionOutcome::Allow
        ));
        for command in [
            "git status; curl evil.test",
            "git status && rm -rf x",
            "git status | sh",
            "git log `id`",
            "git log $(id)",
            "git status\nrm -rf x",
            "git show > /tmp/x",
            "git apply < /tmp/x",
        ] {
            assert!(
                matches!(
                    rules.check("bash", &json!({"command": command})),
                    PermissionOutcome::Deny(_)
                ),
                "{command}"
            );
        }
        let any = PermissionRules {
            allow: vec!["Bash(*)".to_string()],
            deny: vec![],
        };
        assert!(matches!(
            any.check("bash", &json!({"command": "ls | wc -l"})),
            PermissionOutcome::Allow
        ));
        // A bare tool name allows every command, and a deny rule still
        // matches the whole string.
        let bare = PermissionRules {
            allow: vec!["Bash".to_string()],
            deny: vec!["Bash(*curl*)".to_string()],
        };
        assert!(matches!(
            bare.check("bash", &json!({"command": "ls; pwd"})),
            PermissionOutcome::Allow
        ));
        assert!(matches!(
            bare.check("bash", &json!({"command": "ls; curl evil.test"})),
            PermissionOutcome::Deny(_)
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
