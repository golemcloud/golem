use crate::additional_deps::AdditionalDeps;
use crate::auth::{AuthService, AuthServiceDefault};
use crate::config::{AdditionalDebugConfig, DebugConfig};
use crate::debug_context::DebugContext;
use crate::debug_session::{DebugSessions, DebugSessionsDefault};
use crate::oplog::debug_oplog_service::DebugOplogService;
use crate::services::debug_service::DebugServiceDefault;
use anyhow::{Context, Error};
use async_trait::async_trait;
use axum::routing::any;
use axum::Router;
use cloud_common::clients::grant::{GrantService, GrantServiceDefault};
use cloud_common::clients::project::{ProjectService, ProjectServiceDefault};
use golem_common::model::component::ComponentOwner;
use golem_common::model::plugin::{DefaultPluginOwner, DefaultPluginScope};
use golem_worker_executor_base::durable_host::DurableWorkerCtx;
use golem_worker_executor_base::preview2::golem::{api0_2_0, api1_1_0};
use golem_worker_executor_base::services::active_workers::ActiveWorkers;
use golem_worker_executor_base::services::blob_store::BlobStoreService;
use golem_worker_executor_base::services::component::ComponentService;
use golem_worker_executor_base::services::events::Events;
use golem_worker_executor_base::services::file_loader::FileLoader;
use golem_worker_executor_base::services::golem_config::{ComponentServiceConfig, GolemConfig};
use golem_worker_executor_base::services::key_value::KeyValueService;
use golem_worker_executor_base::services::oplog::plugin::OplogProcessorPlugin;
use golem_worker_executor_base::services::oplog::OplogService;
use golem_worker_executor_base::services::plugins::{Plugins, PluginsObservations};
use golem_worker_executor_base::services::promise::PromiseService;
use golem_worker_executor_base::services::rpc::{DirectWorkerInvocationRpc, RemoteInvocationRpc};
use golem_worker_executor_base::services::scheduler::SchedulerService;
use golem_worker_executor_base::services::shard::ShardService;
use golem_worker_executor_base::services::shard_manager::ShardManagerService;
use golem_worker_executor_base::services::worker::WorkerService;
use golem_worker_executor_base::services::worker_activator::{
    LazyWorkerActivator, WorkerActivator,
};
use golem_worker_executor_base::services::worker_enumeration::{
    RunningWorkerEnumerationService, WorkerEnumerationService,
};
use golem_worker_executor_base::services::worker_fork::DefaultWorkerFork;
use golem_worker_executor_base::services::worker_proxy::WorkerProxy;
use golem_worker_executor_base::services::{plugins, All, HasConfig};
use golem_worker_executor_base::wasi_host::create_linker;
use golem_worker_executor_base::workerctx::WorkerCtx;
use golem_worker_executor_base::Bootstrap;
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
pub mod config;
pub mod debug_context;
pub mod services;
pub mod websocket;

mod auth;
pub mod debug_request;
mod debug_session;
pub mod from_value;
mod jrpc;
mod model;
mod oplog;

struct ServerBootstrap {
    additional_config: AdditionalDebugConfig,
}

#[async_trait]
impl Bootstrap<DebugContext> for ServerBootstrap {
    fn create_active_workers(
        &self,
        golem_config: &GolemConfig,
    ) -> Arc<ActiveWorkers<DebugContext>> {
        Arc::new(ActiveWorkers::<DebugContext>::new(&golem_config.memory))
    }

    async fn run_server(
        &self,
        service_dependencies: All<DebugContext>,
        _lazy_worker_activator: Arc<LazyWorkerActivator<DebugContext>>,
        join_set: &mut JoinSet<Result<(), Error>>,
    ) -> anyhow::Result<()> {
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

        info!("Jrpc server started on {local_addr}");

        Ok(())
    }

    fn create_plugins(
        &self,
        golem_config: &GolemConfig,
    ) -> (
        Arc<
            dyn Plugins<
                    <<DebugContext as WorkerCtx>::ComponentOwner as ComponentOwner>::PluginOwner,
                    <DebugContext as WorkerCtx>::PluginScope,
                > + Send
                + Sync,
        >,
        Arc<dyn PluginsObservations + Send + Sync>,
    ) {
        plugins::default_configured(&golem_config.plugin_service)
    }

    async fn create_services(
        &self,
        active_workers: Arc<ActiveWorkers<DebugContext>>,
        engine: Arc<Engine>,
        linker: Arc<Linker<DebugContext>>,
        runtime: Handle,
        component_service: Arc<dyn ComponentService + Send + Sync>,
        shard_manager_service: Arc<dyn ShardManagerService + Send + Sync>,
        worker_service: Arc<dyn WorkerService + Send + Sync>,
        worker_enumeration_service: Arc<dyn WorkerEnumerationService + Send + Sync>,
        running_worker_enumeration_service: Arc<dyn RunningWorkerEnumerationService + Send + Sync>,
        promise_service: Arc<dyn PromiseService + Send + Sync>,
        golem_config: Arc<GolemConfig>,
        shard_service: Arc<dyn ShardService + Send + Sync>,
        key_value_service: Arc<dyn KeyValueService + Send + Sync>,
        blob_store_service: Arc<dyn BlobStoreService + Send + Sync>,
        worker_activator: Arc<dyn WorkerActivator<DebugContext> + Send + Sync>,
        oplog_service: Arc<dyn OplogService + Send + Sync>,
        scheduler_service: Arc<dyn SchedulerService + Send + Sync>,
        worker_proxy: Arc<dyn WorkerProxy + Send + Sync>,
        events: Arc<Events>,
        file_loader: Arc<FileLoader>,
        plugins: Arc<dyn Plugins<DefaultPluginOwner, DefaultPluginScope> + Send + Sync>,
        oplog_processor_plugin: Arc<dyn OplogProcessorPlugin + Send + Sync>,
    ) -> anyhow::Result<All<DebugContext>> {
        let config: ComponentServiceConfig = golem_config.component_service.clone();

        let remote_cloud_service_config = self.additional_config.cloud_service.clone();

        let project_service: Arc<dyn ProjectService + Send + Sync> =
            Arc::new(ProjectServiceDefault::new(&remote_cloud_service_config));
        let grant_service: Arc<dyn GrantService + Send + Sync> =
            Arc::new(GrantServiceDefault::new(&remote_cloud_service_config));

        let component_service_grpc_config = match config {
            ComponentServiceConfig::Grpc(grpc) => Ok(grpc),
            ComponentServiceConfig::Local(_) => {
                Err(anyhow::Error::msg("Cannot create auth_service for debugging service with local component service config".to_string()))
            }
        }?;

        let auth_service: Arc<dyn AuthService + Send + Sync> = Arc::new(AuthServiceDefault::new(
            project_service.clone(),
            grant_service.clone(),
            component_service_grpc_config,
        ));

        let debug_sessions: Arc<dyn DebugSessions + Sync + Send> =
            Arc::new(DebugSessionsDefault::new());

        let oplog_service = Arc::new(DebugOplogService::new(
            oplog_service,
            Arc::clone(&debug_sessions),
        ));

        let addition_deps = AdditionalDeps::new(auth_service, debug_sessions);

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
            oplog_service.clone(),
            scheduler_service.clone(),
            worker_activator.clone(),
            events.clone(),
            file_loader.clone(),
            plugins.clone(),
            oplog_processor_plugin.clone(),
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
            oplog_service.clone(),
            scheduler_service.clone(),
            worker_activator.clone(),
            events.clone(),
            file_loader.clone(),
            plugins.clone(),
            oplog_processor_plugin.clone(),
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
            oplog_service,
            rpc,
            scheduler_service,
            worker_activator.clone(),
            worker_proxy.clone(),
            events.clone(),
            file_loader.clone(),
            plugins.clone(),
            oplog_processor_plugin.clone(),
            addition_deps,
        ))
    }

    fn create_wasmtime_linker(&self, engine: &Engine) -> anyhow::Result<Linker<DebugContext>> {
        let mut linker = create_linker(engine, get_durable_ctx)?;
        api0_2_0::host::add_to_linker_get_host(&mut linker, get_durable_ctx)?;
        api1_1_0::host::add_to_linker_get_host(&mut linker, get_durable_ctx)?;
        golem_wasm_rpc::golem::rpc::types::add_to_linker_get_host(&mut linker, get_durable_ctx)?;
        Ok(linker)
    }
}

fn get_durable_ctx(ctx: &mut DebugContext) -> &mut DurableWorkerCtx<DebugContext> {
    &mut ctx.durable_ctx
}

pub async fn run(
    debug_config: DebugConfig,
    additional_config: AdditionalDebugConfig,
    prometheus_registry: Registry,
    runtime: Handle,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("Golem Debug Worker Executor starting up...");
    let mut join_set = JoinSet::new();
    ServerBootstrap { additional_config }
        .run(
            debug_config.golem_config,
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
