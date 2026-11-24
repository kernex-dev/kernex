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
/// own location (its parent resolved, its name as written). A process that
/// can repoint one of them changes what `path` resolves to next time.
pub(crate) fn links_on(path: &Path) -> Vec<PathBuf> {
    let (mut links, mut budget) = (Vec::new(), MAX_LINKS);
    collect_links(path, &mut budget, &mut links);
    links
}

fn collect_links(path: &Path, budget: &mut u8, links: &mut Vec<PathBuf>) {
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            // As `resolve` does: a component that does not exist is taken
            // as written, so `..` after it goes back lexically.
            Component::ParentDir => {
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
            return;
        }
        *budget -= 1;
        collect_links(&parent.join(target), budget, links);
        current = resolve(&current);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn links_on_names_every_link_where_it_sits() {
        let dir = tempfile::tempdir().unwrap();
        let base = resolve(dir.path());
        std::fs::create_dir_all(base.join("real/inner")).unwrap();
        std::os::unix::fs::symlink(base.join("real"), base.join("a")).unwrap();
        // A chain: `b` points at `a`, which points at `real`.
        std::os::unix::fs::symlink(base.join("a"), base.join("b")).unwrap();
        std::os::unix::fs::symlink("inner", base.join("real/rel")).unwrap();

        assert_eq!(links_on(&base.join("a/inner")), [base.join("a")]);
        assert_eq!(
            links_on(&base.join("b/rel/x")),
            [base.join("b"), base.join("a"), base.join("real/rel")]
        );
        assert!(links_on(&base.join("real/inner")).is_empty());
        // `..` after a component that does not exist goes back as written.
        assert_eq!(links_on(&base.join("missing/../a/inner")), [base.join("a")]);
    }

    #[test]
    fn a_long_chain_is_followed_to_its_end() {
        let dir = tempfile::tempdir().unwrap();
        let base = resolve(dir.path());
        std::fs::create_dir(base.join("real")).unwrap();
        let mut next = base.join("real");
        for i in (0..30).rev() {
            let link = base.join(format!("l{i}"));
            std::os::unix::fs::symlink(&next, &link).unwrap();
            next = link;
        }
        assert_eq!(links_on(&base.join("l0/ws")).len(), 30);
        assert_eq!(resolve(&base.join("l0/ws")), base.join("real/ws"));
    }

    #[test]
    fn a_link_through_itself_costs_a_bounded_walk() {
        let dir = tempfile::tempdir().unwrap();
        let base = resolve(dir.path());
        std::os::unix::fs::symlink("a/../a", base.join("a")).unwrap();
        let start = std::time::Instant::now();
        let _ = resolve(&base.join("a/x"));
        let _ = links_on(&base.join("a/x"));
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
    }
}
