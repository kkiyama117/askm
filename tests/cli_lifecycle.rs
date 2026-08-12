//! The end-to-end happy path through the CLI: install, list, enable, status,
//! disable — plus `--json` output, since this tool is meant to be scripted.

mod common;

use common::{install_demo_plugin, run, Fixture};
use std::fs;

#[test]
fn install_then_list_installed_shows_the_plugin() {
    let fx = Fixture::new();
    let _market_dir = install_demo_plugin(&fx);

    let mut list_cmd = fx.cmd(&["list", "--installed"]);
    let list = run(&mut list_cmd);

    assert!(list.status.success(), "stderr: {}", list.stderr);
    assert!(
        list.stdout.contains("demo-plugin"),
        "stdout: {}",
        list.stdout
    );
    assert!(
        list.stdout.contains("demo-marketplace"),
        "stdout: {}",
        list.stdout
    );
}

#[test]
fn enable_then_status_shows_it_managed_then_disable_removes_it() {
    let fx = Fixture::new();
    let _market_dir = install_demo_plugin(&fx);

    let mut enable_cmd = fx.cmd(&["enable", "demo-skill", "--project"]);
    let enable = run(&mut enable_cmd);
    assert!(enable.status.success(), "stderr: {}", enable.stderr);

    let mut status_cmd = fx.cmd(&["status"]);
    let status = run(&mut status_cmd);
    assert!(status.status.success(), "stderr: {}", status.stderr);
    assert!(
        status.stdout.contains("demo-skill"),
        "stdout: {}",
        status.stdout
    );
    assert!(
        status.stdout.contains("managed"),
        "stdout: {}",
        status.stdout
    );

    let link_path = fx.project_dir.path().join(".agents/skills/demo-skill");
    assert!(
        link_path.join("SKILL.md").is_file(),
        "the link should resolve to a real SKILL.md"
    );

    let mut disable_cmd = fx.cmd(&["disable", "demo-skill", "--project"]);
    let disable = run(&mut disable_cmd);
    assert!(disable.status.success(), "stderr: {}", disable.stderr);
    assert!(!link_path.exists(), "the link should be gone after disable");

    let mut status_after_cmd = fx.cmd(&["status"]);
    let status_after = run(&mut status_after_cmd);
    assert!(
        !status_after.stdout.contains("demo-skill"),
        "stdout: {}",
        status_after.stdout
    );
}

#[test]
fn enable_all_projects_every_skill_the_plugin_has() {
    let fx = Fixture::new();
    let _market_dir = install_demo_plugin(&fx);

    let mut enable_cmd = fx.cmd(&[
        "enable",
        "--all",
        "demo-plugin@demo-marketplace",
        "--project",
    ]);
    let enable = run(&mut enable_cmd);

    assert!(enable.status.success(), "stderr: {}", enable.stderr);
    let link_path = fx.project_dir.path().join(".agents/skills/demo-skill");
    assert!(link_path.join("SKILL.md").is_file());
}

#[test]
fn search_list_and_status_json_output_all_parse_as_json() {
    let fx = Fixture::new();
    let _market_dir = install_demo_plugin(&fx);
    let mut enable_cmd = fx.cmd(&["enable", "demo-skill", "--project"]);
    assert!(run(&mut enable_cmd).status.success());

    for args in [
        vec!["--json", "search", "demo"],
        vec!["--json", "list"],
        vec!["--json", "status"],
    ] {
        let mut cmd = fx.cmd(&args);
        let outcome = run(&mut cmd);
        assert!(
            outcome.status.success(),
            "{args:?} failed: {}",
            outcome.stderr
        );
        let parsed: serde_json::Value = serde_json::from_str(&outcome.stdout)
            .unwrap_or_else(|e| panic!("{args:?} produced invalid JSON: {e}\n{}", outcome.stdout));
        assert!(
            parsed.is_array() || parsed.is_object(),
            "{args:?} should produce a JSON array or object"
        );
    }
}

#[test]
fn enable_with_path_projects_a_skill_from_an_arbitrary_directory() {
    let fx = Fixture::new();
    let src = fx.project_dir.path().join("skills-src");
    let skill_dir = src.join("pi-share");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: pi-share\ndescription: A test skill.\n---\n\nBody.\n",
    )
    .unwrap();

    let mut enable_cmd = fx.cmd(&[
        "enable",
        "pi-share",
        "--path",
        src.to_str().unwrap(),
        "--project",
    ]);
    let enable = run(&mut enable_cmd);
    assert!(enable.status.success(), "stderr: {}", enable.stderr);

    let link_path = fx.project_dir.path().join(".agents/skills/pi-share");
    assert!(
        link_path.join("SKILL.md").is_file(),
        "the link should resolve to the source SKILL.md"
    );

    let mut status_cmd = fx.cmd(&["status"]);
    let status = run(&mut status_cmd);
    assert!(status.status.success(), "stderr: {}", status.stderr);
    assert!(
        status.stdout.contains("pi-share") && status.stdout.contains("managed"),
        "stdout: {}",
        status.stdout
    );

    let mut disable_cmd = fx.cmd(&["disable", "pi-share", "--project"]);
    let disable = run(&mut disable_cmd);
    assert!(disable.status.success(), "stderr: {}", disable.stderr);
    assert!(!link_path.exists(), "the link should be gone after disable");
}

#[test]
fn enable_with_path_and_all_projects_every_skill_in_the_directory() {
    let fx = Fixture::new();
    let src = fx.project_dir.path().join("skills-src");
    for skill in ["alpha", "beta"] {
        let skill_dir = src.join(skill);
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {skill}\ndescription: A test skill.\n---\n\nBody.\n"),
        )
        .unwrap();
    }
    // A directory without a SKILL.md must be ignored by --all.
    fs::create_dir_all(src.join("not-a-skill")).unwrap();

    let mut enable_cmd = fx.cmd(&[
        "enable",
        "--all",
        "--path",
        src.to_str().unwrap(),
        "--project",
    ]);
    let enable = run(&mut enable_cmd);
    assert!(enable.status.success(), "stderr: {}", enable.stderr);

    for skill in ["alpha", "beta"] {
        let link_path = fx.project_dir.path().join(".agents/skills").join(skill);
        assert!(
            link_path.join("SKILL.md").is_file(),
            "{skill} should be enabled"
        );
    }
    assert!(
        !fx.project_dir
            .path()
            .join(".agents/skills/not-a-skill")
            .exists(),
        "a directory without SKILL.md must not be enabled"
    );
}

#[test]
fn enable_with_path_refuses_a_missing_skill() {
    let fx = Fixture::new();
    let src = fx.project_dir.path().join("skills-src");
    fs::create_dir_all(&src).unwrap();

    let mut enable_cmd = fx.cmd(&[
        "enable",
        "missing",
        "--path",
        src.to_str().unwrap(),
        "--project",
    ]);
    let enable = run(&mut enable_cmd);

    assert!(!enable.status.success());
    assert!(
        enable.stderr.contains("no skill named"),
        "stderr: {}",
        enable.stderr
    );
}
