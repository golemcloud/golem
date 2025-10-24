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

pub mod additional_deps;
pub mod api;
pub mod auth;
pub mod config;
pub mod debug_context;
pub mod debug_session;
pub mod from_value;
pub mod jrpc;
pub mod model;
pub mod oplog;
pub mod services;

use crate::additional_deps::AdditionalDeps;
use crate::auth::{AuthService, GrpcAuthService};
use crate::config::DebugConfig;
use crate::debug_context::DebugContext;
use crate::debug_session::{DebugSessions, DebugSessionsDefault};
use crate::oplog::debug_oplog_service::DebugOplogService;
use anyhow::{anyhow, Error};
use async_trait::async_trait;
use golem_service_base::clients::auth::AuthService as BaseAuthService;
use golem_service_base::storage::blob::BlobStorage;
use golem_worker_executor::durable_host::DurableWorkerCtx;
use golem_worker_executor::preview2::{golem_agent, golem_api_1_x, golem_durability};
use golem_worker_executor::services::active_workers::ActiveWorkers;
use golem_worker_executor::services::agent_types::AgentTypesService;
use golem_worker_executor::services::blob_store::BlobStoreService;
use golem_worker_executor::services::component::ComponentService;
use golem_worker_executor::services::events::Events;
use golem_worker_executor::services::file_loader::FileLoader;
use golem_worker_executor::services::golem_config::GolemConfig;
use golem_worker_executor::services::key_value::KeyValueService;
use golem_worker_executor::services::oplog::plugin::OplogProcessorPlugin;
use golem_worker_executor::services::oplog::OplogService;
use golem_worker_executor::services::plugins::{Plugins, PluginsObservations};
use golem_worker_executor::services::projects::ProjectService;
use golem_worker_executor::services::promise::PromiseService;
use golem_worker_executor::services::rpc::{DirectWorkerInvocationRpc, RemoteInvocationRpc};
use golem_worker_executor::services::scheduler::SchedulerService;
use golem_worker_executor::services::shard::ShardService;
use golem_worker_executor::services::shard_manager::ShardManagerService;
use golem_worker_executor::services::worker::WorkerService;
use golem_worker_executor::services::worker_activator::{LazyWorkerActivator, WorkerActivator};
use golem_worker_executor::services::worker_enumeration::{
    RunningWorkerEnumerationService, WorkerEnumerationService,
};
use golem_worker_executor::services::worker_fork::DefaultWorkerFork;
use golem_worker_executor::services::worker_proxy::WorkerProxy;
use golem_worker_executor::services::{rdbms, resource_limits, All};
use golem_worker_executor::wasi_host::create_linker;
use golem_worker_executor::{create_worker_executor_impl, Bootstrap, RunDetails};
use humansize::ISizeFormatter;
use poem::endpoint::PrometheusExporter;
use poem::listener::{Acceptor, Listener};
use poem::middleware::{CookieJarManager, Cors};
use poem::EndpointExt;
use poem::Route;
use prometheus::Registry;
use std::sync::Arc;
use tokio::runtime::Handle;
use tokio::task::JoinSet;
use tracing::{debug, info, Instrument};
use wasmtime::component::Linker;
use wasmtime::Engine;

#[cfg(test)]
test_r::enable!();

pub struct ServerBootstrap {
    pub debug_config: DebugConfig,
}

#[async_trait]
impl Bootstrap<DebugContext> for ServerBootstrap {
    fn create_active_workers(
        &self,
        golem_config: &GolemConfig,
    ) -> Arc<ActiveWorkers<DebugContext>> {
        Arc::new(ActiveWorkers::<DebugContext>::new(&golem_config.memory))
    }

    fn create_component_service(
        &self,
        golem_config: &GolemConfig,
        blob_storage: Arc<dyn BlobStorage>,
        plugins: Arc<dyn PluginsObservations>,
        project_service: Arc<dyn ProjectService>,
    ) -> Arc<dyn ComponentService> {
        golem_worker_executor::services::component::configured(
            &golem_config.component_service,
            &golem_config.component_cache,
            &golem_config.compiled_component_service,
            blob_storage,
            plugins,
            project_service,
        )
    }

    async fn run_grpc_server(
        &self,
        _service_dependencies: All<DebugContext>,
        _lazy_worker_activator: Arc<LazyWorkerActivator<DebugContext>>,
        _join_set: &mut JoinSet<Result<(), Error>>,
    ) -> anyhow::Result<u16> {
        Err(anyhow!("No grpc server in debugging service"))
    }

    fn create_plugins(
        &self,
        golem_config: &GolemConfig,
    ) -> (Arc<dyn Plugins>, Arc<dyn PluginsObservations>) {
        let plugins =
            golem_worker_executor::services::plugins::configured(&golem_config.plugin_service);
        (plugins.clone(), plugins)
    }

    async fn create_services(
        &self,
        active_workers: Arc<ActiveWorkers<DebugContext>>,
        engine: Arc<Engine>,
        linker: Arc<Linker<DebugContext>>,
        runtime: Handle,
        component_service: Arc<dyn ComponentService>,
        shard_manager_service: Arc<dyn ShardManagerService>,
        worker_service: Arc<dyn WorkerService>,
        worker_enumeration_service: Arc<dyn WorkerEnumerationService>,
        running_worker_enumeration_service: Arc<dyn RunningWorkerEnumerationService>,
        promise_service: Arc<dyn PromiseService>,
        golem_config: Arc<GolemConfig>,
        shard_service: Arc<dyn ShardService>,
        key_value_service: Arc<dyn KeyValueService>,
        blob_store_service: Arc<dyn BlobStoreService>,
        rdbms_service: Arc<dyn rdbms::RdbmsService>,
        worker_activator: Arc<dyn WorkerActivator<DebugContext>>,
        oplog_service: Arc<dyn OplogService>,
        scheduler_service: Arc<dyn SchedulerService>,
        worker_proxy: Arc<dyn WorkerProxy>,
        events: Arc<Events>,
        file_loader: Arc<FileLoader>,
        plugins: Arc<dyn Plugins>,
        oplog_processor_plugin: Arc<dyn OplogProcessorPlugin>,
        project_service: Arc<dyn ProjectService>,
        agent_types_service: Arc<dyn AgentTypesService>,
    ) -> anyhow::Result<All<DebugContext>> {
        let remote_cloud_service_config = self.debug_config.cloud_service.clone();

        let auth_service: Arc<dyn AuthService> = Arc::new(GrpcAuthService::new(
            BaseAuthService::new(&remote_cloud_service_config),
            self.debug_config.component_service.clone(),
        ));

        let debug_sessions: Arc<dyn DebugSessions> = Arc::new(DebugSessionsDefault::default());

        let debug_oplog_service = Arc::new(DebugOplogService::new(
            Arc::clone(&oplog_service),
            Arc::clone(&debug_sessions),
        ));

        let addition_deps = AdditionalDeps::new(auth_service, debug_sessions);

        let resource_limits = resource_limits::configured(&golem_config.resource_limits).await;

        // When it comes to fork, we need the original oplog service
        let worker_fork = Arc::new(DefaultWorkerFork::new(
            Arc::new(RemoteInvocationRpc::new(
                worker_proxy.clone(),
                shard_service.clone(),
            )),
            active_workers.clone(),
            engine.clone(),
            linker.clone(),
            runtime.clone(),
            component_service.clone(),
            shard_manager_service.clone(),
            worker_service.clone(),
            worker_proxy.clone(),
            worker_enumeration_service.clone(),
            running_worker_enumeration_service.clone(),
            promise_service.clone(),
            golem_config.clone(),
            shard_service.clone(),
            key_value_service.clone(),
            blob_store_service.clone(),
            rdbms_service.clone(),
            // When it comes to fork, it reads using the debug oplog service
            // (the worker instance's oplog) but writes using the live oplog service)
            oplog_service.clone(),
            scheduler_service.clone(),
            worker_activator.clone(),
            events.clone(),
            file_loader.clone(),
            plugins.clone(),
            oplog_processor_plugin.clone(),
            resource_limits.clone(),
            project_service.clone(),
            agent_types_service.clone(),
            addition_deps.clone(),
        ));

        let rpc = Arc::new(DirectWorkerInvocationRpc::new(
            Arc::new(RemoteInvocationRpc::new(
                worker_proxy.clone(),
                shard_service.clone(),
            )),
            active_workers.clone(),
            engine.clone(),
            linker.clone(),
            runtime.clone(),
            component_service.clone(),
            worker_fork.clone(),
            worker_service.clone(),
            worker_enumeration_service.clone(),
            running_worker_enumeration_service.clone(),
            promise_service.clone(),
            golem_config.clone(),
            shard_service.clone(),
            shard_manager_service.clone(),
            key_value_service.clone(),
            blob_store_service.clone(),
            rdbms_service.clone(),
            debug_oplog_service.clone(),
            scheduler_service.clone(),
            worker_activator.clone(),
            events.clone(),
            file_loader.clone(),
            plugins.clone(),
            oplog_processor_plugin.clone(),
            resource_limits.clone(),
            project_service.clone(),
            agent_types_service.clone(),
            addition_deps.clone(),
        ));

        Ok(All::new(
            active_workers,
            agent_types_service,
            engine,
            linker,
            runtime.clone(),
            component_service,
            shard_manager_service,
            worker_fork,
            worker_service,
            worker_enumeration_service,
            running_worker_enumeration_service,
            promise_service,
            golem_config.clone(),
            shard_service,
            key_value_service,
            blob_store_service,
            rdbms_service,
            debug_oplog_service,
            rpc,
            scheduler_service,
            worker_activator.clone(),
            worker_proxy.clone(),
            events.clone(),
            file_loader.clone(),
            plugins.clone(),
            oplog_processor_plugin.clone(),
            resource_limits,
            project_service.clone(),
            addition_deps,
        ))
    }

    fn create_wasmtime_linker(&self, engine: &Engine) -> anyhow::Result<Linker<DebugContext>> {
        create_debug_wasmtime_linker(engine)
    }

    async fn run(
        &self,
        golem_config: GolemConfig,
        prometheus_registry: Registry,
        runtime: Handle,
        join_set: &mut JoinSet<Result<(), Error>>,
    ) -> anyhow::Result<RunDetails> {
        run_debug_worker_executor(
            self,
            golem_config,
            &self.debug_config.cors_origin_regex,
            prometheus_registry,
            runtime,
            join_set,
        )
        .await
    }
}

pub async fn run_debug_worker_executor<T: Bootstrap<DebugContext> + ?Sized>(
    bootstrap: &T,
    golem_config: GolemConfig,
    cors_origin_regex: &str,
    prometheus_registry: Registry,
    runtime: Handle,
    join_set: &mut JoinSet<Result<(), Error>>,
) -> anyhow::Result<RunDetails> {
    debug!("Initializing debug worker executor");

    let total_system_memory = golem_config.memory.total_system_memory();
    let system_memory = golem_config.memory.system_memory();
    let worker_memory = golem_config.memory.worker_memory();
    info!(
        "Total system memory: {}, Available system memory: {}, Total memory available for workers: {}",
        ISizeFormatter::new(total_system_memory, humansize::BINARY),
        ISizeFormatter::new(system_memory, humansize::BINARY),
        ISizeFormatter::new(worker_memory, humansize::BINARY)
    );

    let lazy_worker_activator = Arc::new(LazyWorkerActivator::new());

    let (worker_executor_impl, epoch_thread) = create_worker_executor_impl::<DebugContext, T>(
        golem_config.clone(),
        bootstrap,
        runtime.clone(),
        &lazy_worker_activator,
    )
    .await?;

    let http_port = start_http_server(
        &worker_executor_impl,
        golem_config.http_port,
        cors_origin_regex,
        &prometheus_registry,
        join_set,
    )
    .await?;

    Ok(RunDetails {
        http_port,
        // TODO: no grpc config running, this should not be exposed here
        grpc_port: golem_config.port,
        epoch_thread: std::sync::Mutex::new(Some(epoch_thread)),
    })
}

fn get_durable_ctx(ctx: &mut DebugContext) -> &mut DurableWorkerCtx<DebugContext> {
    &mut ctx.durable_ctx
}

pub fn create_debug_wasmtime_linker(engine: &Engine) -> anyhow::Result<Linker<DebugContext>> {
    let mut linker = create_linker(engine, get_durable_ctx)?;
    golem_api_1_x::host::add_to_linker_get_host(&mut linker, get_durable_ctx)?;
    golem_api_1_x::oplog::add_to_linker_get_host(&mut linker, get_durable_ctx)?;
    golem_api_1_x::context::add_to_linker_get_host(&mut linker, get_durable_ctx)?;
    golem_durability::durability::add_to_linker_get_host(&mut linker, get_durable_ctx)?;
    golem_agent::host::add_to_linker_get_host(&mut linker, get_durable_ctx)?;
    golem_wasm::golem_rpc_0_2_x::types::add_to_linker_get_host(&mut linker, get_durable_ctx)?;
    Ok(linker)
}

async fn start_http_server(
    services: &All<DebugContext>,
    http_port: u16,
    cors_origin_regex: &str,
    prometheus_registry: &Registry,
    join_set: &mut JoinSet<Result<(), Error>>,
) -> Result<u16, Error> {
    let open_api_service = api::make_open_api_service(services);

    let ui = open_api_service.swagger_ui();
    let spec = open_api_service.spec_endpoint_yaml();
    let metrics = PrometheusExporter::new(prometheus_registry.clone());

    let cors = Cors::new()
        .allow_origin_regex(cors_origin_regex)
        .allow_credentials(true);

    let app = Route::new()
        .nest("/", open_api_service)
        .nest("/docs", ui)
        .nest("/specs", spec)
        .nest("/metrics", metrics)
        .with(CookieJarManager::new())
        .with(cors);

    let poem_listener = poem::listener::TcpListener::bind(format!("0.0.0.0:{http_port}"));

    let acceptor = poem_listener.into_acceptor().await?;
    let port = acceptor.local_addr()[0]
        .as_socket_addr()
        .expect("socket address")
        .port();

    join_set.spawn(
        async move {
            poem::Server::new_with_acceptor(acceptor)
                .run(app)
                .await
                .map_err(|e| e.into())
        }
        .in_current_span(),
    );

    Ok(port)
}

pub async fn run(
    debug_config: DebugConfig,
    prometheus_registry: Registry,
    runtime: Handle,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("Golem Debug Worker Executor starting up...");
    let mut join_set = JoinSet::new();
    ServerBootstrap {
        debug_config: debug_config.clone(),
    }
    .run(
        debug_config.into_golem_config(),
        prometheus_registry,
        runtime,
        &mut join_set,
    )
    .await?;

    while let Some(res) = join_set.join_next().await {
        res??
    }
    Ok(())
}
