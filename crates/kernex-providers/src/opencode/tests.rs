//! Tests for the OpenCode CLI provider.

use super::config::{build_config, permissions};
use super::events::parse_events;
use super::*;
use crate::WebFetchPolicy;
use kernex_core::context::McpServer;
use kernex_core::traits::Provider;
use serde_json::Value;
use std::collections::HashMap;

#[test]
fn test_default_provider() {
    let p = OpenCodeProvider::new();
    assert_eq!(p.name(), "opencode");
    assert!(!p.requires_api_key());
    assert_eq!(p.timeout, Duration::from_secs(3600));
    assert!(p.working_dir.is_none());
    assert!(p.model.is_empty());
}

#[test]
fn test_from_config() {
    let p = OpenCodeProvider::from_config(
        300,
        Some(PathBuf::from("/tmp/ws")),
        "ollama/qwen3-coder:30b".into(),
    )
    .with_env_passthrough(vec!["CUSTOM_KEY".into()]);
    assert_eq!(p.timeout, Duration::from_secs(300));
    assert_eq!(p.working_dir, Some(PathBuf::from("/tmp/ws")));
    assert_eq!(p.model, "ollama/qwen3-coder:30b");
    assert_eq!(p.extra_env, vec!["CUSTOM_KEY".to_string()]);
}

// --- argument building ---

#[test]
fn test_args_minimal() {
    let args = OpenCodeProvider::build_run_args("hi", "", None, None);
    assert_eq!(args, vec!["run", "--format", "json", "--", "hi"]);
}

#[test]
fn test_args_full() {
    let args = OpenCodeProvider::build_run_args(
        "hi",
        "anthropic/claude-sonnet-4-6",
        Some("ses_1"),
        Some("reviewer"),
    );
    assert_eq!(
        args,
        vec![
            "run",
            "--format",
            "json",
            "--agent",
            "reviewer",
            "--model",
            "anthropic/claude-sonnet-4-6",
            "--session",
            "ses_1",
            "--",
            "hi"
        ]
    );
}

#[test]
fn test_args_prompt_starting_with_dash_stays_positional() {
    let args = OpenCodeProvider::build_run_args("--help", "", None, None);
    let sep = args.iter().position(|a| a == "--").unwrap();
    assert_eq!(args[sep + 1], "--help");
    assert_eq!(args.len(), sep + 2);
}

#[test]
fn test_args_reject_flag_injection() {
    let args = OpenCodeProvider::build_run_args(
        "hi",
        "--print-logs",
        Some("--dangerous"),
        Some("--agent=evil"),
    );
    assert_eq!(args, vec!["run", "--format", "json", "--", "hi"]);
}

#[test]
fn test_args_reject_agent_path_traversal() {
    for bad in ["../x", "a/b", "a\\b", ""] {
        let args = OpenCodeProvider::build_run_args("hi", "", None, Some(bad));
        assert!(!args.contains(&"--agent".to_string()), "accepted {bad:?}");
    }
}

// --- permissions ---

fn list(entries: &[&str]) -> Vec<String> {
    entries.iter().map(|s| s.to_string()).collect()
}

const ALL: WebFetchPolicy = WebFetchPolicy::AllowAll;
const OFF: WebFetchPolicy = WebFetchPolicy::Disabled;

#[test]
fn test_permissions_bypass_allows_all_but_prompts() {
    let p = permissions(None, true, &ALL, &[]);
    assert_eq!(p["bash"], "allow");
    assert_eq!(p["edit"], "allow");
    assert_eq!(p["external_directory"], "allow");
    assert_eq!(p["webfetch"], "allow");
    assert_eq!(p["question"], "deny");
    assert_eq!(p["doom_loop"], "deny");
}

#[test]
fn test_permissions_without_bypass_are_read_only() {
    let p = permissions(None, false, &ALL, &[]);
    for key in ["read", "glob", "grep", "list", "lsp", "todowrite"] {
        assert_eq!(p[key], "allow", "{key}");
    }
    for key in [
        "bash",
        "edit",
        "task",
        "external_directory",
        "skill",
        "webfetch",
    ] {
        assert_eq!(p[key], "deny", "{key}");
    }
}

#[test]
fn test_everything_unnamed_is_denied_first() {
    for p in [
        permissions(None, false, &ALL, &[]),
        permissions(Some(&[]), false, &ALL, &[]),
        permissions(Some(&list(&["Read"])), true, &ALL, &[]),
    ] {
        assert_eq!(p["*"], "deny");
        assert_eq!(p.keys().next().map(String::as_str), Some("*"));
    }
    assert_eq!(permissions(None, true, &ALL, &[])["*"], "allow");
}

#[test]
fn test_codesearch_follows_the_fetch_policy() {
    assert_eq!(permissions(None, true, &OFF, &[])["codesearch"], "deny");
    assert_eq!(permissions(None, true, &ALL, &[])["codesearch"], "allow");
    let l = list(&["codesearch"]);
    assert_eq!(
        permissions(Some(&l), false, &ALL, &[])["codesearch"],
        "allow"
    );
}

#[test]
fn test_passed_mcp_servers_are_allowed_by_name() {
    let fs = McpServer {
        name: "fs".into(),
        command: "npx".into(),
        network: true,
        allowed_commands: vec!["npx".into()],
        ..Default::default()
    };
    let servers = [fs];
    let p = permissions(None, false, &OFF, &servers);
    assert_eq!(p["fs_*"], "allow");
    let l = list(&["mcp__fs__read_file", "Read"]);
    let p = permissions(Some(&l), false, &OFF, &servers);
    assert_eq!(p["fs_read_file"], "allow");
    assert!(p.get("fs_*").is_none());
    let l = list(&["mcp__fs"]);
    let p = permissions(Some(&l), false, &OFF, &servers);
    assert_eq!(p["fs_*"], "allow");
}

#[test]
fn test_permissions_empty_list_denies_all() {
    let p = permissions(Some(&[]), true, &ALL, &[]);
    assert!(p.values().all(|v| v == "deny"));
}

#[test]
fn test_permissions_list_maps_claude_names_and_ignores_the_bypass() {
    let l = list(&["Read", "Write", "grep", "Bash(git *)", "mcp__fs__read"]);
    let p = permissions(Some(&l), true, &ALL, &[]);
    assert_eq!(p["read"], "allow");
    assert_eq!(p["edit"], "allow");
    assert_eq!(p["grep"], "allow");
    assert_eq!(p["bash"], "allow");
    assert_eq!(p["webfetch"], "deny");
    assert_eq!(p["task"], "deny");
}

#[test]
fn test_fetch_needs_allow_all() {
    let l = list(&["WebFetch", "websearch"]);
    for policy in [OFF, WebFetchPolicy::AllowHosts(list(&["docs.rs"]))] {
        for p in [
            permissions(Some(&l), false, &policy, &[]),
            permissions(None, true, &policy, &[]),
        ] {
            assert_eq!(p["webfetch"], "deny");
            assert_eq!(p["websearch"], "deny");
        }
    }
    let p = permissions(Some(&l), false, &ALL, &[]);
    assert_eq!(p["webfetch"], "allow");
    assert_eq!(p["websearch"], "allow");
}

#[test]
fn test_permissions_never_ask() {
    for p in [
        permissions(None, false, &OFF, &[]),
        permissions(None, true, &ALL, &[]),
        permissions(Some(&[]), false, &ALL, &[]),
        permissions(Some(&list(&["Bash"])), false, &OFF, &[]),
    ] {
        assert!(p.values().all(|v| v == "allow" || v == "deny"));
    }
}

#[test]
fn test_config_includes_mcp_servers() {
    let mut env = HashMap::new();
    env.insert("TOKEN".to_string(), "x".to_string());
    let servers = vec![McpServer {
        name: "fs".into(),
        command: "npx".into(),
        args: vec!["-y".into(), "server-fs".into()],
        env,
        allowed_commands: vec!["npx".into()],
        network: true,
    }];
    let cfg: Value = serde_json::from_str(&build_config(None, false, &OFF, &servers)).unwrap();
    let fs = &cfg["mcp"]["fs"];
    assert_eq!(fs["type"], "local");
    assert_eq!(fs["command"], serde_json::json!(["npx", "-y", "server-fs"]));
    assert_eq!(fs["environment"]["TOKEN"], "x");
    assert_eq!(fs["enabled"], true);
}

#[test]
fn test_config_omits_empty_mcp() {
    let cfg: Value = serde_json::from_str(&build_config(None, false, &OFF, &[])).unwrap();
    assert!(cfg.get("mcp").is_none());
    assert!(cfg.get("permission").is_some());
}

// --- environment ---

#[test]
fn test_only_the_configured_provider_key_is_named() {
    assert_eq!(
        provider_key_env("anthropic/claude-sonnet-4-6"),
        ["ANTHROPIC_API_KEY"]
    );
    assert_eq!(provider_key_env("openai/gpt-5"), ["OPENAI_API_KEY"]);
    assert_eq!(
        provider_key_env("google/gemini-3-pro"),
        ["GOOGLE_GENERATIVE_AI_API_KEY", "GEMINI_API_KEY"]
    );
    assert!(provider_key_env("ollama/qwen3-coder:30b").is_empty());
    assert!(provider_key_env("").is_empty());
    assert!(provider_key_env("anthropic").is_empty());
}

// --- event parsing ---

const RUN: &str = r#"{"type":"step_start","timestamp":1,"sessionID":"ses_abc","part":{"type":"step-start"}}
{"type":"text","timestamp":2,"sessionID":"ses_abc","part":{"text":"I'll read the file."}}
{"type":"tool_use","timestamp":2,"sessionID":"ses_abc","part":{"tool":"bash","callID":"c1","state":{"status":"completed","output":"hello"}}}
{"type":"step_finish","timestamp":3,"sessionID":"ses_abc","part":{"reason":"tool-calls","tokens":{"total":100,"input":90,"output":10}}}
not json
{"type":"step_start","timestamp":4,"sessionID":"ses_abc","part":{"type":"step-start"}}
{"type":"text","timestamp":4,"sessionID":"ses_abc","part":{"text":"The file says hello."}}
{"type":"step_finish","timestamp":5,"sessionID":"ses_abc","part":{"reason":"stop","tokens":{"input":20,"output":5,"reasoning":0}}}
"#;

#[test]
fn test_parse_run() {
    let out = parse_events(RUN);
    assert_eq!(out.text, "The file says hello.");
    assert_eq!(out.session_id.as_deref(), Some("ses_abc"));
    assert_eq!(out.tokens_used, Some(125));
    assert_eq!(out.stop_reason.as_deref(), Some("stop"));
    assert!(out.error.is_none());
}

#[test]
fn test_parse_joins_text_parts() {
    let s = r#"{"type":"text","part":{"text":"one"}}
{"type":"text","part":{"text":"  "}}
{"type":"text","part":{"text":"two"}}"#;
    assert_eq!(parse_events(s).text, "one\n\ntwo");
}

#[test]
fn test_parse_falls_back_to_last_step_with_text() {
    let s = r#"{"type":"step_start","part":{}}
{"type":"text","part":{"text":"answer"}}
{"type":"step_start","part":{}}
{"type":"step_finish","part":{"reason":"stop"}}"#;
    assert_eq!(parse_events(s).text, "answer");
}

#[test]
fn test_parse_error_event() {
    let s = r#"{"type":"error","sessionID":"s","error":{"name":"APIError","data":{"message":"model not found"}}}"#;
    let out = parse_events(s);
    assert!(out.text.is_empty());
    assert_eq!(out.error.as_deref(), Some("model not found"));
}

#[test]
fn test_parse_empty() {
    assert_eq!(parse_events(""), events::RunOutput::default());
}
