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
use golem_component_service::config::{make_config_loader, ComponentServiceConfig};
use golem_component_service::{metrics, ComponentService};
use golem_service_base::migration::MigrationsDir;
use opentelemetry::global;
use prometheus::Registry;

fn main() -> Result<(), anyhow::Error> {
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
            .build()?;

        global::set_meter_provider(
            opentelemetry_sdk::metrics::MeterProviderBuilder::default()
                .with_reader(exporter)
                .build(),
        );

        Ok(tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?
            .block_on(run(config, prometheus))?)
    } else {
        Ok(())
    }
}

async fn run(config: ComponentServiceConfig, prometheus: Registry) -> Result<(), anyhow::Error> {
    let server = ComponentService::new(
        config,
        prometheus,
        MigrationsDir::new("./db/migration".into()),
    )
    .await?;

    let mut join_set = tokio::task::JoinSet::new();

    server.run(&mut join_set).await?;

    while let Some(res) = join_set.join_next().await {
        res??;
    }

    Ok(())
}

async fn dump_openapi_yaml() -> Result<(), anyhow::Error> {
    let config = ComponentServiceConfig::default();
    let service = ComponentService::new(
        config,
        Registry::default(),
        MigrationsDir::new("../../golem-component-service/db/migration".into()),
    )
    .await?;
    let yaml = service.http_service().spec_yaml();
    println!("{yaml}");
    Ok(())
}
