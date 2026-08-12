//! `askm marketplace add` (local path) then `marketplace list` shows it.

mod common;

use common::{add_demo_marketplace, run, Fixture};

#[test]
fn marketplace_add_with_a_local_path_then_list_shows_it() {
    let fx = Fixture::new();

    let _market_dir = add_demo_marketplace(&fx);

    let mut list_cmd = fx.cmd(&["marketplace", "list"]);
    let list = run(&mut list_cmd);

    assert!(list.status.success(), "stderr: {}", list.stderr);
    assert!(
        list.stdout.contains("demo-marketplace"),
        "stdout: {}",
        list.stdout
    );
}

#[test]
fn marketplace_remove_un_registers_it() {
    let fx = Fixture::new();
    let _market_dir = add_demo_marketplace(&fx);

    let mut remove_cmd = fx.cmd(&["marketplace", "remove", "demo-marketplace"]);
    let remove = run(&mut remove_cmd);
    assert!(remove.status.success(), "stderr: {}", remove.stderr);

    let mut list_cmd = fx.cmd(&["marketplace", "list"]);
    let list = run(&mut list_cmd);
    assert!(list.status.success());
    assert!(
        !list.stdout.contains("demo-marketplace"),
        "stdout: {}",
        list.stdout
    );
}

#[test]
fn marketplace_add_rejects_a_source_that_is_neither_a_path_nor_a_url() {
    let fx = Fixture::new();

    let mut add_cmd = fx.cmd(&["marketplace", "add", "not-a-real-path-or-url"]);
    let add = run(&mut add_cmd);

    assert!(!add.status.success());
    assert!(
        add.stderr.contains("path") || add.stderr.contains("URL"),
        "stderr: {}",
        add.stderr
    );
}
