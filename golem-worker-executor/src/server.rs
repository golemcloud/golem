// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

#[allow(unused_imports)]
use std::sync::Arc;

use golem_common::tracing::init_tracing_with_default_env_filter;
use golem_worker_executor::metrics;
use golem_worker_executor::oss::run;
use golem_worker_executor::services::additional_config::{
    load_or_dump_config, DefaultAdditionalGolemConfig,
};
use golem_worker_executor::services::golem_config::GolemConfig;
use tokio::task::JoinSet;

fn main() -> Result<(), anyhow::Error> {
    match load_or_dump_config() {
        Some((mut config, additional_config)) => {
            config.add_port_to_tracing_file_name_if_enabled();
            init_tracing_with_default_env_filter(&config.tracing);

            let prometheus = metrics::register_all();

            let runtime = Arc::new(
                tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()?,
            );

            runtime.block_on(async_main(
                config,
                additional_config,
                prometheus,
                runtime.clone(),
            ))
        }
        None => Ok(()),
    }
}

async fn async_main(
    config: GolemConfig,
    additional_config: DefaultAdditionalGolemConfig,
    prometheus: prometheus::Registry,
    runtime: Arc<tokio::runtime::Runtime>,
) -> Result<(), anyhow::Error> {
    let mut join_set = JoinSet::new();
    run(
        config,
        additional_config,
        prometheus,
        runtime.handle().clone(),
        &mut join_set,
    )
    .await?;

    while let Some(res) = join_set.join_next().await {
        res??
    }
    Ok(())
}

// use std::sync::Arc;
//
// use golem_common::tracing::init_tracing_with_default_env_filter;
// use golem_worker_executor::metrics as base_metrics;
//
// use cloud_worker_executor::run;
// use cloud_worker_executor::services::config::load_or_dump_config;
//
// fn main() -> Result<(), Box<dyn std::error::Error>> {
//     match load_or_dump_config() {
//         Some((config, additional_config)) => {
//             init_tracing_with_default_env_filter(&config.tracing);
//
//             let prometheus = base_metrics::register_all();
//
//             let runtime = Arc::new(
//                 tokio::runtime::Builder::new_multi_thread()
//                     .enable_all()
//                     .build()
//                     .unwrap(),
//             );
//
//             runtime.block_on(run(
//                 config,
//                 Arc::new(additional_config),
//                 prometheus,
//                 runtime.handle().clone(),
//             ))
//         }
//         None => Ok(()),
//     }
// }
