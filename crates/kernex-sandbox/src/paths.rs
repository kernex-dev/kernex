//! The per-user path lists the default policy is built from.

use std::path::{Path, PathBuf};

/// Credential directories/files under `$HOME` whose READS are denied to
/// sandboxed subprocesses (the credential read-deny list).
///
/// Reads stay broad everywhere else; only these high-value secret stores are
/// blocked, so an agent that is prompted (or tricked) into exfiltrating SSH
/// keys, cloud credentials, auth tokens or shell history is stopped at the OS
/// layer. The list is relative-suffix joined onto the resolved `home`.
///
/// An entry that is itself listed in `SandboxProfile::allowed_paths` is left
/// out of the policy: a host that runs an agent CLI as its provider lists
/// that CLI's own state directory there. Listing a directory above an entry
/// lifts nothing.
///
/// Project `.env` files are not on the list: tests and dev servers read
/// them. A host that wants them unreadable passes them in
/// `SandboxProfile::blocked_paths`.
pub fn credential_read_deny_dirs(home: &Path) -> Vec<PathBuf> {
    [
        ".ssh",
        ".aws",
        ".gnupg",
        ".kube",
        ".docker",
        ".netrc",
        ".npmrc",
        ".git-credentials",
        ".cargo/credentials",
        ".cargo/credentials.toml",
        ".config/git/credentials",
        ".config/gh",
        ".config/gcloud",
        ".config/google-chrome",
        ".mozilla",
        // Agent CLI state: session tokens and transcripts.
        ".claude",
        ".claude.json",
        ".codex",
        ".config/opencode",
        // Shell and REPL history: pasted secrets end up here.
        ".bash_history",
        ".zsh_history",
        ".zsh_sessions",
        ".sh_history",
        ".local/share/fish/fish_history",
        ".python_history",
        ".node_repl_history",
        ".psql_history",
        ".mysql_history",
        ".sqlite_history",
        // macOS browser/credential stores
        "Library/Application Support/Google/Chrome",
        "Library/Application Support/Firefox",
        "Library/Keychains",
    ]
    .iter()
    .map(|p| home.join(p))
    .collect()
}

/// Directories whose WRITES are allowed under the default lockdown posture
/// (writes are otherwise denied outside the workspace and the temp dirs).
///
/// Covers the per-user state/cache dirs that real toolchains write to during
/// normal operation (cargo, rustup, npm/yarn/pnpm, deno, bun, the XDG and
/// macOS cache trees). The programs and startup files inside them stay
/// read-only, so a tool call cannot replace something the host runs later.
/// `~/.rustup` is not on the list for that reason: it holds the compiler
/// itself, so a toolchain cannot be installed from inside the sandbox.
/// `~/.local/share` is not on it either: installers put programs there that
/// run later outside the sandbox (the Claude CLI, whose `~/.local/bin` entry
/// is a link into it, and the tools of uv, pipx and mise).
///
/// **Fallback tuning point:** if a real toolchain writes somewhere outside
/// this list and breaks, the fix is to extend this list or to pass the
/// directory in `SandboxProfile::allowed_paths`. CI cannot exercise every
/// toolchain, so this is the most likely place to need a follow-up.
pub fn write_allow_dirs(home: &Path) -> Vec<PathBuf> {
    [
        ".cache",
        ".cargo",
        ".npm",
        ".yarn",
        ".pnpm-store",
        ".deno",
        ".bun",
        ".local/state",
        ".gradle",
        ".m2",
        ".gem",
        "Library/Caches",
        "Library/Developer",
    ]
    .iter()
    .map(|p| home.join(p))
    .collect()
}

/// Installed programs and startup files that stay read-only even though
/// their parent is a write root: whatever is written there runs later,
/// outside the sandbox, with the user's full rights.
///
/// The list covers the well-known locations. A cache directory can hold
/// other files a tool trusts on its next run; that residue is the price of
/// letting package managers work inside the sandbox.
pub(crate) fn toolchain_exec_paths(home: &Path) -> Vec<PathBuf> {
    [
        ".cargo/bin",
        // Sourced by the shell startup files rustup installs.
        ".cargo/env",
        ".cargo/env.fish",
        ".cargo/env.nu",
        ".cargo/config",
        ".cargo/config.toml",
        ".bun/bin",
        ".deno/bin",
        ".local/bin",
        ".yarn/bin",
        ".gem/bin",
        ".gradle/init.d",
        ".gradle/gradle.properties",
        ".m2/settings.xml",
    ]
    .iter()
    .map(|p| home.join(p))
    .collect()
}
