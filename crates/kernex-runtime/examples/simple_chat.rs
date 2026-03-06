//! Minimal chat example using Ollama (local, no API key needed).
//!
//! Prerequisites:
//!   1. Install Ollama: https://ollama.com
//!   2. Pull a model: `ollama pull llama3.2`
//!   3. Run: `cargo run --example simple_chat`

use kernex_core::context::{Context, ContextEntry};
use kernex_core::traits::Provider;
use kernex_providers::ollama::OllamaProvider;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    // Create Ollama provider (no API key, local server).
    let provider = OllamaProvider::from_config(
        "http://localhost:11434".to_string(),
        "llama3.2".to_string(),
        None,
    )?;

    // Check availability.
    if !provider.is_available().await {
        eprintln!("Ollama not available. Is it running? Try: ollama serve");
        std::process::exit(1);
    }

    println!("Connected to Ollama. Type a message (Ctrl+C to quit).\n");

    let mut history: Vec<ContextEntry> = Vec::new();
    let stdin = std::io::stdin();

    loop {
        // Read user input.
        print!("> ");
        use std::io::Write;
        std::io::stdout().flush()?;

        let mut input = String::new();
        if stdin.read_line(&mut input)? == 0 {
            break;
        }
        let input = input.trim().to_string();
        if input.is_empty() {
            continue;
        }

        // Build context with conversation history.
        let mut context = Context::new(&input);
        context.system_prompt = "You are a helpful assistant.".to_string();
        context.history = history.clone();

        // Send to provider.
        let response = provider.complete(&context).await?;
        println!("\n{}\n", response.text);

        // Append to history for multi-turn conversation.
        history.push(ContextEntry {
            role: "user".to_string(),
            content: input,
        });
        history.push(ContextEntry {
            role: "assistant".to_string(),
            content: response.text.clone(),
        });

        if let Some(model) = &response.metadata.model {
            eprintln!(
                "[{} | {} tokens | {}ms]",
                model,
                response.metadata.tokens_used.unwrap_or(0),
                response.metadata.processing_time_ms,
            );
        }
    }

    Ok(())
}
