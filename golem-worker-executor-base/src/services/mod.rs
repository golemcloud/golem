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

use crate::services::worker_activator::WorkerActivator;
use std::sync::Arc;

use crate::services::events::Events;
use crate::services::plugins::Plugins;
use crate::workerctx::WorkerCtx;
use file_loader::FileLoader;
use golem_common::model::component::ComponentOwner;
use golem_common::model::plugin::{PluginOwner, PluginScope};
use tokio::runtime::Handle;

pub mod active_workers;
pub mod blob_store;
pub mod compiled_component;
pub mod component;
pub mod events;
pub mod file_loader;
pub mod golem_config;
pub mod key_value;
pub mod oplog;
pub mod plugins;
pub mod promise;
pub mod rpc;
pub mod scheduler;
pub mod shard;
pub mod shard_manager;
pub mod worker;
pub mod worker_activator;
pub mod worker_enumeration;
pub mod worker_event;
pub mod worker_fork;
pub mod worker_proxy;

// HasXXX traits for fine-grained control of which dependencies a function needs

pub trait HasActiveWorkers<Ctx: WorkerCtx> {
    fn active_workers(&self) -> Arc<active_workers::ActiveWorkers<Ctx>>;
}

pub trait HasComponentService {
    fn component_service(&self) -> Arc<dyn component::ComponentService + Send + Sync>;
}

pub trait HasShardManagerService {
    fn shard_manager_service(&self) -> Arc<dyn shard_manager::ShardManagerService + Send + Sync>;
}

pub trait HasConfig {
    fn config(&self) -> Arc<golem_config::GolemConfig>;
}

pub trait HasWorkerForkService {
    fn worker_fork_service(&self) -> Arc<dyn worker_fork::WorkerForkService + Send + Sync>;
}

pub trait HasWorkerService {
    fn worker_service(&self) -> Arc<dyn worker::WorkerService + Send + Sync>;
}

pub trait HasWorkerEnumerationService {
    fn worker_enumeration_service(
        &self,
    ) -> Arc<dyn worker_enumeration::WorkerEnumerationService + Send + Sync>;
}

pub trait HasRunningWorkerEnumerationService {
    fn running_worker_enumeration_service(
        &self,
    ) -> Arc<dyn worker_enumeration::RunningWorkerEnumerationService + Send + Sync>;
}

pub trait HasShardService {
    fn shard_service(&self) -> Arc<dyn shard::ShardService + Send + Sync>;
}

pub trait HasPromiseService {
    fn promise_service(&self) -> Arc<dyn promise::PromiseService + Send + Sync>;
}

pub trait HasWasmtimeEngine<Ctx> {
    fn engine(&self) -> Arc<wasmtime::Engine>;
    fn linker(&self) -> Arc<wasmtime::component::Linker<Ctx>>;
    fn runtime(&self) -> Handle;
}

pub trait HasKeyValueService {
    fn key_value_service(&self) -> Arc<dyn key_value::KeyValueService + Send + Sync>;
}

pub trait HasBlobStoreService {
    fn blob_store_service(&self) -> Arc<dyn blob_store::BlobStoreService + Send + Sync>;
}

pub trait HasOplogService {
    fn oplog_service(&self) -> Arc<dyn oplog::OplogService + Send + Sync>;
}

pub trait HasRpc {
    fn rpc(&self) -> Arc<dyn rpc::Rpc + Send + Sync>;
}

pub trait HasSchedulerService {
    fn scheduler_service(&self) -> Arc<dyn scheduler::SchedulerService + Send + Sync>;
}

pub trait HasExtraDeps<Ctx: WorkerCtx> {
    fn extra_deps(&self) -> Ctx::ExtraDeps;
}

pub trait HasWorker<Ctx: WorkerCtx> {
    fn worker(&self) -> Arc<crate::worker::Worker<Ctx>>;
}

pub trait HasOplog {
    fn oplog(&self) -> Arc<dyn oplog::Oplog + Send + Sync>;
}

pub trait HasWorkerActivator<Ctx: WorkerCtx> {
    fn worker_activator(&self) -> Arc<dyn WorkerActivator<Ctx> + Send + Sync>;
}

pub trait HasWorkerProxy {
    fn worker_proxy(&self) -> Arc<dyn worker_proxy::WorkerProxy + Send + Sync>;
}

pub trait HasEvents {
    fn events(&self) -> Arc<Events>;
}

pub trait HasFileLoader {
    fn file_loader(&self) -> Arc<FileLoader>;
}

pub trait HasPlugins<Owner: PluginOwner, Scope: PluginScope> {
    fn plugins(&self) -> Arc<dyn Plugins<Owner, Scope> + Send + Sync>;
}

pub trait HasOplogProcessorPlugin {
    fn oplog_processor_plugin(&self) -> Arc<dyn oplog::plugin::OplogProcessorPlugin + Send + Sync>;
}

/// HasAll is a shortcut for requiring all available service dependencies
pub trait HasAll<Ctx: WorkerCtx>:
    HasActiveWorkers<Ctx>
    + HasComponentService
    + HasConfig
    + HasWorkerForkService
    + HasWorkerService
    + HasWorkerEnumerationService
    + HasRunningWorkerEnumerationService
    + HasPromiseService
    + HasWasmtimeEngine<Ctx>
    + HasKeyValueService
    + HasBlobStoreService
    + HasOplogService
    + HasRpc
    + HasSchedulerService
    + HasWorkerActivator<Ctx>
    + HasWorkerProxy
    + HasEvents
    + HasShardManagerService
    + HasShardService
    + HasFileLoader
    + HasPlugins<<Ctx::ComponentOwner as ComponentOwner>::PluginOwner, Ctx::PluginScope>
    + HasOplogProcessorPlugin
    + HasExtraDeps<Ctx>
    + Clone
{
}

impl<
        Ctx: WorkerCtx,
        T: HasActiveWorkers<Ctx>
            + HasComponentService
            + HasConfig
            + HasWorkerForkService
            + HasWorkerService
            + HasWorkerEnumerationService
            + HasRunningWorkerEnumerationService
            + HasPromiseService
            + HasWasmtimeEngine<Ctx>
            + HasKeyValueService
            + HasBlobStoreService
            + HasOplogService
            + HasRpc
            + HasSchedulerService
            + HasWorkerActivator<Ctx>
            + HasWorkerProxy
            + HasEvents
            + HasShardManagerService
            + HasShardService
            + HasFileLoader
            + HasPlugins<<Ctx::ComponentOwner as ComponentOwner>::PluginOwner, Ctx::PluginScope>
            + HasOplogProcessorPlugin
            + HasExtraDeps<Ctx>
            + Clone,
    > HasAll<Ctx> for T
{
}

/// Helper struct for holding all available service dependencies in one place
/// To be used as a convenient struct member for services that need access to all dependencies
pub struct All<Ctx: WorkerCtx> {
    active_workers: Arc<active_workers::ActiveWorkers<Ctx>>,
    engine: Arc<wasmtime::Engine>,
    linker: Arc<wasmtime::component::Linker<Ctx>>,
    runtime: Handle,
    component_service: Arc<dyn component::ComponentService + Send + Sync>,
    shard_manager_service: Arc<dyn shard_manager::ShardManagerService + Send + Sync>,
    worker_fork: Arc<dyn worker_fork::WorkerForkService + Send + Sync>,
    worker_service: Arc<dyn worker::WorkerService + Send + Sync>,
    worker_enumeration_service: Arc<dyn worker_enumeration::WorkerEnumerationService + Send + Sync>,
    running_worker_enumeration_service:
        Arc<dyn worker_enumeration::RunningWorkerEnumerationService + Send + Sync>,
    promise_service: Arc<dyn promise::PromiseService + Send + Sync>,
    golem_config: Arc<golem_config::GolemConfig>,
    shard_service: Arc<dyn shard::ShardService + Send + Sync>,
    key_value_service: Arc<dyn key_value::KeyValueService + Send + Sync>,
    blob_store_service: Arc<dyn blob_store::BlobStoreService + Send + Sync>,
    oplog_service: Arc<dyn oplog::OplogService + Send + Sync>,
    rpc: Arc<dyn rpc::Rpc + Send + Sync>,
    scheduler_service: Arc<dyn scheduler::SchedulerService + Send + Sync>,
    worker_activator: Arc<dyn WorkerActivator<Ctx> + Send + Sync>,
    worker_proxy: Arc<dyn worker_proxy::WorkerProxy + Send + Sync>,
    events: Arc<Events>,
    file_loader: Arc<FileLoader>,
    plugins: Arc<
        dyn Plugins<<Ctx::ComponentOwner as ComponentOwner>::PluginOwner, Ctx::PluginScope>
            + Send
            + Sync,
    >,
    oplog_processor_plugin: Arc<dyn oplog::plugin::OplogProcessorPlugin + Send + Sync>,
    extra_deps: Ctx::ExtraDeps,
}

impl<Ctx: WorkerCtx> Clone for All<Ctx> {
    fn clone(&self) -> Self {
        Self {
            active_workers: self.active_workers.clone(),
            engine: self.engine.clone(),
            linker: self.linker.clone(),
            runtime: self.runtime.clone(),
            component_service: self.component_service.clone(),
            shard_manager_service: self.shard_manager_service.clone(),
            worker_fork: self.worker_fork.clone(),
            worker_service: self.worker_service.clone(),
            worker_enumeration_service: self.worker_enumeration_service.clone(),
            running_worker_enumeration_service: self.running_worker_enumeration_service.clone(),
            promise_service: self.promise_service.clone(),
            golem_config: self.golem_config.clone(),
            shard_service: self.shard_service.clone(),
            key_value_service: self.key_value_service.clone(),
            blob_store_service: self.blob_store_service.clone(),
            oplog_service: self.oplog_service.clone(),
            rpc: self.rpc.clone(),
            scheduler_service: self.scheduler_service.clone(),
            worker_activator: self.worker_activator.clone(),
            worker_proxy: self.worker_proxy.clone(),
            events: self.events.clone(),
            file_loader: self.file_loader.clone(),
            plugins: self.plugins.clone(),
            oplog_processor_plugin: self.oplog_processor_plugin.clone(),
            extra_deps: self.extra_deps.clone(),
        }
    }
}

impl<Ctx: WorkerCtx> All<Ctx> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        active_workers: Arc<active_workers::ActiveWorkers<Ctx>>,
        engine: Arc<wasmtime::Engine>,
        linker: Arc<wasmtime::component::Linker<Ctx>>,
        runtime: Handle,
        component_service: Arc<dyn component::ComponentService + Send + Sync>,
        shard_manager_service: Arc<dyn shard_manager::ShardManagerService + Send + Sync>,
        worker_fork: Arc<dyn worker_fork::WorkerForkService + Send + Sync>,
        worker_service: Arc<dyn worker::WorkerService + Send + Sync>,
        worker_enumeration_service: Arc<
            dyn worker_enumeration::WorkerEnumerationService + Send + Sync,
        >,
        running_worker_enumeration_service: Arc<
            dyn worker_enumeration::RunningWorkerEnumerationService + Send + Sync,
        >,
        promise_service: Arc<dyn promise::PromiseService + Send + Sync>,
        golem_config: Arc<golem_config::GolemConfig>,
        shard_service: Arc<dyn shard::ShardService + Send + Sync>,
        key_value_service: Arc<dyn key_value::KeyValueService + Send + Sync>,
        blob_store_service: Arc<dyn blob_store::BlobStoreService + Send + Sync>,
        oplog_service: Arc<dyn oplog::OplogService + Send + Sync>,
        rpc: Arc<dyn rpc::Rpc + Send + Sync>,
        scheduler_service: Arc<dyn scheduler::SchedulerService + Send + Sync>,
        worker_activator: Arc<dyn WorkerActivator<Ctx> + Send + Sync>,
        worker_proxy: Arc<dyn worker_proxy::WorkerProxy + Send + Sync>,
        events: Arc<Events>,
        file_loader: Arc<FileLoader>,
        plugins: Arc<
            dyn Plugins<<Ctx::ComponentOwner as ComponentOwner>::PluginOwner, Ctx::PluginScope>
                + Send
                + Sync,
        >,
        oplog_processor_plugin: Arc<dyn oplog::plugin::OplogProcessorPlugin + Send + Sync>,
        extra_deps: Ctx::ExtraDeps,
    ) -> Self {
        Self {
            active_workers,
            engine,
            linker,
            runtime,
            component_service,
            shard_manager_service,
            worker_fork,
            worker_service,
            worker_enumeration_service,
            running_worker_enumeration_service,
            promise_service,
            golem_config,
            shard_service,
            key_value_service,
            blob_store_service,
            oplog_service,
            rpc,
            scheduler_service,
            worker_activator,
            worker_proxy,
            events,
            file_loader,
            plugins,
            oplog_processor_plugin,
            extra_deps,
        }
    }

    pub fn from_other<T: HasAll<Ctx>>(this: &T) -> All<Ctx> {
        All::new(
            this.active_workers(),
            this.engine(),
            this.linker(),
            this.runtime(),
            this.component_service(),
            this.shard_manager_service(),
            this.worker_fork_service(),
            this.worker_service(),
            this.worker_enumeration_service(),
            this.running_worker_enumeration_service(),
            this.promise_service(),
            this.config(),
            this.shard_service(),
            this.key_value_service(),
            this.blob_store_service(),
            this.oplog_service(),
            this.rpc(),
            this.scheduler_service(),
            this.worker_activator(),
            this.worker_proxy(),
            this.events(),
            this.file_loader(),
            this.plugins(),
            this.oplog_processor_plugin(),
            this.extra_deps(),
        )
    }
}

/// Trait to be implemented by services using All to automatically get a HasXXX instance for each dependency
pub trait UsesAllDeps {
    type Ctx: WorkerCtx;

    fn all(&self) -> &All<Self::Ctx>;
}

impl<Ctx: WorkerCtx> UsesAllDeps for All<Ctx> {
    type Ctx = Ctx;
    fn all(&self) -> &All<Ctx> {
        self
    }
}

impl<Ctx: WorkerCtx, T: UsesAllDeps<Ctx = Ctx>> HasActiveWorkers<Ctx> for T {
    fn active_workers(&self) -> Arc<active_workers::ActiveWorkers<Ctx>> {
        self.all().active_workers.clone()
    }
}

impl<Ctx: WorkerCtx, T: UsesAllDeps<Ctx = Ctx>> HasComponentService for T {
    fn component_service(&self) -> Arc<dyn component::ComponentService + Send + Sync> {
        self.all().component_service.clone()
    }
}

impl<Ctx: WorkerCtx, T: UsesAllDeps<Ctx = Ctx>> HasShardManagerService for T {
    fn shard_manager_service(&self) -> Arc<dyn shard_manager::ShardManagerService + Send + Sync> {
        self.all().shard_manager_service.clone()
    }
}

impl<Ctx: WorkerCtx, T: UsesAllDeps<Ctx = Ctx>> HasConfig for T {
    fn config(&self) -> Arc<golem_config::GolemConfig> {
        self.all().golem_config.clone()
    }
}

impl<Ctx: WorkerCtx, T: UsesAllDeps<Ctx = Ctx>> HasWorkerForkService for T {
    fn worker_fork_service(&self) -> Arc<dyn worker_fork::WorkerForkService + Send + Sync> {
        self.all().worker_fork.clone()
    }
}

impl<Ctx: WorkerCtx, T: UsesAllDeps<Ctx = Ctx>> HasWorkerService for T {
    fn worker_service(&self) -> Arc<dyn worker::WorkerService + Send + Sync> {
        self.all().worker_service.clone()
    }
}

impl<Ctx: WorkerCtx, T: UsesAllDeps<Ctx = Ctx>> HasWorkerEnumerationService for T {
    fn worker_enumeration_service(
        &self,
    ) -> Arc<dyn worker_enumeration::WorkerEnumerationService + Send + Sync> {
        self.all().worker_enumeration_service.clone()
    }
}

impl<Ctx: WorkerCtx, T: UsesAllDeps<Ctx = Ctx>> HasRunningWorkerEnumerationService for T {
    fn running_worker_enumeration_service(
        &self,
    ) -> Arc<dyn worker_enumeration::RunningWorkerEnumerationService + Send + Sync> {
        self.all().running_worker_enumeration_service.clone()
    }
}

impl<Ctx: WorkerCtx, T: UsesAllDeps<Ctx = Ctx>> HasShardService for T {
    fn shard_service(&self) -> Arc<dyn shard::ShardService + Send + Sync> {
        self.all().shard_service.clone()
    }
}

impl<Ctx: WorkerCtx, T: UsesAllDeps<Ctx = Ctx>> HasPromiseService for T {
    fn promise_service(&self) -> Arc<dyn promise::PromiseService + Send + Sync> {
        self.all().promise_service.clone()
    }
}

impl<Ctx: WorkerCtx, T: UsesAllDeps<Ctx = Ctx>> HasWasmtimeEngine<Ctx> for T {
    fn engine(&self) -> Arc<wasmtime::Engine> {
        self.all().engine.clone()
    }

    fn linker(&self) -> Arc<wasmtime::component::Linker<Ctx>> {
        self.all().linker.clone()
    }

    fn runtime(&self) -> Handle {
        self.all().runtime.clone()
    }
}

impl<Ctx: WorkerCtx, T: UsesAllDeps<Ctx = Ctx>> HasKeyValueService for T {
    fn key_value_service(&self) -> Arc<dyn key_value::KeyValueService + Send + Sync> {
        self.all().key_value_service.clone()
    }
}

impl<Ctx: WorkerCtx, T: UsesAllDeps<Ctx = Ctx>> HasBlobStoreService for T {
    fn blob_store_service(&self) -> Arc<dyn blob_store::BlobStoreService + Send + Sync> {
        self.all().blob_store_service.clone()
    }
}

impl<Ctx: WorkerCtx, T: UsesAllDeps<Ctx = Ctx>> HasOplogService for T {
    fn oplog_service(&self) -> Arc<dyn oplog::OplogService + Send + Sync> {
        self.all().oplog_service.clone()
    }
}

impl<Ctx: WorkerCtx, T: UsesAllDeps<Ctx = Ctx>> HasRpc for T {
    fn rpc(&self) -> Arc<dyn rpc::Rpc + Send + Sync> {
        self.all().rpc.clone()
    }
}

impl<Ctx: WorkerCtx, T: UsesAllDeps<Ctx = Ctx>> HasSchedulerService for T {
    fn scheduler_service(&self) -> Arc<dyn scheduler::SchedulerService + Send + Sync> {
        self.all().scheduler_service.clone()
    }
}

impl<Ctx: WorkerCtx, T: UsesAllDeps<Ctx = Ctx>> HasWorkerActivator<Ctx> for T {
    fn worker_activator(&self) -> Arc<dyn WorkerActivator<Ctx> + Send + Sync> {
        self.all().worker_activator.clone()
    }
}

impl<Ctx: WorkerCtx, T: UsesAllDeps<Ctx = Ctx>> HasWorkerProxy for T {
    fn worker_proxy(&self) -> Arc<dyn worker_proxy::WorkerProxy + Send + Sync> {
        self.all().worker_proxy.clone()
    }
}

impl<Ctx: WorkerCtx, T: UsesAllDeps<Ctx = Ctx>> HasEvents for T {
    fn events(&self) -> Arc<Events> {
        self.all().events.clone()
    }
}

impl<Ctx: WorkerCtx, T: UsesAllDeps<Ctx = Ctx>> HasFileLoader for T {
    fn file_loader(&self) -> Arc<FileLoader> {
        self.all().file_loader.clone()
    }
}

impl<Ctx: WorkerCtx, T: UsesAllDeps<Ctx = Ctx>>
    HasPlugins<<Ctx::ComponentOwner as ComponentOwner>::PluginOwner, Ctx::PluginScope> for T
{
    fn plugins(
        &self,
    ) -> Arc<
        dyn Plugins<<Ctx::ComponentOwner as ComponentOwner>::PluginOwner, Ctx::PluginScope>
            + Send
            + Sync,
    > {
        self.all().plugins.clone()
    }
}

impl<Ctx: WorkerCtx, T: UsesAllDeps<Ctx = Ctx>> HasOplogProcessorPlugin for T {
    fn oplog_processor_plugin(&self) -> Arc<dyn oplog::plugin::OplogProcessorPlugin + Send + Sync> {
        self.all().oplog_processor_plugin.clone()
    }
}

impl<Ctx: WorkerCtx, T: UsesAllDeps<Ctx = Ctx>> HasExtraDeps<Ctx> for T {
    fn extra_deps(&self) -> Ctx::ExtraDeps {
        self.all().extra_deps.clone()
    }
}
