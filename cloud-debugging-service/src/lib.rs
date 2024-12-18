use crate::additional_deps::AdditionalDeps;
use crate::config::DebugConfig;
use crate::context::DebugContext;
use crate::services::debug_service;
use anyhow::Error;
use async_trait::async_trait;
use golem_common::model::component::ComponentOwner;
use golem_common::model::plugin::{DefaultPluginOwner, DefaultPluginScope};
use golem_worker_executor_base::durable_host::DurableWorkerCtx;
use golem_worker_executor_base::preview2::golem::{api0_2_0, api1_1_0};
use golem_worker_executor_base::services::active_workers::ActiveWorkers;
use golem_worker_executor_base::services::blob_store::BlobStoreService;
use golem_worker_executor_base::services::component::ComponentService;
use golem_worker_executor_base::services::events::Events;
use golem_worker_executor_base::services::file_loader::FileLoader;
use golem_worker_executor_base::services::golem_config::GolemConfig;
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
use golem_worker_executor_base::services::worker_proxy::WorkerProxy;
use golem_worker_executor_base::services::{plugins, All};
use golem_worker_executor_base::wasi_host::create_linker;
use golem_worker_executor_base::workerctx::WorkerCtx;
use golem_worker_executor_base::Bootstrap;
use prometheus::Registry;
use std::sync::Arc;
use tokio::runtime::Handle;
use tokio::task::JoinSet;
use tracing::info;
use wasmtime::component::Linker;
use wasmtime::Engine;

#[cfg(test)]
test_r::enable!();

pub mod additional_deps;
pub mod config;
pub mod context;
pub mod debug;
pub mod services;

struct ServerBootstrap {}

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
        _service_dependencies: All<DebugContext>,
        _lazy_worker_activator: Arc<LazyWorkerActivator<DebugContext>>,
        _join_set: &mut JoinSet<Result<(), Error>>,
    ) -> anyhow::Result<()> {
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
        let debug_service = debug_service::configured();

        let extra_deps = AdditionalDeps::new(debug_service);

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
            extra_deps.clone(),
        ));

        Ok(All::new(
            active_workers,
            engine,
            linker,
            runtime.clone(),
            component_service,
            shard_manager_service,
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
            extra_deps,
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
    prometheus_registry: Registry,
    runtime: Handle,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("Golem Debug Worker Executor starting up...");
    let mut join_set = JoinSet::new();
    ServerBootstrap {}
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
