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

/// The paths a policy is built from, each resolved once: what it resolves
/// to, the links and `..` steps on the way, and whether it can be trusted,
/// all from the same walk, so a change between two walks cannot split them.
#[derive(Default)]
pub(crate) struct Named {
    /// Where the symlinks met sit, each given by its own location, and the
    /// paths a `..` steps out of, whether they exist or not. A process that
    /// can repoint one of those links, or put a link at one of those paths,
    /// changes what a path resolves to next time.
    pub(crate) links: Vec<PathBuf>,
    /// Why a path cannot be resolved, for the first one that cannot.
    pub(crate) problem: Option<String>,
}

impl Named {
    /// Resolve `path` as [`resolve`] does, and record what was met.
    pub(crate) fn resolve(&mut self, path: &Path) -> PathBuf {
        let mut walk = Walk::new(true);
        let result = walk.run(PathBuf::new(), path);
        let why = if walk.exhausted {
            Some("it goes through more symlinks than can be followed".to_string())
        } else {
            // A stop the sandboxed process could have caused (its own
            // directory or link closed to lookups) would hide what is
            // below it. One at another user's directory it cannot cause.
            let stop = walk.stopped_at.as_deref();
            let ours = stop.filter(|stop| caused_by_us(stop, walk.stop_meta.as_ref()));
            ours.map(|stop| format!("{} cannot be looked up", stop.display()))
        };
        if self.problem.is_none() {
            self.problem = why.map(|why| format!("{} cannot be resolved: {why}", path.display()));
        }
        let mut unique = std::collections::HashSet::new();
        walk.seen.retain(|p| unique.insert(p.clone()));
        self.links.extend(walk.seen.iter().map(|p| located(p)));
        if walk.complete {
            stored_case(&result)
        } else {
            outside_data_volume(result)
        }
    }
}

/// Whether this process's user could have caused the walk to stop at
/// `stop`, by changing the mode or ACL of something it owns. `meta` is the
/// stop's own `lstat`, when the walk got it (a link that cannot be read).
/// Unknown counts as yes.
///
/// A stop it cannot have caused is one in a directory of another user's,
/// at an item of another user's, or below such a directory that it cannot
/// search. When the stop's own attributes could not be read and its
/// directory can be searched, the stop itself is what refuses, and its
/// owner is unknown.
fn caused_by_us(stop: &Path, meta: Option<&std::fs::Metadata>) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: `geteuid` takes no arguments, cannot fail and touches no
        // memory of ours.
        let us = unsafe { libc::geteuid() };
        let Some(parent) = stop.parent() else {
            return true;
        };
        if parent.symlink_metadata().map_or(true, |m| m.uid() == us) {
            return true;
        }
        match meta {
            Some(meta) => meta.uid() == us,
            None => searchable(parent),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (stop, meta);
        true
    }
}

/// Whether this process's user can search `dir`. Unknown counts as yes.
#[cfg(unix)]
fn searchable(dir: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(name) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else {
        return true;
    };
    // SAFETY: `name` is a valid NUL-terminated string that outlives the
    // call, which only reads it.
    let found =
        unsafe { libc::faccessat(libc::AT_FDCWD, name.as_ptr(), libc::X_OK, libc::AT_EACCESS) };
    found == 0
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
    stop_meta: Option<std::fs::Metadata>,
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
            stop_meta: None,
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
                        self.stopped_at = Some(current.clone());
                        self.stop_meta = Some(meta);
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

    fn walked(path: &Path) -> Named {
        let mut named = Named::default();
        named.resolve(path);
        named
    }

    fn links(path: &Path) -> Vec<PathBuf> {
        let named = walked(path);
        assert_eq!(named.problem, None);
        named.links
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
        assert!(walked(&base.join("l0/ws")).problem.is_some());
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
            assert!(walked(&base.join(path)).problem.is_some(), "{path}");
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
        let problem = walked(&below).problem;
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        if enforced {
            assert!(checked.is_err());
            // Closed by its own user, who could open it again.
            assert!(problem.unwrap().contains("cannot be looked up"));
        }
    }

    #[test]
    fn a_stop_at_another_users_directory_is_accepted() {
        // A directory of root's that this user cannot search, if there is
        // one and this is not root.
        let closed = ["/private/var/root", "/root"]
            .into_iter()
            .map(Path::new)
            .find(|d| d.exists() && std::fs::read_dir(d).is_err());
        let Some(closed) = closed else {
            return;
        };
        let named = walked(&closed.join("x/y"));
        assert_eq!(named.problem, None);
    }

    #[test]
    fn a_stop_at_this_users_item_in_another_users_directory_is_a_problem() {
        // macOS lets an ACL hide an item's attributes, even from its owner,
        // while lookups through it still work. Placed directly in root's
        // world-writable temp directory, both the item and the link in it.
        let Ok(dir) = tempfile::Builder::new()
            .prefix("kx-acl-")
            .tempdir_in("/private/tmp")
        else {
            return;
        };
        let base = dir.path().to_path_buf();
        std::fs::create_dir(base.join("real")).unwrap();
        let link = dir.path().with_extension("link");
        std::os::unix::fs::symlink(base.join("real"), &link).unwrap();
        std::os::unix::fs::symlink(base.join("real"), base.join("l")).unwrap();
        let acl = |args: &[&str], item: &Path| {
            std::process::Command::new("chmod")
                .args(args)
                .arg(item)
                .status()
                .is_ok_and(|s| s.success())
        };
        let hide = ["+a", "everyone deny readattr"];
        let found = if acl(&hide, &base) && base.symlink_metadata().is_err() {
            let through_dir = walked(&base.join("l/secret")).problem;
            acl(&["-N"], &base);
            let hidden = acl(&["-h", hide[0], hide[1]], &link);
            let through_link = (hidden && link.symlink_metadata().is_err())
                .then(|| walked(&link.join("secret")).problem);
            acl(&["-h", "-N"], &link);
            Some((through_dir, through_link))
        } else {
            None
        };
        let _ = std::fs::remove_file(&link);
        let Some((through_dir, through_link)) = found else {
            return;
        };
        assert!(through_dir.is_some_and(|p| p.contains("cannot be looked up")));
        if let Some(through_link) = through_link {
            assert!(through_link.is_some_and(|p| p.contains("cannot be looked up")));
        }
    }

    #[test]
    fn a_link_that_cannot_be_read_is_a_stop() {
        let dir = tempfile::tempdir().unwrap();
        let base = resolve(dir.path());
        std::fs::create_dir(base.join("real")).unwrap();
        std::os::unix::fs::symlink(base.join("real"), base.join("l")).unwrap();
        // macOS lets a link's own mode deny reading it while lookups still
        // follow it.
        let closed = std::process::Command::new("chmod")
            .args(["-h", "000"])
            .arg(base.join("l"))
            .status()
            .is_ok_and(|s| s.success());
        if !closed || std::fs::read_link(base.join("l")).is_ok() {
            return;
        }
        assert!(walked(&base.join("l/secret")).problem.is_some());
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
        assert!(walked(&base.join("X1/x")).problem.is_some());
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
    }
}
