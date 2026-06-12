use super::context::format_user_profile;
use super::tasks::{descriptions_are_similar, normalize_due_at};
use super::Store;
use kernex_core::config::MemoryConfig;
use kernex_core::context::{CompactionStrategy, ContextNeeds};
use kernex_core::message::Request;
use kernex_core::traits::Summarizer;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::str::FromStr;

/// Create an in-memory store for testing.
async fn test_store() -> Store {
    let _config = MemoryConfig {
        backend: "sqlite".to_string(),
        db_path: ":memory:".to_string(),
        max_context_messages: 10,
        ..Default::default()
    };
    let opts = SqliteConnectOptions::from_str("sqlite::memory:")
        .unwrap()
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
        .unwrap();
    Store::run_migrations(&pool).await.unwrap();
    Store {
        pool,
        max_context_messages: 10,
    }
}

#[tokio::test]
async fn test_create_and_get_tasks() {
    let store = test_store().await;
    let id = store
        .create_task(
            "api",
            "user1",
            "chat1",
            "Call John",
            "2026-12-31T15:00:00",
            None,
            "reminder",
            "",
        )
        .await
        .unwrap();
    assert!(!id.is_empty());

    let tasks = store.get_tasks_for_sender("user1").await.unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].1, "Call John");
    assert_eq!(tasks[0].2, "2026-12-31 15:00:00");
    assert!(tasks[0].3.is_none());
    assert_eq!(tasks[0].4, "reminder");
}

#[tokio::test]
async fn test_get_due_tasks() {
    let store = test_store().await;
    store
        .create_task(
            "api",
            "user1",
            "chat1",
            "Past task",
            "2020-01-01T00:00:00",
            None,
            "reminder",
            "",
        )
        .await
        .unwrap();
    store
        .create_task(
            "api",
            "user1",
            "chat1",
            "Future task",
            "2099-12-31T23:59:59",
            None,
            "reminder",
            "",
        )
        .await
        .unwrap();

    let due = store.get_due_tasks().await.unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].description, "Past task");
    assert_eq!(due[0].task_type, "reminder");
}

#[tokio::test]
async fn test_complete_one_shot() {
    let store = test_store().await;
    let id = store
        .create_task(
            "api",
            "user1",
            "chat1",
            "One shot",
            "2020-01-01T00:00:00",
            None,
            "reminder",
            "",
        )
        .await
        .unwrap();

    store.complete_task(&id, None).await.unwrap();

    let tasks = store.get_tasks_for_sender("user1").await.unwrap();
    assert!(tasks.is_empty());

    let due = store.get_due_tasks().await.unwrap();
    assert!(due.is_empty());
}

#[tokio::test]
async fn test_complete_recurring() {
    let store = test_store().await;
    let id = store
        .create_task(
            "api",
            "user1",
            "chat1",
            "Daily standup",
            "2020-01-01T09:00:00",
            Some("daily"),
            "reminder",
            "",
        )
        .await
        .unwrap();

    store.complete_task(&id, Some("daily")).await.unwrap();

    let tasks = store.get_tasks_for_sender("user1").await.unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].2, "2020-01-02 09:00:00");
}

#[tokio::test]
async fn test_cancel_task() {
    let store = test_store().await;
    let id = store
        .create_task(
            "api",
            "user1",
            "chat1",
            "Cancel me",
            "2099-12-31T00:00:00",
            None,
            "reminder",
            "",
        )
        .await
        .unwrap();

    let prefix = &id[..8];
    let cancelled = store.cancel_task(prefix, "user1").await.unwrap();
    assert!(cancelled);

    let tasks = store.get_tasks_for_sender("user1").await.unwrap();
    assert!(tasks.is_empty());
}

#[tokio::test]
async fn test_cancel_task_wrong_sender() {
    let store = test_store().await;
    let id = store
        .create_task(
            "api",
            "user1",
            "chat1",
            "My task",
            "2099-12-31T00:00:00",
            None,
            "reminder",
            "",
        )
        .await
        .unwrap();

    let prefix = &id[..8];
    let cancelled = store.cancel_task(prefix, "user2").await.unwrap();
    assert!(!cancelled);

    let tasks = store.get_tasks_for_sender("user1").await.unwrap();
    assert_eq!(tasks.len(), 1);
}

#[tokio::test]
async fn test_update_task_description() {
    let store = test_store().await;
    let id = store
        .create_task(
            "api",
            "user1",
            "chat1",
            "Old desc",
            "2099-12-31T00:00:00",
            None,
            "reminder",
            "",
        )
        .await
        .unwrap();

    let prefix = &id[..8];
    let updated = store
        .update_task(prefix, "user1", Some("New desc"), None, None)
        .await
        .unwrap();
    assert!(updated);

    let tasks = store.get_tasks_for_sender("user1").await.unwrap();
    assert_eq!(tasks[0].1, "New desc");
}

#[tokio::test]
async fn test_update_task_repeat() {
    let store = test_store().await;
    let id = store
        .create_task(
            "api",
            "user1",
            "chat1",
            "My task",
            "2099-12-31T00:00:00",
            Some("once"),
            "reminder",
            "",
        )
        .await
        .unwrap();

    let prefix = &id[..8];
    let updated = store
        .update_task(prefix, "user1", None, None, Some("daily"))
        .await
        .unwrap();
    assert!(updated);

    let tasks = store.get_tasks_for_sender("user1").await.unwrap();
    assert_eq!(tasks[0].3, Some("daily".to_string()));
}

#[tokio::test]
async fn test_update_task_wrong_sender() {
    let store = test_store().await;
    let id = store
        .create_task(
            "api",
            "user1",
            "chat1",
            "My task",
            "2099-12-31T00:00:00",
            None,
            "reminder",
            "",
        )
        .await
        .unwrap();

    let prefix = &id[..8];
    let updated = store
        .update_task(prefix, "user2", Some("Hacked"), None, None)
        .await
        .unwrap();
    assert!(!updated);

    let tasks = store.get_tasks_for_sender("user1").await.unwrap();
    assert_eq!(tasks[0].1, "My task");
}

#[tokio::test]
async fn test_update_task_no_fields() {
    let store = test_store().await;
    let id = store
        .create_task(
            "api",
            "user1",
            "chat1",
            "My task",
            "2099-12-31T00:00:00",
            None,
            "reminder",
            "",
        )
        .await
        .unwrap();

    let prefix = &id[..8];
    let updated = store
        .update_task(prefix, "user1", None, None, None)
        .await
        .unwrap();
    assert!(!updated);
}

#[tokio::test]
async fn test_create_task_with_action_type() {
    let store = test_store().await;
    let id = store
        .create_task(
            "api",
            "user1",
            "chat1",
            "Check BTC price",
            "2026-12-31T14:00:00",
            Some("daily"),
            "action",
            "",
        )
        .await
        .unwrap();
    assert!(!id.is_empty());

    let tasks = store.get_tasks_for_sender("user1").await.unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].1, "Check BTC price");
    assert_eq!(tasks[0].4, "action");
}

#[tokio::test]
async fn test_get_due_tasks_returns_task_type() {
    let store = test_store().await;
    store
        .create_task(
            "api",
            "user1",
            "chat1",
            "Reminder task",
            "2020-01-01T00:00:00",
            None,
            "reminder",
            "",
        )
        .await
        .unwrap();
    store
        .create_task(
            "api",
            "user1",
            "chat1",
            "Action task",
            "2020-01-01T00:00:00",
            None,
            "action",
            "",
        )
        .await
        .unwrap();

    let due = store.get_due_tasks().await.unwrap();
    assert_eq!(due.len(), 2);
    let reminder = due
        .iter()
        .find(|t| t.description == "Reminder task")
        .unwrap();
    let action = due.iter().find(|t| t.description == "Action task").unwrap();
    assert_eq!(reminder.task_type, "reminder");
    assert_eq!(action.task_type, "action");
}

#[tokio::test]
async fn test_create_task_dedup() {
    let store = test_store().await;
    let id1 = store
        .create_task(
            "api",
            "user1",
            "chat1",
            "Close all positions",
            "2026-02-20T14:30:00",
            None,
            "action",
            "",
        )
        .await
        .unwrap();

    let id2 = store
        .create_task(
            "api",
            "user1",
            "chat1",
            "Close all positions",
            "2026-02-20T14:30:00",
            None,
            "action",
            "",
        )
        .await
        .unwrap();
    assert_eq!(id1, id2);

    let tasks = store.get_tasks_for_sender("user1").await.unwrap();
    assert_eq!(tasks.len(), 1);
}

#[tokio::test]
async fn test_get_fact() {
    let store = test_store().await;
    assert!(store.get_fact("user1", "color").await.unwrap().is_none());

    store.store_fact("user1", "color", "blue").await.unwrap();
    assert_eq!(
        store.get_fact("user1", "color").await.unwrap(),
        Some("blue".to_string())
    );
}

#[tokio::test]
async fn test_delete_fact() {
    let store = test_store().await;
    assert!(!store.delete_fact("user1", "color").await.unwrap());

    store.store_fact("user1", "color", "blue").await.unwrap();
    assert!(store.delete_fact("user1", "color").await.unwrap());
    assert!(store.get_fact("user1", "color").await.unwrap().is_none());
}

#[tokio::test]
async fn test_soft_delete_fact_hides_from_default_reads() {
    let store = test_store().await;

    // Soft-deleting a missing row returns false.
    assert!(!store.soft_delete_fact("user1", "color").await.unwrap());

    store.store_fact("user1", "color", "blue").await.unwrap();
    store.store_fact("user1", "size", "large").await.unwrap();

    // Soft-delete transitions the row from active to deleted.
    assert!(store.soft_delete_fact("user1", "color").await.unwrap());

    // Default reads do not see soft-deleted rows.
    assert!(store.get_fact("user1", "color").await.unwrap().is_none());
    let facts = store.get_facts("user1").await.unwrap();
    assert_eq!(facts, vec![("size".to_string(), "large".to_string())]);

    // Re-soft-deleting returns false (idempotent).
    assert!(!store.soft_delete_fact("user1", "color").await.unwrap());
}

#[tokio::test]
async fn test_list_soft_deleted_facts() {
    let store = test_store().await;

    store.store_fact("user1", "color", "blue").await.unwrap();
    store.store_fact("user1", "size", "large").await.unwrap();
    store.soft_delete_fact("user1", "color").await.unwrap();

    let deleted = store.list_soft_deleted_facts("user1").await.unwrap();
    assert_eq!(deleted.len(), 1);
    assert_eq!(deleted[0].0, "color");
    assert_eq!(deleted[0].1, "blue");
    assert!(!deleted[0].2.is_empty(), "deleted_at timestamp populated");
    // Ensure the still-active row does not leak into the deleted list.
    assert!(deleted.iter().all(|(k, _, _)| k != "size"));
}

#[tokio::test]
async fn test_memory_stats_excludes_soft_deleted_facts() {
    let store = test_store().await;

    store.store_fact("user1", "color", "blue").await.unwrap();
    store.store_fact("user1", "size", "large").await.unwrap();
    let (_, _, _, fact_count_before) = store.get_memory_stats("user1").await.unwrap();
    assert_eq!(fact_count_before, 2);

    store.soft_delete_fact("user1", "color").await.unwrap();
    let (_, _, _, fact_count_after) = store.get_memory_stats("user1").await.unwrap();
    assert_eq!(
        fact_count_after, 1,
        "get_memory_stats must not count soft-deleted facts"
    );
}

#[tokio::test]
async fn test_store_fact_undeletes_soft_deleted_row() {
    let store = test_store().await;

    store.store_fact("user1", "color", "blue").await.unwrap();
    assert!(store.soft_delete_fact("user1", "color").await.unwrap());
    assert!(store.get_fact("user1", "color").await.unwrap().is_none());

    // Re-storing the same key clears deleted_at and applies the new value.
    store.store_fact("user1", "color", "red").await.unwrap();
    assert_eq!(
        store.get_fact("user1", "color").await.unwrap(),
        Some("red".to_string())
    );
    assert!(store
        .list_soft_deleted_facts("user1")
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn test_soft_delete_facts_with_and_without_key() {
    let store = test_store().await;

    store.store_fact("user1", "color", "blue").await.unwrap();
    store.store_fact("user1", "size", "large").await.unwrap();
    store.store_fact("user1", "weight", "10").await.unwrap();

    // With Some(key): soft-deletes only that key.
    assert_eq!(
        store
            .soft_delete_facts("user1", Some("color"))
            .await
            .unwrap(),
        1
    );
    assert_eq!(store.get_facts("user1").await.unwrap().len(), 2);

    // With None: soft-deletes every remaining active fact.
    assert_eq!(store.soft_delete_facts("user1", None).await.unwrap(), 2);
    assert!(store.get_facts("user1").await.unwrap().is_empty());

    // list_soft_deleted_facts captures all three.
    assert_eq!(
        store.list_soft_deleted_facts("user1").await.unwrap().len(),
        3
    );
}

#[tokio::test]
async fn test_is_new_user() {
    let store = test_store().await;

    assert!(store.is_new_user("fresh_user").await.unwrap());

    store
        .store_fact("fresh_user", "welcomed", "true")
        .await
        .unwrap();

    assert!(!store.is_new_user("fresh_user").await.unwrap());
}

#[tokio::test]
async fn test_get_all_facts() {
    let store = test_store().await;

    let facts = store.get_all_facts().await.unwrap();
    assert!(facts.is_empty());

    store.store_fact("user1", "name", "Alice").await.unwrap();
    store.store_fact("user2", "name", "Bob").await.unwrap();
    store.store_fact("user1", "timezone", "EST").await.unwrap();
    store.store_fact("user1", "welcomed", "true").await.unwrap();

    let facts = store.get_all_facts().await.unwrap();
    assert_eq!(facts.len(), 3, "should exclude 'welcomed' facts");
    assert!(facts.iter().any(|(k, v)| k == "name" && v == "Alice"));
    assert!(facts.iter().any(|(k, v)| k == "name" && v == "Bob"));
    assert!(facts.iter().any(|(k, v)| k == "timezone" && v == "EST"));
}

#[tokio::test]
async fn test_get_all_recent_summaries() {
    let store = test_store().await;

    let summaries = store.get_all_recent_summaries(3).await.unwrap();
    assert!(summaries.is_empty());

    sqlx::query(
        "INSERT INTO conversations (id, channel, sender_id, status, summary, last_activity, updated_at) \
         VALUES ('c1', 'api', 'user1', 'closed', 'Discussed project planning', datetime('now'), datetime('now'))",
    )
    .execute(store.pool())
    .await
    .unwrap();

    let summaries = store.get_all_recent_summaries(3).await.unwrap();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].0, "Discussed project planning");
}

// --- Limitation tests ---

#[tokio::test]
async fn test_store_limitation_new() {
    let store = test_store().await;
    let is_new = store
        .store_limitation("No email", "Cannot send emails", "Add SMTP")
        .await
        .unwrap();
    assert!(is_new, "first insert should return true");
}

#[tokio::test]
async fn test_store_limitation_duplicate() {
    let store = test_store().await;
    store
        .store_limitation("No email", "Cannot send emails", "Add SMTP")
        .await
        .unwrap();
    let is_new = store
        .store_limitation("No email", "Different desc", "Different plan")
        .await
        .unwrap();
    assert!(!is_new, "duplicate title should return false");
}

#[tokio::test]
async fn test_store_limitation_case_insensitive() {
    let store = test_store().await;
    store
        .store_limitation("No Email", "Cannot send emails", "Add SMTP")
        .await
        .unwrap();
    let is_new = store
        .store_limitation("no email", "Different desc", "Different plan")
        .await
        .unwrap();
    assert!(
        !is_new,
        "case-insensitive duplicate title should return false"
    );
}

#[tokio::test]
async fn test_get_open_limitations() {
    let store = test_store().await;
    store
        .store_limitation("No email", "Cannot send emails", "Add SMTP")
        .await
        .unwrap();
    store
        .store_limitation("No calendar", "Cannot access calendar", "Add Google Cal")
        .await
        .unwrap();

    let limitations = store.get_open_limitations().await.unwrap();
    assert_eq!(limitations.len(), 2);
    assert_eq!(limitations[0].0, "No email");
    assert_eq!(limitations[1].0, "No calendar");
}

// --- User profile tests ---

#[test]
fn test_user_profile_filters_system_facts() {
    let facts = vec![
        ("welcomed".to_string(), "true".to_string()),
        ("preferred_language".to_string(), "English".to_string()),
        ("active_project".to_string(), "myproject".to_string()),
        ("name".to_string(), "Alice".to_string()),
    ];
    let profile = format_user_profile(&facts);
    assert!(profile.contains("name: Alice"));
    assert!(!profile.contains("welcomed"));
    assert!(!profile.contains("preferred_language"));
    assert!(!profile.contains("active_project"));
}

#[test]
fn test_user_profile_groups_identity_first() {
    let facts = vec![
        ("timezone".to_string(), "EST".to_string()),
        ("interests".to_string(), "chess".to_string()),
        ("name".to_string(), "Alice".to_string()),
        ("pronouns".to_string(), "she/her".to_string()),
        ("occupation".to_string(), "engineer".to_string()),
    ];
    let profile = format_user_profile(&facts);
    let lines: Vec<&str> = profile.lines().collect();
    assert_eq!(lines[0], "User profile:");
    let name_pos = lines.iter().position(|l| l.contains("name:")).unwrap();
    let pronouns_pos = lines.iter().position(|l| l.contains("pronouns:")).unwrap();
    let timezone_pos = lines.iter().position(|l| l.contains("timezone:")).unwrap();
    let occupation_pos = lines
        .iter()
        .position(|l| l.contains("occupation:"))
        .unwrap();
    let interests_pos = lines.iter().position(|l| l.contains("interests:")).unwrap();
    assert!(name_pos < timezone_pos);
    assert!(pronouns_pos < timezone_pos);
    assert!(timezone_pos < interests_pos);
    assert!(occupation_pos < interests_pos);
}

#[test]
fn test_user_profile_empty_for_system_only() {
    let facts = vec![
        ("welcomed".to_string(), "true".to_string()),
        ("preferred_language".to_string(), "English".to_string()),
    ];
    let profile = format_user_profile(&facts);
    assert!(profile.is_empty());
}

// --- Onboarding hint tests ---

#[test]
fn test_build_system_prompt_shows_action_badge() {
    use super::context::{build_system_prompt, SystemPromptContext};
    let facts = vec![
        ("welcomed".to_string(), "true".to_string()),
        ("preferred_language".to_string(), "English".to_string()),
        ("name".to_string(), "Alice".to_string()),
        ("occupation".to_string(), "engineer".to_string()),
        ("timezone".to_string(), "EST".to_string()),
    ];
    let tasks = vec![(
        "abcd1234-0000".to_string(),
        "Check BTC price".to_string(),
        "2026-02-18T14:00:00".to_string(),
        Some("daily".to_string()),
        "action".to_string(),
        String::new(),
    )];
    let prompt = build_system_prompt(&SystemPromptContext {
        base_rules: "Rules",
        facts: &facts,
        summaries: &[],
        recall: &[],
        pending_tasks: &tasks,
        outcomes: &[],
        lessons: &[],
        language: "English",
        onboarding_hint: None,
    });
    assert!(
        prompt.contains("[action]"),
        "should show [action] badge for action tasks"
    );
}

#[test]
fn test_onboarding_stage0_first_conversation() {
    use super::context::{build_system_prompt, SystemPromptContext};
    let facts = vec![
        ("welcomed".to_string(), "true".to_string()),
        ("preferred_language".to_string(), "Spanish".to_string()),
    ];
    let prompt = build_system_prompt(&SystemPromptContext {
        base_rules: "Rules",
        facts: &facts,
        summaries: &[],
        recall: &[],
        pending_tasks: &[],
        outcomes: &[],
        lessons: &[],
        language: "Spanish",
        onboarding_hint: Some(0),
    });
    assert!(
        prompt.contains("first conversation"),
        "stage 0 should include first-conversation intro"
    );
}

#[test]
fn test_onboarding_stage1_help_hint() {
    use super::context::{build_system_prompt, SystemPromptContext};
    let facts = vec![
        ("welcomed".to_string(), "true".to_string()),
        ("preferred_language".to_string(), "English".to_string()),
        ("name".to_string(), "Alice".to_string()),
    ];
    let prompt = build_system_prompt(&SystemPromptContext {
        base_rules: "Rules",
        facts: &facts,
        summaries: &[],
        recall: &[],
        pending_tasks: &[],
        outcomes: &[],
        lessons: &[],
        language: "English",
        onboarding_hint: Some(1),
    });
    assert!(
        prompt.contains("/help"),
        "stage 1 should mention /help command"
    );
}

#[test]
fn test_onboarding_no_hint_when_none() {
    use super::context::{build_system_prompt, SystemPromptContext};
    let facts = vec![
        ("welcomed".to_string(), "true".to_string()),
        ("preferred_language".to_string(), "English".to_string()),
        ("name".to_string(), "Alice".to_string()),
        ("occupation".to_string(), "engineer".to_string()),
        ("timezone".to_string(), "EST".to_string()),
    ];
    let prompt = build_system_prompt(&SystemPromptContext {
        base_rules: "Rules",
        facts: &facts,
        summaries: &[],
        recall: &[],
        pending_tasks: &[],
        outcomes: &[],
        lessons: &[],
        language: "English",
        onboarding_hint: None,
    });
    assert!(
        !prompt.contains("Onboarding hint"),
        "should NOT include onboarding hint when None"
    );
    assert!(
        !prompt.contains("first conversation"),
        "should NOT include first-conversation intro when None"
    );
}

// --- compute_onboarding_stage tests ---

#[test]
fn test_compute_onboarding_stage_sequential() {
    use super::context::compute_onboarding_stage;
    assert_eq!(compute_onboarding_stage(0, 1, false), 1);
    assert_eq!(compute_onboarding_stage(0, 0, false), 0);
    assert_eq!(compute_onboarding_stage(1, 3, false), 2);
    assert_eq!(compute_onboarding_stage(1, 2, false), 1);
    assert_eq!(compute_onboarding_stage(2, 3, true), 3);
    assert_eq!(compute_onboarding_stage(2, 3, false), 2);
    assert_eq!(compute_onboarding_stage(3, 5, true), 4);
    assert_eq!(compute_onboarding_stage(3, 4, true), 3);
    assert_eq!(compute_onboarding_stage(4, 5, true), 5);
    assert_eq!(compute_onboarding_stage(5, 10, true), 5);
}

#[test]
fn test_compute_onboarding_stage_no_skip() {
    use super::context::compute_onboarding_stage;
    assert_eq!(compute_onboarding_stage(0, 10, true), 1);
}

#[test]
fn test_onboarding_hint_text_contains_commands() {
    use super::context::onboarding_hint_text;
    let hint1 = onboarding_hint_text(1, "English").unwrap();
    assert!(hint1.contains("/help"));
    let hint2 = onboarding_hint_text(2, "English").unwrap();
    assert!(hint2.contains("/personality"));
    let hint3 = onboarding_hint_text(3, "English").unwrap();
    assert!(hint3.contains("/tasks"));
    let hint4 = onboarding_hint_text(4, "English").unwrap();
    assert!(hint4.contains("/projects"));
    assert!(onboarding_hint_text(5, "English").is_none());
}

#[test]
fn test_onboarding_hint_text_includes_language() {
    use super::context::onboarding_hint_text;
    let hint0 = onboarding_hint_text(0, "French").unwrap();
    assert!(
        hint0.contains("French"),
        "stage 0 should reference the language"
    );

    for stage in 1..=4 {
        let hint = onboarding_hint_text(stage, "German").unwrap();
        assert!(
            hint.contains("Respond in German"),
            "stage {stage} should contain 'Respond in German'"
        );
    }
}

#[tokio::test]
async fn test_build_context_advances_onboarding_stage() {
    let store = test_store().await;
    let sender = "onboard_user";

    let msg = Request::text(sender, "hello");
    let needs = ContextNeeds::default();
    let ctx = store
        .build_context("api", &msg, "Base rules", &needs, None, None)
        .await
        .unwrap();
    assert!(
        ctx.system_prompt.contains("first conversation"),
        "first contact should trigger stage 0 intro"
    );

    store.store_fact(sender, "welcomed", "true").await.unwrap();
    store.store_fact(sender, "name", "Alice").await.unwrap();

    let ctx2 = store
        .build_context("api", &msg, "Base rules", &needs, None, None)
        .await
        .unwrap();
    assert!(
        ctx2.system_prompt.contains("/help"),
        "after learning name, should show stage 1 /help hint"
    );

    let ctx3 = store
        .build_context("api", &msg, "Base rules", &needs, None, None)
        .await
        .unwrap();
    assert!(
        !ctx3.system_prompt.contains("Onboarding hint"),
        "no hint when stage hasn't changed"
    );
}

// --- Auto-compact tests ---

struct MockSummarizer;

#[async_trait::async_trait]
impl Summarizer for MockSummarizer {
    async fn summarize(&self, text: &str) -> kernex_core::error::Result<String> {
        Ok(format!("SUMMARY({}chars)", text.len()))
    }
}

/// Build a small store with `max_context_messages = 2` and pre-insert N messages.
async fn compact_test_store(sender: &str, message_count: u32) -> (Store, String) {
    let opts = SqliteConnectOptions::from_str("sqlite::memory:")
        .unwrap()
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
        .unwrap();
    Store::run_migrations(&pool).await.unwrap();

    let store = Store {
        pool,
        max_context_messages: 2,
    };

    let conv_id = store
        .get_or_create_conversation("api", sender, "")
        .await
        .unwrap();

    for i in 0..message_count {
        sqlx::query(
            "INSERT INTO messages (id, conversation_id, role, content) VALUES (?, ?, 'user', ?)",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(&conv_id)
        .bind(format!("msg_{i}"))
        .execute(store.pool())
        .await
        .unwrap();
    }

    (store, conv_id)
}

#[tokio::test]
async fn test_compact_strategy_drop_no_summary_injected() {
    // Default strategy (Drop) should never inject a summary, even with overflow.
    let sender = "compact_drop_user";
    let (store, _) = compact_test_store(sender, 4).await;
    let msg = Request::text(sender, "latest");

    let needs = ContextNeeds {
        compact: CompactionStrategy::Drop,
        ..Default::default()
    };
    let ctx = store
        .build_context("api", &msg, "base", &needs, None, Some(&MockSummarizer))
        .await
        .unwrap();

    assert!(
        !ctx.system_prompt.contains("Earlier conversation summary"),
        "Drop strategy must not inject summary"
    );
}

#[tokio::test]
async fn test_compact_strategy_summarize_injects_summary() {
    let sender = "compact_summarize_user";
    let (store, _) = compact_test_store(sender, 4).await;
    let msg = Request::text(sender, "latest");

    let needs = ContextNeeds {
        compact: CompactionStrategy::Summarize,
        ..Default::default()
    };
    let ctx = store
        .build_context("api", &msg, "base", &needs, None, Some(&MockSummarizer))
        .await
        .unwrap();

    assert!(
        ctx.system_prompt.contains("[Earlier conversation summary]"),
        "Summarize strategy must inject summary header"
    );
    assert!(
        ctx.system_prompt.contains("SUMMARY("),
        "Summary text from MockSummarizer must appear in system prompt"
    );
}

#[tokio::test]
async fn test_compact_no_overflow_no_summary() {
    // Within limit — no summary should appear even with Summarize strategy.
    let sender = "compact_no_overflow_user";
    let (store, _) = compact_test_store(sender, 1).await;
    let msg = Request::text(sender, "latest");

    let needs = ContextNeeds {
        compact: CompactionStrategy::Summarize,
        ..Default::default()
    };
    let ctx = store
        .build_context("api", &msg, "base", &needs, None, Some(&MockSummarizer))
        .await
        .unwrap();

    assert!(
        !ctx.system_prompt.contains("[Earlier conversation summary]"),
        "No overflow means no summary injection"
    );
}

// --- User alias tests ---

#[tokio::test]
async fn test_resolve_sender_id_no_alias() {
    let store = test_store().await;
    let resolved = store.resolve_sender_id("phone123").await.unwrap();
    assert_eq!(resolved, "phone123");
}

#[tokio::test]
async fn test_create_and_resolve_alias() {
    let store = test_store().await;
    store.create_alias("phone123", "tg456").await.unwrap();
    let resolved = store.resolve_sender_id("phone123").await.unwrap();
    assert_eq!(resolved, "tg456");
}

#[tokio::test]
async fn test_create_alias_idempotent() {
    let store = test_store().await;
    store.create_alias("phone123", "tg456").await.unwrap();
    store.create_alias("phone123", "tg456").await.unwrap();
    let resolved = store.resolve_sender_id("phone123").await.unwrap();
    assert_eq!(resolved, "tg456");
}

#[tokio::test]
async fn test_find_canonical_user() {
    let store = test_store().await;
    assert!(store
        .find_canonical_user("new_user")
        .await
        .unwrap()
        .is_none());

    store.store_fact("tg456", "welcomed", "true").await.unwrap();

    let canonical = store
        .find_canonical_user("phone123")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(canonical, "tg456");

    assert!(store.find_canonical_user("tg456").await.unwrap().is_none());
}

#[tokio::test]
async fn test_alias_shares_facts() {
    let store = test_store().await;
    store.store_fact("tg456", "name", "Alice").await.unwrap();
    store.store_fact("tg456", "welcomed", "true").await.unwrap();

    store.create_alias("phone123", "tg456").await.unwrap();

    let resolved = store.resolve_sender_id("phone123").await.unwrap();
    let facts = store.get_facts(&resolved).await.unwrap();
    assert!(facts.iter().any(|(k, v)| k == "name" && v == "Alice"));
}

#[test]
fn test_normalize_due_at_strips_z() {
    assert_eq!(
        normalize_due_at("2026-02-22T07:00:00Z"),
        "2026-02-22 07:00:00"
    );
}

#[test]
fn test_normalize_due_at_replaces_t() {
    assert_eq!(
        normalize_due_at("2026-02-22T07:00:00"),
        "2026-02-22 07:00:00"
    );
}

#[test]
fn test_normalize_due_at_already_normalized() {
    assert_eq!(
        normalize_due_at("2026-02-22 07:00:00"),
        "2026-02-22 07:00:00"
    );
}

#[test]
fn test_descriptions_similar_email_variants() {
    assert!(descriptions_are_similar(
        "Enviar email de amor diario a Adriana (adri_navega@hotmail.com)",
        "Enviar email de amor diario a Adriana (adri_navega@hotmail.com) — escribir un mensaje"
    ));
}

#[test]
fn test_descriptions_similar_hostinger() {
    assert!(descriptions_are_similar(
        "Cancel Hostinger plan — expires March 17",
        "Cancel Hostinger VPS — last reminder, expires TOMORROW"
    ));
}

#[test]
fn test_descriptions_different() {
    assert!(!descriptions_are_similar(
        "Send good morning message to the team",
        "Cancel Hostinger plan and subscription"
    ));
}

#[test]
fn test_descriptions_short_skipped() {
    assert!(!descriptions_are_similar("Reminder task", "Action task"));
}

#[tokio::test]
async fn test_create_task_fuzzy_dedup() {
    let store = test_store().await;

    let id1 = store
        .create_task(
            "api",
            "user1",
            "reply1",
            "Send daily email to Adriana",
            "2026-02-22 07:00:00",
            Some("daily"),
            "action",
            "",
        )
        .await
        .unwrap();

    let id2 = store
        .create_task(
            "api",
            "user1",
            "reply1",
            "Send daily email to Adriana",
            "2026-02-22T07:00:00Z",
            Some("daily"),
            "action",
            "",
        )
        .await
        .unwrap();
    assert_eq!(id1, id2, "exact dedup with normalized datetime");

    let id3 = store
        .create_task(
            "api",
            "user1",
            "reply1",
            "Send daily love email to Adriana via gmail",
            "2026-02-22 07:05:00",
            Some("daily"),
            "action",
            "",
        )
        .await
        .unwrap();
    assert_eq!(id1, id3, "fuzzy dedup: similar description within 30min");

    let id4 = store
        .create_task(
            "api",
            "user2",
            "reply2",
            "Send daily email to Adriana",
            "2026-02-22 07:00:00",
            Some("daily"),
            "action",
            "",
        )
        .await
        .unwrap();
    assert_ne!(id1, id4, "different sender should create new task");
}

#[tokio::test]
async fn test_fail_task_retries() {
    let store = test_store().await;
    let id = store
        .create_task(
            "api",
            "user1",
            "chat1",
            "Send email",
            "2020-01-01T00:00:00",
            None,
            "action",
            "",
        )
        .await
        .unwrap();

    let will_retry = store.fail_task(&id, "SMTP error", 3).await.unwrap();
    assert!(will_retry, "should retry on first failure");

    let tasks = store.get_tasks_for_sender("user1").await.unwrap();
    assert_eq!(tasks.len(), 1, "task should still be pending");

    let will_retry = store.fail_task(&id, "SMTP error again", 3).await.unwrap();
    assert!(will_retry, "should retry on second failure");

    let will_retry = store.fail_task(&id, "SMTP final error", 3).await.unwrap();
    assert!(!will_retry, "should NOT retry after max retries");

    let tasks = store.get_tasks_for_sender("user1").await.unwrap();
    assert!(tasks.is_empty(), "failed task should not appear in pending");
}

#[tokio::test]
async fn test_fail_task_stores_error() {
    let store = test_store().await;
    let id = store
        .create_task(
            "api",
            "user1",
            "chat1",
            "Check price",
            "2020-01-01T00:00:00",
            None,
            "action",
            "",
        )
        .await
        .unwrap();

    store.fail_task(&id, "connection refused", 3).await.unwrap();

    let row: Option<(String, i64)> =
        sqlx::query_as("SELECT last_error, retry_count FROM scheduled_tasks WHERE id = ?")
            .bind(&id)
            .fetch_optional(store.pool())
            .await
            .unwrap();

    let (last_error, retry_count) = row.unwrap();
    assert_eq!(last_error, "connection refused");
    assert_eq!(retry_count, 1);
}

// --- Project-scoped learning tests ---

#[tokio::test]
async fn test_outcomes_project_isolation() {
    let store = test_store().await;

    store
        .store_outcome(
            "user1",
            "communication",
            1,
            "Be concise",
            "conversation",
            "",
        )
        .await
        .unwrap();
    store
        .store_outcome(
            "user1",
            "trading",
            1,
            "Check volume",
            "conversation",
            "my-trader",
        )
        .await
        .unwrap();

    let all = store.get_recent_outcomes("user1", 10, None).await.unwrap();
    assert_eq!(all.len(), 2);

    let trading = store
        .get_recent_outcomes("user1", 10, Some("my-trader"))
        .await
        .unwrap();
    assert_eq!(trading.len(), 1);
    assert_eq!(trading[0].1, "trading");

    let general = store
        .get_recent_outcomes("user1", 10, Some(""))
        .await
        .unwrap();
    assert_eq!(general.len(), 1);
    assert_eq!(general[0].1, "communication");
}

#[tokio::test]
async fn test_lessons_project_layering() {
    let store = test_store().await;

    store
        .store_lesson("user1", "communication", "Be concise", "")
        .await
        .unwrap();
    store
        .store_lesson("user1", "risk", "Never risk more than 2%", "my-trader")
        .await
        .unwrap();

    let general = store.get_lessons("user1", None).await.unwrap();
    assert_eq!(general.len(), 1);
    assert_eq!(general[0].0, "communication");
    assert_eq!(general[0].2, "");

    let layered = store.get_lessons("user1", Some("my-trader")).await.unwrap();
    assert_eq!(layered.len(), 2);
    assert_eq!(
        layered[0].2, "my-trader",
        "project lesson should come first"
    );
    assert_eq!(layered[1].2, "", "general lesson should come second");
}

#[tokio::test]
async fn test_lessons_project_separate() {
    let store = test_store().await;

    store
        .store_lesson("user1", "risk", "General risk rule", "")
        .await
        .unwrap();
    store
        .store_lesson("user1", "risk", "Trading risk rule", "my-trader")
        .await
        .unwrap();

    let all_lessons = store.get_lessons("user1", Some("my-trader")).await.unwrap();
    assert_eq!(
        all_lessons.len(),
        2,
        "same domain, different projects = separate"
    );

    store
        .store_lesson("user1", "risk", "Updated trading risk", "my-trader")
        .await
        .unwrap();
    let updated = store.get_lessons("user1", Some("my-trader")).await.unwrap();
    assert_eq!(
        updated.len(),
        3,
        "different rule text creates new row (multi-lesson)"
    );
    let trading_rules: Vec<&str> = updated
        .iter()
        .filter(|l| l.2 == "my-trader")
        .map(|l| l.1.as_str())
        .collect();
    assert!(trading_rules.contains(&"Trading risk rule"));
    assert!(trading_rules.contains(&"Updated trading risk"));
}

#[tokio::test]
async fn test_lessons_multi_per_domain() {
    let store = test_store().await;

    store
        .store_lesson("user1", "trading", "Always set stop-losses", "")
        .await
        .unwrap();
    store
        .store_lesson("user1", "trading", "Never risk more than 2%", "")
        .await
        .unwrap();
    store
        .store_lesson("user1", "trading", "Check volume before entry", "")
        .await
        .unwrap();

    let lessons = store.get_lessons("user1", None).await.unwrap();
    assert_eq!(lessons.len(), 3, "all 3 distinct rules should be stored");
    let rules: Vec<&str> = lessons.iter().map(|l| l.1.as_str()).collect();
    assert!(rules.contains(&"Always set stop-losses"));
    assert!(rules.contains(&"Never risk more than 2%"));
    assert!(rules.contains(&"Check volume before entry"));
}

#[tokio::test]
async fn test_lessons_content_dedup() {
    let store = test_store().await;

    store
        .store_lesson("user1", "trading", "Always set stop-losses", "")
        .await
        .unwrap();
    store
        .store_lesson("user1", "trading", "Always set stop-losses", "")
        .await
        .unwrap();

    let lessons = store.get_lessons("user1", None).await.unwrap();
    assert_eq!(
        lessons.len(),
        1,
        "duplicate rule text should not create new row"
    );

    let (occurrences,): (i64,) = sqlx::query_as(
        "SELECT occurrences FROM lessons WHERE sender_id = 'user1' AND domain = 'trading'",
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(occurrences, 2, "occurrences should be 2 after dedup");
}

#[tokio::test]
async fn test_lessons_cap_enforcement() {
    let store = test_store().await;

    for i in 0..12 {
        store
            .store_lesson("user1", "trading", &format!("Rule number {i}"), "")
            .await
            .unwrap();
    }

    let lessons = store.get_lessons("user1", None).await.unwrap();
    assert_eq!(
        lessons.len(),
        10,
        "cap should prune to 10 per (sender, domain, project)"
    );

    let rules: Vec<&str> = lessons.iter().map(|l| l.1.as_str()).collect();
    assert!(
        !rules.contains(&"Rule number 0"),
        "oldest rule should be pruned"
    );
    assert!(
        !rules.contains(&"Rule number 1"),
        "second-oldest rule should be pruned"
    );
    assert!(
        rules.contains(&"Rule number 11"),
        "newest rule should remain"
    );
}

#[tokio::test]
async fn test_lessons_cap_project_isolation() {
    let store = test_store().await;

    for i in 0..12 {
        store
            .store_lesson(
                "user1",
                "trading",
                &format!("Project A rule {i}"),
                "project-a",
            )
            .await
            .unwrap();
    }

    for i in 0..3 {
        store
            .store_lesson(
                "user1",
                "trading",
                &format!("Project B rule {i}"),
                "project-b",
            )
            .await
            .unwrap();
    }

    let a_lessons: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT domain, rule, project FROM lessons \
         WHERE sender_id = 'user1' AND project = 'project-a'",
    )
    .fetch_all(store.pool())
    .await
    .unwrap();
    assert_eq!(a_lessons.len(), 10, "project A capped at 10");

    let b_lessons: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT domain, rule, project FROM lessons \
         WHERE sender_id = 'user1' AND project = 'project-b'",
    )
    .fetch_all(store.pool())
    .await
    .unwrap();
    assert_eq!(
        b_lessons.len(),
        3,
        "project B unaffected by project A's cap"
    );
}

#[tokio::test]
async fn test_tasks_project_tag() {
    let store = test_store().await;

    store
        .create_task(
            "api",
            "user1",
            "chat1",
            "General reminder",
            "2099-12-31T00:00:00",
            None,
            "reminder",
            "",
        )
        .await
        .unwrap();

    store
        .create_task(
            "api",
            "user1",
            "chat1",
            "Check BTC",
            "2020-01-01T00:00:00",
            None,
            "action",
            "my-trader",
        )
        .await
        .unwrap();

    let tasks = store.get_tasks_for_sender("user1").await.unwrap();
    assert_eq!(tasks.len(), 2);
    let general = tasks.iter().find(|t| t.1 == "General reminder").unwrap();
    assert_eq!(general.5, "");
    let project = tasks.iter().find(|t| t.1 == "Check BTC").unwrap();
    assert_eq!(project.5, "my-trader");

    let due = store.get_due_tasks().await.unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].description, "Check BTC");
    assert_eq!(due[0].project, "my-trader");
}

#[tokio::test]
async fn test_get_all_lessons_project_filter() {
    let store = test_store().await;

    store
        .store_lesson("user1", "comms", "Be clear", "")
        .await
        .unwrap();
    store
        .store_lesson("user2", "trading", "Check volume", "my-trader")
        .await
        .unwrap();

    let all = store.get_all_lessons(None).await.unwrap();
    assert_eq!(all.len(), 2);

    let filtered = store.get_all_lessons(Some("my-trader")).await.unwrap();
    assert_eq!(filtered.len(), 2);
    assert_eq!(filtered[0].2, "my-trader", "project first");
    assert_eq!(filtered[1].2, "", "general second");
}

#[tokio::test]
async fn test_get_all_facts_by_key() {
    let store = test_store().await;

    store
        .store_fact("user1", "active_project", "my-trader")
        .await
        .unwrap();
    store
        .store_fact("user2", "active_project", "my-trader")
        .await
        .unwrap();
    store.store_fact("user3", "name", "Charlie").await.unwrap();

    let active = store.get_all_facts_by_key("active_project").await.unwrap();
    assert_eq!(active.len(), 2);
    assert!(active.iter().all(|(_, v)| v == "my-trader"));
}

#[tokio::test]
async fn test_migration_existing_data_gets_empty_project() {
    let store = test_store().await;

    store
        .store_outcome("user1", "test", 1, "lesson", "conversation", "")
        .await
        .unwrap();
    store
        .store_lesson("user1", "test", "rule", "")
        .await
        .unwrap();

    let outcomes = store.get_recent_outcomes("user1", 10, None).await.unwrap();
    assert_eq!(outcomes.len(), 1);

    let lessons = store.get_lessons("user1", None).await.unwrap();
    assert_eq!(lessons.len(), 1);
    assert_eq!(lessons[0].2, "", "default project should be empty string");
}

// --- Project session tests ---

#[tokio::test]
async fn test_store_and_get_session() {
    let store = test_store().await;

    let sid = store.get_session("api", "user1", "").await.unwrap();
    assert!(sid.is_none());

    store
        .store_session("api", "user1", "", "session-abc")
        .await
        .unwrap();
    let sid = store.get_session("api", "user1", "").await.unwrap();
    assert_eq!(sid, Some("session-abc".to_string()));
}

#[tokio::test]
async fn test_session_upsert() {
    let store = test_store().await;

    store
        .store_session("api", "user1", "", "session-1")
        .await
        .unwrap();
    store
        .store_session("api", "user1", "", "session-2")
        .await
        .unwrap();

    let sid = store.get_session("api", "user1", "").await.unwrap();
    assert_eq!(sid, Some("session-2".to_string()), "upsert should update");
}

#[tokio::test]
async fn test_session_project_isolation() {
    let store = test_store().await;

    store
        .store_session("api", "user1", "", "personal-session")
        .await
        .unwrap();
    store
        .store_session("api", "user1", "trader", "trader-session")
        .await
        .unwrap();

    let personal = store.get_session("api", "user1", "").await.unwrap();
    assert_eq!(personal, Some("personal-session".to_string()));

    let trader = store.get_session("api", "user1", "trader").await.unwrap();
    assert_eq!(trader, Some("trader-session".to_string()));
}

#[tokio::test]
async fn test_clear_session() {
    let store = test_store().await;

    store
        .store_session("api", "user1", "trader", "session-x")
        .await
        .unwrap();
    store.clear_session("api", "user1", "trader").await.unwrap();

    let sid = store.get_session("api", "user1", "trader").await.unwrap();
    assert!(sid.is_none(), "session should be cleared");
}

#[tokio::test]
async fn test_clear_all_sessions_for_sender() {
    let store = test_store().await;

    store.store_session("api", "user1", "", "s1").await.unwrap();
    store
        .store_session("api", "user1", "trader", "s2")
        .await
        .unwrap();
    store
        .store_session("slack", "user1", "", "s3")
        .await
        .unwrap();

    store.clear_all_sessions_for_sender("user1").await.unwrap();

    assert!(store
        .get_session("api", "user1", "")
        .await
        .unwrap()
        .is_none());
    assert!(store
        .get_session("api", "user1", "trader")
        .await
        .unwrap()
        .is_none());
    assert!(store
        .get_session("slack", "user1", "")
        .await
        .unwrap()
        .is_none());
}

// --- Project-scoped conversation tests ---

#[tokio::test]
async fn test_conversation_project_isolation() {
    let store = test_store().await;

    let personal = store
        .get_or_create_conversation("api", "user1", "")
        .await
        .unwrap();
    let trader = store
        .get_or_create_conversation("api", "user1", "trader")
        .await
        .unwrap();

    assert_ne!(
        personal, trader,
        "different projects should get different conversations"
    );

    let personal2 = store
        .get_or_create_conversation("api", "user1", "")
        .await
        .unwrap();
    assert_eq!(
        personal, personal2,
        "same project should return same conversation"
    );
}

#[tokio::test]
async fn test_close_current_conversation_project_scoped() {
    let store = test_store().await;

    let _personal = store
        .get_or_create_conversation("api", "user1", "")
        .await
        .unwrap();
    let _trader = store
        .get_or_create_conversation("api", "user1", "trader")
        .await
        .unwrap();

    let closed = store
        .close_current_conversation("api", "user1", "trader")
        .await
        .unwrap();
    assert!(closed, "should close trader conversation");

    let personal_again = store
        .get_or_create_conversation("api", "user1", "")
        .await
        .unwrap();
    assert_eq!(
        personal_again, _personal,
        "personal conversation should still be active"
    );

    let trader_new = store
        .get_or_create_conversation("api", "user1", "trader")
        .await
        .unwrap();
    assert_ne!(
        trader_new, _trader,
        "closed trader should create new conversation"
    );
}

#[tokio::test]
async fn test_find_idle_conversations_includes_project() {
    let store = test_store().await;

    sqlx::query(
        "INSERT INTO conversations (id, channel, sender_id, project, status, last_activity) \
         VALUES ('old1', 'api', 'user1', 'trader', 'active', datetime('now', '-3 hours'))",
    )
    .execute(store.pool())
    .await
    .unwrap();

    let idle = store.find_idle_conversations().await.unwrap();
    assert_eq!(idle.len(), 1);
    assert_eq!(idle[0].0, "old1");
    assert_eq!(idle[0].3, "trader", "should include project field");
}

// --- Multi-lesson edge case tests (migration 013) ---

#[tokio::test]
async fn test_lessons_dedup_reorders_by_updated_at() {
    let store = test_store().await;

    store
        .store_lesson("user1", "cooking", "Rule A", "")
        .await
        .unwrap();
    store
        .store_lesson("user1", "cooking", "Rule B", "")
        .await
        .unwrap();
    store
        .store_lesson("user1", "cooking", "Rule C", "")
        .await
        .unwrap();

    sqlx::query("UPDATE lessons SET updated_at = '2026-01-01 00:00:00' WHERE rule = 'Rule A'")
        .execute(store.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE lessons SET updated_at = '2026-01-01 00:01:00' WHERE rule = 'Rule B'")
        .execute(store.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE lessons SET updated_at = '2026-01-01 00:02:00' WHERE rule = 'Rule C'")
        .execute(store.pool())
        .await
        .unwrap();

    let before = store.get_lessons("user1", None).await.unwrap();
    assert_eq!(before[0].1, "Rule C", "newest should be first");
    assert_eq!(before[2].1, "Rule A", "oldest should be last");

    store
        .store_lesson("user1", "cooking", "Rule A", "")
        .await
        .unwrap();

    let after = store.get_lessons("user1", None).await.unwrap();
    assert_eq!(after.len(), 3, "dedup should not create a 4th row");
    assert_eq!(
        after[0].1, "Rule A",
        "reinforced lesson should be first (most recent updated_at)"
    );
}

#[tokio::test]
async fn test_lessons_reinforced_survives_cap() {
    let store = test_store().await;

    for i in 0..10 {
        store
            .store_lesson("user1", "trading", &format!("Rule {i}"), "")
            .await
            .unwrap();
        sqlx::query(&format!(
            "UPDATE lessons SET updated_at = '2026-01-01 00:{:02}:00' WHERE rule = 'Rule {}'",
            i, i
        ))
        .execute(store.pool())
        .await
        .unwrap();
    }

    store
        .store_lesson("user1", "trading", "Rule 0", "")
        .await
        .unwrap();

    store
        .store_lesson("user1", "trading", "Rule 10", "")
        .await
        .unwrap();

    let lessons = store.get_lessons("user1", None).await.unwrap();
    assert_eq!(lessons.len(), 10, "cap should keep 10");
    let rules: Vec<&str> = lessons.iter().map(|l| l.1.as_str()).collect();
    assert!(
        rules.contains(&"Rule 0"),
        "reinforced Rule 0 should survive cap (its updated_at was bumped)"
    );
    assert!(
        !rules.contains(&"Rule 1"),
        "Rule 1 (now oldest) should be pruned"
    );
    assert!(
        rules.contains(&"Rule 10"),
        "newest Rule 10 should be present"
    );
}

#[tokio::test]
async fn test_lessons_dedup_cross_project_isolation() {
    let store = test_store().await;

    store
        .store_lesson("user1", "risk", "Never risk more than 2%", "project-a")
        .await
        .unwrap();
    store
        .store_lesson("user1", "risk", "Never risk more than 2%", "project-b")
        .await
        .unwrap();

    let a: Vec<(String,)> = sqlx::query_as(
        "SELECT rule FROM lessons WHERE sender_id = 'user1' AND project = 'project-a'",
    )
    .fetch_all(store.pool())
    .await
    .unwrap();
    let b: Vec<(String,)> = sqlx::query_as(
        "SELECT rule FROM lessons WHERE sender_id = 'user1' AND project = 'project-b'",
    )
    .fetch_all(store.pool())
    .await
    .unwrap();
    assert_eq!(a.len(), 1, "project-a should have its own row");
    assert_eq!(b.len(), 1, "project-b should have its own row");

    let (occ_a,): (i64,) = sqlx::query_as(
        "SELECT occurrences FROM lessons WHERE sender_id = 'user1' AND project = 'project-a'",
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    let (occ_b,): (i64,) = sqlx::query_as(
        "SELECT occurrences FROM lessons WHERE sender_id = 'user1' AND project = 'project-b'",
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(occ_a, 1, "project-a occurrences should be 1");
    assert_eq!(occ_b, 1, "project-b occurrences should be 1");
}

#[tokio::test]
async fn test_lessons_dedup_cross_sender_isolation() {
    let store = test_store().await;

    store
        .store_lesson("user1", "cooking", "Salt the water", "")
        .await
        .unwrap();
    store
        .store_lesson("user2", "cooking", "Salt the water", "")
        .await
        .unwrap();

    let u1: Vec<(String,)> =
        sqlx::query_as("SELECT rule FROM lessons WHERE sender_id = 'user1' AND domain = 'cooking'")
            .fetch_all(store.pool())
            .await
            .unwrap();
    let u2: Vec<(String,)> =
        sqlx::query_as("SELECT rule FROM lessons WHERE sender_id = 'user2' AND domain = 'cooking'")
            .fetch_all(store.pool())
            .await
            .unwrap();
    assert_eq!(u1.len(), 1, "user1 should have its own row");
    assert_eq!(u2.len(), 1, "user2 should have its own row");
}

#[tokio::test]
async fn test_get_lessons_limit_50() {
    let store = test_store().await;

    for i in 0..11 {
        for domain in &["a", "b", "c", "d", "e"] {
            store
                .store_lesson("user1", domain, &format!("Rule {domain}-{i}"), "")
                .await
                .unwrap();
        }
    }

    for i in 0..5 {
        store
            .store_lesson("user1", "f", &format!("Rule f-{i}"), "")
            .await
            .unwrap();
    }

    let lessons = store.get_lessons("user1", None).await.unwrap();
    assert!(
        lessons.len() <= 50,
        "get_lessons should return at most 50, got {}",
        lessons.len()
    );
}

#[tokio::test]
async fn test_get_all_lessons_limit_50() {
    let store = test_store().await;

    for user in &["u1", "u2", "u3", "u4", "u5", "u6"] {
        for i in 0..10 {
            store
                .store_lesson(user, "general", &format!("Rule {user}-{i}"), "")
                .await
                .unwrap();
        }
    }

    let all = store.get_all_lessons(None).await.unwrap();
    assert!(
        all.len() <= 50,
        "get_all_lessons should return at most 50, got {}",
        all.len()
    );
}

// --- Context truncation UTF-8 safety tests ---

#[test]
fn test_build_system_prompt_recall_multibyte_truncation() {
    use super::context::{build_system_prompt, SystemPromptContext};

    let long_cjk = "\u{4e2d}".repeat(100);
    let recall = vec![crate::types::MessageRow {
        id: "msg-id".to_string(),
        conversation_id: "conv-id".to_string(),
        role: "user".to_string(),
        content: long_cjk,
        timestamp: std::time::SystemTime::UNIX_EPOCH
            + std::time::Duration::from_secs(1_767_268_800),
    }];

    let result = build_system_prompt(&SystemPromptContext {
        base_rules: "base rules",
        facts: &[],
        summaries: &[],
        recall: &recall,
        pending_tasks: &[],
        outcomes: &[],
        lessons: &[],
        language: "en",
        onboarding_hint: None,
    });
    assert!(result.contains("Related past context"));
}

// --- FTS5 query sanitization tests ---

#[tokio::test]
async fn test_search_messages_with_fts5_operators() {
    let store = test_store().await;
    let conv_id = store
        .get_or_create_conversation("api", "user1", "default")
        .await
        .unwrap();

    let incoming = Request::text("user1", "the server is NOT working properly");
    let response = kernex_core::message::Response {
        text: "I will investigate".to_string(),
        metadata: kernex_core::message::CompletionMeta {
            provider_used: "test".to_string(),
            tokens_used: None,
            processing_time_ms: 0,
            model: None,
            session_id: None,
            ..Default::default()
        },
    };
    store
        .store_exchange("api", &incoming, &response, "default")
        .await
        .unwrap();

    let result = store
        .search_messages("NOT working", &conv_id, "user1", 5, None)
        .await;
    assert!(
        result.is_ok(),
        "FTS5 operators in query should not cause an error: {:?}",
        result.err()
    );

    let result = store
        .search_messages("error (crash)", &conv_id, "user1", 5, None)
        .await;
    assert!(
        result.is_ok(),
        "FTS5 parentheses in query should not cause an error: {:?}",
        result.err()
    );

    let result = store
        .search_messages("work*", &conv_id, "user1", 5, None)
        .await;
    assert!(
        result.is_ok(),
        "FTS5 asterisk in query should not cause an error: {:?}",
        result.err()
    );

    let result = store
        .search_messages(r#"say "hello world""#, &conv_id, "user1", 5, None)
        .await;
    assert!(
        result.is_ok(),
        "FTS5 quotes in query should not cause an error: {:?}",
        result.err()
    );
}

// --- Typed-row trait-surface tests ---

#[tokio::test]
async fn test_search_messages_returns_typed_rows() {
    let store = test_store().await;
    let _conv_id = store
        .get_or_create_conversation("api", "user1", "default")
        .await
        .unwrap();
    let incoming = Request::text("user1", "the database is broken today");
    let response = kernex_core::message::Response {
        text: "Looking into it".to_string(),
        metadata: kernex_core::message::CompletionMeta {
            provider_used: "test".to_string(),
            tokens_used: None,
            processing_time_ms: 0,
            model: None,
            session_id: None,
            ..Default::default()
        },
    };
    store
        .store_exchange("api", &incoming, &response, "default")
        .await
        .unwrap();

    let rows = store
        .search_messages("database", "no-conv", "user1", 5, None)
        .await
        .unwrap();
    assert!(!rows.is_empty(), "search should return at least one row");
    let row = &rows[0];
    assert!(!row.id.is_empty(), "id must be a stable identifier");
    assert!(
        !row.conversation_id.is_empty(),
        "conversation_id must be surfaced for follow-up lookups"
    );
    assert_eq!(row.role, "user");
    assert!(row.content.contains("database"));
    // `timestamp` is SystemTime; confirm it parsed from the SQLite TIMESTAMP
    // shape rather than left as a string.
    assert!(
        row.timestamp >= std::time::UNIX_EPOCH,
        "timestamp must be a valid SystemTime"
    );
}

#[tokio::test]
async fn test_search_messages_since_filters_out_older_rows() {
    let store = test_store().await;
    let _conv_id = store
        .get_or_create_conversation("api", "user1", "default")
        .await
        .unwrap();
    let incoming = Request::text("user1", "a unique-marker-phrase appears here");
    let response = kernex_core::message::Response {
        text: "ack".to_string(),
        metadata: kernex_core::message::CompletionMeta {
            provider_used: "test".to_string(),
            tokens_used: None,
            processing_time_ms: 0,
            model: None,
            session_id: None,
            ..Default::default()
        },
    };
    store
        .store_exchange("api", &incoming, &response, "default")
        .await
        .unwrap();

    // Cutoff one hour into the future: no row can satisfy timestamp >= future.
    let future = std::time::SystemTime::now() + std::time::Duration::from_secs(3_600);
    let rows = store
        .search_messages("unique-marker-phrase", "no-conv", "user1", 10, Some(future))
        .await
        .unwrap();
    assert!(
        rows.is_empty(),
        "since=future should filter out the row that was just stored"
    );

    // Cutoff one hour into the past: the row should still be visible.
    let past = std::time::SystemTime::now() - std::time::Duration::from_secs(3_600);
    let rows = store
        .search_messages("unique-marker-phrase", "no-conv", "user1", 10, Some(past))
        .await
        .unwrap();
    assert!(
        !rows.is_empty(),
        "since=one-hour-ago should still return today's row"
    );
}

#[tokio::test]
async fn test_get_message_by_id_returns_typed_row() {
    let store = test_store().await;
    let _conv_id = store
        .get_or_create_conversation("api", "user1", "default")
        .await
        .unwrap();
    let incoming = Request::text("user1", "store this and look it up by id");
    let response = kernex_core::message::Response {
        text: "ack".to_string(),
        metadata: kernex_core::message::CompletionMeta {
            provider_used: "test".to_string(),
            tokens_used: None,
            processing_time_ms: 0,
            model: None,
            session_id: None,
            ..Default::default()
        },
    };
    store
        .store_exchange("api", &incoming, &response, "default")
        .await
        .unwrap();

    let rows = store
        .search_messages("store this and look it up", "no-conv", "user1", 5, None)
        .await
        .unwrap();
    let target = &rows[0];

    let by_id = store.get_message_by_id(&target.id).await.unwrap();
    let by_id = by_id.expect("row should be found by its UUID");
    assert_eq!(by_id.id, target.id);
    assert_eq!(by_id.conversation_id, target.conversation_id);
    assert_eq!(by_id.role, target.role);
    assert_eq!(by_id.content, target.content);
    assert_eq!(by_id.timestamp, target.timestamp);
}

#[tokio::test]
async fn test_get_message_by_id_missing_returns_none() {
    let store = test_store().await;
    let missing = store
        .get_message_by_id("00000000-0000-0000-0000-000000000000")
        .await
        .unwrap();
    assert!(missing.is_none());
}

#[tokio::test]
async fn test_get_history_returns_typed_rows_with_conversation_id() {
    let store = test_store().await;
    let conv_id = store
        .get_or_create_conversation("api", "user1", "default")
        .await
        .unwrap();
    let incoming = Request::text("user1", "first exchange");
    let response = kernex_core::message::Response {
        text: "ack".to_string(),
        metadata: kernex_core::message::CompletionMeta {
            provider_used: "test".to_string(),
            tokens_used: None,
            processing_time_ms: 0,
            model: None,
            session_id: None,
            ..Default::default()
        },
    };
    store
        .store_exchange("api", &incoming, &response, "default")
        .await
        .unwrap();
    store
        .close_current_conversation("api", "user1", "default")
        .await
        .unwrap();

    let history = store.get_history("api", "user1", 10).await.unwrap();
    assert!(!history.is_empty());
    let row = &history[0];
    assert_eq!(row.conversation_id, conv_id);
    assert!(
        row.updated_at >= std::time::UNIX_EPOCH,
        "updated_at must be a valid SystemTime"
    );
}

// --- typed observation tests (kernex-memory 0.8.0) ------------------

use crate::observation::{ObservationType, SaveEntry};

fn obs_entry(sender_id: &str, kind: ObservationType, title: &str) -> SaveEntry {
    SaveEntry {
        sender_id: sender_id.to_string(),
        kind,
        title: title.to_string(),
        what: None,
        why: None,
        where_field: None,
        learned: None,
    }
}

#[tokio::test]
async fn save_round_trip() {
    let store = test_store().await;
    let entry = SaveEntry {
        sender_id: "user".to_string(),
        kind: ObservationType::Bugfix,
        title: "Fixed N+1 query".to_string(),
        what: Some("added eager loading".to_string()),
        why: Some("12s pages on 5k users".to_string()),
        where_field: Some("src/users/list.rs".to_string()),
        learned: Some("FTS5 rewriter cannot fix N+1".to_string()),
    };
    let saved = store.save_observation(entry.clone()).await.unwrap();
    assert!(!saved.id.is_empty(), "id must be generated");
    assert_eq!(saved.sender_id, entry.sender_id);
    assert_eq!(saved.kind, entry.kind);
    assert_eq!(saved.title, entry.title);
    assert_eq!(saved.what, entry.what);
    assert_eq!(saved.why, entry.why);
    assert_eq!(saved.where_field, entry.where_field);
    assert_eq!(saved.learned, entry.learned);
    assert_eq!(saved.created_at, saved.updated_at);
}

#[tokio::test]
async fn save_then_search_finds() {
    let store = test_store().await;
    let saved = store
        .save_observation(obs_entry("user", ObservationType::Bugfix, "N+1 query"))
        .await
        .unwrap();
    let hits = store
        .search_observations("query", "user", 10, None, None)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, saved.id);
}

#[tokio::test]
async fn save_then_get_by_id() {
    let store = test_store().await;
    let saved = store
        .save_observation(obs_entry(
            "user",
            ObservationType::Decision,
            "Adopt rusqlite",
        ))
        .await
        .unwrap();
    let got = store.get_observation_by_id(&saved.id).await.unwrap();
    assert!(got.is_some());
    let obs = got.unwrap();
    assert_eq!(obs.id, saved.id);
    assert_eq!(obs.title, "Adopt rusqlite");
}

#[tokio::test]
async fn save_with_none_optionals() {
    let store = test_store().await;
    let saved = store
        .save_observation(obs_entry("user", ObservationType::Config, "tokio runtime"))
        .await
        .unwrap();
    assert!(saved.what.is_none());
    assert!(saved.why.is_none());
    assert!(saved.where_field.is_none());
    assert!(saved.learned.is_none());
    // Findable by title via FTS.
    let hits = store
        .search_observations("runtime", "user", 10, None, None)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
}

#[tokio::test]
async fn save_rejects_empty_title() {
    let store = test_store().await;
    let entry = SaveEntry {
        sender_id: "user".to_string(),
        kind: ObservationType::Bugfix,
        title: "".to_string(), // CHECK (length(title) > 0) should reject
        what: None,
        why: None,
        where_field: None,
        learned: None,
    };
    let err = store.save_observation(entry).await.unwrap_err();
    assert!(
        matches!(err, crate::error::MemoryError::Sqlite { .. }),
        "expected Sqlite error from CHECK constraint, got {err:?}"
    );
}

#[tokio::test]
async fn save_rejects_unknown_type_at_db() {
    // The Rust enum cannot represent an unknown variant by construction,
    // so we bypass the typed surface and write a raw bogus type string
    // to verify the SQL CHECK fires.
    let store = test_store().await;
    let now = crate::types::format_sqlite_timestamp(std::time::SystemTime::now());
    let result = sqlx::query(
        "INSERT INTO observations \
            (id, sender_id, type, title, what, why, where_field, learned, created_at, updated_at) \
         VALUES ('id-bogus', 'user', 'bogus', 'title', NULL, NULL, NULL, NULL, ?, ?)",
    )
    .bind(&now)
    .bind(&now)
    .execute(&store.pool)
    .await;
    assert!(result.is_err(), "DB CHECK must reject unknown type");
}

#[tokio::test]
async fn search_kind_filter_narrows() {
    let store = test_store().await;
    store
        .save_observation(obs_entry("user", ObservationType::Bugfix, "marker bugfix"))
        .await
        .unwrap();
    store
        .save_observation(obs_entry(
            "user",
            ObservationType::Decision,
            "marker decision",
        ))
        .await
        .unwrap();

    let all = store
        .search_observations("marker", "user", 10, None, None)
        .await
        .unwrap();
    assert_eq!(all.len(), 2);

    let bugfix_only = store
        .search_observations("marker", "user", 10, None, Some(ObservationType::Bugfix))
        .await
        .unwrap();
    assert_eq!(bugfix_only.len(), 1);
    assert_eq!(bugfix_only[0].kind, ObservationType::Bugfix);
}

#[tokio::test]
async fn search_since_filters_by_recency() {
    let store = test_store().await;
    let saved = store
        .save_observation(obs_entry(
            "user",
            ObservationType::Bugfix,
            "recent observation",
        ))
        .await
        .unwrap();

    // since in the past returns the row.
    let past = saved.created_at - std::time::Duration::from_secs(3_600);
    let hits = store
        .search_observations("observation", "user", 10, Some(past), None)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);

    // since in the future filters it out.
    let future = saved.created_at + std::time::Duration::from_secs(3_600);
    let hits = store
        .search_observations("observation", "user", 10, Some(future), None)
        .await
        .unwrap();
    assert_eq!(hits.len(), 0);
}

#[tokio::test]
async fn search_sender_scope_is_hard() {
    let store = test_store().await;
    store
        .save_observation(obs_entry("alice", ObservationType::Bugfix, "shared marker"))
        .await
        .unwrap();
    store
        .save_observation(obs_entry("bob", ObservationType::Bugfix, "shared marker"))
        .await
        .unwrap();

    let alice_hits = store
        .search_observations("marker", "alice", 10, None, None)
        .await
        .unwrap();
    assert_eq!(alice_hits.len(), 1);
    assert_eq!(alice_hits[0].sender_id, "alice");
}

#[tokio::test]
async fn search_empty_corpus_returns_empty_vec() {
    let store = test_store().await;
    let hits = store
        .search_observations("anything", "user", 10, None, None)
        .await
        .unwrap();
    assert!(hits.is_empty());
}

#[tokio::test]
async fn soft_delete_hides_from_default_reads() {
    let store = test_store().await;
    let saved = store
        .save_observation(obs_entry("user", ObservationType::Bugfix, "to be deleted"))
        .await
        .unwrap();

    let ok = store.soft_delete_observation(&saved.id).await.unwrap();
    assert!(ok, "first soft-delete must transition active to deleted");

    // get returns None.
    assert!(store
        .get_observation_by_id(&saved.id)
        .await
        .unwrap()
        .is_none());

    // search misses it.
    let hits = store
        .search_observations("deleted", "user", 10, None, None)
        .await
        .unwrap();
    assert!(hits.is_empty());
}

#[tokio::test]
async fn soft_delete_is_idempotent() {
    let store = test_store().await;
    let saved = store
        .save_observation(obs_entry(
            "user",
            ObservationType::Bugfix,
            "double-delete test",
        ))
        .await
        .unwrap();
    assert!(store.soft_delete_observation(&saved.id).await.unwrap());
    assert!(
        !store.soft_delete_observation(&saved.id).await.unwrap(),
        "second soft-delete must return false (no transition)"
    );
}

#[tokio::test]
async fn soft_delete_missing_id_returns_false() {
    let store = test_store().await;
    assert!(!store
        .soft_delete_observation("not-a-real-id")
        .await
        .unwrap());
}

#[tokio::test]
async fn list_soft_deleted_returns_only_deleted() {
    let store = test_store().await;
    let active = store
        .save_observation(obs_entry("user", ObservationType::Bugfix, "active row"))
        .await
        .unwrap();
    let to_delete = store
        .save_observation(obs_entry("user", ObservationType::Decision, "deleted row"))
        .await
        .unwrap();
    store.soft_delete_observation(&to_delete.id).await.unwrap();

    let deleted = store.list_soft_deleted_observations("user").await.unwrap();
    assert_eq!(deleted.len(), 1);
    assert_eq!(deleted[0].id, to_delete.id);
    assert_ne!(deleted[0].id, active.id);
}

#[tokio::test]
async fn list_soft_deleted_respects_sender_scope() {
    let store = test_store().await;
    let alice = store
        .save_observation(obs_entry("alice", ObservationType::Bugfix, "alice row"))
        .await
        .unwrap();
    let bob = store
        .save_observation(obs_entry("bob", ObservationType::Bugfix, "bob row"))
        .await
        .unwrap();
    store.soft_delete_observation(&alice.id).await.unwrap();
    store.soft_delete_observation(&bob.id).await.unwrap();

    let alice_deleted = store.list_soft_deleted_observations("alice").await.unwrap();
    assert_eq!(alice_deleted.len(), 1);
    assert_eq!(alice_deleted[0].sender_id, "alice");
}

#[tokio::test]
async fn get_memory_stats_returns_four_tuple() {
    let store = test_store().await;
    store
        .save_observation(obs_entry("user", ObservationType::Bugfix, "obs one"))
        .await
        .unwrap();
    store
        .save_observation(obs_entry("user", ObservationType::Decision, "obs two"))
        .await
        .unwrap();
    store.store_fact("user", "k", "v").await.unwrap();

    let (conv, msg, obs, facts) = store.get_memory_stats("user").await.unwrap();
    assert_eq!(conv, 0);
    assert_eq!(msg, 0);
    assert_eq!(obs, 2);
    assert_eq!(facts, 1);
}

#[tokio::test]
async fn get_memory_stats_excludes_soft_deleted_observations() {
    let store = test_store().await;
    let a = store
        .save_observation(obs_entry("user", ObservationType::Bugfix, "active"))
        .await
        .unwrap();
    let b = store
        .save_observation(obs_entry("user", ObservationType::Bugfix, "to delete"))
        .await
        .unwrap();
    store.soft_delete_observation(&b.id).await.unwrap();

    let (_, _, obs_count, _) = store.get_memory_stats("user").await.unwrap();
    assert_eq!(obs_count, 1, "soft-deleted row must not be counted");
    let _ = a; // suppress unused warning
}

#[tokio::test]
async fn migration_018_applies_idempotently() {
    // Construct two pools against the same in-memory DB and run
    // migrations twice; the fast-path must short-circuit the second
    // run (idempotency invariant) and not error.
    let opts = SqliteConnectOptions::from_str("sqlite::memory:")
        .unwrap()
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
        .unwrap();
    Store::run_migrations(&pool).await.unwrap();
    Store::run_migrations(&pool).await.unwrap(); // second call must be a no-op

    // Verify the observations table exists by querying it.
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM observations")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count.0, 0);
}

/// Create a file-backed store so a second pool can open the same database
/// (in-memory SQLite is per-connection, so it cannot model two concurrent
/// claimers). Returns the store and the TempDir guard keeping the file alive.
async fn file_store() -> (Store, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("claims.db");
    let store = open_store_at(&db_path).await;
    (store, dir)
}

async fn open_store_at(db_path: &std::path::Path) -> Store {
    let opts = SqliteConnectOptions::from_str(&format!("sqlite://{}", db_path.display()))
        .unwrap()
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
        .unwrap();
    Store::run_migrations(&pool).await.unwrap();
    Store {
        pool,
        max_context_messages: 10,
    }
}

#[tokio::test]
async fn test_claim_due_tasks_hands_each_task_to_exactly_one_claimer() {
    let (store_a, dir) = file_store().await;
    let store_b = open_store_at(&dir.path().join("claims.db")).await;

    store_a
        .create_task(
            "cron",
            "user1",
            "cli",
            "claimed exactly once",
            "2020-01-01 00:00:00",
            None,
            "scheduled",
            "proj",
        )
        .await
        .unwrap();

    // Two real concurrent claimers against the same database file.
    let (a, b) = tokio::join!(store_a.claim_due_tasks(), store_b.claim_due_tasks());
    let total = a.unwrap().len() + b.unwrap().len();
    assert_eq!(total, 1, "exactly one claimer must win the due task");

    // The task is claimed: a further round sees nothing.
    assert!(store_a.claim_due_tasks().await.unwrap().is_empty());
    assert!(store_b.claim_due_tasks().await.unwrap().is_empty());
}

#[tokio::test]
async fn test_stale_claim_is_reclaimed() {
    let store = test_store().await;
    let id = store
        .create_task(
            "cron",
            "user1",
            "cli",
            "abandoned by a dead claimer",
            "2020-01-01 00:00:00",
            None,
            "scheduled",
            "proj",
        )
        .await
        .unwrap();

    assert_eq!(store.claim_due_tasks().await.unwrap().len(), 1);
    assert!(store.claim_due_tasks().await.unwrap().is_empty());

    // Backdate the claim past the staleness window: the task becomes
    // reclaimable, modeling a claimer that died mid-run.
    sqlx::query(
        "UPDATE scheduled_tasks SET claimed_at = datetime('now', '-11 minutes') WHERE id = ?",
    )
    .bind(&id)
    .execute(&store.pool)
    .await
    .unwrap();

    let reclaimed = store.claim_due_tasks().await.unwrap();
    assert_eq!(reclaimed.len(), 1);
    assert_eq!(reclaimed[0].id, id);
}

#[tokio::test]
async fn test_complete_recurring_releases_claim_into_pending() {
    let store = test_store().await;
    let id = store
        .create_task(
            "cron",
            "user1",
            "cli",
            "recurring claim release",
            "2020-01-01 00:00:00",
            Some("daily"),
            "scheduled",
            "proj",
        )
        .await
        .unwrap();

    let claimed = store.claim_due_tasks().await.unwrap();
    assert_eq!(claimed.len(), 1);

    store.complete_task(&id, Some("daily")).await.unwrap();

    let (status, claimed_at): (String, Option<String>) =
        sqlx::query_as("SELECT status, claimed_at FROM scheduled_tasks WHERE id = ?")
            .bind(&id)
            .fetch_one(&store.pool)
            .await
            .unwrap();
    assert_eq!(status, "pending");
    assert!(claimed_at.is_none());
}

#[tokio::test]
async fn test_fail_task_retry_releases_claim_into_pending() {
    let store = test_store().await;
    let id = store
        .create_task(
            "cron",
            "user1",
            "cli",
            "failing claim release",
            "2020-01-01 00:00:00",
            None,
            "scheduled",
            "proj",
        )
        .await
        .unwrap();

    assert_eq!(store.claim_due_tasks().await.unwrap().len(), 1);
    let will_retry = store.fail_task(&id, "boom", 3).await.unwrap();
    assert!(will_retry);

    let (status, claimed_at): (String, Option<String>) =
        sqlx::query_as("SELECT status, claimed_at FROM scheduled_tasks WHERE id = ?")
            .bind(&id)
            .fetch_one(&store.pool)
            .await
            .unwrap();
    assert_eq!(status, "pending");
    assert!(claimed_at.is_none());
}

#[tokio::test]
async fn test_record_and_list_task_runs() {
    let store = test_store().await;
    let task_id = "0123456789abcdef";

    store
        .record_task_run(
            task_id,
            "2026-06-12 10:00:00",
            "completed",
            Some("first result"),
            None,
            Some(1234),
        )
        .await
        .unwrap();
    store
        .record_task_run(
            task_id,
            "2026-06-12 11:00:00",
            "failed",
            None,
            Some("provider exploded"),
            None,
        )
        .await
        .unwrap();

    // Prefix lookup, newest first.
    let runs = store.list_task_runs("01234567", 10).await.unwrap();
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].status, "failed");
    assert_eq!(runs[0].error.as_deref(), Some("provider exploded"));
    assert_eq!(runs[1].status, "completed");
    assert_eq!(runs[1].result.as_deref(), Some("first result"));
    assert_eq!(runs[1].tokens_used, Some(1234));

    // Limit caps the result set.
    assert_eq!(store.list_task_runs(task_id, 1).await.unwrap().len(), 1);

    // Unknown prefix yields nothing.
    assert!(store.list_task_runs("ffff", 10).await.unwrap().is_empty());

    // The DB CHECK rejects an invalid status value.
    assert!(store
        .record_task_run(task_id, "2026-06-12 12:00:00", "bogus", None, None, None)
        .await
        .is_err());
}
