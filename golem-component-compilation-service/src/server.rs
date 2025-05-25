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

use golem_common::tracing::init_tracing_with_default_env_filter;
use golem_component_compilation_service::config::{make_config_loader, ServerConfig};
use prometheus::Registry;
use tokio::task::JoinSet;
use wasmtime::component::__internal::anyhow;

pub fn main() -> anyhow::Result<()> {
    match make_config_loader().load_or_dump_config() {
        Some(config) => {
            init_tracing_with_default_env_filter(&config.tracing);
            let prometheus = golem_component_compilation_service::metrics::register_all();

            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async_main(config, prometheus))
        }
        None => Ok(()),
    }
}

async fn async_main(config: ServerConfig, prometheus: Registry) -> anyhow::Result<()> {
    let mut join_set = JoinSet::new();
    golem_component_compilation_service::run(config, prometheus, &mut join_set).await?;

    while let Some(res) = join_set.join_next().await {
        res??
    }
    Ok(())
}
