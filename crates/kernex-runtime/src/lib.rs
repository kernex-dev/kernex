//! kernex-runtime: The facade crate that composes all Kernex components.
#![deny(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
//!
//! Provides `Runtime` for configuring and running an AI agent runtime
//! with sandboxed execution, multi-provider support, persistent memory,
//! skills, and multi-agent pipeline orchestration.
//!
//! # Quick Start
//!
//! ```rust,ignore
//! use kernex_runtime::RuntimeBuilder;
//! use kernex_core::traits::Provider;
//! use kernex_core::message::Request;
//! use kernex_providers::ollama::OllamaProvider;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let runtime = RuntimeBuilder::new()
//!         .data_dir("~/.my-agent")
//!         .build()
//!         .await?;
//!
//!     let provider = OllamaProvider::from_config(
//!         "http://localhost:11434".into(),
//!         "llama3.2".into(),
//!         None,
//!     )?;
//!
//!     let request = Request::text("user-1", "Hello!");
//!     let response = runtime.complete(&provider, &request).await?;
//!     println!("{}", response.text);
//!
//!     Ok(())
//! }
//! ```

#[cfg(feature = "sqlite-store")]
use kernex_core::config::MemoryConfig;
use kernex_core::context::ContextNeeds;
use kernex_core::error::KernexError;
use kernex_core::hooks::{HookRunner, NoopHookRunner};
use kernex_core::message::{CompletionMeta, Request, Response};
use kernex_core::stream::StreamEvent;
use kernex_core::traits::StreamingProvider;
use kernex_core::permissions::PermissionRules;
use kernex_core::run::{RunConfig, RunOutcome};
use kernex_core::traits::Provider;
#[cfg(feature = "sqlite-store")]
use kernex_memory::Store;
use kernex_skills::{
    build_skill_prompt, match_skill_toolboxes, match_skill_triggers, Project, Skill,
};
use std::sync::Arc;

/// Re-export sub-crates for convenience.
pub use kernex_core as core;
#[cfg(feature = "sqlite-store")]
pub use kernex_memory as memory;
pub use kernex_pipelines as pipelines;
pub use kernex_providers as providers;
pub use kernex_sandbox as sandbox;
pub use kernex_skills as skills;

/// A configured Kernex runtime with all subsystems initialized.
pub struct Runtime {
    /// Persistent memory store.
    #[cfg(feature = "sqlite-store")]
    pub store: Store,
    /// Loaded skills from the data directory.
    pub skills: Vec<Skill>,
    /// Loaded projects from the data directory.
    pub projects: Vec<Project>,
    /// Data directory path (expanded).
    pub data_dir: String,
    /// Base system prompt prepended to every request.
    pub system_prompt: String,
    /// Communication channel identifier (e.g. "cli", "api", "slack").
    pub channel: String,
    /// Active project key for scoping memory and lessons.
    pub project: Option<String>,
    /// Hook runner for tool lifecycle events.
    pub hook_runner: Arc<dyn HookRunner>,
    /// Declarative allow/deny rules applied before each tool call.
    pub permission_rules: Option<Arc<PermissionRules>>,
}

impl Runtime {
    /// Send a request through the full runtime pipeline:
    /// build context from memory → enrich with skills → complete via provider → save exchange.
    ///
    /// This is the high-level convenience method that wires together all
    /// Kernex subsystems in a single call.
    pub async fn complete(
        &self,
        provider: &dyn Provider,
        request: &Request,
    ) -> Result<Response, KernexError> {
        self.complete_with_needs(provider, request, &ContextNeeds::default())
            .await
    }

    /// Like [`complete`](Self::complete), but with explicit control over which
    /// context blocks are loaded from memory.
    pub async fn complete_with_needs(
        &self,
        provider: &dyn Provider,
        request: &Request,
        #[allow(unused_variables)] needs: &ContextNeeds,
    ) -> Result<Response, KernexError> {
        let project_ref = self.project.as_deref();

        // Build skill context (prompt block + optional model override).
        let skill_ctx = build_skill_prompt(&self.skills);
        let full_system_prompt = if skill_ctx.prompt.is_empty() {
            self.system_prompt.clone()
        } else if self.system_prompt.is_empty() {
            skill_ctx.prompt.clone()
        } else {
            format!("{}\n\n{}", self.system_prompt, skill_ctx.prompt)
        };

        // Build context from memory (history, recall, facts, lessons, etc).
        #[cfg(feature = "sqlite-store")]
        let mut context = self
            .store
            .build_context(
                &self.channel,
                request,
                &full_system_prompt,
                needs,
                project_ref,
                None,
            )
            .await?;

        #[cfg(not(feature = "sqlite-store"))]
        let mut context = {
            let mut ctx = kernex_core::context::Context::new(&request.text);
            ctx.system_prompt = full_system_prompt;
            ctx
        };

        // Apply skill model override when no model was already set on context.
        if context.model.is_none() {
            context.model = skill_ctx.model;
        }

        // Enrich context with triggered MCP servers.
        let mcp_servers = match_skill_triggers(&self.skills, &request.text);
        if !mcp_servers.is_empty() {
            context.mcp_servers = mcp_servers;
        }

        // Enrich context with triggered toolboxes.
        let toolboxes = match_skill_toolboxes(&self.skills, &request.text);
        if !toolboxes.is_empty() {
            context.toolboxes = toolboxes;
        }

        // Wire hooks and permission rules into context.
        context.hook_runner = Some(self.hook_runner.clone());
        context.permission_rules = self.permission_rules.clone();

        // Send to provider.
        let response = provider.complete(&context).await?;

        // Persist exchange in memory.
        #[allow(unused_variables)]
        let project_key = project_ref.unwrap_or("default");

        #[cfg(feature = "sqlite-store")]
        self.store
            .store_exchange(&self.channel, request, &response, project_key)
            .await?;

        // Record token usage if the provider reported a count.
        #[cfg(feature = "sqlite-store")]
        if let Some(tokens) = response.metadata.tokens_used {
            let model = response.metadata.model.as_deref().unwrap_or("unknown");
            let session = response.metadata.session_id.as_deref().unwrap_or("default");
            if let Err(e) = self
                .store
                .record_usage(&request.sender_id, session, tokens, model)
                .await
            {
                tracing::warn!("failed to record token usage: {e}");
            }
        }

        Ok(response)
    }

    /// Stream a request through the runtime pipeline, returning events as they arrive.
    ///
    /// Builds context from memory, enriches with skills, opens a streaming connection
    /// to the provider, and persists the exchange to memory after the stream completes.
    /// Returns a channel receiver that yields [`StreamEvent`]s until `Done` or `Error`.
    pub async fn complete_stream(
        &self,
        provider: &dyn StreamingProvider,
        request: &Request,
    ) -> Result<tokio::sync::mpsc::Receiver<StreamEvent>, KernexError> {
        self.complete_stream_with_needs(provider, request, &ContextNeeds::default())
            .await
    }

    /// Like [`complete_stream`](Self::complete_stream), but with explicit control over which
    /// context blocks are loaded from memory.
    pub async fn complete_stream_with_needs(
        &self,
        provider: &dyn StreamingProvider,
        request: &Request,
        #[allow(unused_variables)] needs: &ContextNeeds,
    ) -> Result<tokio::sync::mpsc::Receiver<StreamEvent>, KernexError> {
        let project_ref = self.project.as_deref();

        let skill_ctx = build_skill_prompt(&self.skills);
        let full_system_prompt = if skill_ctx.prompt.is_empty() {
            self.system_prompt.clone()
        } else if self.system_prompt.is_empty() {
            skill_ctx.prompt.clone()
        } else {
            format!("{}\n\n{}", self.system_prompt, skill_ctx.prompt)
        };

        #[cfg(feature = "sqlite-store")]
        let mut context = self
            .store
            .build_context(
                &self.channel,
                request,
                &full_system_prompt,
                needs,
                project_ref,
                None,
            )
            .await?;

        #[cfg(not(feature = "sqlite-store"))]
        let mut context = {
            let mut ctx = kernex_core::context::Context::new(&request.text);
            ctx.system_prompt = full_system_prompt;
            ctx
        };

        if context.model.is_none() {
            context.model = skill_ctx.model;
        }

        let mcp_servers = match_skill_triggers(&self.skills, &request.text);
        if !mcp_servers.is_empty() {
            context.mcp_servers = mcp_servers;
        }
        let toolboxes = match_skill_toolboxes(&self.skills, &request.text);
        if !toolboxes.is_empty() {
            context.toolboxes = toolboxes;
        }

        context.hook_runner = Some(self.hook_runner.clone());
        context.permission_rules = self.permission_rules.clone();

        // Open streaming connection to provider.
        let provider_name = provider.name().to_string();
        let mut upstream = provider.complete_stream(&context).await?;

        // Forwarding channel returned to the caller.
        let (tx, rx) = tokio::sync::mpsc::channel::<StreamEvent>(64);

        // Background task: forward events and persist exchange when done.
        #[cfg(feature = "sqlite-store")]
        let store = self.store.clone();
        let channel = self.channel.clone();
        let request_clone = request.clone();
        #[allow(unused_variables)]
        let project_key = project_ref.unwrap_or("default").to_string();

        tokio::spawn(async move {
            use kernex_core::stream::{StreamAccumulator, StreamEvent as SE};
            let mut acc = StreamAccumulator::new();
            let started = std::time::Instant::now();

            while let Some(event) = upstream.recv().await {
                acc.push(&event);
                let is_terminal = matches!(event, SE::Done | SE::Error(_));
                // Best-effort forward; drop silently if receiver was dropped.
                let _ = tx.send(event).await;
                if is_terminal {
                    break;
                }
            }

            // Persist accumulated exchange to memory.
            #[cfg(feature = "sqlite-store")]
            {
                let elapsed_ms = started.elapsed().as_millis() as u64;
                let response = Response {
                    text: acc.into_text(),
                    metadata: CompletionMeta {
                        provider_used: provider_name,
                        tokens_used: None,
                        processing_time_ms: elapsed_ms,
                        model: None,
                        session_id: None,
                    },
                };
                if let Err(e) = store
                    .store_exchange(&channel, &request_clone, &response, &project_key)
                    .await
                {
                    tracing::warn!("failed to persist streaming exchange: {e}");
                }
            }
            #[cfg(not(feature = "sqlite-store"))]
            {
                let _ = acc;
                let _ = started;
                let _ = provider_name;
            }
        });

        Ok(rx)
    }

    /// Run the agent with explicit lifecycle control.
    ///
    /// Sets `max_turns` in context so the provider's agentic loop respects it,
    /// wires the runtime hook runner, calls the provider, fires the `on_stop`
    /// hook, and wraps the outcome in [`RunOutcome`].
    pub async fn run(
        &self,
        provider: &dyn Provider,
        request: &Request,
        config: &RunConfig,
    ) -> Result<RunOutcome, KernexError> {
        let needs = ContextNeeds::default();
        let project_ref = self.project.as_deref();

        let skill_ctx = build_skill_prompt(&self.skills);
        let full_system_prompt = if skill_ctx.prompt.is_empty() {
            self.system_prompt.clone()
        } else if self.system_prompt.is_empty() {
            skill_ctx.prompt.clone()
        } else {
            format!("{}\n\n{}", self.system_prompt, skill_ctx.prompt)
        };

        #[cfg(feature = "sqlite-store")]
        let mut context = self
            .store
            .build_context(
                &self.channel,
                request,
                &full_system_prompt,
                &needs,
                project_ref,
                None,
            )
            .await?;

        #[cfg(not(feature = "sqlite-store"))]
        let mut context = {
            let mut ctx = kernex_core::context::Context::new(&request.text);
            ctx.system_prompt = full_system_prompt;
            ctx
        };

        // Apply skill model override when no model was already set on context.
        if context.model.is_none() {
            context.model = skill_ctx.model;
        }

        let mcp_servers = match_skill_triggers(&self.skills, &request.text);
        if !mcp_servers.is_empty() {
            context.mcp_servers = mcp_servers;
        }
        let toolboxes = match_skill_toolboxes(&self.skills, &request.text);
        if !toolboxes.is_empty() {
            context.toolboxes = toolboxes;
        }

        // Set max_turns, hooks, and permission rules.
        context.max_turns = Some(config.max_turns);
        context.hook_runner = Some(self.hook_runner.clone());
        context.permission_rules = self.permission_rules.clone();

        let response = provider.complete(&context).await?;

        // Fire on_stop hook.
        self.hook_runner.on_stop(&response.text).await;

        // Persist exchange.
        #[allow(unused_variables)]
        let project_key = project_ref.unwrap_or("default");
        #[cfg(feature = "sqlite-store")]
        self.store
            .store_exchange(&self.channel, request, &response, project_key)
            .await?;

        // Record token usage if the provider reported a count.
        #[cfg(feature = "sqlite-store")]
        if let Some(tokens) = response.metadata.tokens_used {
            let model = response.metadata.model.as_deref().unwrap_or("unknown");
            let session = response.metadata.session_id.as_deref().unwrap_or("default");
            if let Err(e) = self
                .store
                .record_usage(&request.sender_id, session, tokens, model)
                .await
            {
                tracing::warn!("failed to record token usage: {e}");
            }
        }

        Ok(RunOutcome::EndTurn(response))
    }
}

/// Builder for constructing a `Runtime` with the desired configuration.
pub struct RuntimeBuilder {
    data_dir: String,
    #[cfg(feature = "sqlite-store")]
    db_path: Option<String>,
    system_prompt: String,
    channel: String,
    project: Option<String>,
    hook_runner: Option<Arc<dyn HookRunner>>,
    permission_rules: Option<Arc<PermissionRules>>,
}

impl RuntimeBuilder {
    /// Create a new builder with default settings.
    pub fn new() -> Self {
        Self {
            data_dir: "~/.kernex".to_string(),
            #[cfg(feature = "sqlite-store")]
            db_path: None,
            system_prompt: String::new(),
            channel: "cli".to_string(),
            project: None,
            hook_runner: None,
            permission_rules: None,
        }
    }

    /// Create a new builder configured from environment variables.
    ///
    /// Recognizes:
    /// - `KERNEX_DATA_DIR`
    /// - `KERNEX_DB_PATH` (when `sqlite-store` feature is enabled)
    /// - `KERNEX_SYSTEM_PROMPT`
    /// - `KERNEX_CHANNEL`
    /// - `KERNEX_PROJECT`
    pub fn from_env() -> Self {
        let mut builder = Self::new();

        if let Ok(dir) = std::env::var("KERNEX_DATA_DIR") {
            builder = builder.data_dir(&dir);
        }
        #[cfg(feature = "sqlite-store")]
        if let Ok(path) = std::env::var("KERNEX_DB_PATH") {
            builder = builder.db_path(&path);
        }
        if let Ok(prompt) = std::env::var("KERNEX_SYSTEM_PROMPT") {
            builder = builder.system_prompt(&prompt);
        }
        if let Ok(channel) = std::env::var("KERNEX_CHANNEL") {
            builder = builder.channel(&channel);
        }
        if let Ok(project) = std::env::var("KERNEX_PROJECT") {
            builder = builder.project(&project);
        }

        builder
    }

    /// Set the data directory (default: `~/.kernex`).
    pub fn data_dir(mut self, path: &str) -> Self {
        self.data_dir = path.to_string();
        self
    }

    /// Set a custom database path (default: `{data_dir}/memory.db`).
    #[cfg(feature = "sqlite-store")]
    pub fn db_path(mut self, path: &str) -> Self {
        self.db_path = Some(path.to_string());
        self
    }

    /// Set the base system prompt.
    pub fn system_prompt(mut self, prompt: &str) -> Self {
        self.system_prompt = prompt.to_string();
        self
    }

    /// Set the channel identifier (default: `"cli"`).
    pub fn channel(mut self, channel: &str) -> Self {
        self.channel = channel.to_string();
        self
    }

    /// Set the active project for scoping memory.
    pub fn project(mut self, project: &str) -> Self {
        self.project = Some(project.to_string());
        self
    }

    /// Set a hook runner for tool lifecycle events.
    pub fn hook_runner(mut self, runner: Arc<dyn HookRunner>) -> Self {
        self.hook_runner = Some(runner);
        self
    }

    /// Set declarative allow/deny permission rules for tool calls.
    pub fn permission_rules(mut self, rules: PermissionRules) -> Self {
        self.permission_rules = Some(Arc::new(rules));
        self
    }

    /// Build and initialize the runtime.
    pub async fn build(self) -> Result<Runtime, KernexError> {
        let expanded_dir = kernex_core::shellexpand(&self.data_dir);

        // Ensure data directory exists.
        tokio::fs::create_dir_all(&expanded_dir)
            .await
            .map_err(|e| KernexError::Config(format!("failed to create data dir: {e}")))?;

        // Initialize store.
        #[cfg(feature = "sqlite-store")]
        let store = {
            let db_path = self
                .db_path
                .unwrap_or_else(|| format!("{expanded_dir}/memory.db"));
            let mem_config = MemoryConfig {
                db_path: db_path.clone(),
                ..Default::default()
            };
            Store::new(&mem_config).await?
        };

        // Load skills and projects.
        let skills = kernex_skills::load_skills(&self.data_dir);
        let projects = kernex_skills::load_projects(&self.data_dir);

        tracing::info!(
            "runtime initialized: {} skills, {} projects",
            skills.len(),
            projects.len()
        );

        let hook_runner: Arc<dyn HookRunner> =
            self.hook_runner.unwrap_or_else(|| Arc::new(NoopHookRunner));

        Ok(Runtime {
            #[cfg(feature = "sqlite-store")]
            store,
            skills,
            projects,
            data_dir: expanded_dir,
            system_prompt: self.system_prompt,
            channel: self.channel,
            project: self.project,
            hook_runner,
            permission_rules: self.permission_rules,
        })
    }
}

impl Default for RuntimeBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_runtime_builder_creates_runtime() {
        let tmp = std::env::temp_dir().join("__kernex_test_runtime__");
        let _ = std::fs::remove_dir_all(&tmp);

        let runtime = RuntimeBuilder::new()
            .data_dir(tmp.to_str().unwrap())
            .build()
            .await
            .unwrap();

        assert!(runtime.skills.is_empty());
        assert!(runtime.projects.is_empty());
        assert!(runtime.system_prompt.is_empty());
        assert_eq!(runtime.channel, "cli");
        assert!(runtime.project.is_none());
        assert!(std::path::Path::new(&runtime.data_dir).exists());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn test_runtime_builder_custom_db_path() {
        let tmp = std::env::temp_dir().join("__kernex_test_runtime_db__");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let db = tmp.join("custom.db");
        let runtime = RuntimeBuilder::new()
            .data_dir(tmp.to_str().unwrap())
            .db_path(db.to_str().unwrap())
            .build()
            .await
            .unwrap();

        assert!(db.exists());
        drop(runtime);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn test_runtime_builder_with_config() {
        let tmp = std::env::temp_dir().join("__kernex_test_runtime_cfg__");
        let _ = std::fs::remove_dir_all(&tmp);

        let runtime = RuntimeBuilder::new()
            .data_dir(tmp.to_str().unwrap())
            .system_prompt("You are helpful.")
            .channel("api")
            .project("my-project")
            .build()
            .await
            .unwrap();

        assert_eq!(runtime.system_prompt, "You are helpful.");
        assert_eq!(runtime.channel, "api");
        assert_eq!(runtime.project, Some("my-project".to_string()));

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
