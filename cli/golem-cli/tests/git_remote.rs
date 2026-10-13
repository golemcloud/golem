// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

use test_r::{test, timeout};
use tokio::process::Command;

#[test]
#[timeout("3m")]
async fn git_remote_native_differential() {
    let root = crate::workspace_path();
    let reference = root.join("tmp/git-reference-2.55.0/bin/git");
    let mut command = Command::new("node");
    command.kill_on_drop(true);
    command.current_dir(&root).args([
        "--experimental-strip-types",
        "--test",
        "cli/golem-cli/test-data/git-remote/transfer.test.ts",
    ]);
    if let Some(git) = std::env::var_os("GOLEM_TEST_GIT") {
        command.env("GOLEM_TEST_GIT", git);
    } else if reference.exists() {
        command.env("GOLEM_TEST_GIT", reference);
    }
    let output = command
        .output()
        .await
        .expect("start Node Git differential tests");
    assert!(
        output.status.success(),
        "Git differential tests failed; run cargo make prepare-git-differential-tests first:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
