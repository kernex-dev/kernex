//! TOML preset loader for kernex install bundles.
//!
//! Workspace-internal. Five preset TOMLs ship empty in this scaffold; their
//! bodies are filled in a follow-up change alongside the configurator pipeline.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use kernex_adapter_core::AdapterId;
use thiserror::Error;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct Preset {
    pub adapters: Vec<AdapterId>,
    pub components: Vec<String>,
}

impl Preset {
    pub fn new(adapters: Vec<AdapterId>, components: Vec<String>) -> Self {
        Self {
            adapters,
            components,
        }
    }
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum PresetError {
    #[error("preset {0:?} is not known to this build")]
    Unknown(String),

    #[error("preset {0:?} body is empty in this scaffold")]
    Empty(String),

    #[error("preset {0:?} parse error: {1}")]
    Parse(String, basic_toml::Error),
}

/// Load a preset by name. Bodies ship empty in this scaffold, so any known
/// name returns [`PresetError::Empty`] until a follow-up change fills them.
pub fn load_preset(name: &str) -> Result<Preset, PresetError> {
    let raw = match name {
        "full-kernex" => include_str!("../presets/full-kernex.toml"),
        "security-hardened" => include_str!("../presets/security-hardened.toml"),
        "airgapped-defense" => include_str!("../presets/airgapped-defense.toml"),
        "solo-dev" => include_str!("../presets/solo-dev.toml"),
        "ci-only" => include_str!("../presets/ci-only.toml"),
        other => return Err(PresetError::Unknown(other.to_string())),
    };

    let has_data = raw
        .lines()
        .map(|line| line.split_once('#').map_or(line, |(pre, _)| pre).trim())
        .any(|stripped| !stripped.is_empty());
    if !has_data {
        return Err(PresetError::Empty(name.to_string()));
    }

    basic_toml::from_str(raw).map_err(|e| PresetError::Parse(name.to_string(), e))
}
