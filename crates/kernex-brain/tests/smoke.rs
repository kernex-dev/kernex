#![allow(clippy::unwrap_used, clippy::expect_used)]

#[test]
fn health_score_roundtrips_serde() {
    let json = r#"{"project":"demo","score":42}"#;
    let parsed: kernex_brain::HealthScore = serde_json::from_str(json).expect("deserialize");
    assert_eq!(parsed.project, "demo");
    assert_eq!(parsed.score, 42);

    let reserialized = serde_json::to_string(&parsed).expect("serialize");
    let _round: kernex_brain::HealthScore =
        serde_json::from_str(&reserialized).expect("deserialize");
}

#[test]
fn observation_id_roundtrips() {
    let id = kernex_brain::ObservationId(42);
    let json = serde_json::to_string(&id).expect("serialize");
    let back: kernex_brain::ObservationId = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(id, back);
}
