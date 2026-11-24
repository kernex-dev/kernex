//! Symlink resolution for the sandbox policy.

use std::path::{Component, Path, PathBuf};

/// How many symlinks one call follows in all, more than the kernel follows
/// for one path. One budget per call keeps a link that points back through
/// itself from multiplying the work.
const MAX_LINKS: u8 = 40;

/// Resolve symlinks in `path`, including when its last components do not
/// exist yet.
///
/// An existing path is canonicalized. Otherwise the path is walked one
/// component at a time: every component that exists is canonicalized, so a
/// symlinked parent of a file that is about to be created is followed, and a
/// dangling symlink is followed to the place it would create. Components that
/// do not exist are appended as written.
pub(crate) fn resolve(path: &Path) -> PathBuf {
    let mut budget = MAX_LINKS;
    let resolved = resolve_within(path, &mut budget);
    // macOS mounts the data volume at this path and firmlinks it into `/`,
    // so both spellings name the same file and neither is a symlink.
    #[cfg(target_os = "macos")]
    if let Ok(rest) = resolved.strip_prefix("/System/Volumes/Data") {
        return Path::new("/").join(rest);
    }
    resolved
}

fn resolve_within(path: &Path, budget: &mut u8) -> PathBuf {
    if let Ok(real) = std::fs::canonicalize(path) {
        return real;
    }
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => current.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                current.pop();
            }
            Component::Normal(name) => {
                let parent = current.clone();
                current.push(name);
                if current.symlink_metadata().is_err() {
                    continue;
                }
                if let Ok(real) = std::fs::canonicalize(&current) {
                    current = real;
                } else if let Ok(target) = std::fs::read_link(&current) {
                    if *budget > 0 {
                        *budget -= 1;
                        current = resolve_within(&parent.join(target), budget);
                    }
                }
            }
        }
    }
    current
}

/// Where the symlinks met while resolving `path` sit, each given by its
/// own location (its parent resolved, its name as written), and the
/// directories a `..` steps out of. A process that can repoint one of those
/// links, or put a link in place of one of those directories, changes what
/// `path` resolves to next time. `None` when there are more links than can
/// be followed.
pub(crate) fn links_on(path: &Path) -> Option<Vec<PathBuf>> {
    let (mut links, mut budget) = (Vec::new(), MAX_LINKS);
    collect_links(path, &mut budget, &mut links).then_some(links)
}

/// Returns false when the budget ran out.
fn collect_links(path: &Path, budget: &mut u8, links: &mut Vec<PathBuf>) -> bool {
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            // As `resolve` does: a component that does not exist is taken
            // as written, so `..` after it goes back lexically.
            Component::ParentDir => {
                if current.symlink_metadata().is_ok() {
                    links.push(current.clone());
                }
                current.pop();
                continue;
            }
            Component::CurDir => continue,
            _ => current.push(component.as_os_str()),
        }
        let Ok(target) = std::fs::read_link(&current) else {
            continue;
        };
        let parent = current.parent().map(resolve).unwrap_or_default();
        if let Some(name) = current.file_name() {
            links.push(parent.join(name));
        }
        if *budget == 0 {
            return false;
        }
        *budget -= 1;
        if !collect_links(&parent.join(target), budget, links) {
            return false;
        }
        current = resolve(&current);
    }
    true
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn links(path: &Path) -> Vec<PathBuf> {
        links_on(path).unwrap()
    }

    #[test]
    fn links_on_names_every_link_where_it_sits() {
        let dir = tempfile::tempdir().unwrap();
        let base = resolve(dir.path());
        std::fs::create_dir_all(base.join("real/inner")).unwrap();
        std::os::unix::fs::symlink(base.join("real"), base.join("a")).unwrap();
        // A chain: `b` points at `a`, which points at `real`.
        std::os::unix::fs::symlink(base.join("a"), base.join("b")).unwrap();
        std::os::unix::fs::symlink("inner", base.join("real/rel")).unwrap();

        assert_eq!(links(&base.join("a/inner")), [base.join("a")]);
        assert_eq!(
            links(&base.join("b/rel/x")),
            [base.join("b"), base.join("a"), base.join("real/rel")]
        );
        assert!(links(&base.join("real/inner")).is_empty());
        // `..` after a component that does not exist goes back as written.
        assert_eq!(links(&base.join("missing/../a/inner")), [base.join("a")]);
        // The directory a `..` steps out of is named too: a link put in its
        // place would change where the `..` lands.
        assert_eq!(
            links(&base.join("a/../real/inner")),
            [base.join("a"), base.join("real")]
        );
    }

    fn chain(base: &Path, len: usize) {
        std::fs::create_dir(base.join("real")).unwrap();
        let mut next = base.join("real");
        for i in (0..len).rev() {
            let link = base.join(format!("l{i}"));
            std::os::unix::fs::symlink(&next, &link).unwrap();
            next = link;
        }
    }

    #[test]
    fn a_chain_within_the_budget_is_followed_to_its_end() {
        let dir = tempfile::tempdir().unwrap();
        let base = resolve(dir.path());
        // Longer than the kernel follows for one path, within the budget.
        chain(&base, 36);
        assert_eq!(links(&base.join("l0/ws")).len(), 36);
        assert_eq!(resolve(&base.join("l0/ws")), base.join("real/ws"));
    }

    #[test]
    fn a_chain_longer_than_the_budget_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let base = resolve(dir.path());
        chain(&base, 45);
        assert_eq!(links_on(&base.join("l0/ws")), None);
    }

    #[test]
    fn a_loop_does_not_hide_the_links_after_it() {
        let dir = tempfile::tempdir().unwrap();
        let base = resolve(dir.path());
        let outside = base.join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::os::unix::fs::symlink("loop", base.join("loop")).unwrap();
        std::os::unix::fs::symlink(&outside, base.join("h")).unwrap();
        assert_eq!(resolve(&base.join("loop/../h/x")), outside.join("x"));
    }

    #[test]
    fn a_link_through_itself_costs_a_bounded_walk() {
        let dir = tempfile::tempdir().unwrap();
        let base = resolve(dir.path());
        std::os::unix::fs::symlink("a/../a", base.join("a")).unwrap();
        let start = std::time::Instant::now();
        let _ = resolve(&base.join("a/x"));
        assert_eq!(links_on(&base.join("a/x")), None);
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
    }
}
