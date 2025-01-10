// Copyright 2024-2025 Golem Cloud
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

use std::sync::RwLock;

use crate::metrics::workers::record_worker_call;
use crate::model::ExecutionStatus;
use crate::services::oplog::CommitLevel;
use crate::services::rpc::Rpc;
use crate::services::{rpc, HasOplog, HasWorkerForkService};
use golem_common::model::oplog::{OplogIndex, OplogIndexRange};
use golem_common::model::{AccountId, Timestamp, WorkerMetadata, WorkerStatusRecord};
use std::sync::Arc;

use super::file_loader::FileLoader;
use crate::error::GolemError;
use crate::services::events::Events;
use crate::services::oplog::plugin::OplogProcessorPlugin;
use crate::services::plugins::Plugins;
use crate::services::shard::ShardService;
use crate::services::worker_proxy::WorkerProxy;
use crate::services::{
    active_workers, blob_store, component, golem_config, key_value, oplog, promise, scheduler,
    shard, shard_manager, worker, worker_activator, worker_enumeration, HasActiveWorkers,
    HasBlobStoreService, HasComponentService, HasConfig, HasEvents, HasExtraDeps, HasFileLoader,
    HasKeyValueService, HasOplogProcessorPlugin, HasOplogService, HasPlugins, HasPromiseService,
    HasRpc, HasRunningWorkerEnumerationService, HasSchedulerService, HasShardManagerService,
    HasShardService, HasWasmtimeEngine, HasWorkerActivator, HasWorkerEnumerationService,
    HasWorkerProxy, HasWorkerService,
};
use crate::worker::Worker;
use crate::workerctx::WorkerCtx;
use async_trait::async_trait;
use golem_common::model::component::ComponentOwner;
use golem_common::model::{OwnedWorkerId, WorkerId};
use tokio::runtime::Handle;

#[async_trait]
pub trait WorkerForkService {
    async fn fork(
        &self,
        source_worker_id: &OwnedWorkerId,
        target_worker_id: &WorkerId,
        oplog_index_cut_off: OplogIndex,
    ) -> Result<(), GolemError>;
}

pub struct DefaultWorkerFork<Ctx: WorkerCtx> {
    pub rpc: Arc<dyn rpc::Rpc + Send + Sync>,
    pub active_workers: Arc<active_workers::ActiveWorkers<Ctx>>,
    pub engine: Arc<wasmtime::Engine>,
    pub linker: Arc<wasmtime::component::Linker<Ctx>>,
    pub runtime: Handle,
    pub component_service: Arc<dyn component::ComponentService + Send + Sync>,
    pub shard_manager_service: Arc<dyn shard_manager::ShardManagerService + Send + Sync>,
    pub worker_service: Arc<dyn worker::WorkerService + Send + Sync>,
    pub worker_proxy: Arc<dyn WorkerProxy + Send + Sync>,
    pub worker_enumeration_service:
        Arc<dyn worker_enumeration::WorkerEnumerationService + Send + Sync>,
    pub running_worker_enumeration_service:
        Arc<dyn worker_enumeration::RunningWorkerEnumerationService + Send + Sync>,
    pub promise_service: Arc<dyn promise::PromiseService + Send + Sync>,
    pub golem_config: Arc<golem_config::GolemConfig>,
    pub shard_service: Arc<dyn shard::ShardService + Send + Sync>,
    pub key_value_service: Arc<dyn key_value::KeyValueService + Send + Sync>,
    pub blob_store_service: Arc<dyn blob_store::BlobStoreService + Send + Sync>,
    pub oplog_service: Arc<dyn oplog::OplogService + Send + Sync>,
    pub scheduler_service: Arc<dyn scheduler::SchedulerService + Send + Sync>,
    pub worker_activator: Arc<dyn worker_activator::WorkerActivator<Ctx> + Send + Sync>,
    pub events: Arc<Events>,
    pub file_loader: Arc<FileLoader>,
    pub plugins: Arc<
        dyn Plugins<<Ctx::ComponentOwner as ComponentOwner>::PluginOwner, Ctx::PluginScope>
            + Send
            + Sync,
    >,
    pub oplog_processor_plugin: Arc<dyn OplogProcessorPlugin + Send + Sync>,
    pub extra_deps: Ctx::ExtraDeps,
}

impl<Ctx: WorkerCtx> HasEvents for DefaultWorkerFork<Ctx> {
    fn events(&self) -> Arc<Events> {
        self.events.clone()
    }
}

impl<Ctx: WorkerCtx> HasActiveWorkers<Ctx> for DefaultWorkerFork<Ctx> {
    fn active_workers(&self) -> Arc<active_workers::ActiveWorkers<Ctx>> {
        self.active_workers.clone()
    }
}

impl<Ctx: WorkerCtx> HasComponentService for DefaultWorkerFork<Ctx> {
    fn component_service(&self) -> Arc<dyn component::ComponentService + Send + Sync> {
        self.component_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasConfig for DefaultWorkerFork<Ctx> {
    fn config(&self) -> Arc<golem_config::GolemConfig> {
        self.golem_config.clone()
    }
}

impl<Ctx: WorkerCtx> HasWorkerService for DefaultWorkerFork<Ctx> {
    fn worker_service(&self) -> Arc<dyn worker::WorkerService + Send + Sync> {
        self.worker_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasWorkerEnumerationService for DefaultWorkerFork<Ctx> {
    fn worker_enumeration_service(
        &self,
    ) -> Arc<dyn worker_enumeration::WorkerEnumerationService + Send + Sync> {
        self.worker_enumeration_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasRunningWorkerEnumerationService for DefaultWorkerFork<Ctx> {
    fn running_worker_enumeration_service(
        &self,
    ) -> Arc<dyn worker_enumeration::RunningWorkerEnumerationService + Send + Sync> {
        self.running_worker_enumeration_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasPromiseService for DefaultWorkerFork<Ctx> {
    fn promise_service(&self) -> Arc<dyn promise::PromiseService + Send + Sync> {
        self.promise_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasWasmtimeEngine<Ctx> for DefaultWorkerFork<Ctx> {
    fn engine(&self) -> Arc<wasmtime::Engine> {
        self.engine.clone()
    }

    fn linker(&self) -> Arc<wasmtime::component::Linker<Ctx>> {
        self.linker.clone()
    }

    fn runtime(&self) -> Handle {
        self.runtime.clone()
    }
}

impl<Ctx: WorkerCtx> HasKeyValueService for DefaultWorkerFork<Ctx> {
    fn key_value_service(&self) -> Arc<dyn key_value::KeyValueService + Send + Sync> {
        self.key_value_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasBlobStoreService for DefaultWorkerFork<Ctx> {
    fn blob_store_service(&self) -> Arc<dyn blob_store::BlobStoreService + Send + Sync> {
        self.blob_store_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasSchedulerService for DefaultWorkerFork<Ctx> {
    fn scheduler_service(&self) -> Arc<dyn scheduler::SchedulerService + Send + Sync> {
        self.scheduler_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasOplogService for DefaultWorkerFork<Ctx> {
    fn oplog_service(&self) -> Arc<dyn oplog::OplogService + Send + Sync> {
        self.oplog_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasWorkerForkService for DefaultWorkerFork<Ctx> {
    fn worker_fork_service(&self) -> Arc<dyn WorkerForkService + Send + Sync> {
        Arc::new(self.clone())
    }
}

impl<Ctx: WorkerCtx> HasRpc for DefaultWorkerFork<Ctx> {
    fn rpc(&self) -> Arc<dyn Rpc + Send + Sync> {
        self.rpc.clone()
    }
}

impl<Ctx: WorkerCtx> HasExtraDeps<Ctx> for DefaultWorkerFork<Ctx> {
    fn extra_deps(&self) -> Ctx::ExtraDeps {
        self.extra_deps.clone()
    }
}

impl<Ctx: WorkerCtx> HasShardService for DefaultWorkerFork<Ctx> {
    fn shard_service(&self) -> Arc<dyn shard::ShardService + Send + Sync> {
        self.shard_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasShardManagerService for DefaultWorkerFork<Ctx> {
    fn shard_manager_service(&self) -> Arc<dyn shard_manager::ShardManagerService + Send + Sync> {
        self.shard_manager_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasWorkerActivator<Ctx> for DefaultWorkerFork<Ctx> {
    fn worker_activator(&self) -> Arc<dyn worker_activator::WorkerActivator<Ctx> + Send + Sync> {
        self.worker_activator.clone()
    }
}

impl<Ctx: WorkerCtx> HasWorkerProxy for DefaultWorkerFork<Ctx> {
    fn worker_proxy(&self) -> Arc<dyn WorkerProxy + Send + Sync> {
        self.worker_proxy.clone()
    }
}

impl<Ctx: WorkerCtx> HasFileLoader for DefaultWorkerFork<Ctx> {
    fn file_loader(&self) -> Arc<FileLoader> {
        self.file_loader.clone()
    }
}

impl<Ctx: WorkerCtx>
    HasPlugins<<Ctx::ComponentOwner as ComponentOwner>::PluginOwner, Ctx::PluginScope>
    for DefaultWorkerFork<Ctx>
{
    fn plugins(
        &self,
    ) -> Arc<
        dyn Plugins<<Ctx::ComponentOwner as ComponentOwner>::PluginOwner, Ctx::PluginScope>
            + Send
            + Sync,
    > {
        self.plugins.clone()
    }
}

impl<Ctx: WorkerCtx> HasOplogProcessorPlugin for DefaultWorkerFork<Ctx> {
    fn oplog_processor_plugin(&self) -> Arc<dyn OplogProcessorPlugin + Send + Sync> {
        self.oplog_processor_plugin.clone()
    }
}

impl<Ctx: WorkerCtx> Clone for DefaultWorkerFork<Ctx> {
    fn clone(&self) -> Self {
        Self {
            rpc: self.rpc.clone(),
            active_workers: self.active_workers.clone(),
            engine: self.engine.clone(),
            linker: self.linker.clone(),
            runtime: self.runtime.clone(),
            component_service: self.component_service.clone(),
            shard_manager_service: self.shard_manager_service.clone(),
            worker_service: self.worker_service.clone(),
            worker_proxy: self.worker_proxy.clone(),
            worker_enumeration_service: self.worker_enumeration_service.clone(),
            running_worker_enumeration_service: self.running_worker_enumeration_service.clone(),
            promise_service: self.promise_service.clone(),
            golem_config: self.golem_config.clone(),
            shard_service: self.shard_service.clone(),
            key_value_service: self.key_value_service.clone(),
            blob_store_service: self.blob_store_service.clone(),
            oplog_service: self.oplog_service.clone(),
            scheduler_service: self.scheduler_service.clone(),
            worker_activator: self.worker_activator.clone(),
            events: self.events.clone(),
            file_loader: self.file_loader.clone(),
            plugins: self.plugins.clone(),
            oplog_processor_plugin: self.oplog_processor_plugin.clone(),
            extra_deps: self.extra_deps.clone(),
        }
    }
}

impl<Ctx: WorkerCtx> DefaultWorkerFork<Ctx> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        rpc: Arc<dyn Rpc + Send + Sync>,
        active_workers: Arc<active_workers::ActiveWorkers<Ctx>>,
        engine: Arc<wasmtime::Engine>,
        linker: Arc<wasmtime::component::Linker<Ctx>>,
        runtime: Handle,
        component_service: Arc<dyn component::ComponentService + Send + Sync>,
        shard_manager_service: Arc<dyn shard_manager::ShardManagerService + Send + Sync>,
        worker_service: Arc<dyn worker::WorkerService + Send + Sync>,
        worker_proxy: Arc<dyn WorkerProxy + Send + Sync>,
        worker_enumeration_service: Arc<
            dyn worker_enumeration::WorkerEnumerationService + Send + Sync,
        >,
        running_worker_enumeration_service: Arc<
            dyn worker_enumeration::RunningWorkerEnumerationService + Send + Sync,
        >,
        promise_service: Arc<dyn promise::PromiseService + Send + Sync>,
        golem_config: Arc<golem_config::GolemConfig>,
        shard_service: Arc<dyn ShardService + Send + Sync>,
        key_value_service: Arc<dyn key_value::KeyValueService + Send + Sync>,
        blob_store_service: Arc<dyn blob_store::BlobStoreService + Send + Sync>,
        oplog_service: Arc<dyn oplog::OplogService + Send + Sync>,
        scheduler_service: Arc<dyn scheduler::SchedulerService + Send + Sync>,
        worker_activator: Arc<dyn worker_activator::WorkerActivator<Ctx> + Send + Sync>,
        events: Arc<Events>,
        file_loader: Arc<FileLoader>,
        plugins: Arc<
            dyn Plugins<<Ctx::ComponentOwner as ComponentOwner>::PluginOwner, Ctx::PluginScope>
                + Send
                + Sync,
        >,
        oplog_processor_plugin: Arc<dyn OplogProcessorPlugin + Send + Sync>,
        extra_deps: Ctx::ExtraDeps,
    ) -> Self {
        Self {
            rpc,
            active_workers,
            engine,
            linker,
            runtime,
            component_service,
            shard_manager_service,
            worker_service,
            worker_proxy,
            worker_enumeration_service,
            running_worker_enumeration_service,
            promise_service,
            golem_config,
            shard_service,
            key_value_service,
            blob_store_service,
            oplog_service,
            scheduler_service,
            worker_activator,
            events,
            file_loader,
            plugins,
            oplog_processor_plugin,
            extra_deps,
        }
    }

    async fn validate_worker_forking(
        &self,
        account_id: &AccountId,
        source_worker_id: &WorkerId,
        target_worker_id: &WorkerId,
        oplog_index_cut_off: OplogIndex,
    ) -> Result<(OwnedWorkerId, OwnedWorkerId), GolemError> {
        let second_index = OplogIndex::INITIAL.next();

        if oplog_index_cut_off < second_index {
            return Err(GolemError::invalid_request(
                "oplog_index_cut_off must be at least 2",
            ));
        }

        let owned_target_worker_id = OwnedWorkerId::new(account_id, target_worker_id);

        let target_metadata = self.worker_service.get(&owned_target_worker_id).await;

        // We allow forking only if the target worker does not exist
        if target_metadata.is_some() {
            return Err(GolemError::worker_already_exists(target_worker_id.clone()));
        }

        // We assume the source worker belongs to this executor
        self.shard_service.check_worker(source_worker_id)?;

        let owned_source_worker_id = OwnedWorkerId::new(account_id, source_worker_id);

        self.worker_service
            .get(&owned_source_worker_id)
            .await
            .ok_or(GolemError::worker_not_found(source_worker_id.clone()))?;

        Ok((owned_source_worker_id, owned_target_worker_id))
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> WorkerForkService for DefaultWorkerFork<Ctx> {
    async fn fork(
        &self,
        source_worker_id: &OwnedWorkerId,
        target_worker_id: &WorkerId,
        oplog_index_cut_off: OplogIndex,
    ) -> Result<(), GolemError> {
        record_worker_call("fork");

        let (owned_source_worker_id, owned_target_worker_id) = self
            .validate_worker_forking(
                &source_worker_id.account_id,
                &source_worker_id.worker_id,
                target_worker_id,
                oplog_index_cut_off,
            )
            .await?;

        let target_worker_id = owned_target_worker_id.worker_id.clone();
        let account_id = owned_target_worker_id.account_id.clone();

        let source_worker_instance =
            Worker::get_or_create_suspended(self, &owned_source_worker_id, None, None, None, None)
                .await?;

        let source_worker_metadata = source_worker_instance.get_metadata().await?;

        let target_worker_metadata = WorkerMetadata {
            worker_id: target_worker_id.clone(),
            account_id,
            env: source_worker_metadata.env.clone(),
            args: source_worker_metadata.args.clone(),
            created_at: Timestamp::now_utc(),
            parent: None,
            last_known_status: WorkerStatusRecord::default(),
        };

        let source_oplog = source_worker_instance.oplog();

        source_oplog.commit(CommitLevel::Always).await;

        let initial_oplog_entry = source_oplog.read(OplogIndex::INITIAL).await;

        // Update the oplog initial entry with the new worker
        let target_initial_oplog_entry = initial_oplog_entry
            .update_worker_id(&target_worker_id)
            .ok_or(GolemError::unknown(
                "Failed to update worker id in oplog entry",
            ))?;

        let new_oplog = self
            .oplog_service
            .create(
                &owned_target_worker_id,
                target_initial_oplog_entry,
                target_worker_metadata,
                Arc::new(RwLock::new(ExecutionStatus::Suspended {
                    last_known_status: WorkerStatusRecord::default(),
                    component_type: source_worker_instance.component_type(),
                    timestamp: Timestamp::now_utc(),
                })),
            )
            .await;

        let oplog_range = OplogIndexRange::new(OplogIndex::INITIAL.next(), oplog_index_cut_off);

        for oplog_index in oplog_range {
            let entry = source_oplog.read(oplog_index).await;
            new_oplog.add(entry.clone()).await;
        }

        new_oplog.commit(CommitLevel::Always).await;

        // We go through worker proxy to resume the worker
        // as we need to make sure as it may live in another worker executor,
        // depending on sharding.
        // This will replay until the fork point in the forked worker
        self.worker_proxy
            .resume(&target_worker_id, true)
            .await
            .map_err(|err| {
                GolemError::failed_to_resume_worker(target_worker_id.clone(), err.into())
            })?;

        Ok(())
    }
}
