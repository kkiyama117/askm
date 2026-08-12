//! Local filesystem sources.
//!
//! A local source never copies content into the store: `sync` points `dest`
//! at the canonicalized source path via a symlink, so the same files are
//! shared in place rather than duplicated. This keeps a locally-developed
//! plugin editable at its original location while still landing at the
//! store's expected path.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::source::{remove_path, Source, SyncOutcome};

/// A source backed by a local directory.
pub struct LocalSource {
    path: PathBuf,
}

impl LocalSource {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

impl Source for LocalSource {
    fn sync(&self, dest: &Path) -> Result<SyncOutcome> {
        if !self.path.exists() {
            bail!("local source path does not exist: {}", self.path.display());
        }
        if !self.path.is_dir() {
            bail!(
                "local source path is not a directory: {}",
                self.path.display()
            );
        }
        let canonical = self
            .path
            .canonicalize()
            .with_context(|| format!("canonicalizing {}", self.path.display()))?;

        if existing_symlink_target(dest)?.as_deref() == Some(canonical.as_path()) {
            return Ok(SyncOutcome {
                revision: None,
                changed: false,
            });
        }

        remove_path(dest)?;
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        create_symlink(&canonical, dest)
            .with_context(|| format!("linking {} -> {}", dest.display(), canonical.display()))?;

        Ok(SyncOutcome {
            revision: None,
            changed: true,
        })
    }
}

fn existing_symlink_target(dest: &Path) -> Result<Option<PathBuf>> {
    match fs::symlink_metadata(dest) {
        Ok(meta) if meta.file_type().is_symlink() => Ok(Some(fs::read_link(dest)?)),
        Ok(_) => Ok(None),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("inspecting {}", dest.display())),
    }
}

#[cfg(unix)]
fn create_symlink(target: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn create_symlink(target: &Path, link: &Path) -> io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_creates_a_symlink_pointing_at_the_canonicalized_source() {
        let temp = tempfile::tempdir().unwrap();
        let source_dir = temp.path().join("plugin-src");
        fs::create_dir_all(&source_dir).unwrap();
        fs::write(source_dir.join("marker.txt"), "hello").unwrap();
        let dest = temp.path().join("dest");

        let outcome = LocalSource::new(&source_dir).sync(&dest).unwrap();

        assert!(outcome.changed);
        assert_eq!(outcome.revision, None);
        assert_eq!(
            fs::read_to_string(dest.join("marker.txt")).unwrap(),
            "hello"
        );
        let meta = fs::symlink_metadata(&dest).unwrap();
        assert!(meta.file_type().is_symlink(), "dest should be a symlink");
    }

    #[test]
    fn sync_is_idempotent_when_the_target_is_unchanged() {
        let temp = tempfile::tempdir().unwrap();
        let source_dir = temp.path().join("plugin-src");
        fs::create_dir_all(&source_dir).unwrap();
        let dest = temp.path().join("dest");
        let source = LocalSource::new(&source_dir);
        source.sync(&dest).unwrap();

        let second = source.sync(&dest).unwrap();

        assert!(!second.changed);
    }

    #[test]
    fn sync_rejects_a_nonexistent_path() {
        let temp = tempfile::tempdir().unwrap();
        let source = LocalSource::new(temp.path().join("does-not-exist"));

        let err = source.sync(&temp.path().join("dest")).unwrap_err();

        assert!(err.to_string().contains("does not exist"), "{err}");
    }

    #[test]
    fn sync_rejects_a_path_that_is_a_file_not_a_directory() {
        let temp = tempfile::tempdir().unwrap();
        let file_path = temp.path().join("not-a-dir.txt");
        fs::write(&file_path, "content").unwrap();
        let source = LocalSource::new(&file_path);

        let err = source.sync(&temp.path().join("dest")).unwrap_err();

        assert!(err.to_string().contains("not a directory"), "{err}");
    }

    #[test]
    fn sync_replaces_a_dest_that_previously_pointed_elsewhere() {
        let temp = tempfile::tempdir().unwrap();
        let first_src = temp.path().join("first");
        let second_src = temp.path().join("second");
        fs::create_dir_all(&first_src).unwrap();
        fs::create_dir_all(&second_src).unwrap();
        fs::write(second_src.join("marker.txt"), "second").unwrap();
        let dest = temp.path().join("dest");
        LocalSource::new(&first_src).sync(&dest).unwrap();

        let outcome = LocalSource::new(&second_src).sync(&dest).unwrap();

        assert!(outcome.changed);
        assert_eq!(
            fs::read_to_string(dest.join("marker.txt")).unwrap(),
            "second"
        );
    }
}
