//! Shared fixtures for driving the built `askm` binary from integration tests.
//!
//! Every test must be hermetic: no real `~/.agents`, `~/.claude`, `~/.local`,
//! `~/.cache`, or `~/.config` may be read or written. `--store-root` isolates
//! askm's own store, but `Scope::User` still resolves through `$HOME` inside
//! the library — so [`Fixture`] also overrides the child process's `HOME`
//! environment variable to a tempdir, which redirects `Scope::User` right
//! along with it. `status`/`doctor` scan both scopes, so this matters even
//! for tests that only ever pass `--project`.
//!
//! Not every test binary that includes this module uses every helper below
//! (each `tests/cli_*.rs` file is its own crate) — `dead_code` is allowed
//! wholesale rather than per-item for that reason.
#![allow(dead_code)]

use std::fs;
use std::path::Path;
use std::process::{Command, ExitStatus};

/// An isolated store, home, and project directory for one test.
pub struct Fixture {
    pub store_dir: tempfile::TempDir,
    pub home_dir: tempfile::TempDir,
    pub project_dir: tempfile::TempDir,
}

impl Fixture {
    pub fn new() -> Self {
        let project_dir = tempfile::tempdir().expect("tempdir");
        // A `.git` directory here makes `--project` resolve deterministically
        // to this directory rather than walking further up.
        fs::create_dir_all(project_dir.path().join(".git")).expect("create .git");
        Self {
            store_dir: tempfile::tempdir().expect("tempdir"),
            home_dir: tempfile::tempdir().expect("tempdir"),
            project_dir,
        }
    }

    /// A ready-to-run `askm` invocation: store root, home, and cwd all
    /// pinned to this fixture's tempdirs.
    pub fn cmd(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_askm"));
        cmd.arg("--store-root").arg(self.store_dir.path());
        cmd.args(args);
        cmd.env("HOME", self.home_dir.path());
        cmd.current_dir(self.project_dir.path());
        cmd
    }
}

pub struct Outcome {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

pub fn run(cmd: &mut Command) -> Outcome {
    let output = cmd.output().expect("askm binary should run");
    Outcome {
        status: output.status,
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// Write a self-contained marketplace repo at `root`: one plugin
/// (`demo-plugin`) with one skill (`demo-skill`), listed via a
/// `.claude-plugin/marketplace.json` local `source`.
pub fn write_marketplace_repo(root: &Path) {
    let plugin_dir = root.join("demo-plugin");
    fs::create_dir_all(&plugin_dir).unwrap();
    fs::write(
        plugin_dir.join("plugin.json"),
        r#"{"name":"demo-plugin","version":"1.0.0"}"#,
    )
    .unwrap();

    let skill_dir = plugin_dir.join("skills").join("demo-skill");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: demo-skill\ndescription: A demo skill used by askm's CLI integration tests.\n---\n\nBody.\n",
    )
    .unwrap();

    let marketplace_dir = root.join(".claude-plugin");
    fs::create_dir_all(&marketplace_dir).unwrap();
    let marketplace_json = r#"{
        "name": "demo-marketplace",
        "plugins": [
            {
                "name": "demo-plugin",
                "source": "./demo-plugin",
                "version": "1.0.0",
                "description": "A demo plugin for askm's CLI integration tests."
            }
        ]
    }"#;
    fs::write(marketplace_dir.join("marketplace.json"), marketplace_json).unwrap();
}

/// Register `write_marketplace_repo`'s fixture under the name
/// `demo-marketplace`, asserting the `marketplace add` call succeeded.
/// Returns the marketplace source tempdir, which must outlive the fixture use.
pub fn add_demo_marketplace(fx: &Fixture) -> tempfile::TempDir {
    let market_dir = tempfile::tempdir().expect("tempdir");
    write_marketplace_repo(market_dir.path());

    let mut cmd = fx.cmd(&[
        "marketplace",
        "add",
        market_dir.path().to_str().unwrap(),
        "--name",
        "demo-marketplace",
    ]);
    let outcome = run(&mut cmd);
    assert!(
        outcome.status.success(),
        "marketplace add failed: {}",
        outcome.stderr
    );

    market_dir
}

/// `add_demo_marketplace` plus `install demo-plugin@demo-marketplace`.
pub fn install_demo_plugin(fx: &Fixture) -> tempfile::TempDir {
    let market_dir = add_demo_marketplace(fx);
    let mut cmd = fx.cmd(&["install", "demo-plugin@demo-marketplace"]);
    let outcome = run(&mut cmd);
    assert!(
        outcome.status.success(),
        "install failed: {}",
        outcome.stderr
    );
    market_dir
}
