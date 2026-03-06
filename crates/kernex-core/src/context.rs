//! Conversation context passed to AI providers.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Controls which optional context blocks are loaded and injected.
///
/// Used by the runtime to skip expensive DB queries and prompt sections
/// when the user's message doesn't need them — reducing token overhead.
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
}

impl Default for ContextNeeds {
    fn default() -> Self {
        Self {
            recall: true,
            pending_tasks: true,
            profile: true,
            summaries: true,
            outcomes: true,
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
}

fn default_object_schema() -> serde_json::Value {
    serde_json::json!({"type": "object"})
}

/// Conversation context passed to an AI provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
        }
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
        });
        let json = serde_json::to_string(&ctx).unwrap();
        assert!(json.contains("toolboxes"));
        let deserialized: Context = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.toolboxes.len(), 1);
        assert_eq!(deserialized.toolboxes[0].name, "lint");
    }
}
