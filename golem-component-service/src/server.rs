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

use golem_common::config::DbConfig;
use golem_common::tracing::init_tracing_with_default_env_filter;
use golem_component_service::api::make_open_api_service;
use golem_component_service::config::{make_config_loader, ComponentServiceConfig};
use golem_component_service::service::Services;
use golem_component_service::{api, async_main, grpcapi, metrics};
use golem_service_base::db;
use opentelemetry::global;
use poem::listener::TcpListener;
use poem::middleware::{OpenTelemetryMetrics, Tracing};
use poem::EndpointExt;
use prometheus::Registry;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::Path;
use std::sync::Arc;
use tokio::select;
use tracing::{error, info};

fn main() -> Result<(), std::io::Error> {
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
            opentelemetry_sdk::metrics::MeterProviderBuilder::default()
                .with_reader(exporter)
                .build(),
        );

        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?
            .block_on(async_main(&config, prometheus, Path::new("./db/migration")))
    } else {
        Ok(())
    }
}

async fn dump_openapi_yaml() -> Result<(), std::io::Error> {
    let config = ComponentServiceConfig::default();
    let services = Services::new(&config).await.map_err(|e| {
        error!("Services - init error: {}", e);
        std::io::Error::new(std::io::ErrorKind::Other, e)
    })?;
    let service = make_open_api_service(&services);
    println!("{}", service.spec_yaml());
    Ok(())
}
