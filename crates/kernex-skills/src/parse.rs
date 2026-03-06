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
