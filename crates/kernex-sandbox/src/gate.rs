//! Checks that decide whether a policy may be used to spawn at all.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::process::Command;

use crate::policy::{Policy, WorkspaceRejection};
use crate::{enforcement_required, SandboxProfile, SandboxScope};

/// A program path that cannot exist, used to make a refused spawn fail.
const REFUSED_PROGRAM: &str = "/nonexistent/kernex-sandbox-refused";

static WORKSPACE_WARNING_EMITTED: AtomicBool = AtomicBool::new(false);

/// Refuse a policy that could not be built from its inputs. A scope or a
/// `blocked_paths` entry that is not absolute cannot be protected, so no
/// subprocess runs under it.
pub(crate) fn check_policy(policy: &Policy) -> std::io::Result<()> {
    match policy.invalid() {
        Some(problem) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("sandbox policy cannot be built: {problem}"),
        )),
        None => Ok(()),
    }
}

/// The command returned by the infallible wrapper when the policy is invalid:
/// spawning it fails, so nothing runs unprotected.
pub(crate) fn refused_command(problem: &str) -> Command {
    eprintln!(
        "kernex-sandbox: ERROR: sandbox policy cannot be built ({problem}); \
         refusing to run the subprocess."
    );
    tracing::error!("sandbox policy cannot be built: {problem}");
    Command::new(REFUSED_PROGRAM)
}

/// Warn, once per process, that the workspace is not writable.
pub(crate) fn warn_workspace_rejected_once(workspace: &Path, rejection: WorkspaceRejection) {
    if WORKSPACE_WARNING_EMITTED.swap(true, Ordering::Relaxed) {
        return;
    }
    let reason = rejection.reason();
    eprintln!(
        "kernex-sandbox: WARNING: workspace {} is not accepted as a writable root \
         because {reason}; tool writes are limited to the temp directories.",
        workspace.display()
    );
    tracing::warn!(
        workspace = %workspace.display(),
        "workspace not accepted as a writable root because {reason}"
    );
}

/// Refuse a rejected workspace when enforcement is required; warn otherwise.
pub(crate) fn check_workspace(
    policy: &Policy,
    scope: &SandboxScope,
    profile: &SandboxProfile,
) -> std::io::Result<()> {
    let Some(rejection) = policy.workspace_rejection() else {
        return Ok(());
    };
    if enforcement_required(profile) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "workspace {} cannot be sandboxed because {}",
                scope.workspace.display(),
                rejection.reason()
            ),
        ));
    }
    warn_workspace_rejected_once(&scope.workspace, rejection);
    Ok(())
}
