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

use anyhow::anyhow;
use golem_common::tracing::init_tracing_with_default_env_filter;
use golem_worker_service::config::{make_worker_service_config_loader, WorkerServiceConfig};
use golem_worker_service::service::Services;
use golem_worker_service::WorkerService;
use opentelemetry::global;
use opentelemetry_sdk::metrics::MeterProviderBuilder;
use prometheus::Registry;
use tracing::info;

fn main() -> anyhow::Result<()> {
    if std::env::args().any(|arg| arg == "--dump-openapi-yaml") {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?
            .block_on(dump_openapi_yaml())
    } else if let Some(config) = make_worker_service_config_loader().load_or_dump_config() {
        init_tracing_with_default_env_filter(&config.tracing);

        if config.is_local_env() {
            info!("Golem Worker Service starting up (local mode)...");
        } else {
            info!("Golem Worker Service starting up...");
        }

        let prometheus_registry = prometheus::Registry::new();

        let exporter = opentelemetry_prometheus::exporter()
            .with_registry(prometheus_registry.clone())
            .build()
            .unwrap();

        global::set_meter_provider(
            MeterProviderBuilder::default()
                .with_reader(exporter)
                .build(),
        );

        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?
            .block_on(async_main(config, prometheus_registry))
    } else {
        Ok(())
    }
}

pub async fn dump_openapi_yaml() -> anyhow::Result<()> {
    let config = WorkerServiceConfig::default();
    let services = Services::new(&config)
        .await
        .map_err(|e| anyhow!("Services - init error: {}", e))?;
    let open_api_service = golem_worker_service::api::make_open_api_service(&services);
    println!("{}", open_api_service.spec_yaml());
    Ok(())
}

async fn async_main(config: WorkerServiceConfig, prometheus: Registry) -> anyhow::Result<()> {
    let server = WorkerService::new(config, prometheus).await?;

    let mut join_set = tokio::task::JoinSet::new();

    server.run(&mut join_set).await?;

    while let Some(res) = join_set.join_next().await {
        res??;
    }

    Ok(())
}
