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

use golem_common::SafeDisplay;
use golem_common::tracing::init_tracing_with_default_env_filter;
use golem_worker_service::WorkerService;
use golem_worker_service::bootstrap::Services;
use golem_worker_service::config::{WorkerServiceConfig, make_worker_service_config_loader};
use opentelemetry::global;
use opentelemetry_sdk::metrics::MeterProviderBuilder;
use opentelemetry_sdk::trace::SdkTracer;
use prometheus::Registry;
use tracing::info;

fn main() -> anyhow::Result<()> {
    if std::env::args().any(|arg| arg == "--dump-openapi-yaml") {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?
            .block_on(dump_openapi_yaml())
    } else if let Some(config) = make_worker_service_config_loader().load_or_dump_config() {
        rustls::crypto::ring::default_provider()
            .install_default()
            .expect("Failed to install crypto provider");

        let tracer = init_tracing_with_default_env_filter(&config.tracing);
        info!("Using configuration:\n{}", config.to_safe_string_indented());

        let prometheus_registry = prometheus::Registry::new();

        let exporter = opentelemetry_prometheus_text_exporter::ExporterBuilder::default()
            .without_counter_suffixes()
            .without_units()
            .build();

        global::set_meter_provider(
            MeterProviderBuilder::default()
                .with_reader(exporter)
                .build(),
        );

        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?
            .block_on(async_main(config, prometheus_registry, tracer))
    } else {
        Ok(())
    }
}

pub async fn dump_openapi_yaml() -> anyhow::Result<()> {
    let config = WorkerServiceConfig::default();
    let services = Services::new(&config).await?;
    let open_api_service = golem_worker_service::api::make_open_api_service(&services);
    println!("{}", open_api_service.spec_yaml());
    Ok(())
}

async fn async_main(
    config: WorkerServiceConfig,
    prometheus: Registry,
    tracer: Option<SdkTracer>,
) -> anyhow::Result<()> {
    let server = WorkerService::new(config, prometheus).await?;

    let mut join_set = tokio::task::JoinSet::new();

    server.run(&mut join_set, tracer).await?;

    while let Some(res) = join_set.join_next().await {
        res??;
    }

    Ok(())
}
