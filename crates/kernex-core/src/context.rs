//! Conversation context passed to AI providers.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Strategy applied when conversation history exceeds `max_context_messages`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CompactionStrategy {
    /// Drop the oldest messages silently (default, preserves existing behavior).
    #[default]
    Drop,
    /// Summarize overflow messages and prepend the summary to the system prompt.
    ///
    /// Requires a [`Summarizer`](crate::traits::Summarizer) to be injected at
    /// `build_context` time. Falls back to `Drop` if none is provided.
    Summarize,
}

/// Controls which optional context blocks are loaded and injected.
///
/// Used by the runtime to skip expensive DB queries and prompt sections
/// when the user's message doesn't need them — reducing token overhead.
#[derive(Debug, Clone)]
pub struct ContextNeeds {
    /// Load semantic recall (FTS5 related past messages).
    pub recall: bool,
    /// Load and inject pending scheduled tasks.
    pub pending_tasks: bool,
    /// Inject user profile (facts) into the system prompt.
    pub profile: bool,
    /// Load and inject recent conversation summaries.
    pub summaries: bool,
    /// Load and inject recent reward outcomes.
    pub outcomes: bool,
    /// How to handle history overflow (default: silently drop oldest).
    pub compact: CompactionStrategy,
}

impl Default for ContextNeeds {
    fn default() -> Self {
        Self {
            recall: true,
            pending_tasks: true,
            profile: true,
            summaries: true,
            outcomes: true,
            compact: CompactionStrategy::default(),
        }
    }
}

/// A single entry in the conversation history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextEntry {
    /// "user" or "assistant".
    pub role: String,
    /// The message content.
    pub content: String,
}

/// An MCP server declared by a skill.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpServer {
    /// Server name (used as the key in provider settings).
    pub name: String,
    /// Command to launch the server.
    pub command: String,
    /// Command-line arguments.
    pub args: Vec<String>,
    /// Environment variables passed to the server process.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,
}

/// A simple script-based tool that runs without a full MCP server.
///
/// The script receives tool arguments as JSON on stdin and returns its
/// result on stdout. Exit code 0 means success; non-zero means error.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Toolbox {
    /// Tool name exposed to the AI model.
    pub name: String,
    /// Human-readable description shown in tool definitions.
    pub description: String,
    /// JSON Schema for the tool's input parameters.
    #[serde(default = "default_object_schema")]
    pub parameters: serde_json::Value,
    /// Command to execute (e.g. "bash", "python3").
    pub command: String,
    /// Command-line arguments (e.g. ["scripts/lint.sh"]).
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variables passed to the script process.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,
    /// Whether the tool subprocess may open network connections. Defaults to
    /// `false`: sandboxed tool subprocesses are denied network egress unless
    /// the tool declares `network = true`. Enforced at the OS sandbox layer
    /// (full coverage on macOS Seatbelt; TCP bind/connect on Linux 6.7+).
    #[serde(default)]
    pub network: bool,
    /// Parent environment variable NAMES this tool may receive, resolved at
    /// spawn time. The spawn boundary clears the inherited environment; this
    /// list is the declared, user-approvable opt-in for what gets re-added
    /// (e.g. a skill that needs `GITHUB_TOKEN`). Empty = nothing extra.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env_passthrough: Vec<String>,
    /// Command allow-list this tool's `command` must satisfy at execution
    /// time, carried from the owning skill's declared permissions. Empty =
    /// unrestricted (no allow-list was declared). Enforced by the executor
    /// before spawning, as defense in depth behind the load-time checks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_commands: Vec<String>,
    /// Keywords for dynamic tool discovery via tool search.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub search_hints: Vec<String>,
}

/// Does `command` satisfy a declared command allow-list?
///
/// - An empty `allowed` list means no restriction was declared: `true`.
/// - Entries containing `/` are full paths and must equal `command` exactly.
/// - Entries without `/` are basenames and match any command whose final
///   path segment equals the entry (`npx` permits `/usr/bin/npx`).
///
/// Single source of truth for allow-list semantics: the skills loader and
/// the tool executor both call this, so load-time and run-time enforcement
/// cannot drift apart.
pub fn command_matches_allowlist(allowed: &[String], command: &str) -> bool {
    if allowed.is_empty() {
        return true;
    }
    let basename = command.rsplit('/').next().unwrap_or(command);
    allowed.iter().any(|entry| {
        if entry.contains('/') {
            entry == command
        } else {
            entry == basename
        }
    })
}

fn default_object_schema() -> serde_json::Value {
    serde_json::json!({"type": "object"})
}

fn is_false(b: &bool) -> bool {
    !b
}

/// Conversation context passed to an AI provider.
#[derive(Clone, Serialize, Deserialize)]
pub struct Context {
    /// System prompt prepended to every request.
    pub system_prompt: String,
    /// Conversation history (oldest first).
    pub history: Vec<ContextEntry>,
    /// The current user message.
    pub current_message: String,
    /// MCP servers to activate for this request.
    #[serde(default)]
    pub mcp_servers: Vec<McpServer>,
    /// Script-based tools to activate for this request.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub toolboxes: Vec<Toolbox>,
    /// Override the provider's default max_turns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<u32>,
    /// Override the provider's default allowed tools.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<Vec<String>>,
    /// Override the provider's default model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Session ID for conversation continuity (e.g. Claude Code CLI).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Agent name for agent-mode providers. When set, the provider loads
    /// the agent definition and `to_prompt_string()` emits only `current_message`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    /// Hook runner for tool lifecycle events. Not serialized.
    #[serde(skip)]
    pub hook_runner: Option<std::sync::Arc<dyn crate::hooks::HookRunner>>,
    /// Declarative allow/deny permission rules applied before each tool call.
    /// Not serialized — set at runtime by the caller.
    #[serde(skip)]
    pub permission_rules: Option<std::sync::Arc<crate::permissions::PermissionRules>>,
    /// Request thinking (chain-of-thought) for Anthropic requests. When true,
    /// the Anthropic provider sends `thinking: {"type": "adaptive"}` in the
    /// request body (GA on the Claude 4.6+ family; it also enables interleaved
    /// thinking between tool calls). When false, the field is omitted and
    /// thinking is off.
    #[serde(default, skip_serializing_if = "is_false")]
    pub extended_thinking: bool,
}

// Manual Debug impl: HookRunner is no longer required to be Debug (lifted in
// kernex-core::hooks so SDK clients without Debug derives can be wired in
// directly). We surface a placeholder for the runner / rules so the rest of
// Context still prints usefully in tracing spans.
impl std::fmt::Debug for Context {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Context")
            .field("system_prompt", &self.system_prompt)
            .field("history", &self.history)
            .field("current_message", &self.current_message)
            .field("mcp_servers", &self.mcp_servers)
            .field("toolboxes", &self.toolboxes)
            .field("max_turns", &self.max_turns)
            .field("allowed_tools", &self.allowed_tools)
            .field("model", &self.model)
            .field("session_id", &self.session_id)
            .field("agent_name", &self.agent_name)
            .field(
                "hook_runner",
                &self.hook_runner.as_ref().map(|_| "<runner>"),
            )
            .field(
                "permission_rules",
                &self.permission_rules.as_ref().map(|_| "<rules>"),
            )
            .field("extended_thinking", &self.extended_thinking)
            .finish()
    }
}

/// A structured message for API-based providers (OpenAI, Anthropic, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiMessage {
    /// "user" or "assistant".
    pub role: String,
    /// The message content.
    pub content: String,
}

impl Context {
    /// Create a new context with a current message and empty system prompt.
    pub fn new(message: &str) -> Self {
        Self {
            system_prompt: String::new(),
            history: Vec::new(),
            current_message: message.to_string(),
            mcp_servers: Vec::new(),
            toolboxes: Vec::new(),
            max_turns: None,
            allowed_tools: None,
            model: None,
            session_id: None,
            agent_name: None,
            hook_runner: None,
            permission_rules: None,
            extended_thinking: false,
        }
    }

    /// Attach a hook runner to this context.
    pub fn with_hooks(mut self, runner: std::sync::Arc<dyn crate::hooks::HookRunner>) -> Self {
        self.hook_runner = Some(runner);
        self
    }

    /// Flatten the context into a single prompt string for providers
    /// that accept a single text input (e.g. Claude Code CLI).
    ///
    /// When `agent_name` is set, returns only the current message.
    /// When `session_id` is set (continuation), skips full system prompt and history.
    pub fn to_prompt_string(&self) -> String {
        if self.agent_name.is_some() {
            return self.current_message.clone();
        }

        let mut parts = Vec::new();

        if self.session_id.is_none() {
            if !self.system_prompt.is_empty() {
                parts.push(format!("[System]\n{}", self.system_prompt));
            }
            for entry in &self.history {
                let role = if entry.role == "user" {
                    "User"
                } else {
                    "Assistant"
                };
                parts.push(format!("[{}]\n{}", role, entry.content));
            }
            parts.push(format!("[User]\n{}", self.current_message));
        } else {
            if !self.system_prompt.is_empty() {
                parts.push(format!(
                    "[User]\n{}\n\n{}",
                    self.system_prompt, self.current_message
                ));
            } else {
                parts.push(format!("[User]\n{}", self.current_message));
            }
        }

        parts.join("\n\n")
    }

    /// Convert context to structured API messages.
    ///
    /// Returns `(system_prompt, messages)` — the system prompt is separated
    /// because Anthropic and Gemini require it outside the messages array.
    pub fn to_api_messages(&self) -> (String, Vec<ApiMessage>) {
        let mut messages = Vec::with_capacity(self.history.len() + 1);

        for entry in &self.history {
            messages.push(ApiMessage {
                role: entry.role.clone(),
                content: entry.content.clone(),
            });
        }

        messages.push(ApiMessage {
            role: "user".to_string(),
            content: self.current_message.clone(),
        });

        (self.system_prompt.clone(), messages)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_new_defaults() {
        let ctx = Context::new("hello");
        assert!(ctx.system_prompt.is_empty());
        assert!(ctx.history.is_empty());
        assert!(ctx.mcp_servers.is_empty());
        assert!(ctx.toolboxes.is_empty());
        assert_eq!(ctx.current_message, "hello");
        assert!(ctx.session_id.is_none());
        assert!(ctx.agent_name.is_none());
    }

    #[test]
    fn test_mcp_server_serde_round_trip() {
        let server = McpServer {
            name: "playwright".into(),
            command: "npx".into(),
            args: vec!["@playwright/mcp".into(), "--headless".into()],
            env: HashMap::new(),
        };
        let json = serde_json::to_string(&server).unwrap();
        let deserialized: McpServer = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "playwright");
        assert_eq!(deserialized.args, vec!["@playwright/mcp", "--headless"]);
    }

    #[test]
    fn test_context_serde_without_optional_fields() {
        let json = r#"{"system_prompt":"test","history":[],"current_message":"hi"}"#;
        let ctx: Context = serde_json::from_str(json).unwrap();
        assert!(ctx.mcp_servers.is_empty());
        assert!(ctx.session_id.is_none());
        assert!(ctx.agent_name.is_none());
    }

    #[test]
    fn test_to_api_messages_basic() {
        let ctx = Context::new("hello");
        let (system, messages) = ctx.to_api_messages();
        assert!(system.is_empty());
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].content, "hello");
    }

    #[test]
    fn test_to_api_messages_with_history() {
        let ctx = Context {
            system_prompt: "Be helpful.".into(),
            history: vec![
                ContextEntry {
                    role: "user".into(),
                    content: "Hi".into(),
                },
                ContextEntry {
                    role: "assistant".into(),
                    content: "Hello!".into(),
                },
            ],
            current_message: "How are you?".into(),
            mcp_servers: Vec::new(),
            toolboxes: Vec::new(),
            max_turns: None,
            allowed_tools: None,
            model: None,
            session_id: None,
            agent_name: None,
            hook_runner: None,
            permission_rules: None,
            extended_thinking: false,
        };
        let (system, messages) = ctx.to_api_messages();
        assert_eq!(system, "Be helpful.");
        assert_eq!(messages.len(), 3);
    }

    #[test]
    fn test_to_prompt_string_no_session() {
        let ctx = Context {
            system_prompt: "Be helpful.".into(),
            history: vec![ContextEntry {
                role: "user".into(),
                content: "Hi".into(),
            }],
            current_message: "How are you?".into(),
            mcp_servers: Vec::new(),
            toolboxes: Vec::new(),
            max_turns: None,
            allowed_tools: None,
            model: None,
            session_id: None,
            agent_name: None,
            hook_runner: None,
            permission_rules: None,
            extended_thinking: false,
        };
        let prompt = ctx.to_prompt_string();
        assert!(prompt.contains("[System]\nBe helpful."));
        assert!(prompt.contains("[User]\nHi"));
        assert!(prompt.contains("[User]\nHow are you?"));
    }

    #[test]
    fn test_to_prompt_string_with_session() {
        let ctx = Context {
            system_prompt: "Current time: 2026-03-06".into(),
            history: vec![ContextEntry {
                role: "user".into(),
                content: "Hi".into(),
            }],
            current_message: "How are you?".into(),
            mcp_servers: Vec::new(),
            toolboxes: Vec::new(),
            max_turns: None,
            allowed_tools: None,
            model: None,
            session_id: Some("sess-abc".into()),
            agent_name: None,
            hook_runner: None,
            permission_rules: None,
            extended_thinking: false,
        };
        let prompt = ctx.to_prompt_string();
        assert!(!prompt.contains("[System]"));
        assert!(prompt.contains("[User]\nCurrent time: 2026-03-06\n\nHow are you?"));
    }

    #[test]
    fn test_to_prompt_string_with_agent_name() {
        let ctx = Context {
            system_prompt: "You are a build analyst...".into(),
            history: vec![ContextEntry {
                role: "user".into(),
                content: "prev".into(),
            }],
            current_message: "Build me a task tracker.".into(),
            mcp_servers: Vec::new(),
            toolboxes: Vec::new(),
            max_turns: None,
            allowed_tools: None,
            model: None,
            session_id: None,
            agent_name: Some("build-analyst".into()),
            hook_runner: None,
            permission_rules: None,
            extended_thinking: false,
        };
        let prompt = ctx.to_prompt_string();
        assert_eq!(prompt, "Build me a task tracker.");
    }

    #[test]
    fn test_agent_name_takes_precedence_over_session_id() {
        let ctx = Context {
            system_prompt: "system".into(),
            history: Vec::new(),
            current_message: "Build something.".into(),
            mcp_servers: Vec::new(),
            toolboxes: Vec::new(),
            max_turns: None,
            allowed_tools: None,
            model: None,
            session_id: Some("sess-456".into()),
            agent_name: Some("build-architect".into()),
            hook_runner: None,
            permission_rules: None,
            extended_thinking: false,
        };
        assert_eq!(ctx.to_prompt_string(), "Build something.");
    }

    #[test]
    fn test_session_id_serde_round_trip() {
        let ctx = Context {
            system_prompt: "test".into(),
            history: Vec::new(),
            current_message: "hi".into(),
            mcp_servers: Vec::new(),
            toolboxes: Vec::new(),
            max_turns: None,
            allowed_tools: None,
            model: None,
            session_id: Some("sess-123".into()),
            agent_name: None,
            hook_runner: None,
            permission_rules: None,
            extended_thinking: false,
        };
        let json = serde_json::to_string(&ctx).unwrap();
        let deserialized: Context = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.session_id, Some("sess-123".into()));
    }

    #[test]
    fn test_optional_fields_skipped_in_serialization() {
        let ctx = Context::new("hello");
        let json = serde_json::to_string(&ctx).unwrap();
        assert!(!json.contains("session_id"));
        assert!(!json.contains("agent_name"));
        assert!(!json.contains("max_turns"));
        assert!(!json.contains("toolboxes"));
    }

    #[test]
    fn test_toolbox_serde_round_trip() {
        let tb = Toolbox {
            name: "lint".into(),
            description: "Run linter on a file.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {"file": {"type": "string"}},
                "required": ["file"]
            }),
            command: "bash".into(),
            args: vec!["scripts/lint.sh".into()],
            env: HashMap::new(),
            network: false,
            env_passthrough: Vec::new(),
            allowed_commands: Vec::new(),
            search_hints: Vec::new(),
        };
        let json = serde_json::to_string(&tb).unwrap();
        let deserialized: Toolbox = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "lint");
        assert_eq!(deserialized.command, "bash");
        assert_eq!(deserialized.args, vec!["scripts/lint.sh"]);
    }

    #[test]
    fn test_toolbox_default_parameters() {
        let json = r#"{"name":"test","description":"Test tool.","command":"echo"}"#;
        let tb: Toolbox = serde_json::from_str(json).unwrap();
        assert_eq!(tb.parameters, serde_json::json!({"type": "object"}));
        assert!(tb.args.is_empty());
        assert!(tb.env.is_empty());
    }

    #[test]
    fn test_context_serde_with_toolboxes() {
        let mut ctx = Context::new("run lint");
        ctx.toolboxes.push(Toolbox {
            name: "lint".into(),
            description: "Lint a file.".into(),
            parameters: serde_json::json!({"type": "object"}),
            command: "bash".into(),
            args: vec!["lint.sh".into()],
            env: HashMap::new(),
            network: false,
            env_passthrough: Vec::new(),
            allowed_commands: Vec::new(),
            search_hints: Vec::new(),
        });
        let json = serde_json::to_string(&ctx).unwrap();
        assert!(json.contains("toolboxes"));
        let deserialized: Context = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.toolboxes.len(), 1);
        assert_eq!(deserialized.toolboxes[0].name, "lint");
    }
}
