//! Topology-driven multi-agent execution engine.
#![deny(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
//!
//! Provides TOML-defined topology configuration for sequential agent chains
//! with file-mediated handoffs, bounded corrective loops, pre/post validation,
//! and model tier selection.

mod topology;

pub use topology::{
    load_topology, validate_topology_name, LoadedTopology, ModelTier, Phase, PhaseType,
    RetryConfig, Topology, TopologyMeta, ValidationConfig, ValidationType,
};
