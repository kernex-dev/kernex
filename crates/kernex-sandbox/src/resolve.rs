//! Symlink resolution for the sandbox policy.

use std::path::{Component, Path, PathBuf};

/// Upper bound on chained symlinks followed by [`resolve`] and [`links_on`].
const MAX_LINK_DEPTH: u8 = 16;

/// Resolve symlinks in `path`, including when its last components do not
/// exist yet.
///
/// An existing path is canonicalized. Otherwise the path is walked one
/// component at a time: every component that exists is canonicalized, so a
/// symlinked parent of a file that is about to be created is followed, and a
/// dangling symlink is followed to the place it would create. Components that
/// do not exist are appended as written.
pub(crate) fn resolve(path: &Path) -> PathBuf {
    let resolved = resolve_at_depth(path, 0);
    // macOS mounts the data volume at this path and firmlinks it into `/`,
    // so both spellings name the same file and neither is a symlink.
    #[cfg(target_os = "macos")]
    if let Ok(rest) = resolved.strip_prefix("/System/Volumes/Data") {
        return Path::new("/").join(rest);
    }
    resolved
}

fn resolve_at_depth(path: &Path, depth: u8) -> PathBuf {
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
                    if depth < MAX_LINK_DEPTH {
                        current = resolve_at_depth(&parent.join(target), depth + 1);
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
    let mut links = Vec::new();
    collect_links(path, 0, &mut links);
    links
}

fn collect_links(path: &Path, depth: u8, links: &mut Vec<PathBuf>) {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        let Ok(target) = std::fs::read_link(&current) else {
            continue;
        };
        let parent = current.parent().map(resolve).unwrap_or_default();
        if let Some(name) = current.file_name() {
            links.push(parent.join(name));
        }
        if depth < MAX_LINK_DEPTH {
            collect_links(&parent.join(target), depth + 1, links);
        }
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
    }
}
