use crate::context::Context;
use crate::services::config::AdditionalGolemConfig;
use crate::services::{resource_limits, AdditionalDeps};
use async_trait::async_trait;
use cloud_common::model::CloudComponentOwner;
use golem_common::model::plugin::{DefaultPluginOwner, DefaultPluginScope};
use golem_service_base::storage::blob::BlobStorage;
use golem_worker_executor_base::durable_host::DurableWorkerCtx;
use golem_worker_executor_base::preview2::{golem_api_0_2_x, golem_api_1_x};
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
use golem_worker_executor_base::services::worker_activator::WorkerActivator;
use golem_worker_executor_base::services::worker_enumeration::{
    RunningWorkerEnumerationService, WorkerEnumerationService,
};
use golem_worker_executor_base::services::worker_fork::DefaultWorkerFork;
use golem_worker_executor_base::services::worker_proxy::WorkerProxy;
use golem_worker_executor_base::services::{compiled_component, plugins, rdbms, All};
use golem_worker_executor_base::wasi_host::create_linker;
use golem_worker_executor_base::{Bootstrap, GolemTypes};
use prometheus::Registry;
use std::sync::Arc;
use tokio::runtime::Handle;
use tokio::task::JoinSet;
use tracing::info;
use uuid::Uuid;
use wasmtime::component::Linker;
use wasmtime::Engine;

use self::services::component::ComponentServiceCloudGrpc;
use self::services::plugins::CloudPluginsWrapper;

#[cfg(test)]
test_r::enable!();

pub mod context;
pub mod metrics;
pub mod services;

pub struct CloudGolemTypes;

impl GolemTypes for CloudGolemTypes {
    type ComponentOwner = CloudComponentOwner;

    // TODO: These should eventually be cloud types
    type PluginOwner = DefaultPluginOwner;
    type PluginScope = DefaultPluginScope;
}

struct ServerBootstrap {
    additional_golem_config: Arc<AdditionalGolemConfig>,
}

#[async_trait]
impl Bootstrap<Context> for ServerBootstrap {
    fn create_active_workers(&self, golem_config: &GolemConfig) -> Arc<ActiveWorkers<Context>> {
        Arc::new(ActiveWorkers::<Context>::new(&golem_config.memory))
    }

    fn create_plugins(
        &self,
        golem_config: &GolemConfig,
    ) -> (
        Arc<dyn Plugins<CloudGolemTypes>>,
        Arc<dyn PluginsObservations>,
    ) {
        let (plugins, plugin_observations) =
            plugins::default_configured(&golem_config.plugin_service);
        let wrapper = Arc::new(CloudPluginsWrapper::new(plugin_observations, plugins));
        (wrapper.clone(), wrapper)
    }

    fn create_component_service(
        &self,
        golem_config: &GolemConfig,
        blob_storage: Arc<dyn BlobStorage + Send + Sync>,
        plugin_observations: Arc<dyn PluginsObservations>,
    ) -> Arc<dyn ComponentService<CloudGolemTypes>> {
        let compiled_component_service =
            compiled_component::configured(&golem_config.compiled_component_service, blob_storage);
        let component_service_config = &self.additional_golem_config.component_service;
        let component_cache_config = &self.additional_golem_config.component_cache;

        let access_token = component_service_config
            .access_token
            .parse::<Uuid>()
            .expect("Access token must be an UUID");

        info!(
            "Creating component service with config: {{ host: {}, port: {}, project_host: {}, project_port: {} }}",
            component_service_config.host,
            component_service_config.port,
            component_service_config.project_host,
            component_service_config.project_port
        );

        Arc::new(ComponentServiceCloudGrpc::new(
            component_service_config.component_uri(),
            component_service_config.project_uri(),
            access_token,
            component_cache_config.max_capacity,
            component_cache_config.max_metadata_capacity,
            component_cache_config.max_resolved_component_capacity,
            component_cache_config.max_resolved_project_capacity,
            component_cache_config.time_to_idle,
            golem_config.retry.clone(),
            compiled_component_service,
            component_service_config.max_component_size,
            plugin_observations,
        ))
    }

    async fn create_services(
        &self,
        active_workers: Arc<ActiveWorkers<Context>>,
        engine: Arc<Engine>,
        linker: Arc<Linker<Context>>,
        runtime: Handle,
        component_service: Arc<dyn ComponentService<CloudGolemTypes>>,
        shard_manager_service: Arc<dyn ShardManagerService + Send + Sync>,
        worker_service: Arc<dyn WorkerService + Send + Sync>,
        worker_enumeration_service: Arc<dyn WorkerEnumerationService + Send + Sync>,
        running_worker_enumeration_service: Arc<dyn RunningWorkerEnumerationService + Send + Sync>,
        promise_service: Arc<dyn PromiseService + Send + Sync>,
        golem_config: Arc<GolemConfig>,
        shard_service: Arc<dyn ShardService + Send + Sync>,
        key_value_service: Arc<dyn KeyValueService + Send + Sync>,
        blob_store_service: Arc<dyn BlobStoreService + Send + Sync>,
        rdbms_service: Arc<dyn rdbms::RdbmsService + Send + Sync>,
        worker_activator: Arc<dyn WorkerActivator<Context> + Send + Sync>,
        oplog_service: Arc<dyn OplogService + Send + Sync>,
        scheduler_service: Arc<dyn SchedulerService + Send + Sync>,
        worker_proxy: Arc<dyn WorkerProxy + Send + Sync>,
        events: Arc<Events>,
        file_loader: Arc<FileLoader>,
        plugins: Arc<dyn Plugins<CloudGolemTypes>>,
        oplog_processor_plugin: Arc<dyn OplogProcessorPlugin + Send + Sync>,
    ) -> anyhow::Result<All<Context>> {
        let additional_golem_config = self.additional_golem_config.clone();
        let resource_limits =
            resource_limits::configured(&self.additional_golem_config.resource_limits);

        let additional_deps = AdditionalDeps::new(additional_golem_config, resource_limits);

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
            oplog_service.clone(),
            scheduler_service.clone(),
            worker_activator.clone(),
            events.clone(),
            file_loader.clone(),
            plugins.clone(),
            oplog_processor_plugin.clone(),
            additional_deps.clone(),
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
            oplog_service.clone(),
            scheduler_service.clone(),
            worker_activator.clone(),
            events.clone(),
            file_loader.clone(),
            plugins.clone(),
            oplog_processor_plugin.clone(),
            additional_deps.clone(),
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
            rdbms_service.clone(),
            oplog_service,
            rpc,
            scheduler_service,
            worker_activator.clone(),
            worker_proxy.clone(),
            events.clone(),
            file_loader.clone(),
            plugins.clone(),
            oplog_processor_plugin.clone(),
            additional_deps,
        ))
    }

    fn create_wasmtime_linker(&self, engine: &Engine) -> anyhow::Result<Linker<Context>> {
        let mut linker = create_linker(engine, get_durable_ctx)?;
        golem_api_0_2_x::host::add_to_linker_get_host(&mut linker, get_durable_ctx)?;
        golem_api_1_x::host::add_to_linker_get_host(&mut linker, get_durable_ctx)?;
        golem_wasm_rpc::golem_rpc_0_1_x::types::add_to_linker_get_host(
            &mut linker,
            get_durable_ctx,
        )?;
        Ok(linker)
    }
}

fn get_durable_ctx(ctx: &mut Context) -> &mut DurableWorkerCtx<Context> {
    &mut ctx.durable_ctx
}

pub async fn run(
    golem_config: GolemConfig,
    additional_golem_config: Arc<AdditionalGolemConfig>,
    prometheus_registry: Registry,
    runtime: Handle,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("Golem Worker Executor starting up...");
    let mut join_set = JoinSet::new();
    ServerBootstrap {
        additional_golem_config,
    }
    .run(golem_config, prometheus_registry, runtime, &mut join_set)
    .await?;

    while let Some(res) = join_set.join_next().await {
        res??
    }
    Ok(())
}
