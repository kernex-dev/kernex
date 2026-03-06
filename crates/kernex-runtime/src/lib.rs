//! kernex-runtime: The facade crate that composes all Kernex components.
//!
//! Provides `Runtime` for configuring and running an AI agent runtime
//! with sandboxed execution, multi-provider support, persistent memory,
//! skills, and multi-agent pipeline orchestration.
//!
//! # Quick Start
//!
//! ```rust,ignore
//! use kernex_runtime::RuntimeBuilder;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let runtime = RuntimeBuilder::new()
//!         .data_dir("~/.kernex")
//!         .build()
//!         .await?;
//!     Ok(())
//! }
//! ```

use kernex_core::config::MemoryConfig;
use kernex_core::error::KernexError;
use kernex_memory::Store;
use kernex_skills::{Project, Skill};

/// Re-export sub-crates for convenience.
pub use kernex_core as core;
pub use kernex_memory as memory;
pub use kernex_pipelines as pipelines;
pub use kernex_providers as providers;
pub use kernex_sandbox as sandbox;
pub use kernex_skills as skills;

/// A configured Kernex runtime with all subsystems initialized.
pub struct Runtime {
    /// Persistent memory store.
    pub store: Store,
    /// Loaded skills from the data directory.
    pub skills: Vec<Skill>,
    /// Loaded projects from the data directory.
    pub projects: Vec<Project>,
    /// Data directory path (expanded).
    pub data_dir: String,
}

/// Builder for constructing a `Runtime` with the desired configuration.
pub struct RuntimeBuilder {
    data_dir: String,
    db_path: Option<String>,
}

impl RuntimeBuilder {
    /// Create a new builder with default settings.
    pub fn new() -> Self {
        Self {
            data_dir: "~/.kernex".to_string(),
            db_path: None,
        }
    }

    /// Set the data directory (default: `~/.kernex`).
    pub fn data_dir(mut self, path: &str) -> Self {
        self.data_dir = path.to_string();
        self
    }

    /// Set a custom database path (default: `{data_dir}/memory.db`).
    pub fn db_path(mut self, path: &str) -> Self {
        self.db_path = Some(path.to_string());
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
        let db_path = self
            .db_path
            .unwrap_or_else(|| format!("{expanded_dir}/memory.db"));
        let mem_config = MemoryConfig {
            db_path: db_path.clone(),
            ..Default::default()
        };
        let store = Store::new(&mem_config).await?;

        // Load skills and projects.
        let skills = kernex_skills::load_skills(&self.data_dir);
        let projects = kernex_skills::load_projects(&self.data_dir);

        tracing::info!(
            "runtime initialized: {} skills, {} projects",
            skills.len(),
            projects.len()
        );

        Ok(Runtime {
            store,
            skills,
            projects,
            data_dir: expanded_dir,
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
}
