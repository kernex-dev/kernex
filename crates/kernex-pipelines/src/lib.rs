//! Topology-driven multi-agent execution engine.
//!
//! Provides TOML-defined topology configuration for sequential agent chains
//! with file-mediated handoffs, bounded corrective loops, pre/post validation,
//! and model tier selection.

mod topology;

pub use topology::{
    load_topology, validate_topology_name, LoadedTopology, ModelTier, Phase, PhaseType,
    RetryConfig, Topology, TopologyMeta, ValidationConfig, ValidationType,
};
