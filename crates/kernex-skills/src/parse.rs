//! Shared parsing utilities for skill and project frontmatter.

use std::path::Path;

/// Strip surrounding quotes (single or double) from a string.
pub(crate) fn unquote(s: &str) -> String {
    let s = s.trim();
    if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')) {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

/// Parse a YAML-style inline list: `[a, b, c]` or `["a", "b"]`.
pub(crate) fn parse_yaml_list(val: &str) -> Vec<String> {
    let trimmed = val.trim();
    let inner = trimmed
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or("");
    if inner.is_empty() {
        return Vec::new();
    }
    inner
        .split(',')
        .map(|s| unquote(s.trim()))
        .filter(|s| !s.is_empty())
        .collect()
}

/// Extract `bins` from an openclaw metadata JSON blob.
///
/// Looks for `"requires":{"bins":["tool1","tool2"]}` without a full JSON parser.
pub(crate) fn extract_bins_from_metadata(meta: &str) -> Vec<String> {
    let Some(idx) = meta.find("\"bins\"") else {
        return Vec::new();
    };
    let rest = &meta[idx..];
    let Some(start) = rest.find('[') else {
        return Vec::new();
    };
    let Some(end) = rest[start..].find(']') else {
        return Vec::new();
    };
    let inner = &rest[start + 1..start + end];
    inner
        .split(',')
        .map(|s| unquote(s.trim()))
        .filter(|s| !s.is_empty())
        .collect()
}

/// Check whether a CLI tool exists on `$PATH`.
///
/// Uses a pure-Rust PATH search instead of spawning a `which` subprocess
/// to avoid blocking I/O in the async runtime.
pub(crate) fn which_exists(tool: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths).any(|dir| {
                let candidate = dir.join(tool);
                candidate.is_file()
            })
        })
        .unwrap_or(false)
}

/// Resolve `{data_dir}/` with tilde expansion, appending a subdirectory.
pub(crate) fn data_path(data_dir: &str, sub: &str) -> std::path::PathBuf {
    Path::new(&kernex_core::shellexpand(data_dir)).join(sub)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unquote_double_quotes() {
        assert_eq!(unquote(r#""hello""#), "hello");
    }

    #[test]
    fn unquote_single_quotes() {
        assert_eq!(unquote("'hello'"), "hello");
    }

    #[test]
    fn unquote_no_quotes() {
        assert_eq!(unquote("hello"), "hello");
    }

    #[test]
    fn unquote_trims_whitespace() {
        assert_eq!(unquote("  hello  "), "hello");
        assert_eq!(unquote("  \"hello\"  "), "hello");
    }

    #[test]
    fn unquote_mismatched_quotes() {
        // Mismatched quotes should not be stripped
        assert_eq!(unquote("\"hello'"), "\"hello'");
        assert_eq!(unquote("'hello\""), "'hello\"");
    }

    #[test]
    fn parse_yaml_list_simple() {
        let result = parse_yaml_list("[a, b, c]");
        assert_eq!(result, vec!["a", "b", "c"]);
    }

    #[test]
    fn parse_yaml_list_quoted() {
        let result = parse_yaml_list(r#"["foo", "bar"]"#);
        assert_eq!(result, vec!["foo", "bar"]);
    }

    #[test]
    fn parse_yaml_list_mixed() {
        let result = parse_yaml_list(r#"[foo, "bar", 'baz']"#);
        assert_eq!(result, vec!["foo", "bar", "baz"]);
    }

    #[test]
    fn parse_yaml_list_empty() {
        let result = parse_yaml_list("[]");
        assert!(result.is_empty());
    }

    #[test]
    fn parse_yaml_list_single() {
        let result = parse_yaml_list("[npx]");
        assert_eq!(result, vec!["npx"]);
    }

    #[test]
    fn parse_yaml_list_whitespace() {
        let result = parse_yaml_list("[  a  ,  b  ,  c  ]");
        assert_eq!(result, vec!["a", "b", "c"]);
    }

    #[test]
    fn parse_yaml_list_no_brackets() {
        let result = parse_yaml_list("not a list");
        assert!(result.is_empty());
    }

    #[test]
    fn extract_bins_simple() {
        let meta = r#"{"requires":{"bins":["git","curl"]}}"#;
        let bins = extract_bins_from_metadata(meta);
        assert_eq!(bins, vec!["git", "curl"]);
    }

    #[test]
    fn extract_bins_with_quotes() {
        let meta = r#"{"requires":{"bins":["npx", "node"]}}"#;
        let bins = extract_bins_from_metadata(meta);
        assert_eq!(bins, vec!["npx", "node"]);
    }

    #[test]
    fn extract_bins_empty() {
        let meta = r#"{"requires":{"bins":[]}}"#;
        let bins = extract_bins_from_metadata(meta);
        assert!(bins.is_empty());
    }

    #[test]
    fn extract_bins_missing() {
        let meta = r#"{"requires":{}}"#;
        let bins = extract_bins_from_metadata(meta);
        assert!(bins.is_empty());
    }

    #[test]
    fn extract_bins_no_requires() {
        let meta = r#"{"name":"test"}"#;
        let bins = extract_bins_from_metadata(meta);
        assert!(bins.is_empty());
    }

    #[test]
    fn which_exists_common_tools() {
        // These should exist on most systems
        assert!(which_exists("ls") || which_exists("dir"));
    }

    #[test]
    fn which_exists_nonexistent() {
        assert!(!which_exists("__nonexistent_tool_xyz_12345__"));
    }

    #[test]
    fn data_path_simple() {
        let path = data_path("/home/user/.kernex", "skills");
        assert_eq!(path, std::path::PathBuf::from("/home/user/.kernex/skills"));
    }

    #[test]
    fn data_path_nested() {
        let path = data_path("/data", "a/b/c");
        assert_eq!(path, std::path::PathBuf::from("/data/a/b/c"));
    }
}
