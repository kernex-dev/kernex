//! The paths the OS profile keeps in place inside the write roots.

use std::path::{Path, PathBuf};

use crate::policy::push_unique;
use crate::resolve::links_on;

/// The guards and pins of a policy (see [`crate::policy::Policy::guards`]
/// and [`crate::policy::Policy::pins`]), and an error when one of `named`
/// goes through more symlinks than can be followed.
///
/// `denied` are the protected paths; `named` are the paths the policy was
/// built from, as the host gave them.
pub(crate) fn guards_and_pins<'a>(
    roots: &[PathBuf],
    denied: impl Iterator<Item = &'a Path>,
    named: &[PathBuf],
) -> (Vec<PathBuf>, Vec<PathBuf>, Option<String>) {
    let guards = guard_dirs(roots, denied.filter_map(Path::parent));
    let (mut links, mut too_many) = (Vec::new(), None);
    for path in named.iter().filter(|p| p.is_absolute()) {
        match links_on(path) {
            Some(found) => links.extend(found),
            None => {
                let what = format!("{} has too many symlinks to follow", path.display());
                too_many = too_many.or(Some(what));
            }
        }
    }
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
    (guards, guard_dirs(roots, starts), too_many)
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
