// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! The command line of the filesystem snapshot benchmark.
//!
//! `plan` prints the phases of each tree of the scenarios, one JSON line for each tree. `run` runs
//! one phase and prints its result as the last line of the standard output. The blob storage
//! comes from the `GOLEM__BLOB_STORAGE__*` environment variables of the executor, with the object
//! prefix of the run. The exit code is 0 when each step succeeded, 1 when a step failed, the
//! restored tree differs or the result could not be written, and 2 for an error of the arguments
//! or of the configuration, or for an error of the environment that each later phase finds again,
//! such as an async runtime that does not start.

use super::{Selection, is_key_segment, plan, run_phase};
use clap::Parser;
use figment::Figment;
use figment::providers::{Env, Serialized};
use golem_common::tracing::{OutputConfig, TracingConfig, init_tracing_with_default_env_filter};
use golem_service_base::config::{BlobStorageConfig, S3BlobStorageConfig};
use golem_service_base::storage::blob::BlobStorage;
use golem_service_base::storage::blob::fs::FileSystemBlobStorage;
use golem_service_base::storage::blob::s3::S3BlobStorage;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

/// The start of each object prefix that the benchmark accepts.
const OBJECT_PREFIX_START: &str = "fs-snapshot-bench/";

const ENV_PREFIX: &str = "GOLEM__BLOB_STORAGE__";

#[derive(Debug, PartialEq, Eq, Parser)]
#[command(name = "fs-snapshot-benchmark")]
enum Command {
    /// Prints the phases of each tree of the scenarios, one JSON line for each tree.
    Plan {
        /// The names of the scenarios, separated by commas.
        #[arg(long, value_delimiter = ',', required = true)]
        scenarios: Vec<String>,
    },
    /// Runs one phase of one tree of a scenario.
    Run(RunArguments),
}

#[derive(Debug, PartialEq, Eq, clap::Args)]
struct RunArguments {
    /// The id of the run: 1 to 64 ASCII letters, digits, `-` or `_`.
    #[arg(long)]
    run_id: String,
    #[arg(long)]
    scenario: String,
    #[arg(long)]
    tree: String,
    #[arg(long)]
    phase: String,
    /// The label of the CPU setting of the pod, which the result records.
    #[arg(long)]
    cpu_setting: String,
    /// The directory on the benchmark volume.
    #[arg(long)]
    work_dir: PathBuf,
    /// The object prefix of the run: `fs-snapshot-bench/<run id>`, with or without one `/` at the
    /// end.
    #[arg(long)]
    object_prefix: String,
}

/// Runs the command of the arguments of the process.
pub fn main() -> ExitCode {
    match Command::try_parse() {
        Ok(Command::Plan { scenarios }) => print_plan(&scenarios),
        Ok(Command::Run(arguments)) => run(arguments),
        Err(error) => {
            let _ = error.print();
            ExitCode::from(u8::try_from(error.exit_code()).unwrap_or(2))
        }
    }
}

fn print_plan(scenarios: &[String]) -> ExitCode {
    match plan(scenarios) {
        Ok(entries) => {
            entries
                .iter()
                .for_each(|entry| println!("{}", serde_json::to_string(entry).unwrap_or_default()));
            ExitCode::SUCCESS
        }
        Err(name) => usage_error(&format!("no scenario has the name {name:?}")),
    }
}

fn usage_error(message: &str) -> ExitCode {
    eprintln!("error: {message}");
    ExitCode::from(2)
}

fn run(arguments: RunArguments) -> ExitCode {
    let selection = match validate(&arguments) {
        Ok(selection) => selection,
        Err(message) => return usage_error(&message),
    };
    let config = match storage_config(&arguments.object_prefix) {
        Ok(config) => config,
        Err(message) => return usage_error(&message),
    };
    let _ = rustls::crypto::ring::default_provider().install_default();
    let _ = init_tracing_with_default_env_filter(&TracingConfig {
        stdout: OutputConfig::disabled(),
        stderr: OutputConfig::text(),
        ..TracingConfig::local_dev("fs-snapshot-benchmark")
    });
    let runtime = match crate::bootstrap::create_runtime() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("error: failed to start the async runtime: {error}");
            return ExitCode::from(2);
        }
    };
    runtime.block_on(async {
        let storage = match storage(&config).await {
            Ok(storage) => storage,
            Err(error) => {
                eprintln!("error: failed to open the blob storage: {error:#}");
                return ExitCode::from(2);
            }
        };
        let (result, written) = run_phase(
            &arguments.run_id,
            &arguments.cpu_setting,
            selection,
            &arguments.work_dir,
            storage,
            environment(&config),
        )
        .await;
        println!("{}", serde_json::to_string(&result).unwrap_or_default());
        if let Err(error) = &written {
            eprintln!("error: failed to write the result into the blob storage: {error:#}");
        }
        if written.is_ok() && result.outcome == super::report::Outcome::Ok {
            ExitCode::SUCCESS
        } else {
            ExitCode::from(1)
        }
    })
}

/// Checks the arguments of `run`, and gives the phase that they name.
fn validate(arguments: &RunArguments) -> Result<Selection, String> {
    if !is_key_segment(&arguments.run_id) {
        return Err(format!(
            "the run id {:?} does not have 1 to 64 ASCII letters, digits, `-` or `_`",
            arguments.run_id
        ));
    }
    if !is_key_segment(&arguments.cpu_setting) {
        return Err(format!(
            "the CPU setting {:?} does not have 1 to 64 ASCII letters, digits, `-` or `_`",
            arguments.cpu_setting
        ));
    }
    let expected = format!("{OBJECT_PREFIX_START}{}", arguments.run_id);
    if arguments.object_prefix != expected && arguments.object_prefix != format!("{expected}/") {
        return Err(format!(
            "the object prefix {:?} is not {expected:?}, the prefix of the run",
            arguments.object_prefix
        ));
    }
    Selection::find(&arguments.scenario, &arguments.tree, &arguments.phase)
}

/// Reads the blob storage configuration of the executor from the environment, with the object
/// prefix of the run.
fn storage_config(object_prefix: &str) -> Result<BlobStorageConfig, String> {
    let config = Figment::from(Serialized::defaults(BlobStorageConfig::default_s3()))
        .merge(Env::prefixed(ENV_PREFIX).split("__"))
        .extract::<BlobStorageConfig>()
        .map_err(|error| format!("the blob storage configuration is not valid: {error}"))?;
    match config {
        BlobStorageConfig::S3(config) => Ok(BlobStorageConfig::S3(S3BlobStorageConfig {
            object_prefix: object_prefix.trim_end_matches('/').to_string(),
            ..config
        })),
        BlobStorageConfig::LocalFileSystem(config) => {
            Ok(BlobStorageConfig::LocalFileSystem(config))
        }
        _ => {
            Err("the benchmark supports the blob storage types S3 and LocalFileSystem".to_string())
        }
    }
}

async fn storage(config: &BlobStorageConfig) -> anyhow::Result<Arc<dyn BlobStorage>> {
    match config {
        BlobStorageConfig::S3(config) => Ok(Arc::new(S3BlobStorage::new(config.clone()).await)),
        BlobStorageConfig::LocalFileSystem(config) => {
            Ok(Arc::new(FileSystemBlobStorage::new(&config.root).await?))
        }
        _ => anyhow::bail!("the benchmark supports the blob storage types S3 and LocalFileSystem"),
    }
}

/// Records the pod, the host and the storage of the run.
fn environment(config: &BlobStorageConfig) -> Value {
    let read = |path: &str| {
        std::fs::read_to_string(Path::new(path))
            .ok()
            .map(|text| text.trim().to_string())
    };
    json!({
        "pod": std::env::var("POD_NAME").ok(),
        "node": std::env::var("NODE_NAME").ok(),
        "kernel": read("/proc/sys/kernel/osrelease"),
        "available_parallelism": std::thread::available_parallelism().map(|count| count.get()).ok(),
        "tokio_workers": tokio::runtime::Handle::try_current().ok().map(|handle| handle.metrics().num_workers()),
        "rayon_threads": rayon::current_num_threads(),
        "cgroup": {
            "cpu_max": read("/sys/fs/cgroup/cpu.max"),
            "cpuset": read("/sys/fs/cgroup/cpuset.cpus.effective"),
            "memory_max": read("/sys/fs/cgroup/memory.max").and_then(|text| text.parse::<u64>().ok()),
        },
        "storage": match config {
            BlobStorageConfig::S3(config) => json!({
                "type": "S3",
                "bucket": config.initial_agent_files_bucket,
                "region": config.region,
                "object_prefix": config.object_prefix,
                "retries": {
                    "max_attempts": config.retries.max_attempts,
                    "min_delay_ms": config.retries.min_delay.as_millis() as u64,
                    "max_delay_ms": config.retries.max_delay.as_millis() as u64,
                    "multiplier": config.retries.multiplier,
                },
            }),
            BlobStorageConfig::LocalFileSystem(config) => json!({
                "type": "LocalFileSystem",
                "root": config.root.display().to_string(),
            }),
            _ => Value::Null,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::{Command, RunArguments, storage_config, validate};
    use clap::Parser;
    use golem_service_base::config::BlobStorageConfig;
    use pretty_assertions::assert_eq;
    use std::path::PathBuf;
    use test_r::test;

    fn run_arguments() -> RunArguments {
        RunArguments {
            run_id: "123-1".to_string(),
            scenario: "base".to_string(),
            tree: "files-1g".to_string(),
            phase: "save".to_string(),
            cpu_setting: "limit-3".to_string(),
            work_dir: PathBuf::from("/data"),
            object_prefix: "fs-snapshot-bench/123-1".to_string(),
        }
    }

    #[test]
    fn the_plan_and_run_command_lines_of_the_workflow_parse() {
        let plan =
            Command::try_parse_from(["fs-snapshot-benchmark", "plan", "--scenarios", "base,smoke"]);
        let run = Command::try_parse_from([
            "fs-snapshot-benchmark",
            "run",
            "--run-id",
            "123-1",
            "--scenario",
            "base",
            "--tree",
            "files-1g",
            "--phase",
            "save",
            "--cpu-setting",
            "limit-3",
            "--work-dir",
            "/data",
            "--object-prefix",
            "fs-snapshot-bench/123-1",
        ]);
        let missing = Command::try_parse_from(["fs-snapshot-benchmark", "plan"])
            .map_err(|error| error.exit_code());

        assert_eq!(
            (plan.ok(), run.ok(), missing),
            (
                Some(Command::Plan {
                    scenarios: vec!["base".to_string(), "smoke".to_string()],
                }),
                Some(Command::Run(run_arguments())),
                Err(2),
            )
        );
    }

    #[test]
    fn a_run_needs_a_valid_run_id_cpu_setting_prefix_and_selection() {
        let with = |change: fn(&mut RunArguments)| {
            let mut arguments = run_arguments();
            change(&mut arguments);
            validate(&arguments).is_ok()
        };

        assert_eq!(
            [
                with(|_| {}),
                with(|arguments| arguments.run_id = "a/b".to_string()),
                with(|arguments| arguments.cpu_setting = String::new()),
                with(|arguments| arguments.object_prefix = "release".to_string()),
                with(|arguments| arguments.object_prefix = "fs-snapshot-bench/".to_string()),
                with(
                    |arguments| arguments.object_prefix = "fs-snapshot-bench/other-run".to_string()
                ),
                with(|arguments| {
                    arguments.object_prefix = "fs-snapshot-bench/123-1/extra".to_string()
                }),
                with(|arguments| arguments.object_prefix = "fs-snapshot-bench/123-1//".to_string()),
                with(|arguments| arguments.object_prefix = "fs-snapshot-bench/123-1/".to_string()),
                with(|arguments| arguments.tree = "files-tiny".to_string()),
            ],
            [
                true, false, false, false, false, false, false, false, true, false
            ]
        );
    }

    #[test]
    fn the_object_prefix_of_the_run_replaces_the_prefix_of_the_configuration() {
        let config = storage_config("fs-snapshot-bench/123-1/").unwrap();

        assert_eq!(
            match config {
                BlobStorageConfig::S3(config) => Some(config.object_prefix),
                _ => None,
            },
            Some("fs-snapshot-bench/123-1".to_string())
        );
    }
}
