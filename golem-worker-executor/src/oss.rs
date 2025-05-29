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

use std::sync::Arc;

use crate::durable_host::DurableWorkerCtx;
use crate::preview2::golem::durability;
use crate::preview2::golem_api_1_x;
use crate::services::active_workers::ActiveWorkers;
use crate::services::blob_store::BlobStoreService;
use crate::services::component::ComponentService;
use crate::services::events::Events;
use crate::services::file_loader::FileLoader;
use crate::services::golem_config::GolemConfig;
use crate::services::key_value::KeyValueService;
use crate::services::oplog::plugin::OplogProcessorPlugin;
use crate::services::oplog::OplogService;
use crate::services::plugins::{Plugins, PluginsObservations};
use crate::services::promise::PromiseService;
use crate::services::rpc::{DirectWorkerInvocationRpc, RemoteInvocationRpc};
use crate::services::scheduler::SchedulerService;
use crate::services::shard::ShardService;
use crate::services::shard_manager::ShardManagerService;
use crate::services::worker::WorkerService;
use crate::services::worker_activator::WorkerActivator;
use crate::services::worker_enumeration::{
    RunningWorkerEnumerationService, WorkerEnumerationService,
};
use crate::services::worker_fork::DefaultWorkerFork;
use crate::services::worker_proxy::WorkerProxy;
use crate::services::{component, plugins, rdbms, resource_limits, All, NoAdditionalDeps};
use crate::wasi_host::create_linker;
use crate::workerctx::oss::Context;
use crate::{Bootstrap, DefaultGolemTypes, RunDetails};
use async_trait::async_trait;
use golem_service_base::storage::blob::BlobStorage;
use prometheus::Registry;
use tokio::runtime::Handle;
use tokio::task::JoinSet;
use tracing::info;
use wasmtime::component::Linker;
use wasmtime::Engine;

#[cfg(test)]
test_r::enable!();

struct ServerBootstrap {}

#[async_trait]
impl Bootstrap<Context> for ServerBootstrap {
    fn create_active_workers(&self, golem_config: &GolemConfig) -> Arc<ActiveWorkers<Context>> {
        Arc::new(ActiveWorkers::<Context>::new(&golem_config.memory))
    }

    fn create_plugins(
        &self,
        golem_config: &GolemConfig,
    ) -> (
        Arc<dyn Plugins<DefaultGolemTypes>>,
        Arc<dyn PluginsObservations>,
    ) {
        plugins::default_configured(&golem_config.plugin_service)
    }

    fn create_component_service(
        &self,
        golem_config: &GolemConfig,
        blob_storage: Arc<dyn BlobStorage + Send + Sync>,
        plugin_observations: Arc<dyn PluginsObservations>,
    ) -> Arc<dyn ComponentService<DefaultGolemTypes>> {
        component::configured(
            &golem_config.component_service,
            &golem_config.component_cache,
            &golem_config.compiled_component_service,
            blob_storage,
            plugin_observations,
        )
    }

    async fn create_services(
        &self,
        active_workers: Arc<ActiveWorkers<Context>>,
        engine: Arc<Engine>,
        linker: Arc<Linker<Context>>,
        runtime: Handle,
        component_service: Arc<dyn ComponentService<DefaultGolemTypes>>,
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
        worker_activator: Arc<dyn WorkerActivator<Context>>,
        oplog_service: Arc<dyn OplogService>,
        scheduler_service: Arc<dyn SchedulerService>,
        worker_proxy: Arc<dyn WorkerProxy>,
        events: Arc<Events>,
        file_loader: Arc<FileLoader>,
        plugins: Arc<dyn Plugins<DefaultGolemTypes>>,
        oplog_processor_plugin: Arc<dyn OplogProcessorPlugin>,
    ) -> anyhow::Result<All<Context>> {
        let additional_deps = NoAdditionalDeps {};
        let resource_limits = resource_limits::configured(&golem_config.resource_limits);

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
            resource_limits.clone(),
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
            resource_limits.clone(),
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
            rdbms_service,
            oplog_service,
            rpc,
            scheduler_service,
            worker_activator.clone(),
            worker_proxy.clone(),
            events.clone(),
            file_loader.clone(),
            plugins.clone(),
            oplog_processor_plugin,
            resource_limits,
            additional_deps,
        ))
    }

    fn create_wasmtime_linker(&self, engine: &Engine) -> anyhow::Result<Linker<Context>> {
        let mut linker = create_linker(engine, get_durable_ctx)?;
        golem_api_1_x::host::add_to_linker_get_host(&mut linker, get_durable_ctx)?;
        golem_api_1_x::oplog::add_to_linker_get_host(&mut linker, get_durable_ctx)?;
        golem_api_1_x::context::add_to_linker_get_host(&mut linker, get_durable_ctx)?;
        durability::durability::add_to_linker_get_host(&mut linker, get_durable_ctx)?;
        golem_wasm_rpc::golem_rpc_0_2_x::types::add_to_linker_get_host(
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
    prometheus_registry: Registry,
    runtime: Handle,
    join_set: &mut JoinSet<Result<(), anyhow::Error>>,
) -> Result<RunDetails, anyhow::Error> {
    info!("Golem Worker Executor starting up...");
    ServerBootstrap {}
        .run(golem_config, prometheus_registry, runtime, join_set)
        .await
}
