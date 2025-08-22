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
use golem_registry_service::api::make_open_api_service;
use golem_registry_service::bootstrap::Services;
use golem_registry_service::config::{RegistryServiceConfig, make_config_loader};
use golem_registry_service::{RegistryService, metrics};
use opentelemetry::global;
use opentelemetry_sdk::metrics::MeterProviderBuilder;
use prometheus::Registry;
use std::panic;
use tokio::task::JoinSet;

fn main() -> anyhow::Result<()> {
    if std::env::args().any(|arg| arg == "--dump-openapi-yaml") {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?
            .block_on(dump_openapi_yaml())
    } else if let Some(config) = make_config_loader().load_or_dump_config() {
        init_tracing_with_default_env_filter(&config.tracing);

        let prometheus = metrics::register_all();

        let exporter = opentelemetry_prometheus::exporter()
            .with_registry(prometheus.clone())
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
            .block_on(async_main(config, prometheus))
    } else {
        Ok(())
    }
}

async fn dump_openapi_yaml() -> anyhow::Result<()> {
    let config = RegistryServiceConfig::default();
    let services = Services::new(&config).await?;

    let open_api_service = make_open_api_service(&services);
    println!("{}", open_api_service.spec_yaml());
    Ok(())
}

async fn async_main(
    config: RegistryServiceConfig,
    prometheus_registry: Registry,
) -> anyhow::Result<()> {
    let bootstrap = RegistryService::new(config, prometheus_registry).await?;

    let mut join_set = JoinSet::<anyhow::Result<()>>::new();

    bootstrap.start(&mut join_set).await?;

    while let Some(res) = join_set.join_next().await {
        match res {
            Ok(Ok(())) => {}
            Ok(Err(err)) => Err(err)?,
            Err(err) if err.is_panic() => panic::resume_unwind(err.into_panic()),
            Err(err) => Err(err)?,
        }
    }

    Ok(())
}
