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
/// component at a time: an existing directory or file is taken under its
/// real name, a symlink is read and followed, a dangling symlink to the
/// place it would create, and components that do not exist are appended as
/// written. When a path cannot be resolved completely (too many links, or a
/// component that cannot be looked up) the result is what was resolved
/// before that point; use [`resolve_checked`] to tell.
pub(crate) fn resolve(path: &Path) -> PathBuf {
    match resolve_checked(path) {
        Ok(resolved) | Err(resolved) => resolved,
    }
}

/// [`resolve`], or `Err` with the partial result when the path could not be
/// resolved completely. A check must refuse such a path: what the kernel
/// opens through it is unknown.
pub(crate) fn resolve_checked(path: &Path) -> Result<PathBuf, PathBuf> {
    let (resolved, complete) = match std::fs::canonicalize(path) {
        Ok(real) => (real, true),
        Err(_) => {
            let (mut budget, mut complete) = (MAX_LINKS, true);
            let resolved = resolve_within(path, &mut budget, &mut complete);
            (resolved, complete)
        }
    };
    // macOS mounts the data volume at this path and firmlinks it into `/`,
    // so both spellings name the same file and neither is a symlink.
    #[cfg(target_os = "macos")]
    let resolved = match resolved.strip_prefix("/System/Volumes/Data") {
        Ok(rest) => Path::new("/").join(rest),
        Err(_) => resolved,
    };
    if complete {
        Ok(resolved)
    } else {
        Err(resolved)
    }
}

/// Whether a lookup error means only that the path does not exist.
fn is_absent(e: &std::io::Error) -> bool {
    #[cfg(unix)]
    if e.raw_os_error() == Some(libc::ENOTDIR) {
        return true;
    }
    e.kind() == std::io::ErrorKind::NotFound
}

/// One `lstat` per component and one read per link, so the work grows with
/// the length of the path and its link targets, never with how the kernel
/// would expand a link.
fn resolve_within(path: &Path, budget: &mut u8, complete: &mut bool) -> PathBuf {
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
                if !*complete {
                    continue;
                }
                match current.symlink_metadata() {
                    Err(e) if is_absent(&e) => {}
                    Err(_) => *complete = false,
                    Ok(meta) if meta.is_symlink() => match std::fs::read_link(&current) {
                        Ok(target) if *budget > 0 => {
                            *budget -= 1;
                            current = resolve_within(&parent.join(target), budget, complete);
                        }
                        _ => *complete = false,
                    },
                    // No link up to here, so this is cheap; it gives the
                    // name as stored.
                    Ok(_) => match std::fs::canonicalize(&current) {
                        Ok(real) => current = real,
                        Err(_) => *complete = false,
                    },
                }
            }
        }
    }
    current
}

/// Where the symlinks met while resolving `path` sit, each given by its
/// own location (its parent resolved, its name as written), and the paths
/// a `..` steps out of, whether they exist or not. A process that can
/// repoint one of those links, or put a link at one of those paths, changes
/// what `path` resolves to next time. `None` when the path cannot be
/// resolved completely.
pub(crate) fn links_on(path: &Path) -> Option<Vec<PathBuf>> {
    let (mut links, mut budget) = (Vec::new(), MAX_LINKS);
    collect_links(path, &mut budget, &mut links).then_some(links)
}

/// Returns false when `path` cannot be resolved completely.
fn collect_links(path: &Path, budget: &mut u8, links: &mut Vec<PathBuf>) -> bool {
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            // As `resolve` does: a component that does not exist is taken
            // as written, so `..` after it goes back lexically.
            Component::ParentDir => {
                if current.parent().is_some() {
                    links.push(current.clone());
                }
                current.pop();
                continue;
            }
            Component::CurDir => continue,
            _ => current.push(component.as_os_str()),
        }
        let meta = match current.symlink_metadata() {
            Ok(meta) => meta,
            Err(e) if is_absent(&e) => continue,
            Err(_) => return false,
        };
        if !meta.is_symlink() {
            continue;
        }
        let Ok(target) = std::fs::read_link(&current) else {
            return false;
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
        match resolve_checked(&current) {
            Ok(resolved) => current = resolved,
            Err(_) => return false,
        }
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
        // `..` after a component that does not exist goes back as written,
        // and the missing path is named: a link created there would change
        // where the `..` lands.
        assert_eq!(
            links(&base.join("missing/../a/inner")),
            [base.join("missing"), base.join("a")]
        );
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
    fn a_path_through_a_loop_is_not_resolved() {
        let dir = tempfile::tempdir().unwrap();
        let base = resolve(dir.path());
        std::fs::create_dir(base.join("outside")).unwrap();
        std::os::unix::fs::symlink("loop", base.join("loop")).unwrap();
        std::os::unix::fs::symlink(base.join("outside"), base.join("h")).unwrap();
        // With and without a `..` in the path itself.
        std::os::unix::fs::symlink("loop/..", base.join("x")).unwrap();
        for path in ["loop/../h/f", "x/h/f"] {
            assert!(resolve_checked(&base.join(path)).is_err(), "{path}");
            assert_eq!(links_on(&base.join(path)), None, "{path}");
        }
        assert!(resolve_checked(&base.join("h/f")).is_ok());
    }

    #[test]
    fn a_component_that_cannot_be_looked_up_is_not_resolved() {
        let dir = tempfile::tempdir().unwrap();
        let base = resolve(dir.path());
        let long = base.join("n".repeat(300)).join("f");
        assert!(resolve_checked(&long).is_err());
        assert_eq!(links_on(&long), None);
    }

    #[test]
    fn links_that_expand_through_loops_cost_a_bounded_walk() {
        let dir = tempfile::tempdir().unwrap();
        let base = resolve(dir.path());
        std::fs::create_dir(base.join("d")).unwrap();
        let link = |name: &str, target: String| {
            std::os::unix::fs::symlink(target, base.join(name)).unwrap();
        };
        // Every step through `E` is a loop the kernel expands at length,
        // and every `X` walks through `E` two hundred times.
        link("E", "d/../".repeat(200) + "E");
        for i in 1..40 {
            link(
                &format!("X{i}"),
                format!("X{}", i + 1) + &"/E/..".repeat(200),
            );
        }
        link("X40", "X41/..".to_string() + &"/E/..".repeat(199));
        link("X41", "X41".to_string());
        let start = std::time::Instant::now();
        assert!(resolve_checked(&base.join("X1/x")).is_err());
        assert_eq!(links_on(&base.join("X1/x")), None);
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
    }
}
