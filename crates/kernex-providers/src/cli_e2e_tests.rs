//! The real Claude CLI under the real sandbox profile.
//!
//! Ignored by default: it calls the model and uses the caller's Claude
//! account. Run it by hand with a token from `claude setup-token` or an API
//! key:
//!
//! ```text
//! CLAUDE_CODE_OAUTH_TOKEN=... cargo test -p kernex-providers --lib \
//!     real_claude_cli -- --ignored --nocapture --test-threads=1
//! ```
//!
//! It makes three short calls with the `haiku` model and prints what it saw.

use std::path::Path;

use kernex_core::context::Context;
use kernex_core::traits::Provider;

use crate::claude_code::ClaudeCodeProvider;
use crate::SandboxDirs;

fn provider(workspace: &Path, data_dir: &Path) -> ClaudeCodeProvider {
    let token = std::env::var("CLAUDE_CODE_OAUTH_TOKEN")
        .ok()
        .filter(|t| !t.is_empty());
    ClaudeCodeProvider::from_config(
        4,
        vec![],
        300,
        Some(workspace.to_path_buf()),
        0,
        "haiku".into(),
        token,
    )
    .with_sandbox_dirs(SandboxDirs::new().with_data_dir(data_dir))
}

async fn ask(p: &ClaudeCodeProvider, prompt: &str, tools: Option<Vec<String>>) -> String {
    let mut ctx = Context::new(prompt);
    ctx.allowed_tools = tools;
    match p.complete(&ctx).await {
        Ok(r) => r.text,
        Err(e) => format!("ERROR: {e}"),
    }
}

#[tokio::test]
#[ignore = "calls the real Claude CLI and the model"]
async fn real_claude_cli_end_to_end() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("ws");
    let data_dir = root.path().join("state");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(
        workspace.join("CLAUDE.md"),
        "The codeword for this project is PLUM.\n",
    )
    .unwrap();
    std::fs::write(workspace.join("input.txt"), "kernex-e2e-marker\n").unwrap();
    let p = provider(&workspace, &data_dir);

    // 1. No tool list, no bypass: reading is allowed, writing is refused.
    let read = ask(
        &p,
        "Read input.txt and reply with its exact contents only. Then reply \
         with the project codeword if you know one, else NONE.",
        None,
    )
    .await;
    let blocked = ask(
        &p,
        "Create a file named blocked.txt containing the word hi. Reply DONE \
         or FAILED.",
        None,
    )
    .await;
    // 2. A list that names Write: the same request succeeds.
    let allowed = ask(
        &p,
        "Create a file named allowed.txt containing the word hi. Reply DONE \
         or FAILED.",
        Some(vec!["Write".into()]),
    )
    .await;

    let config_dir = p.cli_config_dir();
    let mut state: Vec<String> = std::fs::read_dir(&config_dir)
        .map(|d| {
            d.filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    state.sort();

    println!("--- read (expect the marker; codeword shows whether CLAUDE.md was loaded)\n{read}");
    println!("--- write without permission (expect FAILED)\n{blocked}");
    println!("--- write with Write listed (expect DONE)\n{allowed}");
    println!(
        "--- blocked.txt exists: {}",
        workspace.join("blocked.txt").exists()
    );
    println!(
        "--- allowed.txt exists: {}",
        workspace.join("allowed.txt").exists()
    );
    println!("--- CLI state in {}: {state:?}", config_dir.display());

    assert!(read.contains("kernex-e2e-marker"), "read failed: {read}");
    assert!(!workspace.join("blocked.txt").exists());
    assert!(
        workspace.join("allowed.txt").exists(),
        "write failed: {allowed}"
    );
}
