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

#[test]
fn detection_new_roundtrips() {
    use kernex_adapter_core::Detection;
    use std::path::{Path, PathBuf};

    let d = Detection::new(true, Some(PathBuf::from("/x")), Some("1.2.3".into()));
    assert!(d.installed);
    assert_eq!(d.config_root.as_deref(), Some(Path::new("/x")));
    assert_eq!(d.version.as_deref(), Some("1.2.3"));

    let json = serde_json::to_value(&d).expect("serialize");
    let back: Detection = serde_json::from_value(json).expect("roundtrip");
    assert_eq!(back.installed, d.installed);
    assert_eq!(back.config_root, d.config_root);
    assert_eq!(back.version, d.version);
}
