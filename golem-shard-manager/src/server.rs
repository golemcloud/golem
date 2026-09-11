// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use golem_common::SafeDisplay;
use golem_common::tracing::init_tracing_with_default_env_filter;
use golem_shard_manager::config::{
    ShardManagerConfig, make_config_loader, reject_legacy_db_env_vars,
};
use golem_shard_manager::{Deployment, ShardManagerError};
use prometheus::default_registry;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

fn main() -> Result<(), anyhow::Error> {
    // Before the configuration is loaded at all, so that `--dump-config` cannot print a config
    // that silently ignores a deployment's legacy settings.
    reject_legacy_db_env_vars().map_err(|err| anyhow::anyhow!(err))?;

    match make_config_loader().load_or_dump_config() {
        Some(config) => {
            rustls::crypto::ring::default_provider()
                .install_default()
                .expect("Failed to install crypto provider");

            init_tracing_with_default_env_filter(&config.tracing);
            info!("Using configuration:\n{}", config.to_safe_string_indented());

            let registry = default_registry().clone();

            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?
                .block_on(async_main(config, registry))
        }
        None => Ok(()),
    }
}

async fn async_main(
    config: ShardManagerConfig,
    registry: prometheus::Registry,
) -> anyhow::Result<()> {
    let shutdown = CancellationToken::new();
    tokio::spawn({
        let shutdown = shutdown.clone();
        async move {
            shutdown_signal().await;
            info!("Received a shutdown signal; stopping the shard manager");
            shutdown.cancel();
        }
    });

    let mut join_set = JoinSet::new();
    let details = match golem_shard_manager::run(
        &config,
        Deployment::Standalone {
            shutdown: shutdown.clone(),
        },
        registry,
        &mut join_set,
    )
    .await
    {
        Ok(details) => details,
        Err(err)
            if matches!(
                err.downcast_ref::<ShardManagerError>(),
                Some(ShardManagerError::ShutdownRequested)
            ) =>
        {
            info!("Stopped campaigning for leadership; exiting");
            return Ok(());
        }
        Err(err) => return Err(err),
    };

    golem_shard_manager::serve_until_stopped(details, join_set, shutdown).await
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            warn!(error = %err, "Cannot listen for Ctrl-C");
            std::future::pending::<()>().await
        }
    };

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate_signal() => {}
    }
}

/// SIGTERM, which is what a container runtime sends first.
#[cfg(unix)]
async fn terminate_signal() {
    use tokio::signal::unix::{SignalKind, signal};

    match signal(SignalKind::terminate()) {
        Ok(mut terminate) => {
            terminate.recv().await;
        }
        Err(err) => {
            warn!(
                error = %err,
                "Cannot listen for SIGTERM; only Ctrl-C will release the leadership on shutdown"
            );
            std::future::pending::<()>().await
        }
    }
}

#[cfg(not(unix))]
async fn terminate_signal() {
    std::future::pending::<()>().await
}
