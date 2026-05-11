#![allow(clippy::unwrap_used, clippy::expect_used)]

#[test]
fn preset_roundtrips_serde() {
    // Verifies Preset's serde derive round-trips cleanly through a
    // serialization format. JSON is sufficient here; the prod path
    // (load_preset) exercises basic_toml::from_str against real TOML
    // fixtures embedded via include_str!. basic_toml is parse-only, so
    // the roundtrip uses serde_json to avoid pulling toml_edit back in
    // through a TOML serializer dev-dep.
    let preset = kernex_presets::Preset::new(
        vec![kernex_adapter_core::AdapterId::ClaudeCode],
        vec!["skills".to_string()],
    );
    let s = serde_json::to_string(&preset).expect("serialize");
    let back: kernex_presets::Preset = serde_json::from_str(&s).expect("deserialize");
    assert_eq!(back.adapters, preset.adapters);
    assert_eq!(back.components, preset.components);
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
