// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://license.golem.cloud/LICENSE

use anyhow::{Context, bail};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

/// Runs the official Durable Streams client against an integration-test deployment.
pub async fn run(driver: &str, origin: &str, host: &str) -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    for (name, contents) in [
        ("package.json", include_str!("package.json")),
        ("package-lock.json", include_str!("package-lock.json")),
        ("routing.mjs", include_str!("routing.mjs")),
        ("driver.mjs", driver),
    ] {
        tokio::fs::write(directory.path().join(name), contents).await?;
    }

    execute(
        Command::new("npm")
            .args(["ci", "--ignore-scripts", "--no-audit", "--no-fund"])
            .current_dir(directory.path()),
        Duration::from_secs(120),
        "installing the Durable Streams reference client",
    )
    .await?;
    execute(
        Command::new("node")
            .args(["--import", "./routing.mjs", "driver.mjs"])
            .arg(format!("http://{host}"))
            .env("GOLEM_TEST_ORIGIN", origin)
            .current_dir(directory.path()),
        Duration::from_secs(180),
        "running the Durable Streams reference client",
    )
    .await
}

async fn execute(command: &mut Command, timeout: Duration, action: &str) -> anyhow::Result<()> {
    let output = tokio::time::timeout(
        timeout,
        command
            .kill_on_drop(true)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
    )
    .await
    .with_context(|| format!("timed out {action}"))?
    .with_context(|| format!("failed {action}"))?;
    if !output.status.success() {
        bail!(
            "{action} failed ({})\n--- stdout ---\n{}\n--- stderr ---\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
    Ok(())
}
