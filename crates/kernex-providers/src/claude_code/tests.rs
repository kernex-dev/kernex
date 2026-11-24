//! Tests for the Claude Code CLI provider.

use super::command::CliCall;
use super::mcp;
use super::permissions::ToolAccess;
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
    assert!(provider.allowed_tools.is_none());
    assert!(!provider.permission_bypass);
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
    assert_eq!(provider.allowed_tools, Some(vec!["Bash".to_string()]));
    // An empty list sets no default.
    let none = ClaudeCodeProvider::from_config(5, vec![], 300, None, 3, String::new(), None);
    assert!(none.allowed_tools.is_none());
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
async fn test_write_and_cleanup_mcp_config() {
    let servers = vec![McpServer {
        name: "playwright".into(),
        command: "npx".into(),
        args: vec!["@playwright/mcp".into(), "--headless".into()],
        allowed_commands: vec!["npx".into()],
        ..Default::default()
    }];

    // Writes a throwaway temp file (not under any workspace .claude/ dir).
    let path = mcp::write_mcp_config_tempfile(&std::env::temp_dir(), &servers)
        .await
        .unwrap();
    assert!(path.exists());
    assert!(
        !path.components().any(|c| c.as_os_str() == ".claude"),
        "MCP config must not be written under a .claude/ dir; got {path:?}"
    );

    // Verify JSON structure (the --mcp-config shape).
    let content = std::fs::read_to_string(&path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    let mcp_val = &parsed["mcpServers"]["playwright"];
    assert_eq!(mcp_val["command"], "npx");
    assert_eq!(mcp_val["args"][0], "@playwright/mcp");
    assert_eq!(mcp_val["args"][1], "--headless");

    // Cleanup removes the temp file.
    mcp::cleanup_mcp_config(&path).await;
    assert!(!path.exists());
}

#[tokio::test]
async fn test_cleanup_mcp_config_nonexistent() {
    // Should not panic on missing file.
    mcp::cleanup_mcp_config(Path::new("/tmp/__kernex_nonexistent_mcp_config__")).await;
}

#[cfg(unix)]
#[tokio::test]
async fn test_mcp_config_tempfile_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let servers = vec![McpServer {
        name: "perm-check".into(),
        command: "kx".into(),
        args: vec!["mcp".into()],
        ..Default::default()
    }];
    let path = mcp::write_mcp_config_tempfile(&std::env::temp_dir(), &servers)
        .await
        .unwrap();
    // Configured MCP env values can carry credentials; the file must be
    // owner-readable only, regardless of umask.
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "expected 0600, got {mode:o}");
    mcp::cleanup_mcp_config(&path).await;
}

// --- CLI argument construction tests ---

/// Arguments for one call with no tool list, no bypass, fetch disabled.
fn call_args(
    prompt: &str,
    max_turns: u32,
    model: &str,
    session_id: Option<&str>,
    agent_name: Option<&str>,
) -> Vec<String> {
    args_with(
        prompt, max_turns, model, session_id, agent_name, None, false,
    )
}

fn args_with(
    prompt: &str,
    max_turns: u32,
    model: &str,
    session_id: Option<&str>,
    agent_name: Option<&str>,
    allowed: Option<&[String]>,
    bypass: bool,
) -> Vec<String> {
    let web = crate::WebFetchPolicy::Disabled;
    ClaudeCodeProvider::build_run_cli_args(&CliCall {
        prompt,
        max_turns,
        model,
        session_id,
        agent_name,
        access: ToolAccess {
            allowed,
            bypass,
            web_fetch: &web,
            mcp_servers: &[],
        },
        mcp_config_path: None,
    })
}

/// The prompt: the last argument, after `--`.
fn prompt_of(args: &[String]) -> Option<&str> {
    match args {
        [.., sep, prompt] if sep == "--" => Some(prompt),
        _ => None,
    }
}

fn value_of<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == flag)
        .map(|i| args[i + 1].as_str())
}

#[test]
fn test_build_run_cli_args_no_agent_name() {
    let args = call_args("hello world", 100, "claude-sonnet-4-6", None, None);
    assert!(!args.contains(&"--agent".to_string()));
    assert!(args.contains(&"-p".to_string()));
    assert_eq!(prompt_of(&args), Some("hello world"));
    assert_eq!(value_of(&args, "--output-format"), Some("json"));
    assert_eq!(value_of(&args, "--max-turns"), Some("100"));
    assert_eq!(value_of(&args, "--model"), Some("claude-sonnet-4-6"));
}

#[test]
fn test_build_run_cli_args_rejects_flag_injection_in_model() {
    // Context-poisoned model that tries to smuggle a flag must be dropped,
    // not pushed as a value the CLI's argv parser would re-interpret.
    let args = call_args("hello", 100, "--system-prompt=evil", None, None);
    assert!(!args.contains(&"--system-prompt=evil".to_string()));
    assert!(!args.contains(&"--model".to_string()));
}

#[test]
fn test_a_prompt_that_looks_like_an_option_stays_the_prompt() {
    let args = call_args("--debug-file=/tmp/x", 100, "", None, Some("helper"));
    assert_eq!(prompt_of(&args), Some("--debug-file=/tmp/x"));
    assert_eq!(
        args.iter().filter(|a| *a == "--debug-file=/tmp/x").count(),
        1
    );
}

#[test]
fn test_build_run_cli_args_rejects_flag_injection_in_session_id() {
    let args = call_args(
        "hello",
        100,
        "claude-sonnet-4-6",
        Some("--dangerously-skip-permissions"),
        None,
    );
    assert!(!args.contains(&"--resume".to_string()));
    assert!(!args.contains(&"--dangerously-skip-permissions".to_string()));
}

#[test]
fn test_build_run_cli_args_rejects_flag_injection_in_agent_name() {
    let args = call_args(
        "hello",
        100,
        "claude-sonnet-4-6",
        None,
        Some("--system-prompt=evil"),
    );
    assert!(!args.contains(&"--agent".to_string()));
    assert!(!args.contains(&"--system-prompt=evil".to_string()));
}

#[test]
fn test_build_run_cli_args_with_agent_name() {
    let args = call_args(
        "Build me a task tracker.",
        100,
        "claude-opus-4-6",
        None,
        Some("build-analyst"),
    );
    assert_eq!(value_of(&args, "--agent"), Some("build-analyst"));
    assert_eq!(prompt_of(&args), Some("Build me a task tracker."));
    assert_eq!(value_of(&args, "--model"), Some("claude-opus-4-6"));
}

#[test]
fn test_agent_mode_keeps_permission_checks_without_the_bypass() {
    let args = call_args("Begin.", 100, "", None, Some("build-developer"));
    assert!(args.contains(&"--agent".to_string()));
    assert!(!args.contains(&"--dangerously-skip-permissions".to_string()));
    assert_eq!(value_of(&args, "--permission-mode"), Some("dontAsk"));
    let bypass = args_with("Begin.", 100, "", None, Some("build-developer"), None, true);
    assert!(bypass.contains(&"--dangerously-skip-permissions".to_string()));
}

#[test]
fn test_build_run_cli_args_agent_with_max_turns() {
    let args = call_args("Begin.", 25, "", None, Some("build-analyst"));
    assert_eq!(value_of(&args, "--max-turns"), Some("25"));
}

#[test]
fn test_build_run_cli_args_agent_name_rejected_forms() {
    for name in ["", "../../../etc/passwd", "foo/bar", "foo\\bar"] {
        let args = call_args("Begin.", 100, "", None, Some(name));
        assert!(!args.contains(&"--agent".to_string()), "{name:?}");
    }
}

#[test]
fn test_build_run_cli_args_agent_name_with_session_id() {
    let args = call_args("Begin.", 100, "", Some("sess-123"), Some("build-qa"));
    assert!(args.contains(&"--agent".to_string()));
    assert!(!args.contains(&"--resume".to_string()));
}

#[test]
fn test_build_run_cli_args_explicit_allowed_tools() {
    let list = vec!["Bash".to_string(), "Read".to_string()];
    let args = args_with("hello", 50, "", None, None, Some(&list), true);
    let at_count = args.iter().filter(|a| *a == "--allowedTools").count();
    assert_eq!(at_count, 2);
    assert_eq!(value_of(&args, "--tools"), Some("Bash,Read"));
    assert!(!args.contains(&"--dangerously-skip-permissions".to_string()));
}

#[test]
fn test_a_resume_keeps_the_tool_restriction() {
    // Auto-resume goes through the same builder: a request that disabled
    // tools resumes with them disabled, not with the bypass.
    let args = args_with(
        "Continue where you left off.",
        5,
        "",
        Some("sess-1"),
        None,
        Some(&[]),
        true,
    );
    assert_eq!(value_of(&args, "--resume"), Some("sess-1"));
    assert_eq!(value_of(&args, "--tools"), Some(""));
    assert!(!args.contains(&"--dangerously-skip-permissions".to_string()));
}

#[test]
fn test_mcp_config_path_is_passed() {
    let web = crate::WebFetchPolicy::Disabled;
    let path = Path::new("/tmp/kernex-mcp-1-0.json");
    let args = ClaudeCodeProvider::build_run_cli_args(&CliCall {
        prompt: "hi",
        max_turns: 1,
        model: "",
        session_id: None,
        agent_name: None,
        access: ToolAccess {
            allowed: None,
            bypass: false,
            web_fetch: &web,
            mcp_servers: &["fs".to_string()],
        },
        mcp_config_path: Some(path),
    });
    assert_eq!(
        value_of(&args, "--mcp-config"),
        Some("/tmp/kernex-mcp-1-0.json")
    );
    assert_eq!(value_of(&args, "--allowedTools"), Some("mcp__fs__*"));
}

#[test]
fn test_the_cli_config_dir_is_inside_the_data_dir() {
    let p = ClaudeCodeProvider::new()
        .with_sandbox_dirs(crate::SandboxDirs::new().with_data_dir("/srv/agent/state"));
    assert_eq!(
        p.cli_config_dir(),
        PathBuf::from("/srv/agent/state/cli/claude")
    );
}
