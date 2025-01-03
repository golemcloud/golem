// Copyright 2024-2025 Golem Cloud
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

#[allow(unused_imports)]
use std::sync::Arc;

use golem_common::tracing::init_tracing_with_default_env_filter;
use golem_worker_executor::run;
use golem_worker_executor_base::metrics;
use golem_worker_executor_base::services::golem_config::{make_config_loader, GolemConfig};
use tokio::task::JoinSet;

fn main() -> Result<(), anyhow::Error> {
    match make_config_loader().load_or_dump_config() {
        Some(mut config) => {
            config.add_port_to_tracing_file_name_if_enabled();
            init_tracing_with_default_env_filter(&config.tracing);

            let prometheus = metrics::register_all();

            let runtime = Arc::new(
                tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .unwrap(),
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
    let mut join_set = JoinSet::new();
    run(config, prometheus, runtime.handle().clone(), &mut join_set).await?;

    while let Some(res) = join_set.join_next().await {
        res??
    }
    Ok(())
}
