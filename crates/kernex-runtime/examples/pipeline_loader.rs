//! Demonstrates loading and validating a multi-agent pipeline topology.
//!
//! Run: `cargo run --example pipeline_loader`
//!
//! By default, loads from `examples/topologies/`. Copy to your data dir:
//!
//!   cp -r examples/topologies/ ~/.kernex/topologies/

use kernex_pipelines::load_topology;
use tracing_subscriber::EnvFilter;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    // Use the examples directory as the data_dir.
    let data_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples");
    let name = "code-review";

    match load_topology(data_dir, name) {
        Ok(loaded) => {
            let meta = &loaded.topology.topology;
            println!("Pipeline: {}", meta.name);
            println!("Description: {}", meta.description);
            println!("Version: {}", meta.version);
            println!();

            println!("Phases ({}):", loaded.topology.phases.len());
            for (i, phase) in loaded.topology.phases.iter().enumerate() {
                println!(
                    "  {}. [{}] agent={} tier={:?}",
                    i + 1,
                    phase.name,
                    phase.agent,
                    phase.model_tier,
                );
                if let Some(turns) = phase.max_turns {
                    println!("     max_turns: {turns}");
                }
                if let Some(ref retry) = phase.retry {
                    println!(
                        "     retry: max={}, fix_agent={}",
                        retry.max, retry.fix_agent,
                    );
                }
                if let Some(ref pre) = phase.pre_validation {
                    println!(
                        "     pre_validation: {:?} paths={:?}",
                        pre.validation_type, pre.paths,
                    );
                }
            }

            println!("\nLoaded agents ({}):", loaded.agents.len());
            for (agent_name, content) in loaded.agents.iter() {
                let first_line: &str = content.lines().next().unwrap_or("(empty)");
                println!("  {agent_name}: {first_line}");
            }

            // Demonstrate model resolution.
            println!("\nModel resolution (fast=llama3.2, complex=claude-sonnet):");
            for phase in &loaded.topology.phases {
                let model = loaded.resolve_model(phase, "llama3.2", "claude-sonnet-4-20250514");
                println!("  {} -> {model}", phase.name);
            }
        }
        Err(e) => {
            eprintln!("Failed to load topology: {e}");
            eprintln!("\nEnsure examples/topologies/code-review/ exists with:");
            eprintln!("  TOPOLOGY.toml + agents/*.md");
            std::process::exit(1);
        }
    }
}
