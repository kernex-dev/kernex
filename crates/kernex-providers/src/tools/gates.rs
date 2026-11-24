//! The two gates a tool call passes before it runs: the allowlist of tool
//! names and the permission rules.
//!
//! The allowlist decides which tools exist for a request. The rules then
//! match one value per call. For a file tool that value is the file the tool
//! would open, so a relative path, a symlink or another name for the
//! workspace all meet the same rule. Search output is filtered by the same
//! rules: a search that starts above a denied directory does not return what
//! is inside it.

use std::path::Path;

use kernex_core::permissions::{PermissionOutcome, PermissionRules};
use serde_json::Value;

use super::ToolExecutor;

impl ToolExecutor {
    /// Whether the allowlist admits `tool_name`.
    pub(super) fn tool_allowed(&self, tool_name: &str) -> bool {
        match &self.allowed_tools {
            None => true,
            Some(list) => list.iter().any(|entry| self.entry_names(entry, tool_name)),
        }
    }

    /// Whether one allowlist entry names `tool_name`.
    ///
    /// Names compare without regard to case or underscores, so `Read`,
    /// `web_fetch` and `WebFetch` all work. A pattern suffix as in
    /// `Bash(git *)` is ignored here (permission rules are where patterns
    /// apply). `mcp__server` names every tool of that MCP server and
    /// `mcp__server__tool` one of them.
    fn entry_names(&self, entry: &str, tool_name: &str) -> bool {
        let entry = entry.split('(').next().unwrap_or(entry).trim();
        if let Some(rest) = entry.strip_prefix("mcp__") {
            let (server, tool) = match rest.split_once("__") {
                Some((server, tool)) => (server, Some(tool)),
                None => (rest, None),
            };
            let owned = self
                .mcp_tool_map
                .get(tool_name)
                .is_some_and(|owner| owner == server);
            return owned && tool.map_or(true, |tool| tool == "*" || tool == tool_name);
        }
        let fold = |name: &str| -> String {
            name.chars()
                .filter(|c| *c != '_')
                .map(|c| c.to_ascii_lowercase())
                .collect()
        };
        !entry.is_empty() && fold(entry) == fold(tool_name)
    }

    /// What the permission rules match for this call.
    ///
    /// A file tool's path is the file the tool would open: made absolute
    /// against the workspace, symlinks resolved, and spelled with the
    /// workspace path as the host configured it. A search root is given with
    /// and without a trailing slash, so a rule for `dir/*` covers a search
    /// that starts at `dir`.
    pub(super) fn rule_subjects(&self, tool_name: &str, args: &Value) -> Vec<String> {
        let given = PermissionRules::subject(tool_name, args);
        match tool_name.to_ascii_lowercase().as_str() {
            "read" | "write" | "edit" => vec![self.rule_path(&given)],
            "grep" | "glob" => {
                let root = self.rule_path(&given);
                vec![format!("{}/", root.trim_end_matches('/')), root]
            }
            _ => vec![given],
        }
    }

    /// The path a rule sees for the argument `given` (empty: the workspace).
    fn rule_path(&self, given: &str) -> String {
        let lexical = if given.is_empty() {
            self.workspace_path.clone()
        } else {
            self.resolve_path(given)
        };
        self.rule_spelling(&kernex_sandbox::resolved_path(&lexical))
    }

    /// A resolved path, spelled for the rules: one inside the workspace is
    /// written from the workspace path the host configured, which may itself
    /// be reached through a symlink (`/tmp` on macOS).
    fn rule_spelling(&self, resolved: &Path) -> String {
        let workspace = kernex_sandbox::resolved_path(&self.workspace_path);
        let spelled = match resolved.strip_prefix(&workspace) {
            Ok(rest) if rest.as_os_str().is_empty() => self.workspace_path.clone(),
            Ok(rest) => self.workspace_path.join(rest),
            Err(_) => resolved.to_path_buf(),
        };
        spelled.to_string_lossy().into_owned()
    }

    /// Drop the lines of search output that name a file the rules would not
    /// let `tool_name` or `read` reach. A search that starts above a denied
    /// directory must not return what is inside it.
    pub(super) fn without_rule_denied(&self, tool_name: &str, output: String) -> String {
        let Some(rules) = &self.permission_rules else {
            return output;
        };
        let mut kept = String::with_capacity(output.len());
        for line in output.lines() {
            let path = search_result_path(line);
            let hidden = path.starts_with('/') && {
                let subject = self.rule_spelling(Path::new(path));
                rules.denies("read", &subject)
                    || matches!(
                        rules.check_subject(tool_name, &subject),
                        PermissionOutcome::Deny(_)
                    )
            };
            if !hidden {
                kept.push_str(line);
                kept.push('\n');
            }
        }
        kept
    }
}

/// The path one line of `grep` or `find` output names: the whole line for
/// `find`, the part before `:<line number>:` for `grep`, and the file of a
/// `Binary file <path> matches` notice.
fn search_result_path(line: &str) -> &str {
    if let Some(rest) = line.strip_prefix("Binary file ") {
        return rest.strip_suffix(" matches").unwrap_or(rest);
    }
    let bytes = line.as_bytes();
    let mut start = 0;
    while let Some(offset) = line[start..].find(':') {
        let colon = start + offset;
        let digits = bytes[colon + 1..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count();
        if digits > 0 && bytes.get(colon + 1 + digits) == Some(&b':') {
            return &line[..colon];
        }
        start = colon + 1;
    }
    line
}
