//! The two agent-CLI providers, run against a stand-in CLI.
//!
//! The stand-in is a shell script started exactly as the real CLI would be:
//! through the sandbox wrapper, with the provider's environment and working
//! directory. It reports its working directory, its environment and its
//! arguments, and tries to write the runtime's protected state, so these
//! tests check what the subprocess really gets rather than the argument
//! list alone.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use kernex_core::context::{Context, McpServer};
use kernex_core::traits::Provider;

use crate::claude_code::ClaudeCodeProvider;
use crate::opencode::OpenCodeProvider;
use crate::{SandboxDirs, WebFetchPolicy};

/// Write the stand-in CLI. `name` decides the output format it prints:
/// a script whose name contains `opencode` prints OpenCode's event stream,
/// any other the Claude CLI's JSON result. `probes` are paths it tries to
/// write, reported in order.
fn stand_in(dir: &Path, name: &str, probes: &[&Path]) -> PathBuf {
    let path = dir.join(name);
    let probe_lines: String = probes
        .iter()
        .map(|p| {
            format!(
                "if ( : > '{}' ) 2>/dev/null; then echo PROBE=WRITTEN; else echo PROBE=DENIED; fi; ",
                p.display()
            )
        })
        .collect();
    let script = format!(
        r#"#!/bin/sh
text=$( {{ echo "CWD=$(pwd -P)"; {probe_lines} if mkdir -p "${{XDG_CONFIG_HOME:-/nonexistent}}/opencode" 2>/dev/null; then echo XDGCONF=ok; else echo XDGCONF=fail; fi; echo ENV; env | sort; for a in "$@"; do echo "ARG=$a"; done; }} \
  | tr -d '\t\r' | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' | awk '{{printf "%s\\n", $0}}' )
case "$0" in
  *opencode*) printf '{{"type":"text","sessionID":"s1","part":{{"text":"%s"}}}}\n' "$text" ;;
  *) printf '{{"type":"result","subtype":"success","result":"%s"}}\n' "$text" ;;
esac
"#
    );
    std::fs::write(&path, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

/// What the stand-in reported.
struct Report {
    cwd: PathBuf,
    probes: Vec<String>,
    /// Whether `mkdir -p $XDG_CONFIG_HOME/opencode` worked.
    config_dir_ok: bool,
    env: BTreeMap<String, String>,
    args: Vec<String>,
}

impl Report {
    fn parse(text: &str) -> Self {
        let mut report = Report {
            cwd: PathBuf::new(),
            probes: Vec::new(),
            config_dir_ok: false,
            env: BTreeMap::new(),
            args: Vec::new(),
        };
        let mut in_env = false;
        for line in text.lines() {
            if let Some(v) = line.strip_prefix("CWD=") {
                report.cwd = PathBuf::from(v);
            } else if let Some(v) = line.strip_prefix("PROBE=") {
                report.probes.push(v.to_string());
            } else if let Some(v) = line.strip_prefix("XDGCONF=") {
                report.config_dir_ok = v == "ok";
            } else if line == "ENV" {
                in_env = true;
            } else if let Some(v) = line.strip_prefix("ARG=") {
                report.args.push(v.to_string());
                in_env = false;
            } else if in_env {
                if let Some((k, v)) = line.split_once('=') {
                    report.env.insert(k.to_string(), v.to_string());
                }
            }
        }
        report
    }

    fn value_of(&self, flag: &str) -> Option<&str> {
        self.args
            .iter()
            .position(|a| a == flag)
            .map(|i| self.args[i + 1].as_str())
    }

    /// The prompt: the last argument, after `--`.
    fn prompt(&self) -> Option<&str> {
        match self.args.as_slice() {
            [.., sep, prompt] if sep == "--" => Some(prompt),
            _ => None,
        }
    }
}

/// Whether the sandbox assertions apply on this host. Fails when the run
/// requires the sandbox and the host cannot enforce it.
fn enforcing() -> bool {
    let required =
        std::env::var("KERNEX_TEST_REQUIRE_SANDBOX").is_ok_and(|v| !v.is_empty() && v != "0");
    if kernex_sandbox::os_enforcement_available() {
        return true;
    }
    assert!(!required, "no OS-level sandbox enforcement on this host");
    eprintln!("skipping the denial probes: no OS-level sandbox enforcement");
    false
}

/// Variables the subprocess may see: the hardened base set, what a shell
/// adds by itself, and what the provider passes on purpose.
fn expected_env(name: &str, provider_vars: &[&str]) -> bool {
    kernex_sandbox::BASE_ENV_ALLOWLIST.contains(&name)
        || name.starts_with("LC_")
        || ["PWD", "OLDPWD", "SHLVL", "_"].contains(&name)
        || provider_vars.contains(&name)
}

fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap()
}

/// A declared MCP server that asks for no network, and one that does.
fn mcp_servers() -> Vec<McpServer> {
    let server = |name: &str, network: bool| McpServer {
        name: name.into(),
        command: "npx".into(),
        args: vec!["-y".into(), "pkg".into(), "{workspace}".into()],
        env: HashMap::new(),
        network,
        allowed_commands: vec!["npx".into()],
    };
    vec![server("offline", false), server("online", true)]
}

#[tokio::test]
async fn claude_code_runs_sandboxed_in_a_temporary_workspace() {
    let root = tempfile::tempdir().unwrap();
    let data_dir = root.path().join("state");
    std::fs::create_dir_all(data_dir.join("data")).unwrap();
    let probe = data_dir.join("data").join("planted");
    // The MCP config the CLI reads again on resume, and the CLI's own state.
    let mcp_probe = data_dir.join("cli").join("mcp-config").join("planted");
    let own_probe = data_dir.join("cli").join("claude").join("written");
    let cli = stand_in(
        root.path(),
        "fake-claude",
        &[&probe, &mcp_probe, &own_probe],
    );

    let provider = ClaudeCodeProvider::new()
        .with_sandbox_dirs(SandboxDirs::new().with_data_dir(&data_dir))
        .with_program(cli.to_string_lossy());
    // Agent mode sends the message alone, so it can start with `-`.
    let mut ctx = Context::new("-hello");
    ctx.agent_name = Some("helper".into());
    ctx.mcp_servers = mcp_servers();
    let first = Report::parse(&provider.complete(&ctx).await.unwrap().text);
    let second = Report::parse(&provider.complete(&ctx).await.unwrap().text);

    // Each call gets its own empty directory, removed afterwards.
    let tmp = canonical(&std::env::temp_dir());
    assert!(first.cwd.starts_with(&tmp), "{:?}", first.cwd);
    assert_ne!(first.cwd, second.cwd);
    assert!(!first.cwd.exists() && !second.cwd.exists());

    // The CLI keeps its state in its own directory inside the data dir.
    let config_dir = data_dir.join("cli").join("claude");
    assert_eq!(
        first.env.get("CLAUDE_CONFIG_DIR").map(PathBuf::from),
        Some(config_dir.clone())
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&config_dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }
    let provider_vars = ["CLAUDE_CONFIG_DIR", "ANTHROPIC_API_KEY"];
    for name in first.env.keys() {
        assert!(
            expected_env(name, &provider_vars),
            "unexpected variable {name}"
        );
    }

    // No permission bypass by default, no settings files, only kernex's
    // MCP servers, and only the one that declared network.
    assert!(!first
        .args
        .contains(&"--dangerously-skip-permissions".to_string()));
    assert_eq!(first.value_of("--permission-mode"), Some("dontAsk"));
    assert_eq!(first.value_of("--setting-sources"), Some(""));
    assert!(first.args.contains(&"--strict-mcp-config".to_string()));
    assert_eq!(first.value_of("--allowedTools"), Some("mcp__online__*"));
    let mcp_config = first.value_of("--mcp-config").map(PathBuf::from).unwrap();
    assert!(mcp_config.starts_with(data_dir.join("cli").join("mcp-config")));
    // A prompt that starts with `-` stays the prompt.
    assert_eq!(first.prompt(), Some("-hello"));

    assert_eq!(first.probes[2], "WRITTEN");
    if enforcing() {
        assert_eq!(first.probes[..2], ["DENIED", "DENIED"]);
        assert!(!probe.exists() && !mcp_probe.exists());
    }
}

#[tokio::test]
async fn claude_code_works_in_the_configured_workspace() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("ws");
    std::fs::create_dir_all(&workspace).unwrap();
    let probe = workspace.join("written");
    let cli = stand_in(root.path(), "fake-claude", &[&probe]);

    let provider = ClaudeCodeProvider::from_config(
        3,
        vec![],
        60,
        Some(workspace.clone()),
        0,
        String::new(),
        None,
    )
    .with_sandbox_dirs(SandboxDirs::new().with_data_dir(root.path().join("state")))
    .with_permission_bypass(true)
    .with_web_fetch_policy(WebFetchPolicy::AllowAll)
    .with_program(cli.to_string_lossy());
    let mut ctx = Context::new("hello");
    ctx.allowed_tools = Some(vec!["Read".into()]);
    let report = Report::parse(&provider.complete(&ctx).await.unwrap().text);

    assert_eq!(report.cwd, canonical(&workspace));
    assert_eq!(report.probes, ["WRITTEN"]);
    // A request with a list never gets the bypass.
    assert!(!report
        .args
        .contains(&"--dangerously-skip-permissions".to_string()));
    assert_eq!(report.value_of("--tools"), Some("Read"));
}

fn fake_key(name: &str) -> Option<String> {
    Some(format!("value-of-{name}"))
}

#[tokio::test]
async fn opencode_gets_only_its_provider_key() {
    let root = tempfile::tempdir().unwrap();
    let data_dir = root.path().join("state");
    std::fs::create_dir_all(data_dir.join("data")).unwrap();
    let probe = data_dir.join("data").join("planted");
    let state = data_dir.join("cli").join("opencode");
    let claude_probe = data_dir.join("cli").join("claude").join("planted");
    let cli = stand_in(root.path(), "fake-opencode", &[&probe, &claude_probe]);
    std::fs::create_dir_all(data_dir.join("cli").join("claude")).unwrap();

    let provider = OpenCodeProvider::from_config(60, None, "anthropic/claude-sonnet-4-6".into())
        .with_sandbox_dirs(SandboxDirs::new().with_data_dir(&data_dir))
        .with_program(cli.to_string_lossy())
        .with_env_source(fake_key);
    let mut ctx = Context::new("hello");
    ctx.mcp_servers = mcp_servers();
    let report = Report::parse(&provider.complete(&ctx).await.unwrap().text);

    let tmp = canonical(&std::env::temp_dir());
    assert!(report.cwd.starts_with(&tmp), "{:?}", report.cwd);
    assert!(!report.cwd.exists());

    assert_eq!(
        report.env.get("ANTHROPIC_API_KEY").map(String::as_str),
        Some("value-of-ANTHROPIC_API_KEY")
    );
    // New OpenCode directories for this call, never the user's, removed
    // afterwards; no project config.
    let config_home = report
        .env
        .get("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap();
    let run = config_home.parent().unwrap().to_path_buf();
    assert_eq!(run.parent(), Some(state.as_path()));
    assert!(run
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("run-"));
    for (var, sub) in [
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_CACHE_HOME", "cache"),
    ] {
        assert_eq!(report.env.get(var).map(PathBuf::from), Some(run.join(sub)));
    }
    assert!(!run.exists());
    // OpenCode creates `$XDG_CONFIG_HOME/opencode` at startup.
    assert!(report.config_dir_ok);
    // No session survives the call, so none is offered for continuation.
    let second = provider.complete(&ctx).await.unwrap();
    assert!(second.metadata.session_id.is_none());
    assert!(!Report::parse(&second.text)
        .args
        .contains(&"--session".to_string()));
    assert_eq!(
        report
            .env
            .get("OPENCODE_DISABLE_PROJECT_CONFIG")
            .map(String::as_str),
        Some("1")
    );
    assert!(!report.env.contains_key("OPENCODE_CONFIG"));
    let provider_vars = [
        "ANTHROPIC_API_KEY",
        "OPENCODE_DISABLE_PROJECT_CONFIG",
        "OPENCODE_CONFIG_CONTENT",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
        "XDG_STATE_HOME",
    ];
    for name in report.env.keys() {
        assert!(
            expected_env(name, &provider_vars),
            "unexpected variable {name}"
        );
    }

    // Read-only permissions without the bypass; only the server that
    // declared network, with the workspace filled in.
    let config: serde_json::Value =
        serde_json::from_str(&report.env["OPENCODE_CONFIG_CONTENT"]).unwrap();
    assert_eq!(config["permission"]["*"], "deny");
    assert_eq!(config["permission"]["online_*"], "allow");
    assert_eq!(config["permission"]["bash"], "deny");
    assert_eq!(config["permission"]["read"], "allow");
    assert_eq!(config["permission"]["webfetch"], "deny");
    assert!(config["mcp"]["offline"].is_null());
    // The workspace the server is given is the call's own directory (it is
    // gone by now, so compare the name rather than the resolved path).
    let workspace_arg = config["mcp"]["online"]["command"][3].as_str().unwrap();
    assert_eq!(Path::new(workspace_arg).file_name(), report.cwd.file_name());

    if enforcing() {
        // Runtime state and the Claude CLI's state.
        assert_eq!(report.probes, ["DENIED", "DENIED"]);
        assert!(!probe.exists() && !claude_probe.exists());
    }
}

#[tokio::test]
async fn a_timed_out_cli_is_ended_with_what_it_started() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("grandchild");
    let cli = root.path().join("slow-claude");
    std::fs::write(
        &cli,
        format!(
            "#!/bin/sh\n( sleep 2; : > '{}' ) &\nsleep 5\n",
            marker.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let provider = ClaudeCodeProvider::from_config(1, vec![], 1, None, 0, String::new(), None)
        .with_sandbox_dirs(SandboxDirs::new().with_data_dir(root.path().join("state")))
        .with_program(cli.to_string_lossy());
    let err = provider.complete(&Context::new("hello")).await.unwrap_err();
    assert!(err.to_string().contains("timed out"), "{err}");
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    assert!(
        !marker.exists(),
        "a process the CLI started outlived the call"
    );
}

#[tokio::test]
async fn a_background_job_ends_with_the_call() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("background");
    let cli = root.path().join("quick-claude");
    std::fs::write(
        &cli,
        format!(
            r#"#!/bin/sh
( sleep 2; : > '{}' ) > /dev/null 2>&1 &
printf '{{"type":"result","subtype":"success","result":"ok"}}'
"#,
            marker.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let provider = ClaudeCodeProvider::new()
        .with_sandbox_dirs(SandboxDirs::new().with_data_dir(root.path().join("state")))
        .with_program(cli.to_string_lossy());
    let reply = provider.complete(&Context::new("hello")).await.unwrap();
    assert_eq!(reply.text, "ok");
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    assert!(
        !marker.exists(),
        "a job the CLI left running outlived the call"
    );
}

#[tokio::test]
async fn a_cli_state_directory_replaced_by_a_link_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let data_dir = root.path().join("state");
    let victim = root.path().join("victim");
    std::fs::create_dir_all(data_dir.join("cli")).unwrap();
    std::fs::create_dir_all(&victim).unwrap();
    let probe = victim.join("written");
    for dir in ["claude", "opencode"] {
        std::os::unix::fs::symlink(&victim, data_dir.join("cli").join(dir)).unwrap();
    }
    let dirs = SandboxDirs::new().with_data_dir(&data_dir);

    let cli = stand_in(root.path(), "fake-claude", &[&probe]);
    let claude = ClaudeCodeProvider::from_config(3, vec![], 60, None, 0, String::new(), None)
        .with_sandbox_dirs(dirs.clone())
        .with_program(cli.to_string_lossy());
    assert!(claude.complete(&Context::new("hello")).await.is_err());

    let cli = stand_in(root.path(), "fake-opencode", &[&probe]);
    let opencode = OpenCodeProvider::from_config(60, None, "anthropic/claude-sonnet-4-6".into())
        .with_sandbox_dirs(dirs)
        .with_program(cli.to_string_lossy())
        .with_env_source(fake_key);
    assert!(opencode.complete(&Context::new("hello")).await.is_err());

    // Neither CLI ran, and the link target was left as it was.
    assert_eq!(std::fs::read_dir(&victim).unwrap().count(), 0);
}
