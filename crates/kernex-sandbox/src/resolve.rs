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
/// component at a time: a symlink is read and followed from the directory
/// it sits in, a dangling symlink to the place it would create, and
/// components that do not exist are appended as written. When a path cannot
/// be resolved completely (too many links, or a component that cannot be
/// looked up) the result is what was resolved before that point; use
/// [`resolve_checked`] to tell.
pub(crate) fn resolve(path: &Path) -> PathBuf {
    match resolve_checked(path) {
        Ok(resolved) | Err(resolved) => resolved,
    }
}

/// [`resolve`], or `Err` with the partial result when the path could not be
/// resolved completely. A check must refuse such a path: what the kernel
/// opens through it is unknown.
pub(crate) fn resolve_checked(path: &Path) -> Result<PathBuf, PathBuf> {
    if let Ok(real) = std::fs::canonicalize(path) {
        return Ok(outside_data_volume(real));
    }
    let mut walk = Walk::new(false);
    let resolved = walk.run(PathBuf::new(), path);
    if walk.complete {
        Ok(stored_case(&resolved))
    } else {
        Err(outside_data_volume(resolved))
    }
}

/// What [`links_on`] found on a path.
pub(crate) struct Links {
    /// Where the symlinks met sit, each given by its own location, and the
    /// paths a `..` steps out of, whether they exist or not.
    pub(crate) found: Vec<PathBuf>,
    /// The component the walk stopped at because it could not be looked up
    /// (a directory that cannot be searched, a name that is too long).
    pub(crate) stopped_at: Option<PathBuf>,
}

/// The links and `..` steps met while resolving `path`. A process that can
/// repoint one of those links, or put a link at one of those paths, changes
/// what `path` resolves to next time. `None` when `path` goes through more
/// links than can be followed.
pub(crate) fn links_on(path: &Path) -> Option<Links> {
    let mut walk = Walk::new(true);
    walk.run(PathBuf::new(), path);
    if walk.exhausted {
        return None;
    }
    let mut unique = std::collections::HashSet::new();
    walk.seen.retain(|p| unique.insert(p.clone()));
    Some(Links {
        found: walk.seen.iter().map(|p| located(p)).collect(),
        stopped_at: walk.stopped_at.as_deref().map(located),
    })
}

/// `path` under the stored case of its directory, its own name as written.
fn located(path: &Path) -> PathBuf {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => stored_case(parent).join(name),
        _ => path.to_path_buf(),
    }
}

/// One walk over a path and the link targets it meets: one `lstat` per
/// component and one read per link, each target continued from the
/// directory the link sits in.
struct Walk {
    budget: u8,
    complete: bool,
    exhausted: bool,
    record: bool,
    seen: Vec<PathBuf>,
    stopped_at: Option<PathBuf>,
}

impl Walk {
    fn new(record: bool) -> Self {
        Self {
            budget: MAX_LINKS,
            complete: true,
            exhausted: false,
            record,
            seen: Vec::new(),
            stopped_at: None,
        }
    }

    /// Walk `path` from `current`, a path with no link left in it.
    fn run(&mut self, mut current: PathBuf, path: &Path) -> PathBuf {
        for component in path.components() {
            match component {
                Component::Prefix(_) | Component::RootDir => current.push(component.as_os_str()),
                Component::CurDir => {}
                Component::ParentDir => {
                    if self.record && self.complete && current.parent().is_some() {
                        self.seen.push(current.clone());
                    }
                    current.pop();
                }
                Component::Normal(name) => {
                    let parent = current.clone();
                    current.push(name);
                    if self.complete {
                        current = self.step(parent, current);
                    }
                }
            }
        }
        current
    }

    fn step(&mut self, parent: PathBuf, current: PathBuf) -> PathBuf {
        match current.symlink_metadata() {
            Err(e) if is_absent(&e) => current,
            Ok(meta) if meta.is_symlink() => {
                if self.record {
                    self.seen.push(current.clone());
                }
                if self.budget == 0 {
                    self.complete = false;
                    self.exhausted = true;
                    return current;
                }
                self.budget -= 1;
                match std::fs::read_link(&current) {
                    Ok(target) => self.run(parent, &target),
                    Err(_) => {
                        self.complete = false;
                        current
                    }
                }
            }
            Ok(_) => current,
            Err(_) => {
                self.complete = false;
                self.stopped_at = Some(current.clone());
                current
            }
        }
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

/// `path`, which has no link in it, with its existing part under the name
/// the filesystem stores (a case-insensitive volume keeps the caller's
/// spelling otherwise). One canonicalize for the whole path.
fn stored_case(path: &Path) -> PathBuf {
    for existing in path.ancestors() {
        if let Ok(real) = std::fs::canonicalize(existing) {
            let rest = path.strip_prefix(existing).unwrap_or(Path::new(""));
            // Joining an empty rest would add a trailing `/`, which a
            // Seatbelt `literal` does not match.
            let whole = if rest.as_os_str().is_empty() {
                real
            } else {
                real.join(rest)
            };
            return outside_data_volume(whole);
        }
    }
    outside_data_volume(path.to_path_buf())
}

/// macOS mounts the data volume at `/System/Volumes/Data` and firmlinks it
/// into `/`, so both spellings name the same file and neither is a symlink.
fn outside_data_volume(path: PathBuf) -> PathBuf {
    #[cfg(target_os = "macos")]
    if let Ok(rest) = path.strip_prefix("/System/Volumes/Data") {
        return Path::new("/").join(rest);
    }
    path
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn links(path: &Path) -> Vec<PathBuf> {
        links_on(path).unwrap().found
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
        assert!(links_on(&base.join("l0/ws")).is_none());
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
            assert!(links_on(&base.join(path)).is_none(), "{path}");
        }
        assert!(resolve_checked(&base.join("h/f")).is_ok());
    }

    #[test]
    fn a_component_that_cannot_be_looked_up_is_not_resolved() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let base = resolve(dir.path());
        let long = base.join("n".repeat(300)).join("f");
        assert!(resolve_checked(&long).is_err());
        // A directory that cannot be searched: checks refuse what is below
        // it, while walking a named path for links just stops there.
        let locked = base.join("locked");
        std::fs::create_dir_all(locked.join("sub")).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let below = locked.join("sub/f");
        // Root searches it anyway; there is nothing to check then.
        let enforced = std::fs::symlink_metadata(locked.join("sub")).is_err();
        let checked = resolve_checked(&below);
        let found = links_on(&below).map(|l| (l.found, l.stopped_at));
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        if enforced {
            assert!(checked.is_err());
            assert_eq!(found, Some((Vec::new(), Some(locked.join("sub")))));
        }
    }

    #[test]
    fn a_resolved_directory_has_no_trailing_separator() {
        let dir = tempfile::tempdir().unwrap();
        let base = resolve(dir.path());
        std::fs::write(base.join("file"), "x").unwrap();
        for path in ["nope/..", "file/.."] {
            let resolved = resolve_checked(&base.join(path)).unwrap();
            assert_eq!(resolved.as_os_str(), base.as_os_str(), "{path}");
        }
    }

    #[test]
    fn a_deep_chain_costs_a_walk_proportional_to_its_parts() {
        let dir = tempfile::tempdir().unwrap();
        let base = resolve(dir.path());
        let deep = base.join("d/".repeat(300));
        std::fs::create_dir_all(deep.join("real")).unwrap();
        for i in 0..39 {
            let next = if i == 38 {
                "real".to_string()
            } else {
                format!("L{}", i + 1)
            };
            std::os::unix::fs::symlink(next, deep.join(format!("L{i}"))).unwrap();
        }
        std::os::unix::fs::symlink(deep.join("L0"), base.join("deep")).unwrap();
        let start = std::time::Instant::now();
        assert_eq!(
            resolve_checked(&base.join("deep/x")),
            Ok(deep.join("real/x"))
        );
        assert_eq!(links(&base.join("deep/x")).len(), 40);
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_dotdot_is_named_under_the_stored_case() {
        let dir = tempfile::tempdir().unwrap();
        let base = resolve(dir.path());
        let name = base.file_name().unwrap().to_string_lossy().to_uppercase();
        let shouted = base.with_file_name(name);
        assert_eq!(links(&shouted.join("missing/../x")), [base.join("missing")]);
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
        assert!(links_on(&base.join("X1/x")).is_none());
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
    }
}
