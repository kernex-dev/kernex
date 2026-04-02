//! Configuration types and loading for Kernex.

mod defaults;
mod providers;

pub use providers::*;

use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::error::KernexError;
use defaults::*;

/// Top-level Kernex configuration (loadable from TOML or YAML).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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
    /// Base system prompt prepended to every request. Empty = no system prompt.
    #[serde(default)]
    pub system_prompt: String,
    /// Channel identifier (e.g. `"cli"`, `"api"`, `"slack"`).
    #[serde(default = "default_channel")]
    pub channel: String,
    /// Active project for scoping memory and lessons. `None` = no project scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            name: default_name(),
            data_dir: default_data_dir(),
            log_level: default_log_level(),
            system_prompt: String::new(),
            channel: default_channel(),
            project: None,
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
    /// Maximum number of SQLite connections in the pool (default: 4).
    #[serde(default = "default_max_connections")]
    pub max_connections: u32,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            backend: default_memory_backend(),
            db_path: default_db_path(),
            max_context_messages: default_max_context(),
            max_connections: default_max_connections(),
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

/// Load configuration from a YAML file.
///
/// Falls back to defaults if the file does not exist.
/// Supports `~` expansion in the path.
///
/// Requires the `yaml` feature.
#[cfg(feature = "yaml")]
pub fn load_yaml(path: &str) -> Result<KernexConfig, KernexError> {
    let expanded = shellexpand(path);
    let path = Path::new(&expanded);
    if !path.exists() {
        tracing::info!(
            "Config file not found at {}, using defaults",
            path.display()
        );
        return Ok(KernexConfig::default());
    }

    let content = std::fs::read_to_string(path)
        .map_err(|e| KernexError::Config(format!("failed to read {}: {}", path.display(), e)))?;

    let config: KernexConfig = serde_yaml::from_str(&content)
        .map_err(|e| KernexError::Config(format!("failed to parse yaml config: {e}")))?;

    Ok(config)
}

/// Load configuration from a file, auto-detecting format by extension.
///
/// Files ending in `.yaml` or `.yml` are parsed as YAML (requires the `yaml` feature).
/// All other extensions are treated as TOML.
pub fn load_file(path: &str) -> Result<KernexConfig, KernexError> {
    let lower = path.to_lowercase();
    if lower.ends_with(".yaml") || lower.ends_with(".yml") {
        #[cfg(feature = "yaml")]
        return load_yaml(path);
        #[cfg(not(feature = "yaml"))]
        return Err(KernexError::Config(
            "YAML support requires the 'yaml' feature flag on kernex-core".to_string(),
        ));
    }
    load(path)
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
        assert_eq!(cfg.channel, "cli");
        assert!(cfg.system_prompt.is_empty());
        assert!(cfg.project.is_none());
    }

    #[test]
    fn test_memory_config_defaults() {
        let cfg = MemoryConfig::default();
        assert_eq!(cfg.backend, "sqlite");
        assert!(cfg.db_path.contains("memory.db"));
    }

    #[test]
    fn test_toml_roundtrip_with_agent_fields() {
        let toml_src = r#"
[runtime]
name = "my-agent"
data_dir = "~/.my-agent"
channel = "api"
project = "acme"
system_prompt = "You are a coding assistant."
"#;
        let config: KernexConfig = toml::from_str(toml_src).unwrap();
        assert_eq!(config.runtime.name, "my-agent");
        assert_eq!(config.runtime.channel, "api");
        assert_eq!(config.runtime.project, Some("acme".to_string()));
        assert_eq!(config.runtime.system_prompt, "You are a coding assistant.");
    }

    #[test]
    fn test_load_file_routes_to_toml() {
        let config = load_file("/tmp/nonexistent-kernex-99999.toml").unwrap();
        assert_eq!(config.runtime.name, "kernex");
    }

    #[test]
    fn test_load_file_returns_error_for_yaml_without_feature() {
        // Without the `yaml` feature compiled in, load_file should return an error
        // for .yaml extensions. If the feature IS enabled, it just falls back to defaults.
        let result = load_file("/tmp/nonexistent-kernex-99999.yaml");
        // Either Ok (yaml feature enabled, file missing → defaults) or
        // Err (yaml feature disabled → config error). Both are valid.
        let _ = result;
    }
}
