//! Topology data structures, TOML deserialization, and loader.
//!
//! Defines the config-driven topology format for multi-agent pipelines.
//! Topologies are loaded from `{data_dir}/topologies/{name}/TOPOLOGY.toml`
//! with agent definitions in `{name}/agents/*.md`.

use kernex_core::error::KernexError;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Schema structs — TOML deserialization targets
// ---------------------------------------------------------------------------

/// Root topology document.
#[derive(Debug, Clone, Deserialize)]
pub struct Topology {
    /// Metadata header (name, description, version).
    pub topology: TopologyMeta,
    /// Ordered list of pipeline phases.
    pub phases: Vec<Phase>,
}

/// Topology metadata header.
#[derive(Debug, Clone, Deserialize)]
pub struct TopologyMeta {
    pub name: String,
    pub description: String,
    pub version: u32,
}

/// A single phase in the pipeline.
#[derive(Debug, Clone, Deserialize)]
pub struct Phase {
    pub name: String,
    pub agent: String,
    #[serde(default = "default_model_tier")]
    pub model_tier: ModelTier,
    #[serde(default)]
    pub max_turns: Option<u32>,
    #[serde(default = "default_phase_type")]
    pub phase_type: PhaseType,
    #[serde(default)]
    pub retry: Option<RetryConfig>,
    #[serde(default)]
    pub pre_validation: Option<ValidationConfig>,
    #[serde(default)]
    pub post_validation: Option<Vec<String>>,
}

fn default_model_tier() -> ModelTier {
    ModelTier::Complex
}

fn default_phase_type() -> PhaseType {
    PhaseType::Standard
}

/// Which model tier to use for a phase.
#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ModelTier {
    Fast,
    #[default]
    Complex,
}

/// Phase execution behavior.
#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum PhaseType {
    #[default]
    Standard,
    ParseBrief,
    CorrectiveLoop,
    ParseSummary,
}

/// Retry configuration for corrective loop phases.
#[derive(Debug, Clone, Deserialize)]
pub struct RetryConfig {
    pub max: u32,
    pub fix_agent: String,
}

/// Pre-phase validation rules.
#[derive(Debug, Clone, Deserialize)]
pub struct ValidationConfig {
    #[serde(rename = "type")]
    pub validation_type: ValidationType,
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub patterns: Vec<String>,
}

/// Validation strategies for pre-phase checks.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ValidationType {
    FileExists,
    FilePatterns,
}

// ---------------------------------------------------------------------------
// LoadedTopology — runtime object with parsed TOML + agent contents
// ---------------------------------------------------------------------------

/// A fully resolved topology: parsed TOML + all agent .md contents loaded.
#[derive(Debug)]
pub struct LoadedTopology {
    pub topology: Topology,
    /// Map of agent name -> agent .md file content.
    pub agents: HashMap<String, String>,
}

impl LoadedTopology {
    /// Get agent content by name.
    pub fn agent_content(&self, name: &str) -> Result<&str, KernexError> {
        self.agents.get(name).map(|s| s.as_str()).ok_or_else(|| {
            KernexError::Pipeline(format!(
                "agent '{name}' referenced in topology but .md file not found"
            ))
        })
    }

    /// Resolve the model string for a phase based on its `ModelTier`.
    pub fn resolve_model<'a>(
        &self,
        phase: &Phase,
        model_fast: &'a str,
        model_complex: &'a str,
    ) -> &'a str {
        match phase.model_tier {
            ModelTier::Fast => model_fast,
            ModelTier::Complex => model_complex,
        }
    }

    /// Collect all (agent_name, agent_content) pairs.
    pub fn all_agents(&self) -> Vec<(&str, &str)> {
        self.agents
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Loader functions
// ---------------------------------------------------------------------------

/// Load a topology by name from `{data_dir}/topologies/{name}/`.
pub fn load_topology(data_dir: &str, name: &str) -> Result<LoadedTopology, KernexError> {
    validate_topology_name(name)?;

    let base = PathBuf::from(kernex_core::shellexpand(data_dir))
        .join("topologies")
        .join(name);

    if !base.exists() {
        return Err(KernexError::Pipeline(format!(
            "topology '{name}' not found at {}",
            base.display()
        )));
    }

    // Parse TOPOLOGY.toml.
    let toml_path = base.join("TOPOLOGY.toml");
    let toml_content = std::fs::read_to_string(&toml_path)
        .map_err(|e| KernexError::Pipeline(format!("failed to read TOPOLOGY.toml: {e}")))?;
    let topology: Topology = toml::from_str(&toml_content)
        .map_err(|e| KernexError::Pipeline(format!("failed to parse TOPOLOGY.toml: {e}")))?;

    // Collect unique agent names from phases (including fix_agent references).
    let mut required_agents: Vec<&str> = topology.phases.iter().map(|p| p.agent.as_str()).collect();
    for phase in &topology.phases {
        if let Some(retry) = &phase.retry {
            required_agents.push(&retry.fix_agent);
        }
    }
    required_agents.sort_unstable();
    required_agents.dedup();

    // Load all referenced agent .md files.
    let mut agents = HashMap::new();
    let agents_dir = base.join("agents");

    for agent_name in required_agents {
        let agent_path = agents_dir.join(format!("{agent_name}.md"));
        let content = std::fs::read_to_string(&agent_path).map_err(|e| {
            KernexError::Pipeline(format!(
                "agent '{agent_name}' referenced in topology but file not found: {e}"
            ))
        })?;
        agents.insert(agent_name.to_string(), content);
    }

    // Scan agents/ directory for any additional .md files not referenced by phases.
    if agents_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&agents_dir) {
            for entry in entries.flatten() {
                let file_name = entry.file_name().to_string_lossy().to_string();
                if file_name.ends_with(".md") {
                    let agent_name = file_name.trim_end_matches(".md").to_string();
                    agents.entry(agent_name).or_insert_with_key(|_| {
                        std::fs::read_to_string(entry.path()).unwrap_or_default()
                    });
                }
            }
        }
    }

    Ok(LoadedTopology { topology, agents })
}

/// Validate a topology name: alphanumeric + hyphens + underscores, max 64 chars.
/// Rejects path traversal, shell metacharacters, empty names.
pub fn validate_topology_name(name: &str) -> Result<(), KernexError> {
    if name.is_empty() {
        return Err(KernexError::Pipeline(
            "topology name cannot be empty".to_string(),
        ));
    }
    if name.len() > 64 {
        return Err(KernexError::Pipeline(format!(
            "topology name too long ({} chars, max 64)",
            name.len()
        )));
    }
    if name.contains("..") || name.contains('/') || name.contains('\\') {
        return Err(KernexError::Pipeline(format!(
            "topology name '{name}' contains path traversal characters"
        )));
    }
    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        return Err(KernexError::Pipeline(format!(
            "topology name '{name}' contains invalid characters (only alphanumeric, hyphens, underscores allowed)"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_topology_deserialize_minimal_valid_toml() {
        let toml_str = r#"
[topology]
name = "test"
description = "A test topology"
version = 1

[[phases]]
name = "analyst"
agent = "build-analyst"
"#;
        let topo: Topology = toml::from_str(toml_str).unwrap();
        assert_eq!(topo.topology.name, "test");
        assert_eq!(topo.phases.len(), 1);
        assert_eq!(topo.phases[0].name, "analyst");
    }

    #[test]
    fn test_topology_deserialize_defaults_applied() {
        let toml_str = r#"
[topology]
name = "test"
description = "Test defaults"
version = 1

[[phases]]
name = "basic"
agent = "build-basic"
"#;
        let topo: Topology = toml::from_str(toml_str).unwrap();
        let phase = &topo.phases[0];
        assert_eq!(phase.model_tier, ModelTier::Complex);
        assert_eq!(phase.phase_type, PhaseType::Standard);
        assert!(phase.max_turns.is_none());
        assert!(phase.retry.is_none());
        assert!(phase.pre_validation.is_none());
        assert!(phase.post_validation.is_none());
    }

    #[test]
    fn test_topology_deserialize_all_phase_types() {
        let toml_str = r#"
[topology]
name = "test"
description = "Phase types"
version = 1

[[phases]]
name = "a"
agent = "build-a"
phase_type = "standard"

[[phases]]
name = "b"
agent = "build-b"
phase_type = "parse-brief"

[[phases]]
name = "c"
agent = "build-c"
phase_type = "corrective-loop"

[[phases]]
name = "d"
agent = "build-d"
phase_type = "parse-summary"
"#;
        let topo: Topology = toml::from_str(toml_str).unwrap();
        assert_eq!(topo.phases[0].phase_type, PhaseType::Standard);
        assert_eq!(topo.phases[1].phase_type, PhaseType::ParseBrief);
        assert_eq!(topo.phases[2].phase_type, PhaseType::CorrectiveLoop);
        assert_eq!(topo.phases[3].phase_type, PhaseType::ParseSummary);
    }

    #[test]
    fn test_topology_deserialize_model_tiers() {
        let toml_str = r#"
[topology]
name = "test"
description = "Model tiers"
version = 1

[[phases]]
name = "fast-phase"
agent = "build-fast"
model_tier = "fast"

[[phases]]
name = "complex-phase"
agent = "build-complex"
model_tier = "complex"
"#;
        let topo: Topology = toml::from_str(toml_str).unwrap();
        assert_eq!(topo.phases[0].model_tier, ModelTier::Fast);
        assert_eq!(topo.phases[1].model_tier, ModelTier::Complex);
    }

    #[test]
    fn test_topology_deserialize_retry_config() {
        let toml_str = r#"
[topology]
name = "test"
description = "Retry"
version = 1

[[phases]]
name = "qa"
agent = "build-qa"
phase_type = "corrective-loop"

[phases.retry]
max = 3
fix_agent = "build-developer"
"#;
        let topo: Topology = toml::from_str(toml_str).unwrap();
        let retry = topo.phases[0].retry.as_ref().unwrap();
        assert_eq!(retry.max, 3);
        assert_eq!(retry.fix_agent, "build-developer");
    }

    #[test]
    fn test_topology_deserialize_validation_file_exists() {
        let toml_str = r#"
[topology]
name = "test"
description = "Validation"
version = 1

[[phases]]
name = "test-writer"
agent = "build-test-writer"

[phases.pre_validation]
type = "file_exists"
paths = ["specs/architecture.md"]
"#;
        let topo: Topology = toml::from_str(toml_str).unwrap();
        let validation = topo.phases[0].pre_validation.as_ref().unwrap();
        assert_eq!(validation.validation_type, ValidationType::FileExists);
        assert_eq!(validation.paths, vec!["specs/architecture.md"]);
    }

    #[test]
    fn test_topology_deserialize_validation_file_patterns() {
        let toml_str = r#"
[topology]
name = "test"
description = "Patterns"
version = 1

[[phases]]
name = "developer"
agent = "build-developer"

[phases.pre_validation]
type = "file_patterns"
patterns = ["test", "spec", "_test."]
"#;
        let topo: Topology = toml::from_str(toml_str).unwrap();
        let validation = topo.phases[0].pre_validation.as_ref().unwrap();
        assert_eq!(validation.validation_type, ValidationType::FilePatterns);
        assert_eq!(validation.patterns, vec!["test", "spec", "_test."]);
    }

    #[test]
    fn test_topology_deserialize_post_validation() {
        let toml_str = r#"
[topology]
name = "test"
description = "Post validation"
version = 1

[[phases]]
name = "architect"
agent = "build-architect"
post_validation = ["specs/architecture.md"]
"#;
        let topo: Topology = toml::from_str(toml_str).unwrap();
        let post = topo.phases[0].post_validation.as_ref().unwrap();
        assert_eq!(post, &vec!["specs/architecture.md".to_string()]);
    }

    #[test]
    fn test_topology_deserialize_max_turns() {
        let toml_str = r#"
[topology]
name = "test"
description = "Max turns"
version = 1

[[phases]]
name = "analyst"
agent = "build-analyst"
max_turns = 25
"#;
        let topo: Topology = toml::from_str(toml_str).unwrap();
        assert_eq!(topo.phases[0].max_turns, Some(25));
    }

    #[test]
    fn test_topology_deserialize_invalid_toml_returns_err() {
        let result: Result<Topology, _> = toml::from_str("this is not valid TOML {{{");
        assert!(result.is_err());
    }

    #[test]
    fn test_topology_deserialize_missing_required_field() {
        let toml_str = r#"
[[phases]]
name = "analyst"
agent = "build-analyst"
"#;
        let result: Result<Topology, _> = toml::from_str(toml_str);
        assert!(result.is_err());
    }

    #[test]
    fn test_topology_deserialize_wrong_type_returns_err() {
        let toml_str = r#"
[topology]
name = "test"
description = "Test"
version = "not-a-number"

[[phases]]
name = "analyst"
agent = "build-analyst"
"#;
        let result: Result<Topology, _> = toml::from_str(toml_str);
        assert!(result.is_err());
    }

    #[test]
    fn test_topology_deserialize_unknown_phase_type_returns_err() {
        let toml_str = r#"
[topology]
name = "test"
description = "Test"
version = 1

[[phases]]
name = "custom"
agent = "build-custom"
phase_type = "nonexistent-type"
"#;
        let result: Result<Topology, _> = toml::from_str(toml_str);
        assert!(result.is_err());
    }

    #[test]
    fn test_topology_deserialize_full_development_topology() {
        let toml_str = r#"
[topology]
name = "development"
description = "Default 7-phase TDD build pipeline"
version = 1

[[phases]]
name = "analyst"
agent = "build-analyst"
model_tier = "complex"
max_turns = 25
phase_type = "parse-brief"

[[phases]]
name = "architect"
agent = "build-architect"
model_tier = "complex"
post_validation = ["specs/architecture.md"]

[[phases]]
name = "test-writer"
agent = "build-test-writer"
model_tier = "complex"

[phases.pre_validation]
type = "file_exists"
paths = ["specs/architecture.md"]

[[phases]]
name = "developer"
agent = "build-developer"
model_tier = "complex"

[phases.pre_validation]
type = "file_patterns"
patterns = ["test", "spec", "_test."]

[[phases]]
name = "qa"
agent = "build-qa"
model_tier = "complex"
phase_type = "corrective-loop"

[phases.pre_validation]
type = "file_patterns"
patterns = [".rs", ".py", ".js", ".ts", ".go", ".java", ".rb", ".c", ".cpp"]

[phases.retry]
max = 3
fix_agent = "build-developer"

[[phases]]
name = "reviewer"
agent = "build-reviewer"
model_tier = "complex"
phase_type = "corrective-loop"

[phases.retry]
max = 2
fix_agent = "build-developer"

[[phases]]
name = "delivery"
agent = "build-delivery"
model_tier = "complex"
phase_type = "parse-summary"
"#;
        let topo: Topology = toml::from_str(toml_str).unwrap();
        assert_eq!(topo.topology.name, "development");
        assert_eq!(topo.phases.len(), 7);

        let names: Vec<&str> = topo.phases.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "analyst",
                "architect",
                "test-writer",
                "developer",
                "qa",
                "reviewer",
                "delivery"
            ]
        );

        assert_eq!(topo.phases[0].phase_type, PhaseType::ParseBrief);
        assert_eq!(topo.phases[0].max_turns, Some(25));

        let qa_retry = topo.phases[4].retry.as_ref().unwrap();
        assert_eq!(qa_retry.max, 3);
        assert_eq!(qa_retry.fix_agent, "build-developer");

        assert_eq!(topo.phases[6].phase_type, PhaseType::ParseSummary);
    }

    // --- Loader tests ---

    #[test]
    fn test_load_topology_reads_valid_topology() {
        let tmp = std::env::temp_dir().join("__kernex_test_topo_valid__");
        let _ = std::fs::remove_dir_all(&tmp);
        let topo_dir = tmp.join("topologies/test-topo");
        let agents_dir = topo_dir.join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();

        let toml_content = r#"
[topology]
name = "test-topo"
description = "Test topology"
version = 1

[[phases]]
name = "only-phase"
agent = "build-only"
"#;
        std::fs::write(topo_dir.join("TOPOLOGY.toml"), toml_content).unwrap();
        std::fs::write(agents_dir.join("build-only.md"), "Agent content").unwrap();

        let loaded = load_topology(tmp.to_str().unwrap(), "test-topo").unwrap();
        assert_eq!(loaded.topology.topology.name, "test-topo");
        assert_eq!(loaded.topology.phases.len(), 1);
        assert!(loaded.agents.contains_key("build-only"));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_load_topology_loads_all_referenced_agents() {
        let tmp = std::env::temp_dir().join("__kernex_test_topo_multi__");
        let _ = std::fs::remove_dir_all(&tmp);
        let topo_dir = tmp.join("topologies/multi");
        let agents_dir = topo_dir.join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();

        let toml_content = r#"
[topology]
name = "multi"
description = "Multi-agent"
version = 1

[[phases]]
name = "phase-a"
agent = "agent-a"

[[phases]]
name = "phase-b"
agent = "agent-b"
phase_type = "corrective-loop"

[phases.retry]
max = 2
fix_agent = "agent-a"
"#;
        std::fs::write(topo_dir.join("TOPOLOGY.toml"), toml_content).unwrap();
        std::fs::write(agents_dir.join("agent-a.md"), "Agent A content").unwrap();
        std::fs::write(agents_dir.join("agent-b.md"), "Agent B content").unwrap();

        let loaded = load_topology(tmp.to_str().unwrap(), "multi").unwrap();
        assert_eq!(loaded.agents.len(), 2);
        assert_eq!(loaded.agent_content("agent-a").unwrap(), "Agent A content");
        assert_eq!(loaded.agent_content("agent-b").unwrap(), "Agent B content");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_load_topology_loads_fix_agent() {
        let tmp = std::env::temp_dir().join("__kernex_test_topo_fix__");
        let _ = std::fs::remove_dir_all(&tmp);
        let topo_dir = tmp.join("topologies/fix-test");
        let agents_dir = topo_dir.join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();

        let toml_content = r#"
[topology]
name = "fix-test"
description = "Fix agent test"
version = 1

[[phases]]
name = "qa"
agent = "build-qa"
phase_type = "corrective-loop"

[phases.retry]
max = 3
fix_agent = "build-developer"
"#;
        std::fs::write(topo_dir.join("TOPOLOGY.toml"), toml_content).unwrap();
        std::fs::write(agents_dir.join("build-qa.md"), "QA content").unwrap();
        std::fs::write(agents_dir.join("build-developer.md"), "Dev content").unwrap();

        let loaded = load_topology(tmp.to_str().unwrap(), "fix-test").unwrap();
        assert!(loaded.agents.contains_key("build-developer"));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_load_topology_includes_non_phase_agents() {
        let tmp = std::env::temp_dir().join("__kernex_test_topo_disc__");
        let _ = std::fs::remove_dir_all(&tmp);
        let topo_dir = tmp.join("topologies/disc-test");
        let agents_dir = topo_dir.join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();

        let toml_content = r#"
[topology]
name = "disc-test"
description = "Discovery test"
version = 1

[[phases]]
name = "analyst"
agent = "build-analyst"
"#;
        std::fs::write(topo_dir.join("TOPOLOGY.toml"), toml_content).unwrap();
        std::fs::write(agents_dir.join("build-analyst.md"), "Analyst content").unwrap();
        std::fs::write(agents_dir.join("build-discovery.md"), "Discovery content").unwrap();

        let loaded = load_topology(tmp.to_str().unwrap(), "disc-test").unwrap();
        assert!(loaded.agents.contains_key("build-analyst"));
        assert!(loaded.agents.contains_key("build-discovery"));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_load_topology_corrupt_toml_returns_error() {
        let tmp = std::env::temp_dir().join("__kernex_test_topo_corrupt__");
        let _ = std::fs::remove_dir_all(&tmp);
        let topo_dir = tmp.join("topologies/corrupt");
        std::fs::create_dir_all(&topo_dir).unwrap();
        std::fs::write(topo_dir.join("TOPOLOGY.toml"), "not valid {{toml}}").unwrap();

        let result = load_topology(tmp.to_str().unwrap(), "corrupt");
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_load_topology_missing_returns_error() {
        let result = load_topology("/tmp/__kernex_test_no_topo__", "nonexistent");
        assert!(result.is_err());
    }

    // --- Name validation tests ---

    #[test]
    fn test_validate_topology_name_valid() {
        assert!(validate_topology_name("development").is_ok());
        assert!(validate_topology_name("my-pipeline").is_ok());
        assert!(validate_topology_name("test_123").is_ok());
    }

    #[test]
    fn test_validate_topology_name_rejects_empty() {
        assert!(validate_topology_name("").is_err());
    }

    #[test]
    fn test_validate_topology_name_rejects_traversal() {
        assert!(validate_topology_name("../etc").is_err());
        assert!(validate_topology_name("foo/bar").is_err());
        assert!(validate_topology_name("foo\\bar").is_err());
    }

    #[test]
    fn test_validate_topology_name_rejects_special_chars() {
        assert!(validate_topology_name("foo bar").is_err());
        assert!(validate_topology_name("foo;bar").is_err());
    }

    #[test]
    fn test_validate_topology_name_rejects_too_long() {
        let long_name = "a".repeat(65);
        assert!(validate_topology_name(&long_name).is_err());
    }
}
