//! Ending a sandboxed subprocess together with what it started.

/// Send `SIGKILL` to every process in the process group `group`.
///
/// For a child started in its own group (`Command::process_group(0)`) the
/// group id is the child's pid, so this also ends what the child started
/// (shell commands, servers) when the child is abandoned on a timeout. Call
/// it while the child is still unreaped, so the id cannot name a group that
/// came into being later. Errors (no such group) are ignored. Does nothing
/// on platforms without process groups.
pub fn kill_process_group(group: u32) {
    #[cfg(unix)]
    {
        let Ok(group) = libc::pid_t::try_from(group) else {
            return;
        };
        if group <= 1 {
            return;
        }
        // SAFETY: `killpg` takes two integers and only sends a signal; it
        // reads and writes no memory of this process. `group` is above 1, so
        // it can never mean "the caller's own group" (0) or init (1).
        unsafe {
            libc::killpg(group, libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    let _ = group;
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn the_whole_group_is_ended() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("grandchild");
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c")
            .arg(format!(
                "( sleep 2; touch '{}' ) & sleep 5",
                marker.display()
            ))
            .process_group(0)
            .kill_on_drop(true);
        let child = cmd.spawn().unwrap();
        let id = child.id().unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        kill_process_group(id);
        drop(child);
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert!(!marker.exists(), "the background job outlived its group");
    }
}
