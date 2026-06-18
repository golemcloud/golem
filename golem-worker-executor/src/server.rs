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

use golem_common::SafeDisplay;
use golem_common::tracing::init_tracing_with_default_env_filter;
use golem_worker_executor::bootstrap;
use golem_worker_executor::metrics;
use golem_worker_executor::services::golem_config::{GolemConfig, make_config_loader};
use std::sync::Arc;
use tokio::task::JoinSet;
use tracing::info;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> Result<(), anyhow::Error> {
    match make_config_loader().load_or_dump_config() {
        Some(mut config) => {
            rustls::crypto::ring::default_provider()
                .install_default()
                .expect("Failed to install crypto provider");

            config.add_port_to_tracing_file_name_if_enabled();
            init_tracing_with_default_env_filter(&config.tracing);
            info!("Using configuration:\n{}", config.to_safe_string_indented());

            let prometheus = metrics::register_all();

            let runtime = Arc::new(
                tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()?,
            );

            runtime.block_on(async_main(config, prometheus, runtime.clone()))
        }
        None => Ok(()),
    }
}

async fn async_main(
    config: GolemConfig,
    prometheus: prometheus::Registry,
    runtime: Arc<tokio::runtime::Runtime>,
) -> Result<(), anyhow::Error> {
    spawn_task_dump_on_signal(runtime.handle().clone());

    let mut join_set = JoinSet::new();

    let _run_details =
        bootstrap::run(config, prometheus, runtime.handle().clone(), &mut join_set).await?;

    while let Some(res) = join_set.join_next().await {
        res??
    }
    Ok(())
}

/// On builds compiled with `--cfg tokio_unstable`, install a SIGUSR1 handler
/// that dumps the async backtrace of every task on the runtime.
///
/// This is the debug-image escape hatch for diagnosing hangs without exposing
/// the tokio-console port: reproduce the wedge, then
/// `kubectl exec <pod> -- kill -USR1 1` and read the dump from `kubectl logs`
/// (or `kubectl cp` the file written to `/tmp/golem-task-dump-*.txt`).
///
/// The dump groups tasks by identical async backtrace and prints the groups
/// largest-first, so with thousands of parked tasks the resource the deadlock
/// is stuck on shows up as the dominant group at the top. `Handle::dump()` is
/// wrapped in a timeout so the handler always responds even on a fully wedged
/// runtime.
///
/// `Handle::dump()` is only available under `tokio_unstable` with the tokio
/// `taskdump` feature, and only on Linux (the deployment target); on a normal
/// build, on macOS, or on any other platform this compiles to a no-op so
/// behaviour is unchanged.
#[cfg(all(feature = "taskdump", tokio_unstable, target_os = "linux"))]
fn spawn_task_dump_on_signal(handle: tokio::runtime::Handle) {
    use std::collections::BTreeMap;
    use std::io::Write;
    use tracing::{error, info, warn};

    handle.clone().spawn(async move {
        let mut sigusr1 =
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::user_defined1()) {
                Ok(stream) => stream,
                Err(err) => {
                    error!("failed to install SIGUSR1 task-dump handler: {err}");
                    return;
                }
            };
        info!("SIGUSR1 task-dump handler installed (tokio_unstable)");
        loop {
            sigusr1.recv().await;
            info!("SIGUSR1 received — dumping all tokio tasks");

            // A fully-wedged runtime can make `dump()` itself block (it needs
            // each task to reach a yield point); bound it so the handler always
            // responds and we still get whatever traces were captured.
            let dump = match tokio::time::timeout(std::time::Duration::from_secs(30), handle.dump())
                .await
            {
                Ok(dump) => dump,
                Err(_) => {
                    warn!("SIGUSR1 task dump timed out after 30s (runtime fully wedged)");
                    continue;
                }
            };

            // Group tasks by identical backtrace so the dominant stuck await is
            // obvious; `BTreeMap<count-of-tasks>` is built then printed
            // largest-first.
            let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
            let mut total = 0usize;
            for task in dump.tasks().iter() {
                total += 1;
                let trace = format!("{}", task.trace());
                groups.entry(trace).or_default().push(task.id().to_string());
            }
            let mut ranked: Vec<(usize, &String, &Vec<String>)> = groups
                .iter()
                .map(|(trace, ids)| (ids.len(), trace, ids))
                .collect();
            ranked.sort_by(|a, b| b.0.cmp(&a.0));

            let mut report = String::new();
            report.push_str(&format!(
                "===== GOLEM TASK DUMP: {total} tasks in {} distinct backtraces =====\n",
                ranked.len()
            ));
            for (rank, (count, trace, ids)) in ranked.iter().enumerate() {
                let sample: Vec<&str> = ids.iter().take(5).map(|s| s.as_str()).collect();
                report.push_str(&format!(
                    "\n----- GROUP {rank}: {count} tasks (sample ids: {}) -----\n{trace}\n",
                    sample.join(", ")
                ));
            }
            report.push_str("===== END GOLEM TASK DUMP =====\n");

            // Emit to the logs (delimited so it is easy to extract from
            // interleaved JSON) ...
            info!("{report}");
            // ... and to a file for `kubectl cp`.
            let path = format!(
                "/tmp/golem-task-dump-{}.txt",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0)
            );
            match std::fs::File::create(&path).and_then(|mut f| f.write_all(report.as_bytes())) {
                Ok(()) => info!("SIGUSR1 task dump written to {path} ({total} tasks)"),
                Err(err) => warn!("failed to write task dump to {path}: {err}"),
            }
        }
    });
}

#[cfg(not(all(feature = "taskdump", tokio_unstable, target_os = "linux")))]
fn spawn_task_dump_on_signal(_handle: tokio::runtime::Handle) {}
