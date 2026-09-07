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
use std::time::Duration;
use tokio::task::JoinSet;
use tracing::{info, warn};

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> Result<(), anyhow::Error> {
    match make_config_loader().load_or_dump_config() {
        Some(mut config) => {
            config.durable_stream.validate()?;
            config.invocation_results.validate()?;
            rustls::crypto::ring::default_provider()
                .install_default()
                .expect("Failed to install crypto provider");

            config.add_port_to_tracing_file_name_if_enabled();
            init_tracing_with_default_env_filter(&config.tracing);
            info!("Using configuration:\n{}", config.to_safe_string_indented());

            let prometheus = metrics::register_all();

            let runtime = Arc::new(bootstrap::create_runtime()?);

            runtime.block_on(async_main(config, prometheus, runtime.clone()))
        }
        None => Ok(()),
    }
}

/// How long a termination signal waits for the shard lease deregistration to
/// land before the process exits. It is one RPC to the shard manager; the lease
/// expiring on its own is the fallback if it does not make it.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

async fn async_main(
    config: GolemConfig,
    prometheus: prometheus::Registry,
    runtime: Arc<tokio::runtime::Runtime>,
) -> Result<(), anyhow::Error> {
    let mut join_set = JoinSet::new();

    let run_details =
        bootstrap::run(config, prometheus, runtime.handle().clone(), &mut join_set).await?;

    // Armed once, outside the loop: a signal that arrives while no listener is
    // registered is not queued for the next one.
    let terminated = termination_signal();
    tokio::pin!(terminated);

    loop {
        tokio::select! {
            joined = join_set.join_next() => match joined {
                Some(res) => res??,
                None => break,
            },
            _ = &mut terminated => {
                info!("Termination signal received; shutting down");
                break;
            }
        }
    }

    // Trips the graph-wide token. The shard lease renewal loop answers it by
    // deregistering, which lets the shard manager re-home this executor's
    // shards now rather than after the lease expires - so the RPC is waited
    // for instead of being cut off when the runtime is dropped.
    run_details.shutdown.cancel();
    if !run_details.shutdown.wait_for_tracked(SHUTDOWN_GRACE).await {
        warn!(
            grace = ?SHUTDOWN_GRACE,
            "Background tasks did not finish within the shutdown grace period"
        );
    }
    join_set.shutdown().await;
    Ok(())
}

/// Resolves on SIGTERM or SIGINT, which is how an orchestrator or a terminal
/// stops this process. Other platforms get Ctrl-C.
async fn termination_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut terminate =
            signal(SignalKind::terminate()).expect("failed to install the SIGTERM handler");
        let mut interrupt =
            signal(SignalKind::interrupt()).expect("failed to install the SIGINT handler");
        tokio::select! {
            _ = terminate.recv() => {}
            _ = interrupt.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
