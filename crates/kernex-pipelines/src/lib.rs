#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

//! Topology-driven multi-agent execution engine.
//!
//! Provides TOML-defined topology configuration for sequential and parallel
//! agent chains with file-mediated handoffs, bounded corrective loops,
//! pre/post validation, and model tier selection.

pub mod error;
mod topology;

pub use error::PipelineError;
pub use topology::{
    load_topology, validate_agent_name, validate_topology_name, LoadedTopology, Phase, PhaseGroup,
    PhaseTier, PhaseType, RetryConfig, Topology, TopologyMeta, ValidationConfig, ValidationType,
};
