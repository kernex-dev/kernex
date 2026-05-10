#![allow(clippy::unwrap_used, clippy::expect_used)]

#[test]
fn preset_roundtrips_serde() {
    let preset = kernex_presets::Preset {
        adapters: vec![kernex_adapter_core::AdapterId::ClaudeCode],
        components: vec!["skills".to_string()],
    };
    let s = toml::to_string(&preset).expect("serialize");
    let _back: kernex_presets::Preset = toml::from_str(&s).expect("deserialize");
}

#[test]
fn empty_preset_returns_empty_error() {
    let err = kernex_presets::load_preset("solo-dev").expect_err("scaffold bodies are empty");
    assert!(matches!(err, kernex_presets::PresetError::Empty(_)));
}

#[test]
fn unknown_preset_returns_unknown_error() {
    let err = kernex_presets::load_preset("does-not-exist").expect_err("unknown");
    assert!(matches!(err, kernex_presets::PresetError::Unknown(_)));
}
