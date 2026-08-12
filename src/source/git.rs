//! Git sources: shell out to the `git` binary (a hard dependency of this
//! tool — there is no bundled libgit2).
//!
//! Every value that ultimately comes from marketplace or catalog data
//! (`url`, `reference`, `sha`) is checked against a leading `-` before it
//! reaches [`Command`], so a crafted value cannot be smuggled in as a flag.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use anyhow::{bail, Context, Result};

use crate::source::{Source, SyncOutcome};

/// A source backed by a git remote, optionally pinned to a ref or a sha.
pub struct GitSource {
    url: String,
    reference: Option<String>,
    sha: Option<String>,
}

impl GitSource {
    pub fn new(url: impl Into<String>, reference: Option<String>, sha: Option<String>) -> Self {
        Self {
            url: url.into(),
            reference,
            sha,
        }
    }

    /// The refspec to resolve: a pinned sha wins over a ref, which wins over
    /// the remote's default branch.
    fn refspec(&self) -> &str {
        self.sha
            .as_deref()
            .or(self.reference.as_deref())
            .unwrap_or("HEAD")
    }

    fn fetch_and_reset(&self, dest: &Path) -> Result<String> {
        let refspec = self.refspec();
        run_git(
            Some(dest),
            &["fetch", "--quiet", "--depth", "1", "origin", refspec],
        )
        .with_context(|| format!("fetching {refspec:?} from {}", self.url))?;
        run_git(Some(dest), &["reset", "--quiet", "--hard", "FETCH_HEAD"])
            .context("resetting checkout to FETCH_HEAD")?;
        current_revision(dest)
    }
}

impl Source for GitSource {
    /// Clone shallow when no revision is pinned; when a `sha` or `ref` is
    /// given, fetch exactly that and check it out. Re-syncing an existing
    /// checkout fetches and resets rather than re-cloning.
    fn sync(&self, dest: &Path) -> Result<SyncOutcome> {
        reject_flag_like("git url", &self.url)?;
        if let Some(reference) = &self.reference {
            reject_flag_like("git ref", reference)?;
        }
        if let Some(sha) = &self.sha {
            reject_flag_like("git sha", sha)?;
        }

        let is_existing_checkout = dest.join(".git").is_dir();
        let before = is_existing_checkout
            .then(|| current_revision(dest).ok())
            .flatten();

        if !is_existing_checkout {
            init_checkout(dest, &self.url)?;
        }

        let revision = self.fetch_and_reset(dest)?;
        let changed = before.as_deref() != Some(revision.as_str());

        Ok(SyncOutcome {
            revision: Some(revision),
            changed,
        })
    }
}

fn init_checkout(dest: &Path, url: &str) -> Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    run_git(None, &["init", "--quiet", &path_arg(dest)?])
        .with_context(|| format!("initializing {}", dest.display()))?;
    run_git(Some(dest), &["remote", "add", "origin", url]).context("adding origin remote")?;
    Ok(())
}

fn current_revision(dest: &Path) -> Result<String> {
    Ok(run_git(Some(dest), &["rev-parse", "HEAD"])?
        .trim()
        .to_string())
}

fn path_arg(path: &Path) -> Result<String> {
    path.to_str()
        .map(str::to_string)
        .with_context(|| format!("{} is not valid UTF-8", path.display()))
}

fn reject_flag_like(field: &str, value: &str) -> Result<()> {
    if value.starts_with('-') {
        bail!("{field} must not start with \"-\": {value:?}");
    }
    Ok(())
}

fn run_git(dir: Option<&Path>, args: &[&str]) -> Result<String> {
    let mut command = Command::new("git");
    if let Some(dir) = dir {
        command.arg("-C").arg(dir);
    }
    command.args(args);
    let output: Output = command
        .output()
        .with_context(|| format!("failed to spawn `git {}`", args.join(" ")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("`git {}` failed: {}", args.join(" "), stderr.trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bare-ish local repo with one commit on its default branch, plus a
    /// second commit so tests can pin a specific sha.
    struct TestRepo {
        _dir: tempfile::TempDir,
        path: std::path::PathBuf,
        first_sha: String,
        second_sha: String,
    }

    fn init_test_repo() -> TestRepo {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let git = |args: &[&str]| {
            let status = Command::new("git")
                .arg("-C")
                .arg(&path)
                .args(args)
                .status()
                .expect("git should run");
            assert!(status.success(), "git {args:?} failed");
        };

        git(&["init", "--quiet"]);
        git(&[
            "-c",
            "user.email=test@example.com",
            "-c",
            "user.name=Test",
            "commit",
            "--allow-empty",
            "--quiet",
            "-m",
            "first",
        ]);
        let first_sha = run_git(Some(&path), &["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_string();
        fs::write(path.join("file.txt"), "content").unwrap();
        git(&["add", "file.txt"]);
        git(&[
            "-c",
            "user.email=test@example.com",
            "-c",
            "user.name=Test",
            "commit",
            "--quiet",
            "-m",
            "second",
        ]);
        let second_sha = run_git(Some(&path), &["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_string();

        TestRepo {
            _dir: dir,
            path,
            first_sha,
            second_sha,
        }
    }

    #[test]
    fn sync_clones_the_default_branch_and_reports_its_revision() {
        let repo = init_test_repo();
        let dest = tempfile::tempdir().unwrap();
        let dest = dest.path().join("checkout");
        let source = GitSource::new(repo.path.to_str().unwrap(), None, None);

        let outcome = source.sync(&dest).unwrap();

        assert!(outcome.changed);
        assert_eq!(outcome.revision.as_deref(), Some(repo.second_sha.as_str()));
        assert!(dest.join("file.txt").is_file());
    }

    #[test]
    fn sync_checks_out_a_pinned_sha_even_when_its_not_the_tip() {
        let repo = init_test_repo();
        let dest = tempfile::tempdir().unwrap();
        let dest = dest.path().join("checkout");
        let source = GitSource::new(
            repo.path.to_str().unwrap(),
            None,
            Some(repo.first_sha.clone()),
        );

        let outcome = source.sync(&dest).unwrap();

        assert_eq!(outcome.revision.as_deref(), Some(repo.first_sha.as_str()));
        assert!(
            !dest.join("file.txt").is_file(),
            "first commit predates file.txt"
        );
    }

    #[test]
    fn resyncing_an_unchanged_checkout_is_idempotent() {
        let repo = init_test_repo();
        let dest = tempfile::tempdir().unwrap();
        let dest = dest.path().join("checkout");
        let source = GitSource::new(repo.path.to_str().unwrap(), None, None);
        source.sync(&dest).unwrap();

        let second = source.sync(&dest).unwrap();

        assert!(!second.changed);
        assert_eq!(second.revision.as_deref(), Some(repo.second_sha.as_str()));
    }

    #[test]
    fn resync_fetches_new_commits_instead_of_recloning() {
        let repo = init_test_repo();
        let dest = tempfile::tempdir().unwrap();
        let dest = dest.path().join("checkout");
        let source = GitSource::new(repo.path.to_str().unwrap(), None, None);
        source.sync(&dest).unwrap();

        fs::write(repo.path.join("more.txt"), "more").unwrap();
        let git = |args: &[&str]| {
            let status = Command::new("git")
                .arg("-C")
                .arg(&repo.path)
                .args(args)
                .status()
                .expect("git should run");
            assert!(status.success(), "git {args:?} failed");
        };
        git(&["add", "more.txt"]);
        git(&[
            "-c",
            "user.email=test@example.com",
            "-c",
            "user.name=Test",
            "commit",
            "--quiet",
            "-m",
            "third",
        ]);

        let outcome = source.sync(&dest).unwrap();

        assert!(outcome.changed);
        assert!(dest.join("more.txt").is_file());
        assert!(
            dest.join(".git").is_dir(),
            "resync must reuse the existing checkout"
        );
    }

    #[test]
    fn sync_rejects_a_url_starting_with_a_dash() {
        let dest = tempfile::tempdir().unwrap();
        let source = GitSource::new("-evil-flag", None, None);

        let err = source.sync(&dest.path().join("checkout")).unwrap_err();

        assert!(err.to_string().contains("must not start with"), "{err}");
    }

    #[test]
    fn sync_rejects_a_ref_starting_with_a_dash() {
        let repo = init_test_repo();
        let dest = tempfile::tempdir().unwrap();
        let source = GitSource::new(
            repo.path.to_str().unwrap(),
            Some("--upload-pack=evil".to_string()),
            None,
        );

        let err = source.sync(&dest.path().join("checkout")).unwrap_err();

        assert!(err.to_string().contains("must not start with"), "{err}");
    }
}
