#![allow(clippy::unwrap_used, clippy::expect_used)]

#[test]
fn health_score_roundtrips_serde() {
    let h = kernex_brain::HealthScore {
        project: "demo".to_string(),
        score: 42,
    };
    let json = serde_json::to_string(&h).expect("serialize");
    let _back: kernex_brain::HealthScore = serde_json::from_str(&json).expect("deserialize");
}
