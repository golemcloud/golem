use crate::additional_deps::AdditionalDeps;
use crate::auth::{AuthService, AuthServiceDefault};
use crate::config::DebugConfig;
use crate::debug_context::DebugContext;
use crate::debug_session::{DebugSessions, DebugSessionsDefault};
use crate::oplog::debug_oplog_service::DebugOplogService;
use crate::services::debug_service::DebugServiceDefault;
use anyhow::{Context, Error};
use async_trait::async_trait;
use axum::routing::any;
use axum::Router;
use cloud_common::clients::auth::CloudAuthService;
use golem_service_base::storage::blob::BlobStorage;
use golem_worker_executor::cloud::CloudGolemTypes;
use golem_worker_executor::durable_host::DurableWorkerCtx;
use golem_worker_executor::preview2::{golem_api_1_x, golem_durability};
use golem_worker_executor::services::active_workers::ActiveWorkers;
use golem_worker_executor::services::blob_store::BlobStoreService;
use golem_worker_executor::services::component::ComponentService;
use golem_worker_executor::services::events::Events;
use golem_worker_executor::services::file_loader::FileLoader;
use golem_worker_executor::services::golem_config::GolemConfig;
use golem_worker_executor::services::key_value::KeyValueService;
use golem_worker_executor::services::oplog::plugin::OplogProcessorPlugin;
use golem_worker_executor::services::oplog::OplogService;
use golem_worker_executor::services::plugins::{Plugins, PluginsObservations};
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
use golem_worker_executor::services::{rdbms, resource_limits, All, HasConfig};
use golem_worker_executor::wasi_host::create_linker;
use golem_worker_executor::{Bootstrap, GolemTypes};
use prometheus::Registry;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::runtime::Handle;
use tokio::task::JoinSet;
use tracing::{info, Instrument};
use wasmtime::component::Linker;
use wasmtime::Engine;

#[cfg(test)]
test_r::enable!();

pub mod additional_deps;
pub mod auth;
pub mod config;
pub mod debug_context;
pub mod debug_request;
pub mod debug_session;
pub mod from_value;
pub mod jrpc;
pub mod model;
pub mod oplog;
pub mod services;
pub mod websocket;

pub struct ServerBootstrap {
    pub debug_config: DebugConfig,
}

#[async_trait]
impl Bootstrap<DebugContext<CloudGolemTypes>> for ServerBootstrap {
    fn create_active_workers(
        &self,
        golem_config: &GolemConfig,
    ) -> Arc<ActiveWorkers<DebugContext<CloudGolemTypes>>> {
        Arc::new(ActiveWorkers::<DebugContext<CloudGolemTypes>>::new(
            &golem_config.memory,
        ))
    }

    fn create_component_service(
        &self,
        golem_config: &GolemConfig,
        blob_storage: Arc<dyn BlobStorage + Send + Sync>,
        plugins: Arc<dyn PluginsObservations>,
    ) -> Arc<dyn ComponentService<CloudGolemTypes>> {
        golem_worker_executor::services::cloud::component::configured(
            &golem_config.component_service,
            &golem_config.project_service,
            &golem_config.component_cache,
            &golem_config.compiled_component_service,
            blob_storage,
            plugins,
        )
    }

    async fn run_grpc_server(
        &self,
        service_dependencies: All<DebugContext<CloudGolemTypes>>,
        _lazy_worker_activator: Arc<LazyWorkerActivator<DebugContext<CloudGolemTypes>>>,
        join_set: &mut JoinSet<Result<(), Error>>,
    ) -> anyhow::Result<u16> {
        run_debug_server(service_dependencies, join_set).await
    }

    fn create_plugins(
        &self,
        golem_config: &GolemConfig,
    ) -> (
        Arc<dyn Plugins<CloudGolemTypes>>,
        Arc<dyn PluginsObservations>,
    ) {
        let plugins = golem_worker_executor::services::cloud::plugins::cloud_configured(
            &golem_config.plugin_service,
        );
        (plugins.clone(), plugins)
    }

    async fn create_services(
        &self,
        active_workers: Arc<ActiveWorkers<DebugContext<CloudGolemTypes>>>,
        engine: Arc<Engine>,
        linker: Arc<Linker<DebugContext<CloudGolemTypes>>>,
        runtime: Handle,
        component_service: Arc<dyn ComponentService<CloudGolemTypes>>,
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
        worker_activator: Arc<dyn WorkerActivator<DebugContext<CloudGolemTypes>>>,
        oplog_service: Arc<dyn OplogService>,
        scheduler_service: Arc<dyn SchedulerService>,
        worker_proxy: Arc<dyn WorkerProxy>,
        events: Arc<Events>,
        file_loader: Arc<FileLoader>,
        plugins: Arc<dyn Plugins<CloudGolemTypes>>,
        oplog_processor_plugin: Arc<dyn OplogProcessorPlugin>,
    ) -> anyhow::Result<All<DebugContext<CloudGolemTypes>>> {
        let remote_cloud_service_config = self.debug_config.cloud_service.clone();

        let auth_service: Arc<dyn AuthService> = Arc::new(AuthServiceDefault::new(
            CloudAuthService::new(&remote_cloud_service_config),
            self.debug_config.component_service.clone(),
        ));

        let debug_sessions: Arc<dyn DebugSessions> = Arc::new(DebugSessionsDefault::default());

        let debug_oplog_service = Arc::new(DebugOplogService::new(
            Arc::clone(&oplog_service),
            Arc::clone(&debug_sessions),
        ));

        let addition_deps = AdditionalDeps::new(auth_service, debug_sessions);

        let resource_limits = resource_limits::configured(&golem_config.resource_limits);

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
            addition_deps.clone(),
        ));

        Ok(All::new(
            active_workers,
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
            addition_deps,
        ))
    }

    fn create_wasmtime_linker(
        &self,
        engine: &Engine,
    ) -> anyhow::Result<Linker<DebugContext<CloudGolemTypes>>> {
        create_debug_wasmtime_linker(engine)
    }
}

fn get_durable_ctx<T: GolemTypes>(
    ctx: &mut DebugContext<T>,
) -> &mut DurableWorkerCtx<DebugContext<T>> {
    &mut ctx.durable_ctx
}

pub fn create_debug_wasmtime_linker<T: GolemTypes>(
    engine: &Engine,
) -> anyhow::Result<Linker<DebugContext<T>>> {
    let mut linker = create_linker(engine, get_durable_ctx)?;
    golem_api_1_x::host::add_to_linker_get_host(&mut linker, get_durable_ctx)?;
    golem_api_1_x::oplog::add_to_linker_get_host(&mut linker, get_durable_ctx)?;
    golem_api_1_x::context::add_to_linker_get_host(&mut linker, get_durable_ctx)?;
    golem_durability::durability::add_to_linker_get_host(&mut linker, get_durable_ctx)?;
    golem_wasm_rpc::golem_rpc_0_2_x::types::add_to_linker_get_host(&mut linker, get_durable_ctx)?;
    Ok(linker)
}

pub async fn run_debug_server<T: GolemTypes>(
    service_dependencies: All<DebugContext<T>>,
    join_set: &mut JoinSet<Result<(), Error>>,
) -> anyhow::Result<u16> {
    let debug_service = Arc::new(DebugServiceDefault::new(service_dependencies.clone()));

    let handle_ws = |ws| websocket::handle_ws(ws, debug_service);

    let config = service_dependencies.config();

    let app = Router::new().route("/ws", any(handle_ws));

    let addr = SocketAddrV4::new(
        config
            .http_address
            .parse::<Ipv4Addr>()
            .context("http_address configuration")?,
        config.port,
    );

    let listener = TcpListener::bind(addr).await?;
    let local_addr = listener.local_addr()?;

    join_set.spawn(
        async move {
            axum::serve(listener, app).await?;
            Ok(())
        }
        .in_current_span(),
    );

    info!("Debug server started on {local_addr}");

    Ok(config.port)
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
