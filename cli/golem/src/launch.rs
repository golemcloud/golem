// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use crate::StartedComponents;
use crate::compat::{preflight_registry_db_compat, write_registry_db_compat};
use crate::config::{GolemServerConfig, INTERNAL_HOST};
use crate::router::bind_router;
use crate::well_known_secrets::{
    accounts_with_well_known_token, check_exposure, revoke_well_known_token,
};
use anyhow::Context;
use golem_component_compilation_service::config::ServerConfig as ComponentCompilationServiceConfig;
use golem_registry_service::RegistryService;
use golem_registry_service::config::{ComponentCompilationConfig, RegistryServiceConfig};
use golem_registry_service::startup::{StartupPhase, StartupProgress};
use golem_shard_manager::config::ShardManagerConfig;
use golem_worker_executor::services::golem_config::GolemConfig as WorkerExecutorConfig;
use golem_worker_executor::services::shutdown::Shutdown;
use golem_worker_service::WorkerService;
use golem_worker_service::config::WorkerServiceConfig;
use opentelemetry::global;
use opentelemetry_sdk::metrics::MeterProviderBuilder;
use serde::Serialize;
use std::path::Path;
use tokio::runtime::Handle;
use tokio::task::JoinSet;
use tracing::{Instrument, info, warn};

/// The ports the local server actually bound, which differ from the requested ones when those
/// were `0` (OS-assigned).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartupPorts {
    pub router_port: u16,
    pub custom_request_port: u16,
    pub mcp_port: u16,
}

pub async fn launch_golem_services(
    config: &GolemServerConfig,
) -> anyhow::Result<(JoinSet<anyhow::Result<()>>, StartupPorts, Shutdown)> {
    check_exposure(config)?;

    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install crypto provider");

    let exporter = opentelemetry_prometheus_text_exporter::ExporterBuilder::default()
        .without_counter_suffixes()
        .without_units()
        .build();

    global::set_meter_provider(
        MeterProviderBuilder::default()
            .with_reader(exporter)
            .build(),
    );

    let mut join_set: JoinSet<anyhow::Result<()>> = JoinSet::new();

    tokio::fs::create_dir_all(&config.data_dir)
        .await
        .with_context(|| {
            format!(
                "Failed to create data directory at {}",
                config.data_dir.display()
            )
        })?;

    let uses_local_registry_db = config.uses_local_registry_db();
    if uses_local_registry_db {
        preflight_registry_db_compat(&config.data_dir)
            .await
            .map_err(anyhow::Error::from)?;
    }

    let startup_progress = StartupProgress::default();
    let router = bind_router(
        &config.router.address,
        config.router.port,
        None,
        startup_progress.clone(),
        &mut join_set,
    )
    .await?;
    let router_port = router.port();
    info!(
        router_port,
        "Golem server is starting, requests are answered with 503 Service Unavailable until it is ready"
    );

    let started_components = start_components(config, &startup_progress, &mut join_set).await?;
    if uses_local_registry_db {
        write_registry_db_compat(&config.data_dir).await?;
    }

    if accounts_with_well_known_token(config).is_empty()
        && revoke_well_known_token(&started_components.registry_service.endpoint).await?
    {
        warn!(
            "Deleted the well-known local admin token from the registry, because no initial account is configured with it"
        );
    }

    let custom_request_port = started_components.worker_service.custom_request_port;
    let mcp_port = started_components.worker_service.mcp_port;
    let worker_shutdown = started_components.worker_executor.shutdown.clone();

    router.serve(
        started_components,
        &config.registry_service.cors_origin_regex,
    );

    let startup_ports = StartupPorts {
        router_port,
        custom_request_port,
        mcp_port,
    };
    if let Some(ports_file) = config.ports_file.as_ref() {
        write_startup_ports_file(ports_file, &startup_ports).await?;
    }

    info!(
        router_port,
        custom_request_port, mcp_port, "Golem server is ready"
    );

    Ok((join_set, startup_ports, worker_shutdown))
}

async fn write_startup_ports_file(path: &Path, ports: &StartupPorts) -> anyhow::Result<()> {
    let temp_path = path.with_extension("tmp");
    let content = serde_json::to_vec_pretty(ports)?;
    tokio::fs::write(&temp_path, content).await?;
    tokio::fs::rename(&temp_path, path).await?;
    info!(path = %path.display(), "Wrote startup ports file");
    Ok(())
}

async fn start_components(
    config: &GolemServerConfig,
    startup_progress: &StartupProgress,
    join_set: &mut JoinSet<anyhow::Result<()>>,
) -> Result<StartedComponents, anyhow::Error> {
    let component_compilation_service_config = config.component_compilation_service.clone();
    golem_service_base::observability::install_runtime_metrics(
        Handle::current(),
        prometheus::default_registry().clone(),
        component_compilation_service_config.runtime_metrics_sampling_interval,
        join_set,
    );
    let component_compilation_service =
        run_component_compilation_service(component_compilation_service_config, join_set).await?;

    let registry_service = run_registry_service(
        registry_service_config(config, &component_compilation_service),
        startup_progress.clone(),
        join_set,
    )
    .await?;
    startup_progress.set_phase(StartupPhase::StartingServices);

    let shard_manager =
        run_shard_manager(shard_manager_config(config, &registry_service), join_set).await?;

    let worker_service = run_worker_service(
        worker_service_config(config, &shard_manager, &registry_service),
        join_set,
    )
    .await?;

    let worker_executor = run_worker_executor(
        worker_executor_config(config, &shard_manager, &registry_service, &worker_service),
        join_set,
    )
    .await?;

    Ok(StartedComponents {
        registry_service,
        shard_manager,
        worker_executor,
        worker_service,
        prometheus_registry: prometheus::default_registry().clone(),
    })
}

fn registry_service_config(
    config: &GolemServerConfig,
    component_compilation_service: &golem_component_compilation_service::RunDetails,
) -> RegistryServiceConfig {
    let mut config = config.registry_service.clone();
    if let ComponentCompilationConfig::Enabled(compilation) = &mut config.component_compilation {
        compilation.host = INTERNAL_HOST.to_string();
        compilation.port = component_compilation_service.grpc_port;
    }
    config
}

fn shard_manager_config(
    config: &GolemServerConfig,
    registry_service: &golem_registry_service::SingleExecutableRunDetails,
) -> ShardManagerConfig {
    let mut config = config.shard_manager.clone();
    config.registry_service.host = INTERNAL_HOST.to_string();
    config.registry_service.port = registry_service.grpc_port;
    config
}

fn worker_service_config(
    config: &GolemServerConfig,
    shard_manager: &golem_shard_manager::RunDetails,
    registry_service: &golem_registry_service::SingleExecutableRunDetails,
) -> WorkerServiceConfig {
    let mut config = config.worker_service.clone();
    config.shard_manager.host = INTERNAL_HOST.to_string();
    config.shard_manager.port = shard_manager.grpc_port;
    config.registry_service.host = INTERNAL_HOST.to_string();
    config.registry_service.port = registry_service.grpc_port;
    config
}

fn worker_executor_config(
    config: &GolemServerConfig,
    shard_manager: &golem_shard_manager::RunDetails,
    registry_service: &golem_registry_service::SingleExecutableRunDetails,
    worker_service: &golem_worker_service::TrafficReadyEndpoints,
) -> WorkerExecutorConfig {
    let mut config = config.worker_executor.clone();
    config.shard_manager.host = INTERNAL_HOST.to_string();
    config.shard_manager.port = shard_manager.grpc_port;
    config.registry_service.host = INTERNAL_HOST.to_string();
    config.registry_service.port = registry_service.grpc_port;
    config.public_worker_api.host = INTERNAL_HOST.to_string();
    config.public_worker_api.port = worker_service.grpc_port;
    config.add_port_to_tracing_file_name_if_enabled();
    config
}

async fn run_shard_manager(
    config: ShardManagerConfig,
    join_set: &mut JoinSet<anyhow::Result<()>>,
) -> Result<golem_shard_manager::RunDetails, anyhow::Error> {
    let prometheus_registry = prometheus::default_registry().clone();
    let span = tracing::info_span!("shard-manager");
    golem_shard_manager::run(
        &config,
        golem_shard_manager::Deployment::Embedded,
        prometheus_registry,
        join_set,
    )
    .instrument(span)
    .await
}

async fn run_component_compilation_service(
    config: ComponentCompilationServiceConfig,
    join_set: &mut JoinSet<anyhow::Result<()>>,
) -> Result<golem_component_compilation_service::RunDetails, anyhow::Error> {
    let prometheus_registry = golem_component_compilation_service::metrics::register_all();
    let span = tracing::info_span!("component-compilation-service");
    golem_component_compilation_service::run(config, prometheus_registry, join_set)
        .instrument(span)
        .await
}

async fn run_registry_service(
    config: RegistryServiceConfig,
    startup_progress: StartupProgress,
    join_set: &mut JoinSet<anyhow::Result<()>>,
) -> Result<golem_registry_service::SingleExecutableRunDetails, anyhow::Error> {
    let prometheus_registry = golem_registry_service::metrics::register_all();
    let span = tracing::info_span!("registry-service", component = "registry-service");
    RegistryService::new(config, prometheus_registry)
        .with_startup_progress(startup_progress)
        .start_for_single_executable(join_set)
        .instrument(span)
        .await
}

async fn run_worker_executor(
    config: WorkerExecutorConfig,
    join_set: &mut JoinSet<anyhow::Result<()>>,
) -> Result<golem_worker_executor::RunDetails, anyhow::Error> {
    let prometheus_registry = golem_worker_executor::metrics::register_all();

    let span = tracing::info_span!("worker-executor");
    golem_worker_executor::bootstrap::run(config, prometheus_registry, Handle::current(), join_set)
        .instrument(span)
        .await
}

async fn run_worker_service(
    config: WorkerServiceConfig,
    join_set: &mut JoinSet<anyhow::Result<()>>,
) -> Result<golem_worker_service::TrafficReadyEndpoints, anyhow::Error> {
    let prometheus_registry = golem_worker_executor::metrics::register_all();
    let span = tracing::info_span!("worker-service");
    WorkerService::new(config, prometheus_registry)
        .instrument(span.clone())
        .await?
        .start_endpoints(join_set, None)
        .instrument(span)
        .await
}
