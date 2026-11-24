//! The paths the OS profile keeps in place inside the write roots.

use std::path::{Path, PathBuf};

use crate::policy::push_unique;

/// The guards and pins of a policy (see [`crate::policy::Policy::guards`]
/// and [`crate::policy::Policy::pins`]). `denied` are the protected paths;
/// `links` the links and `..` steps met on the paths the policy was built
/// from ([`crate::resolve::Named`]).
pub(crate) fn guards_and_pins<'a>(
    roots: &[PathBuf],
    denied: impl Iterator<Item = &'a Path>,
    links: &[PathBuf],
) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let guards = guard_dirs(roots, denied.filter_map(Path::parent));
    // A file root itself is left out, so it can be saved by renaming over
    // it (a link put in its place is ignored by the next policy); its
    // directory is pinned.
    let root_dirs = roots.iter().map(|r| {
        if r.is_file() {
            r.parent()
        } else {
            Some(r.as_path())
        }
    });
    let starts = root_dirs
        .flatten()
        .chain(links.iter().map(PathBuf::as_path));
    (guards, guard_dirs(roots, starts))
}

/// Every path from a write root down to one of `starts` that the root
/// contains.
fn guard_dirs<'a>(roots: &[PathBuf], starts: impl Iterator<Item = &'a Path>) -> Vec<PathBuf> {
    let mut guards = Vec::new();
    for start in starts {
        for root in roots {
            let mut dir = Some(start);
            while let Some(current) = dir {
                if !current.starts_with(root) {
                    break;
                }
                push_unique(&mut guards, current.to_path_buf());
                dir = current.parent();
            }
        }
    }
    guards
}
