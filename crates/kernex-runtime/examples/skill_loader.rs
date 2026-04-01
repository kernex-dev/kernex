//! Demonstrates loading skills and matching triggers to activate MCP servers.
//!
//! Run: `cargo run --example skill_loader`
//!
//! By default, looks for skills in `~/.kernex/skills/`. To use the reference
//! skills from this repo, copy them first:
//!
//!   mkdir -p ~/.kernex/skills
//!   cp -r examples/skills/*/ ~/.kernex/skills/

use kernex_skills::{build_skill_prompt, load_skills, match_skill_triggers};
use tracing_subscriber::EnvFilter;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let data_dir = "~/.kernex";
    let skills = load_skills(data_dir);

    println!("Loaded {} skills from {data_dir}/skills/\n", skills.len());

    for skill in &skills {
        let status = if skill.available {
            "ready"
        } else {
            "missing deps"
        };
        println!("  {} — {} [{}]", skill.name, skill.description, status);
        if let Some(ref trigger) = skill.trigger {
            println!("    triggers: {trigger}");
        }
        if !skill.mcp_servers.is_empty() {
            for srv in &skill.mcp_servers {
                println!(
                    "    mcp: {} -> {} {}",
                    srv.name,
                    srv.command,
                    srv.args.join(" ")
                );
            }
        }
        println!();
    }

    // Simulate trigger matching against user messages.
    let test_messages = [
        "Please browse google.com and extract the headlines",
        "Show me the git log for the last week",
        "Search for Rust async patterns online",
        "Read the contents of main.rs",
        "What is the weather today?",
    ];

    println!("--- Trigger matching ---\n");
    for msg in &test_messages {
        let servers = match_skill_triggers(&skills, msg);
        if servers.is_empty() {
            println!("  \"{msg}\" -> no skills matched");
        } else {
            let names: Vec<&str> = servers.iter().map(|s| s.name.as_str()).collect();
            println!("  \"{msg}\" -> activate: {}", names.join(", "));
        }
    }

    // Show what gets injected into the system prompt.
    let skill_ctx = build_skill_prompt(&skills);
    if !skill_ctx.prompt.is_empty() {
        println!("\n--- System prompt injection ---\n");
        println!("{}", skill_ctx.prompt);
        if let Some(model) = &skill_ctx.model {
            println!("Model override: {model}");
        }
    }
}
