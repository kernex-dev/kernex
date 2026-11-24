//! Which tools the Claude CLI may use, as command-line flags.
//!
//! The rules follow the tool allowlist of the HTTP providers:
//!
//! - `None` offers every tool. Without the permission bypass the CLI runs
//!   in `dontAsk` mode: what it would ask about is refused, so only its
//!   read-only tools, the declared MCP servers' tools and, under
//!   [`WebFetchPolicy::AllowAll`], its fetch tool run. With the bypass,
//!   every tool runs.
//! - `Some([])` offers no tool and starts no MCP server.
//! - `Some(list)` offers the built-in tools the list names and pre-approves
//!   each entry, patterns included (`Bash(git *)`). The bypass never applies
//!   to a list.
//!
//! The CLI splits a rule argument on commas and spaces outside parentheses,
//! so an entry that would split into several rules is dropped rather than
//! passed.
//!
//! Settings files are not read (`--setting-sources ""`) and only the MCP
//! servers kernex passes are started (`--strict-mcp-config`): a settings or
//! `.mcp.json` file the agent wrote in an earlier call cannot widen what the
//! next call may do.
//!
//! The CLI's `WebFetch` and `WebSearch` run only under
//! [`WebFetchPolicy::AllowAll`]. A host list cannot be held to: the CLI
//! approves a built-in list of documentation hosts without asking. The CLI
//! keeps the network it needs to reach the model, so the policy governs the
//! fetch tools only.

use crate::WebFetchPolicy;

/// Built-in tool names in kernex spelling (folded) and CLI spelling.
const BUILTINS: &[(&str, &str)] = &[
    ("bash", "Bash"),
    ("read", "Read"),
    ("write", "Write"),
    ("edit", "Edit"),
    ("multiedit", "MultiEdit"),
    ("grep", "Grep"),
    ("glob", "Glob"),
    ("ls", "LS"),
    ("notebookedit", "NotebookEdit"),
    ("task", "Task"),
    ("todowrite", "TodoWrite"),
    ("webfetch", "WebFetch"),
    ("websearch", "WebSearch"),
];

/// What one CLI call may do.
#[derive(Clone, Copy)]
pub(super) struct ToolAccess<'a> {
    /// `None` offers every tool; see the module docs.
    pub allowed: Option<&'a [String]>,
    /// Run with the CLI's permission checks off (only without a list).
    pub bypass: bool,
    pub web_fetch: &'a WebFetchPolicy,
    /// Names of the MCP servers passed through `--mcp-config`.
    pub mcp_servers: &'a [String],
}

/// `name` folded for comparison: lowercase, underscores dropped.
fn fold(name: &str) -> String {
    name.chars()
        .filter(|c| *c != '_')
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Split an entry into its tool name and pattern suffix (`Bash(git *)`
/// gives `Bash` and `(git *)`).
fn split_entry(entry: &str) -> (&str, &str) {
    let entry = entry.trim();
    match entry.find('(') {
        Some(i) => (entry[..i].trim(), &entry[i..]),
        None => (entry, ""),
    }
}

/// Whether the CLI reads `entry` as the one rule it looks like: a name of
/// ASCII letters, digits, `_`, `-` and `*`, optionally followed by one
/// parenthesized pattern that ends the entry. Anything else (a comma or a
/// space outside the parentheses, a second group, text after the group)
/// would reach the CLI as more than one rule.
fn is_single_rule(entry: &str) -> bool {
    let entry = entry.trim();
    let (name, suffix) = match entry.find('(') {
        Some(i) => (&entry[..i], &entry[i..]),
        None => (entry, ""),
    };
    let name_ok = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '*'));
    let suffix_ok = suffix.is_empty()
        || (suffix.len() >= 2
            && suffix.ends_with(')')
            && !suffix[1..suffix.len() - 1].contains(['(', ')']));
    name_ok && suffix_ok
}

/// The entry with a built-in tool name in CLI spelling (`web_fetch` becomes
/// `WebFetch`). Other names are kept as written.
fn cli_spelling(entry: &str) -> String {
    let (name, suffix) = split_entry(entry);
    let folded = fold(name);
    match BUILTINS.iter().find(|(k, _)| *k == folded) {
        Some((_, cli)) => format!("{cli}{suffix}"),
        None => format!("{name}{suffix}"),
    }
}

fn is_tool(entry: &str, cli_name: &str) -> bool {
    split_entry(entry).0 == cli_name
}

/// The permission flags for one call.
pub(super) fn permission_args(access: &ToolAccess<'_>) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "--setting-sources".into(),
        String::new(),
        "--strict-mcp-config".into(),
    ];
    let fetch_allowed = matches!(access.web_fetch, WebFetchPolicy::AllowAll);
    let deny: &[&str] = if fetch_allowed {
        &[]
    } else {
        &["WebFetch", "WebSearch"]
    };
    let mut allow: Vec<String> = Vec::new();

    match access.allowed {
        Some(list) => {
            for raw in list {
                if !is_single_rule(raw) {
                    tracing::warn!(
                        "claude-code: tool entry {raw:?} dropped: the CLI would read it as \
                         more than one rule"
                    );
                    continue;
                }
                let entry = cli_spelling(raw);
                let blocked = deny.iter().any(|d| is_tool(&entry, d));
                if !blocked && !allow.contains(&entry) {
                    allow.push(entry);
                }
            }
            let mut builtins: Vec<&str> = Vec::new();
            for entry in &allow {
                let name = split_entry(entry).0;
                if BUILTINS.iter().any(|(_, cli)| *cli == name) && !builtins.contains(&name) {
                    builtins.push(name);
                }
            }
            args.extend(["--tools".into(), builtins.join(",")]);
            args.extend(["--permission-mode".into(), "dontAsk".into()]);
        }
        None if access.bypass => {
            args.push("--dangerously-skip-permissions".into());
        }
        None => {
            if fetch_allowed {
                allow.push("WebFetch".into());
            }
            for server in access.mcp_servers {
                allow.push(format!("mcp__{server}__*"));
            }
            args.extend(["--permission-mode".into(), "dontAsk".into()]);
        }
    }

    if matches!(access.web_fetch, WebFetchPolicy::AllowHosts(_)) {
        tracing::warn!(
            "claude-code: WebFetch is turned off: the CLI cannot be held to a host list"
        );
    }
    for entry in allow {
        args.extend(["--allowedTools".into(), entry]);
    }
    for entry in deny {
        args.extend(["--disallowedTools".into(), (*entry).into()]);
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(
        allowed: Option<&[String]>,
        bypass: bool,
        web: WebFetchPolicy,
        servers: &[&str],
    ) -> Vec<String> {
        let servers: Vec<String> = servers.iter().map(|s| s.to_string()).collect();
        permission_args(&ToolAccess {
            allowed,
            bypass,
            web_fetch: &web,
            mcp_servers: &servers,
        })
    }

    fn values<'a>(args: &'a [String], flag: &str) -> Vec<&'a str> {
        args.windows(2)
            .filter(|w| w[0] == flag)
            .map(|w| w[1].as_str())
            .collect()
    }

    fn list(entries: &[&str]) -> Vec<String> {
        entries.iter().map(|s| s.to_string()).collect()
    }

    fn hosts() -> WebFetchPolicy {
        WebFetchPolicy::AllowHosts(list(&["docs.rs"]))
    }

    #[test]
    fn settings_files_and_other_mcp_servers_are_never_loaded() {
        for a in [
            args(None, false, WebFetchPolicy::Disabled, &[]),
            args(None, true, WebFetchPolicy::AllowAll, &[]),
            args(Some(&[]), false, WebFetchPolicy::Disabled, &[]),
        ] {
            assert_eq!(values(&a, "--setting-sources"), vec![""]);
            assert!(a.contains(&"--strict-mcp-config".to_string()));
        }
    }

    #[test]
    fn no_list_without_bypass_asks_nothing_and_refuses_the_rest() {
        let a = args(None, false, WebFetchPolicy::Disabled, &["fs"]);
        assert_eq!(values(&a, "--permission-mode"), vec!["dontAsk"]);
        assert!(!a.contains(&"--dangerously-skip-permissions".to_string()));
        assert!(!a.contains(&"--tools".to_string()));
        assert_eq!(values(&a, "--allowedTools"), vec!["mcp__fs__*"]);
        assert_eq!(
            values(&a, "--disallowedTools"),
            vec!["WebFetch", "WebSearch"]
        );
    }

    #[test]
    fn the_bypass_needs_the_flag() {
        let a = args(None, true, WebFetchPolicy::AllowAll, &["fs"]);
        assert!(a.contains(&"--dangerously-skip-permissions".to_string()));
        assert!(!a.contains(&"--permission-mode".to_string()));
        assert!(values(&a, "--disallowedTools").is_empty());
    }

    #[test]
    fn a_host_list_turns_fetch_off_in_every_mode() {
        let l = list(&["Read", "WebFetch", "WebFetch(domain:docs.rs)"]);
        for a in [
            args(None, true, hosts(), &[]),
            args(None, false, hosts(), &[]),
            args(Some(&l), false, hosts(), &[]),
        ] {
            assert_eq!(
                values(&a, "--disallowedTools"),
                vec!["WebFetch", "WebSearch"]
            );
            assert!(!values(&a, "--allowedTools")
                .iter()
                .any(|e| e.starts_with("WebFetch")));
        }
        let a = args(Some(&l), false, hosts(), &[]);
        assert_eq!(values(&a, "--tools"), vec!["Read"]);
    }

    #[test]
    fn an_empty_list_offers_no_tool() {
        let a = args(Some(&[]), true, WebFetchPolicy::AllowAll, &[]);
        assert_eq!(values(&a, "--tools"), vec![""]);
        assert_eq!(values(&a, "--permission-mode"), vec!["dontAsk"]);
        assert!(values(&a, "--allowedTools").is_empty());
        assert!(!a.contains(&"--dangerously-skip-permissions".to_string()));
    }

    #[test]
    fn a_list_names_tools_in_cli_spelling_and_keeps_patterns() {
        let l = list(&[
            "bash(git *)",
            "read",
            "Grep",
            "mcp__fs__read_file",
            "todo_write",
        ]);
        let a = args(Some(&l), true, WebFetchPolicy::Disabled, &[]);
        assert_eq!(values(&a, "--tools"), vec!["Bash,Read,Grep,TodoWrite"]);
        assert_eq!(
            values(&a, "--allowedTools"),
            vec![
                "Bash(git *)",
                "Read",
                "Grep",
                "mcp__fs__read_file",
                "TodoWrite"
            ]
        );
        assert!(!a.contains(&"--dangerously-skip-permissions".to_string()));
    }

    #[test]
    fn an_entry_the_cli_would_split_is_dropped() {
        let l = list(&[
            "Grep,WebFetch",
            "Read WebFetch(domain:evil.test)",
            "Bash(git *),Write",
            "Bash(a)(b)",
            "Bash(a",
            "",
            "Glob",
        ]);
        let a = args(Some(&l), false, WebFetchPolicy::AllowAll, &[]);
        assert_eq!(values(&a, "--allowedTools"), vec!["Glob"]);
        assert_eq!(values(&a, "--tools"), vec!["Glob"]);
    }

    #[test]
    fn a_rule_pattern_may_hold_spaces_and_commas() {
        let l = list(&["Bash(git commit -m a,b)"]);
        let a = args(Some(&l), false, WebFetchPolicy::Disabled, &[]);
        assert_eq!(
            values(&a, "--allowedTools"),
            vec!["Bash(git commit -m a,b)"]
        );
    }

    #[test]
    fn a_disabled_fetch_policy_removes_fetch_from_a_list() {
        let l = list(&["Read", "web_fetch", "WebSearch"]);
        let a = args(Some(&l), false, WebFetchPolicy::Disabled, &[]);
        assert_eq!(values(&a, "--tools"), vec!["Read"]);
        assert_eq!(values(&a, "--allowedTools"), vec!["Read"]);
    }

    #[test]
    fn allow_all_keeps_fetch_entries() {
        let l = list(&["WebFetch"]);
        let a = args(Some(&l), false, WebFetchPolicy::AllowAll, &[]);
        assert_eq!(values(&a, "--allowedTools"), vec!["WebFetch"]);
        assert_eq!(values(&a, "--tools"), vec!["WebFetch"]);
        let a = args(None, false, WebFetchPolicy::AllowAll, &[]);
        assert_eq!(values(&a, "--allowedTools"), vec!["WebFetch"]);
    }
}
