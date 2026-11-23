// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

use test_r::{test, timeout};
use tokio::process::Command;

#[test]
#[timeout("1m")]
async fn published_ci_benchmark_analysis() {
    let output = Command::new("node")
        .kill_on_drop(true)
        .current_dir(crate::workspace_path())
        .args([
            "--test",
            "cli/golem-cli/test-data/benchmark-analysis/consumer.node-test.mjs",
        ])
        .output()
        .await
        .expect("start published CI benchmark consumer tests");
    assert!(
        output.status.success(),
        "Published CI benchmark consumer tests failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
