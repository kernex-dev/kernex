//! Configuration types and loading for Kernex.

mod defaults;
mod providers;

pub use providers::*;

use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::error::KernexError;
use defaults::*;

/// Top-level Kernex configuration (loadable from TOML).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KernexConfig {
    #[serde(default)]
    pub runtime: RuntimeConfig,
    #[serde(default)]
    pub provider: ProviderConfig,
    #[serde(default)]
    pub memory: MemoryConfig,
    #[serde(default)]
    pub heartbeat: HeartbeatConfig,
    #[serde(default)]
    pub scheduler: SchedulerConfig,
}

/// General runtime settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeConfig {
    #[serde(default = "default_name")]
    pub name: String,
    #[serde(default = "default_data_dir")]
    pub data_dir: String,
    #[serde(default = "default_log_level")]
    pub log_level: String,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            name: default_name(),
            data_dir: default_data_dir(),
            log_level: default_log_level(),
        }
    }
}

/// Memory/storage config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    #[serde(default = "default_memory_backend")]
    pub backend: String,
    #[serde(default = "default_db_path")]
    pub db_path: String,
    #[serde(default = "default_max_context")]
    pub max_context_messages: usize,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            backend: default_memory_backend(),
            db_path: default_db_path(),
            max_context_messages: default_max_context(),
        }
    }
}

/// Heartbeat configuration — periodic AI check-ins.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_heartbeat_interval")]
    pub interval_minutes: u64,
    #[serde(default)]
    pub active_start: String,
    #[serde(default)]
    pub active_end: String,
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_minutes: default_heartbeat_interval(),
            active_start: String::new(),
            active_end: String::new(),
        }
    }
}

/// Scheduler configuration — user-scheduled reminders and tasks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_interval_secs: default_poll_interval(),
        }
    }
}

/// Fact keys reserved by the system (hidden from user profile display).
pub const SYSTEM_FACT_KEYS: &[&str] = &[
    "welcomed",
    "preferred_language",
    "active_project",
    "personality",
    "onboarding_stage",
];

/// Expand `~` to home directory.
pub fn shellexpand(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return format!("{}/{rest}", home.to_string_lossy());
        }
    }
    path.to_string()
}

/// Load configuration from a TOML file.
///
/// Falls back to defaults if the file does not exist.
/// Supports `~` expansion in the path.
pub fn load(path: &str) -> Result<KernexConfig, KernexError> {
    let expanded = shellexpand(path);
    let path = Path::new(&expanded);
    if !path.exists() {
        tracing::info!(
            "Config file not found at {}, using defaults",
            path.display()
        );
        return Ok(KernexConfig {
            runtime: RuntimeConfig::default(),
            provider: ProviderConfig {
                default: default_provider(),
                claude_code: Some(ClaudeCodeConfig::default()),
                ..Default::default()
            },
            memory: MemoryConfig::default(),
            heartbeat: HeartbeatConfig::default(),
            scheduler: SchedulerConfig::default(),
        });
    }

    let content = std::fs::read_to_string(path)
        .map_err(|e| KernexError::Config(format!("failed to read {}: {}", path.display(), e)))?;

    let config: KernexConfig = toml::from_str(&content)
        .map_err(|e| KernexError::Config(format!("failed to parse config: {e}")))?;

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shellexpand_tilde() {
        let expanded = shellexpand("~/test");
        assert!(!expanded.starts_with('~'));
        assert!(expanded.ends_with("/test"));
    }

    #[test]
    fn test_shellexpand_no_tilde() {
        assert_eq!(shellexpand("/absolute/path"), "/absolute/path");
        assert_eq!(shellexpand("relative/path"), "relative/path");
    }

    #[test]
    fn test_load_nonexistent_returns_defaults() {
        let config = load("/tmp/nonexistent-kernex-config-12345.toml").unwrap();
        assert_eq!(config.runtime.name, "kernex");
        assert_eq!(config.runtime.data_dir, "~/.kernex");
        assert!(config.provider.claude_code.is_some());
    }

    #[test]
    fn test_runtime_config_defaults() {
        let cfg = RuntimeConfig::default();
        assert_eq!(cfg.name, "kernex");
        assert_eq!(cfg.data_dir, "~/.kernex");
        assert_eq!(cfg.log_level, "info");
    }

    #[test]
    fn test_memory_config_defaults() {
        let cfg = MemoryConfig::default();
        assert_eq!(cfg.backend, "sqlite");
        assert!(cfg.db_path.contains("memory.db"));
    }
}
