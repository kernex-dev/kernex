//! Tests for the OpenCode CLI provider.

use super::config::{build_config, permissions};
use super::events::parse_events;
use super::*;
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

#[test]
fn test_permissions_full_access_allows_all_but_prompts() {
    let p = permissions(&[], false);
    assert_eq!(p["bash"], "allow");
    assert_eq!(p["edit"], "allow");
    assert_eq!(p["external_directory"], "allow");
    assert_eq!(p["question"], "deny");
    assert_eq!(p["doom_loop"], "deny");
}

#[test]
fn test_permissions_disabled_denies_all() {
    let p = permissions(&["Bash".into()], true);
    assert!(p.values().all(|v| v == "deny"));
}

#[test]
fn test_permissions_whitelist_maps_claude_names() {
    let p = permissions(&["Read".into(), "Write".into(), "grep".into()], false);
    assert_eq!(p["read"], "allow");
    assert_eq!(p["edit"], "allow");
    assert_eq!(p["grep"], "allow");
    assert_eq!(p["bash"], "deny");
    assert_eq!(p["webfetch"], "deny");
}

#[test]
fn test_permissions_never_ask() {
    for p in [
        permissions(&[], false),
        permissions(&[], true),
        permissions(&["Bash".into()], false),
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
    }];
    let cfg: Value = serde_json::from_str(&build_config(&[], false, &servers)).unwrap();
    let fs = &cfg["mcp"]["fs"];
    assert_eq!(fs["type"], "local");
    assert_eq!(fs["command"], serde_json::json!(["npx", "-y", "server-fs"]));
    assert_eq!(fs["environment"]["TOKEN"], "x");
    assert_eq!(fs["enabled"], true);
}

#[test]
fn test_config_omits_empty_mcp() {
    let cfg: Value = serde_json::from_str(&build_config(&[], false, &[])).unwrap();
    assert!(cfg.get("mcp").is_none());
    assert!(cfg.get("permission").is_some());
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

#[tokio::test]
async fn test_required_enforcement_without_workdir_errors() {
    let profile = kernex_sandbox::SandboxProfile {
        require_os_enforcement: true,
        ..Default::default()
    };
    let p = OpenCodeProvider::new().with_sandbox_profile(profile);
    let err = p.run_cli(&["--version".into()], "{}").await.unwrap_err();
    assert!(err.to_string().contains("unsandboxed"));
}
