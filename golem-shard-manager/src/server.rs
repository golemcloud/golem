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

use golem_common::tracing::init_tracing_with_default_env_filter;
use golem_shard_manager::shard_manager_config::{make_config_loader, ShardManagerConfig};
use prometheus::default_registry;
use tokio::task::JoinSet;

fn main() -> Result<(), anyhow::Error> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install crypto provider");
    match make_config_loader().load_or_dump_config() {
        Some(config) => {
            init_tracing_with_default_env_filter(&config.tracing);
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
    let mut join_set = JoinSet::new();
    golem_shard_manager::run(&config, registry, &mut join_set).await?;

    while let Some(res) = join_set.join_next().await {
        res??;
    }
    Ok(())
}
