// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand};
use golem_test_framework::benchmark::{
    BenchmarkArtifact, BenchmarkComparison, BenchmarkSuiteResult, BenchmarkSuiteResultCollection,
};
use std::path::{Path, PathBuf};

#[derive(Parser)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check explicit remote deployment identity without contacting the deployment.
    ValidateEnvironment,
    /// Compare saved base/head process results, in pair order, without measuring.
    Compare {
        #[arg(long, required = true)]
        base: Vec<PathBuf>,
        #[arg(long, required = true)]
        head: Vec<PathBuf>,
        #[arg(long)]
        output: PathBuf,
    },
    /// Combine repeated executions of one density cell, retaining raw iteration evidence.
    MergeIterations {
        #[arg(required = true)]
        inputs: Vec<PathBuf>,
        #[arg(long)]
        output: PathBuf,
    },
    /// Interleave prepared base/head runtimes on one isolated CI measurement runner.
    Paired {
        #[arg(long)]
        base_root: PathBuf,
        #[arg(long)]
        head_root: PathBuf,
        #[arg(long)]
        base_source: String,
        #[arg(long)]
        head_source: String,
        #[arg(long)]
        suite: PathBuf,
        #[arg(long, default_value_t = 3, value_parser = clap::value_parser!(u32).range(3..=10))]
        pairs: u32,
        /// Overall execution budget, not a new clock for each process.
        #[arg(long, default_value_t = 3600, value_parser = clap::value_parser!(u64).range(1..=36000))]
        timeout_secs: u64,
        #[arg(long)]
        runner_id: String,
        /// A new directory; existing evidence is never overwritten.
        #[arg(long)]
        output: PathBuf,
    },
}

fn load(path: &Path) -> Result<BenchmarkSuiteResult> {
    let mut collection: BenchmarkSuiteResultCollection = serde_json::from_slice(
        &std::fs::read(path).with_context(|| format!("read {}", path.display()))?,
    )?;
    ensure!(
        collection.runs.len() == 1,
        "expected one run in {}",
        path.display()
    );
    Ok(collection.runs.remove(0))
}

fn save(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    // Serialize before opening the file: invalid numeric output cannot leave a partial report.
    let bytes = serde_json::to_vec_pretty(value)?;
    use std::io::Write;
    std::fs::File::create_new(path)?.write_all(&bytes)?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    match Args::parse().command {
        Command::ValidateEnvironment => {
            golem_test_framework::benchmark::MeasurementEnvironment::remote_from_env()?;
            println!("Remote measurement environment is complete.");
        }
        Command::Compare { base, head, output } => {
            let read = |paths: Vec<PathBuf>| {
                paths
                    .iter()
                    .map(|path| load(path))
                    .collect::<Result<Vec<_>>>()
            };
            save(
                &output,
                &BenchmarkComparison::from_pairs(&read(base)?, &read(head)?)?,
            )?;
        }
        Command::MergeIterations { inputs, output } => {
            let merged = BenchmarkSuiteResult::merge_iterations(
                inputs
                    .iter()
                    .map(|path| load(path))
                    .collect::<Result<_>>()?,
            )?;
            save(
                &output,
                &BenchmarkSuiteResultCollection { runs: vec![merged] },
            )?;
        }
        Command::Paired {
            base_root,
            head_root,
            base_source,
            head_source,
            suite,
            pairs,
            timeout_secs,
            runner_id,
            output,
        } => {
            ensure!(
                std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true"),
                "measurement requires an isolated GitHub Actions runner"
            );
            #[cfg(unix)]
            {
                let execution = paired(
                    &base_root,
                    &head_root,
                    &base_source,
                    &head_source,
                    &suite,
                    pairs,
                    &runner_id,
                    &output,
                );
                let mut terminate =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
                tokio::select! {
                    result = tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), execution) => result.context("paired execution budget expired; no replacement run started")??,
                    _ = tokio::signal::ctrl_c() => anyhow::bail!("paired execution interrupted"),
                    _ = terminate.recv() => anyhow::bail!("paired execution terminated"),
                }
            }
            #[cfg(not(unix))]
            anyhow::bail!("paired measurement requires a Unix CI runner");
        }
    }
    Ok(())
}

#[cfg(unix)]
struct MeasurementProcess(tokio::process::Child);

#[cfg(unix)]
impl Drop for MeasurementProcess {
    fn drop(&mut self) {
        if let Some(pid) = self.0.id() {
            // The unreaped child owns this group. Never signal a reaped/reusable PID.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
    }
}

#[cfg(unix)]
#[allow(clippy::too_many_arguments)]
async fn paired(
    base: &Path,
    head: &Path,
    base_source: &str,
    head_source: &str,
    suite: &Path,
    pairs: u32,
    runner: &str,
    output: &Path,
) -> Result<()> {
    use std::process::Stdio;
    use tokio::process::Command;
    ensure!(!runner.is_empty(), "runner ID must not be empty");
    let base = base.canonicalize()?;
    let head = head.canonicalize()?;
    for (root, source) in [(&base, base_source), (&head, head_source)] {
        let revision = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(root)
            .kill_on_drop(true)
            .output()
            .await?;
        ensure!(
            revision.status.success() && String::from_utf8(revision.stdout)?.trim() == source,
            "prepared checkout source mismatch"
        );
    }
    std::fs::create_dir(output)?;
    let output = output.canonicalize()?;
    let suite_path = output.join("suite.yaml");
    std::fs::copy(suite, &suite_path)?;
    let suite_hash = BenchmarkArtifact::from_path(&suite_path)?;
    let harness = head.join("target/benchmarks/benchmarks");
    let harness_hash = BenchmarkArtifact::from_path(&harness)?;
    let id = uuid::Uuid::new_v4();
    let orders: Vec<_> = (0..pairs)
        .map(|pair| {
            if pair % 2 == 0 {
                ["base", "head"]
            } else {
                ["head", "base"]
            }
        })
        .collect();
    save(
        &output.join("plan.json"),
        &serde_json::json!({ "base_source": base_source, "head_source": head_source, "harness": harness_hash, "suite": suite_hash, "pairs": pairs, "runner": runner, "order": orders, "run_id": id }),
    )?;
    let mut bases = Vec::new();
    let mut heads = Vec::new();
    for (pair, order) in orders.iter().enumerate() {
        for (position, side) in order.iter().enumerate() {
            let (root, source, results) = match *side {
                "base" => (&base, base_source, &mut bases),
                _ => (&head, head_source, &mut heads),
            };
            ensure!(
                BenchmarkArtifact::from_path(&harness)? == harness_hash
                    && BenchmarkArtifact::from_path(&suite_path)? == suite_hash,
                "harness or suite changed"
            );
            // An isolated runner must be empty before/after each normal harness lifecycle.
            let containers = Command::new("docker")
                .args(["ps", "-q"])
                .kill_on_drop(true)
                .output()
                .await?;
            ensure!(
                containers.status.success() && containers.stdout.is_empty(),
                "running containers remain; stop rather than disturb other workloads"
            );
            let prefix = format!("{pair}-{side}");
            let result_path = output.join(format!("{prefix}.json"));
            let log = std::fs::File::create_new(output.join(format!("{prefix}.log")))?;
            let mut command = Command::new(&harness);
            command
                .current_dir(root)
                .kill_on_drop(true)
                .process_group(0)
                .stdout(Stdio::from(log.try_clone()?))
                .stderr(Stdio::from(log))
                .args([
                    "--primary-only",
                    "--retain-selected-zero-counts",
                    "suite",
                    "--run-id",
                    &format!("{id}-{prefix}"),
                    "--runner-id",
                    runner,
                    "--source-repository",
                    "golemcloud/golem",
                    "--source-commit-sha",
                    source,
                    "--source-ref",
                    source,
                    "--save-to-json",
                ])
                .arg(&result_path)
                .arg(&suite_path)
                .args(["spawned", "--workspace-root"])
                .arg(root)
                .arg("--component-directory")
                .arg(root.join("test-components"))
                .args(["--registry-service-http-port", "18081"]);
            let inventory: std::collections::BTreeMap<String, BuiltinArtifact> =
                serde_json::from_slice(&std::fs::read(root.join("builtin-artifacts.json"))?)?;
            for (name, item) in inventory {
                let path = root.join(item.path).canonicalize()?;
                ensure!(
                    path.starts_with(root),
                    "built-in artifact escapes its checkout"
                );
                command.env(
                    format!(
                        "GOLEM_ADVANCED__REGISTRY_SERVICE__BUILTIN_ARTIFACTS__SOURCE_OVERRIDES__{}__URL",
                        name.to_uppercase()
                    ),
                    reqwest::Url::from_file_path(path)
                        .map_err(|_| anyhow::anyhow!("invalid artifact path"))?
                        .to_string(),
                );
            }
            let mut process = MeasurementProcess(command.spawn()?);
            let status = process.0.wait().await?;
            save(
                &output.join(format!("{prefix}.status.json")),
                &serde_json::json!({ "pair": pair, "position": position, "side": side, "exit_code": status.code(), "success": status.success() }),
            )?;
            ensure!(
                status.success(),
                "{prefix} failed; no replacement process started"
            );
            let run = load(&result_path)?;
            ensure!(
                run.source
                    .as_ref()
                    .and_then(|source| source.commit_sha.as_deref())
                    == Some(source)
                    && run.run_id.as_deref() == Some(&format!("{id}-{prefix}"))
                    && run
                        .artifacts
                        .as_ref()
                        .is_some_and(|artifacts| artifacts.runner.sha256 == harness_hash.sha256),
                "unexpected measured identity"
            );
            results.push(run);
        }
    }
    let containers = Command::new("docker")
        .args(["ps", "-q"])
        .kill_on_drop(true)
        .output()
        .await?;
    ensure!(
        containers.status.success() && containers.stdout.is_empty(),
        "containers remain after the last process"
    );
    save(
        &output.join("comparison.json"),
        &BenchmarkComparison::from_pairs(&bases, &heads)?,
    )
}

#[cfg(unix)]
#[derive(serde::Deserialize)]
struct BuiltinArtifact {
    path: PathBuf,
}
