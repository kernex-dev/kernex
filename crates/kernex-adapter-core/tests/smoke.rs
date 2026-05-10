#![allow(clippy::unwrap_used, clippy::expect_used)]

#[test]
fn adapter_id_roundtrips_serde() {
    let id = kernex_adapter_core::AdapterId::ClaudeCode;
    let json = serde_json::to_string(&id).expect("serialize");
    let back: kernex_adapter_core::AdapterId = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(id, back);
}

#[test]
fn unsupported_factory_arm() {
    let res = kernex_adapter_core::new_adapter(kernex_adapter_core::AdapterId::ClaudeCode);
    assert!(res.is_err());
}

#[test]
fn default_registry_is_empty_in_scaffold() {
    let registry = kernex_adapter_core::default_registry().expect("default registry");
    assert!(registry
        .get(kernex_adapter_core::AdapterId::ClaudeCode)
        .is_none());
}
