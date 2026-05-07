//! Helpers for spawning subprocesses with skill/MCP-supplied environment maps.
//!
//! Skill metadata (`mcp.json`, `toolbox.json`) is authored by third parties
//! and only loosely trusted. The dynamic linker honours `LD_PRELOAD` /
//! `DYLD_INSERT_LIBRARIES` and similar variables *before* any sandbox
//! restriction runs in `pre_exec`, so a hostile skill that injects one of
//! these env keys can hijack the spawned process and bypass Landlock /
//! Seatbelt entirely.
//!
//! Route every skill-controlled environment map through
//! [`filter_unsafe_env`] before applying it to a `Command`.

use std::collections::HashMap;

/// Environment variable names the dynamic linker honours that, if attacker-
/// controlled, can subvert the spawned process before any sandbox is applied.
///
/// Names are matched case-insensitively. Includes Linux (LD_*), macOS
/// (DYLD_*), and the auditing/profiler hooks that are equivalent in effect.
pub const UNSAFE_ENV_KEYS: &[&str] = &[
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "LD_AUDIT",
    "LD_DEBUG",
    "LD_DEBUG_OUTPUT",
    "DYLD_INSERT_LIBRARIES",
    "DYLD_LIBRARY_PATH",
    "DYLD_FALLBACK_LIBRARY_PATH",
    "DYLD_FRAMEWORK_PATH",
    "DYLD_FALLBACK_FRAMEWORK_PATH",
    "DYLD_PRINT_LIBRARIES",
    "DYLD_FORCE_FLAT_NAMESPACE",
];

/// Returns a copy of `env` with every dynamic-linker key from
/// [`UNSAFE_ENV_KEYS`] removed. Dropped keys are returned as the second
/// element so callers can `tracing::warn!` on them.
///
/// Matching is ASCII case-insensitive.
pub fn filter_unsafe_env(env: &HashMap<String, String>) -> (HashMap<String, String>, Vec<String>) {
    let mut safe = HashMap::with_capacity(env.len());
    let mut dropped = Vec::new();
    for (k, v) in env {
        if is_unsafe_env_key(k) {
            dropped.push(k.clone());
        } else {
            safe.insert(k.clone(), v.clone());
        }
    }
    (safe, dropped)
}

/// True if `k` matches any entry in [`UNSAFE_ENV_KEYS`] (case-insensitive).
pub fn is_unsafe_env_key(k: &str) -> bool {
    UNSAFE_ENV_KEYS
        .iter()
        .any(|banned| k.eq_ignore_ascii_case(banned))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ld_preload_dropped() {
        let mut env = HashMap::new();
        env.insert("LD_PRELOAD".into(), "/tmp/x.so".into());
        env.insert("PATH".into(), "/usr/bin".into());
        let (safe, dropped) = filter_unsafe_env(&env);
        assert!(!safe.contains_key("LD_PRELOAD"));
        assert_eq!(safe.get("PATH").map(String::as_str), Some("/usr/bin"));
        assert_eq!(dropped, vec!["LD_PRELOAD".to_string()]);
    }

    #[test]
    fn case_insensitive_dyld() {
        let mut env = HashMap::new();
        env.insert("dyld_insert_libraries".into(), "/tmp/x.dylib".into());
        let (safe, dropped) = filter_unsafe_env(&env);
        assert!(safe.is_empty());
        assert_eq!(dropped.len(), 1);
    }

    #[test]
    fn benign_keys_preserved() {
        let mut env = HashMap::new();
        env.insert("HOME".into(), "/home/user".into());
        env.insert("MY_TOOL_TOKEN".into(), "abc".into());
        let (safe, dropped) = filter_unsafe_env(&env);
        assert_eq!(safe.len(), 2);
        assert!(dropped.is_empty());
    }

    #[test]
    fn unsafe_key_check() {
        assert!(is_unsafe_env_key("LD_PRELOAD"));
        assert!(is_unsafe_env_key("ld_preload"));
        assert!(is_unsafe_env_key("DYLD_LIBRARY_PATH"));
        assert!(!is_unsafe_env_key("PATH"));
        assert!(!is_unsafe_env_key("LD_PRELOADX"));
    }
}
