//! Tests for the Claude Code CLI provider.

use super::mcp;
use super::*;
use kernex_core::context::McpServer;
use kernex_core::traits::Provider;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[test]
fn test_default_provider() {
    let provider = ClaudeCodeProvider::new();
    assert_eq!(provider.name(), "claude-code");
    assert!(!provider.requires_api_key());
    assert_eq!(provider.max_turns, 25);
    assert!(provider.allowed_tools.is_empty());
    assert_eq!(provider.timeout, Duration::from_secs(3600));
    assert!(provider.working_dir.is_none());
    assert_eq!(provider.max_resume_attempts, 5);
    assert!(provider.model.is_empty());
}

#[test]
fn test_from_config_with_timeout() {
    let provider = ClaudeCodeProvider::from_config(
        5,
        vec!["Bash".into()],
        300,
        None,
        3,
        "claude-sonnet-4-6".into(),
        None,
    );
    assert_eq!(provider.max_turns, 5);
    assert_eq!(provider.timeout, Duration::from_secs(300));
    assert!(provider.working_dir.is_none());
    assert_eq!(provider.max_resume_attempts, 3);
    assert_eq!(provider.model, "claude-sonnet-4-6");
}

#[test]
fn test_from_config_with_working_dir() {
    let dir = PathBuf::from("/home/user/.kernex/workspace");
    let provider = ClaudeCodeProvider::from_config(
        10,
        vec!["Bash".into()],
        600,
        Some(dir.clone()),
        5,
        String::new(),
        None,
    );
    assert_eq!(provider.working_dir, Some(dir));
}

#[test]
fn test_parse_response_max_turns_with_session() {
    let provider = ClaudeCodeProvider::new();
    let json = r#"{"type":"result","subtype":"error_max_turns","result":"partial work done","session_id":"sess-123","model":"claude-sonnet-4-20250514"}"#;
    let (text, model) = provider.parse_response(json, 100);
    assert_eq!(text, "partial work done");
    assert_eq!(model, Some("claude-sonnet-4-20250514".to_string()));
}

#[test]
fn test_parse_response_success() {
    let provider = ClaudeCodeProvider::new();
    let json = r#"{"type":"result","subtype":"success","result":"all done","model":"claude-sonnet-4-20250514"}"#;
    let (text, model) = provider.parse_response(json, 100);
    assert_eq!(text, "all done");
    assert_eq!(model, Some("claude-sonnet-4-20250514".to_string()));
}

// --- MCP tests ---

#[test]
fn test_mcp_tool_patterns_empty() {
    assert!(mcp::mcp_tool_patterns(&[]).is_empty());
}

#[test]
fn test_mcp_tool_patterns() {
    let servers = vec![
        McpServer {
            name: "playwright".into(),
            command: "npx".into(),
            args: vec!["@playwright/mcp".into()],
            ..Default::default()
        },
        McpServer {
            name: "postgres".into(),
            command: "npx".into(),
            args: vec!["@pg/mcp".into()],
            ..Default::default()
        },
    ];
    let patterns = mcp::mcp_tool_patterns(&servers);
    assert_eq!(patterns, vec!["mcp__playwright__*", "mcp__postgres__*"]);
}

#[tokio::test]
async fn test_write_and_cleanup_mcp_settings() {
    let tmp = std::env::temp_dir().join("__kernex_test_mcp_settings__");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let servers = vec![McpServer {
        name: "playwright".into(),
        command: "npx".into(),
        args: vec!["@playwright/mcp".into(), "--headless".into()],
        ..Default::default()
    }];

    let path = mcp::write_mcp_settings(&tmp, &servers).await.unwrap();
    assert!(path.exists());

    // Verify JSON structure.
    let content = std::fs::read_to_string(&path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    let mcp_val = &parsed["mcpServers"]["playwright"];
    assert_eq!(mcp_val["command"], "npx");
    assert_eq!(mcp_val["args"][0], "@playwright/mcp");
    assert_eq!(mcp_val["args"][1], "--headless");

    // Cleanup.
    mcp::cleanup_mcp_settings(&path).await;
    assert!(!path.exists());

    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn test_cleanup_mcp_settings_nonexistent() {
    // Should not panic on missing file.
    mcp::cleanup_mcp_settings(Path::new("/tmp/__kernex_nonexistent_mcp_settings__")).await;
}

// --- CLI argument construction tests ---

#[test]
fn test_build_run_cli_args_no_agent_name() {
    let args = ClaudeCodeProvider::build_run_cli_args(
        "hello world",
        &[],
        100,
        &[],
        "claude-sonnet-4-6",
        false,
        None,
        None,
    );
    assert!(
        !args.contains(&"--agent".to_string()),
        "Without agent_name, --agent flag must NOT be present"
    );
    let p_idx = args
        .iter()
        .position(|a| a == "-p")
        .expect("-p flag must be present");
    assert_eq!(args[p_idx + 1], "hello world", "prompt must follow -p");
    assert!(args.contains(&"--output-format".to_string()));
    assert!(args.contains(&"json".to_string()));
    assert!(args.contains(&"--max-turns".to_string()));
    assert!(args.contains(&"100".to_string()));
    assert!(args.contains(&"--model".to_string()));
    assert!(args.contains(&"claude-sonnet-4-6".to_string()));
}

#[test]
fn test_build_run_cli_args_with_agent_name() {
    let args = ClaudeCodeProvider::build_run_cli_args(
        "Build me a task tracker.",
        &[],
        100,
        &[],
        "claude-opus-4-6",
        false,
        None,
        Some("build-analyst"),
    );
    let agent_idx = args
        .iter()
        .position(|a| a == "--agent")
        .expect("--agent flag must be present when agent_name is Some");
    assert_eq!(
        args[agent_idx + 1],
        "build-analyst",
        "--agent must be followed by the agent name"
    );
    let p_idx = args
        .iter()
        .position(|a| a == "-p")
        .expect("-p flag must be present");
    assert_eq!(
        args[p_idx + 1],
        "Build me a task tracker.",
        "prompt must follow -p"
    );
}

#[test]
fn test_build_run_cli_args_agent_with_model_override() {
    let args = ClaudeCodeProvider::build_run_cli_args(
        "Begin.",
        &[],
        100,
        &[],
        "claude-opus-4-6",
        false,
        None,
        Some("build-architect"),
    );
    assert!(args.contains(&"--agent".to_string()));
    assert!(args.contains(&"--model".to_string()));
    assert!(args.contains(&"claude-opus-4-6".to_string()));
}

#[test]
fn test_build_run_cli_args_agent_with_skip_permissions() {
    let args = ClaudeCodeProvider::build_run_cli_args(
        "Begin.",
        &[],
        100,
        &[],
        "",
        false,
        None,
        Some("build-developer"),
    );
    assert!(args.contains(&"--agent".to_string()));
    assert!(args.contains(&"--dangerously-skip-permissions".to_string()));
}

#[test]
fn test_build_run_cli_args_agent_with_max_turns() {
    let args = ClaudeCodeProvider::build_run_cli_args(
        "Begin.",
        &[],
        25,
        &[],
        "",
        false,
        None,
        Some("build-analyst"),
    );
    let mt_idx = args
        .iter()
        .position(|a| a == "--max-turns")
        .expect("--max-turns must be present");
    assert_eq!(args[mt_idx + 1], "25");
}

#[test]
fn test_build_run_cli_args_agent_name_empty_string() {
    let args =
        ClaudeCodeProvider::build_run_cli_args("Begin.", &[], 100, &[], "", false, None, Some(""));
    assert!(!args.contains(&"--agent".to_string()));
}

#[test]
fn test_build_run_cli_args_agent_name_with_session_id() {
    let args = ClaudeCodeProvider::build_run_cli_args(
        "Begin.",
        &[],
        100,
        &[],
        "",
        false,
        Some("sess-123"),
        Some("build-qa"),
    );
    assert!(args.contains(&"--agent".to_string()));
    assert!(!args.contains(&"--resume".to_string()));
}

#[test]
fn test_build_run_cli_args_agent_name_path_traversal() {
    let args = ClaudeCodeProvider::build_run_cli_args(
        "Begin.",
        &[],
        100,
        &[],
        "",
        false,
        None,
        Some("../../../etc/passwd"),
    );
    assert!(!args.contains(&"--agent".to_string()));
}

#[test]
fn test_build_run_cli_args_agent_name_with_slash() {
    let args = ClaudeCodeProvider::build_run_cli_args(
        "Begin.",
        &[],
        100,
        &[],
        "",
        false,
        None,
        Some("foo/bar"),
    );
    assert!(!args.contains(&"--agent".to_string()));
}

#[test]
fn test_build_run_cli_args_agent_name_with_backslash() {
    let args = ClaudeCodeProvider::build_run_cli_args(
        "Begin.",
        &[],
        100,
        &[],
        "",
        false,
        None,
        Some("foo\\bar"),
    );
    assert!(!args.contains(&"--agent".to_string()));
}

#[test]
fn test_build_run_cli_args_explicit_allowed_tools_no_agent() {
    let args = ClaudeCodeProvider::build_run_cli_args(
        "hello",
        &[],
        50,
        &["Bash".to_string(), "Read".to_string()],
        "claude-sonnet-4-6",
        false,
        None,
        None,
    );
    let at_count = args.iter().filter(|a| *a == "--allowedTools").count();
    assert_eq!(at_count, 2);
    assert!(!args.contains(&"--dangerously-skip-permissions".to_string()));
}

#[test]
fn test_build_run_cli_args_disabled_tools() {
    let args =
        ClaudeCodeProvider::build_run_cli_args("classify this", &[], 5, &[], "", true, None, None);
    let at_idx = args.iter().position(|a| a == "--allowedTools").unwrap();
    assert_eq!(args[at_idx + 1], "");
}
