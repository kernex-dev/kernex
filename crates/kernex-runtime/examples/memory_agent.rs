//! Demonstrates Kernex's persistent memory: facts, outcomes, and lessons.
//!
//! Run: `cargo run --example memory_agent`

use kernex_core::config::MemoryConfig;
use kernex_memory::Store;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    // Create an in-memory store (use a file path for persistence).
    let config = MemoryConfig {
        db_path: ":memory:".to_string(),
        ..Default::default()
    };
    let store = Store::new(&config).await?;
    let sender = "demo-user";
    let project = "";

    // --- Facts: key-value pairs about a user ---
    store.store_fact(sender, "name", "Alice").await?;
    store.store_fact(sender, "language", "Rust").await?;
    store.store_fact(sender, "role", "Backend Engineer").await?;

    println!("Stored 3 facts about {sender}");
    let facts = store.get_all_facts().await?;
    for (key, value) in &facts {
        println!("  {key}: {value}");
    }

    // --- Lessons: reward-based learning ---
    store
        .store_lesson(
            sender,
            "code_style",
            "User prefers functional iterator chains over explicit for loops.",
            project,
        )
        .await?;
    store
        .store_lesson(
            sender,
            "communication",
            "User likes concise answers with code examples.",
            project,
        )
        .await?;

    // get_lessons returns Vec<(domain, rule, project)>
    let lessons = store.get_lessons(sender, None).await?;
    println!("\nLearned {} lessons:", lessons.len());
    for (domain, rule, _proj) in &lessons {
        println!("  [{domain}] {rule}");
    }

    // --- Outcomes: raw reward signals ---
    store
        .store_outcome(
            sender,
            "helpfulness",
            5,
            "Great code review",
            "user",
            project,
        )
        .await?;

    println!("\nStored 1 outcome (reward signal)");
    println!("\nMemory system ready for agent integration.");

    Ok(())
}
