//! Filesystem roots: the folders an agent on `helios-mcp` may read and
//! write.
//!
//! `helios-mcp --read-root DIR` (repeated) and `--write-root DIR` confine
//! every path an agent gives a tool: a path is read only under a read root
//! or the write root, and written only under the write root. Started
//! without a root, the server is not confined (as before roots existed).
//!
//! A path is checked once resolved: it must be absolute, holds no `..`,
//! and its deepest existing ancestor is resolved through the links on the
//! way (`canonicalize`), so a link leading out of a root is refused like
//! the folder it leads to. A link that leads nowhere is refused: writing
//! through it would create its target wherever it points.

use std::path::{Component, Path, PathBuf};

/// What a tool does with a path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
}

/// The roots granted to an agent; none at all when not confined.
#[derive(Clone, Debug, Default)]
pub struct Roots {
    read: Vec<PathBuf>,
    write: Option<PathBuf>,
}

impl Roots {
    /// No root: every path is allowed.
    pub fn unconfined() -> Self {
        Self::default()
    }

    /// Roots at these folders, each resolved now; a folder that does not
    /// exist is refused.
    pub fn new(read: &[PathBuf], write: Option<&Path>) -> Result<Self, String> {
        let root = |dir: &Path| {
            let resolved = dir
                .canonicalize()
                .map_err(|e| format!("root {}: {e}", dir.display()))?;
            if resolved.is_dir() {
                Ok(resolved)
            } else {
                Err(format!("root {}: not a folder", dir.display()))
            }
        };
        Ok(Self {
            read: read.iter().map(|d| root(d)).collect::<Result<_, _>>()?,
            write: write.map(root).transpose()?,
        })
    }

    /// Whether any root was granted.
    pub fn is_confined(&self) -> bool {
        !self.read.is_empty() || self.write.is_some()
    }

    /// `path` resolved, when `access` to it is granted; the refusal says
    /// which roots are.
    pub fn check(&self, path: &Path, access: Access) -> Result<PathBuf, String> {
        if !self.is_confined() {
            return Ok(path.to_path_buf());
        }
        let resolved = resolve(path)?;
        let under = |root: &PathBuf| resolved.starts_with(root);
        let granted = match access {
            Access::Read => self.read.iter().any(under) || self.write.as_ref().is_some_and(under),
            Access::Write => self.write.as_ref().is_some_and(under),
        };
        if granted {
            Ok(resolved)
        } else {
            Err(format!(
                "{} is outside the roots granted to this agent for {} ({})",
                path.display(),
                match access {
                    Access::Read => "reading",
                    Access::Write => "writing",
                },
                self.describe(access),
            ))
        }
    }

    fn describe(&self, access: Access) -> String {
        let list = |dirs: &[PathBuf]| {
            dirs.iter()
                .map(|d| d.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        };
        let write = self.write.as_slice();
        match access {
            Access::Write if write.is_empty() => "no write root".into(),
            Access::Write => format!("write: {}", list(write)),
            Access::Read => {
                let read: Vec<PathBuf> = self.read.iter().chain(write).cloned().collect();
                format!("read: {}", list(&read))
            }
        }
    }
}

/// `path` with its deepest existing ancestor resolved through links, the
/// rest appended as given.
fn resolve(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err(format!("{} is not an absolute path", path.display()));
    }
    if path.components().any(|c| c == Component::ParentDir) {
        return Err(format!("{}: `..` is refused in a path", path.display()));
    }
    let mut existing = path;
    let mut rest = Vec::new();
    loop {
        match existing.canonicalize() {
            Ok(mut resolved) => {
                resolved.extend(rest.iter().rev());
                return Ok(resolved);
            }
            Err(_) if existing.symlink_metadata().is_ok() => {
                return Err(format!(
                    "{}: {} is a link that leads nowhere",
                    path.display(),
                    existing.display()
                ));
            }
            Err(_) => match (existing.parent(), existing.file_name()) {
                (Some(parent), Some(name)) => {
                    rest.push(name.to_os_string());
                    existing = parent;
                }
                _ => return Err(format!("{}: cannot be resolved", path.display())),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tree(PathBuf);

    impl Tree {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("helios-roots-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            for sub in ["photos", "out", "elsewhere"] {
                std::fs::create_dir_all(dir.join(sub)).unwrap();
            }
            std::fs::write(dir.join("photos/a.png"), b"png").unwrap();
            std::fs::write(dir.join("elsewhere/secret.txt"), b"secret").unwrap();
            Tree(dir.canonicalize().unwrap())
        }

        fn roots(&self) -> Roots {
            Roots::new(&[self.0.join("photos")], Some(&self.0.join("out"))).unwrap()
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn unconfined_allows_any_path() {
        let roots = Roots::unconfined();
        assert!(!roots.is_confined());
        assert!(roots.check(Path::new("relative"), Access::Write).is_ok());
    }

    #[test]
    fn reads_under_read_and_write_roots_writes_under_the_write_root() {
        let t = Tree::new("grant");
        let roots = t.roots();
        assert!(roots.check(&t.0.join("photos/a.png"), Access::Read).is_ok());
        assert!(
            roots
                .check(&t.0.join("out/new/b.png"), Access::Read)
                .is_ok()
        );
        assert!(
            roots
                .check(&t.0.join("out/new/b.png"), Access::Write)
                .is_ok()
        );
        let e = roots
            .check(&t.0.join("photos/a.png"), Access::Write)
            .unwrap_err();
        assert!(
            e.contains("outside the roots") && e.contains("write:"),
            "{e}"
        );
    }

    #[test]
    fn escapes_are_refused() {
        let t = Tree::new("escape");
        let roots = t.roots();
        let outside = t.0.join("elsewhere/secret.txt");
        assert!(roots.check(&outside, Access::Read).is_err());
        let dotdot = t.0.join("photos/../elsewhere/secret.txt");
        let e = roots.check(&dotdot, Access::Read).unwrap_err();
        assert!(e.contains("`..`"), "{e}");
        let e = roots
            .check(Path::new("photos/a.png"), Access::Read)
            .unwrap_err();
        assert!(e.contains("not an absolute path"), "{e}");
        // A root's sibling sharing its name as a prefix is not under it.
        std::fs::create_dir_all(t.0.join("photos2")).unwrap();
        assert!(
            roots
                .check(&t.0.join("photos2/x.png"), Access::Read)
                .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn links_out_of_a_root_are_refused() {
        let t = Tree::new("links");
        let roots = t.roots();
        std::os::unix::fs::symlink(t.0.join("elsewhere"), t.0.join("photos/away")).unwrap();
        let e = roots
            .check(&t.0.join("photos/away/secret.txt"), Access::Read)
            .unwrap_err();
        assert!(e.contains("outside the roots"), "{e}");
        // Through a link out, a file that does not exist yet.
        std::os::unix::fs::symlink(t.0.join("elsewhere"), t.0.join("out/away")).unwrap();
        assert!(
            roots
                .check(&t.0.join("out/away/new.png"), Access::Write)
                .is_err()
        );
        // A link to nowhere: writing through it would create its target.
        std::os::unix::fs::symlink(t.0.join("elsewhere/planted"), t.0.join("out/dangling"))
            .unwrap();
        let e = roots
            .check(&t.0.join("out/dangling"), Access::Write)
            .unwrap_err();
        assert!(e.contains("leads nowhere"), "{e}");
    }

    #[test]
    fn a_missing_root_is_refused() {
        let t = Tree::new("missing");
        assert!(Roots::new(&[t.0.join("nope")], None).is_err());
        assert!(Roots::new(&[t.0.join("photos/a.png")], None).is_err());
    }
}
