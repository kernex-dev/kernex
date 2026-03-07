//! The simplest possible Kernex agent.
//!
//! This example sends one message to a local Ollama server and prints the response.
//! No memory, no skills, no conversation history - just a single request.
//!
//! Prerequisites:
//!   1. Install Ollama: https://ollama.com
//!   2. Pull a model: `ollama pull llama3.2`
//!   3. Start the server: `ollama serve` (in a separate terminal)
//!
//! Run:
//!   cargo run --example hello_world
//!
//! Expected output:
//!
//! ```text
//! Connected to Ollama. Type a message:
//! > What is Rust?
//! (AI response about Rust)
//! ```

use kernex_core::context::Context;
use kernex_core::traits::Provider;
use kernex_providers::ollama::OllamaProvider;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Step 1: Create an Ollama provider.
    // This connects to the local Ollama server at http://localhost:11434.
    // The model "llama3.2" must be pulled first: `ollama pull llama3.2`
    let provider = OllamaProvider::from_config(
        "http://localhost:11434".to_string(),
        "llama3.2".to_string(),
        None, // No system prompt for this simple example
    )?;

    // Step 2: Check if Ollama is available.
    // If not, print a helpful error message.
    if !provider.is_available().await {
        eprintln!("Error: Ollama not available.");
        eprintln!();
        eprintln!("To fix this:");
        eprintln!("  1. Install Ollama: https://ollama.com");
        eprintln!("  2. Pull a model: ollama pull llama3.2");
        eprintln!("  3. Start the server: ollama serve");
        std::process::exit(1);
    }

    println!("Connected to Ollama. Type a message:");

    // Step 3: Read user input.
    print!("> ");
    use std::io::Write;
    std::io::stdout().flush()?;

    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let input = input.trim();

    if input.is_empty() {
        println!("No input provided. Exiting.");
        return Ok(());
    }

    // Step 4: Create a context with the user's message.
    // Context is what gets sent to the AI provider.
    let mut context = Context::new(input);
    context.system_prompt = "You are a helpful assistant. Keep responses concise.".to_string();

    // Step 5: Send the context to the provider and get a response.
    let response = provider.complete(&context).await?;

    // Step 6: Print the response.
    println!("\n{}", response.text);

    // Optional: Show metadata (model used, token count, processing time).
    if let Some(model) = &response.metadata.model {
        eprintln!(
            "\n[Model: {} | Tokens: {} | Time: {}ms]",
            model,
            response.metadata.tokens_used.unwrap_or(0),
            response.metadata.processing_time_ms,
        );
    }

    Ok(())
}
