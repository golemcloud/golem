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

use std::sync::Arc;
#[cfg(any(feature = "mocks", test))]
use std::time::Duration;

use tokio::runtime::Handle;

use crate::workerctx::WorkerCtx;

pub mod active_workers;
pub mod blob_store;
pub mod compiled_template;
pub mod golem_config;
pub mod invocation_key;
pub mod invocation_queue;
pub mod key_value;
pub mod oplog;
pub mod promise;
pub mod recovery;
pub mod rpc;
pub mod scheduler;
pub mod shard;
pub mod shard_manager;
pub mod template;
pub mod worker;
pub mod worker_activator;
pub mod worker_enumeration;
pub mod worker_event;

// HasXXX traits for fine-grained control of which dependencies a function needs

pub trait HasActiveWorkers<Ctx: WorkerCtx> {
    fn active_workers(&self) -> Arc<active_workers::ActiveWorkers<Ctx>>;
}

pub trait HasTemplateService {
    fn template_service(&self) -> Arc<dyn template::TemplateService + Send + Sync>;
}

pub trait HasShardManagerService {
    fn shard_manager_service(&self) -> Arc<dyn shard_manager::ShardManagerService + Send + Sync>;
}

pub trait HasConfig {
    fn config(&self) -> Arc<golem_config::GolemConfig>;
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

pub trait HasInvocationKeyService {
    fn invocation_key_service(&self)
        -> Arc<dyn invocation_key::InvocationKeyService + Send + Sync>;
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

pub trait HasRecoveryManagement {
    fn recovery_management(&self) -> Arc<dyn recovery::RecoveryManagement + Send + Sync>;
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

pub trait HasInvocationQueue {
    fn invocation_queue(&self) -> Arc<dyn invocation_queue::InvocationQueue>;

    /// Because an invocation queue belongs to a worker, but requires the worker to perform the invocation,
    /// we need to attach the invocation queue to the worker after it is created.
    fn attach_invocation_queue(&self, invocation_queue: Arc<dyn invocation_queue::InvocationQueue>);
}

pub trait HasOplog {
    fn oplog(&self) -> Arc<dyn oplog::Oplog + Send + Sync>;
}

/// HasAll is a shortcut for requiring all available service dependencies
pub trait HasAll<Ctx: WorkerCtx>:
    HasActiveWorkers<Ctx>
    + HasTemplateService
    + HasConfig
    + HasWorkerService
    + HasWorkerEnumerationService
    + HasRunningWorkerEnumerationService
    + HasInvocationKeyService
    + HasPromiseService
    + HasWasmtimeEngine<Ctx>
    + HasKeyValueService
    + HasBlobStoreService
    + HasOplogService
    + HasRecoveryManagement
    + HasRpc
    + HasSchedulerService
    + HasExtraDeps<Ctx>
    + Clone
{
}

impl<
        Ctx: WorkerCtx,
        T: HasActiveWorkers<Ctx>
            + HasTemplateService
            + HasConfig
            + HasWorkerService
            + HasWorkerEnumerationService
            + HasRunningWorkerEnumerationService
            + HasInvocationKeyService
            + HasPromiseService
            + HasWasmtimeEngine<Ctx>
            + HasKeyValueService
            + HasBlobStoreService
            + HasOplogService
            + HasRecoveryManagement
            + HasRpc
            + HasSchedulerService
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
    template_service: Arc<dyn template::TemplateService + Send + Sync>,
    shard_manager_service: Arc<dyn shard_manager::ShardManagerService + Send + Sync>,
    worker_service: Arc<dyn worker::WorkerService + Send + Sync>,
    worker_enumeration_service: Arc<dyn worker_enumeration::WorkerEnumerationService + Send + Sync>,
    running_worker_enumeration_service:
        Arc<dyn worker_enumeration::RunningWorkerEnumerationService + Send + Sync>,
    promise_service: Arc<dyn promise::PromiseService + Send + Sync>,
    golem_config: Arc<golem_config::GolemConfig>,
    invocation_key_service: Arc<dyn invocation_key::InvocationKeyService + Send + Sync>,
    shard_service: Arc<dyn shard::ShardService + Send + Sync>,
    key_value_service: Arc<dyn key_value::KeyValueService + Send + Sync>,
    blob_store_service: Arc<dyn blob_store::BlobStoreService + Send + Sync>,
    oplog_service: Arc<dyn oplog::OplogService + Send + Sync>,
    recovery_management: Arc<dyn recovery::RecoveryManagement + Send + Sync>,
    rpc: Arc<dyn rpc::Rpc + Send + Sync>,
    scheduler_service: Arc<dyn scheduler::SchedulerService + Send + Sync>,
    extra_deps: Ctx::ExtraDeps,
}

impl<Ctx: WorkerCtx> Clone for All<Ctx> {
    fn clone(&self) -> Self {
        Self {
            active_workers: self.active_workers.clone(),
            engine: self.engine.clone(),
            linker: self.linker.clone(),
            runtime: self.runtime.clone(),
            template_service: self.template_service.clone(),
            shard_manager_service: self.shard_manager_service.clone(),
            worker_service: self.worker_service.clone(),
            worker_enumeration_service: self.worker_enumeration_service.clone(),
            running_worker_enumeration_service: self.running_worker_enumeration_service.clone(),
            promise_service: self.promise_service.clone(),
            golem_config: self.golem_config.clone(),
            invocation_key_service: self.invocation_key_service.clone(),
            shard_service: self.shard_service.clone(),
            key_value_service: self.key_value_service.clone(),
            blob_store_service: self.blob_store_service.clone(),
            oplog_service: self.oplog_service.clone(),
            recovery_management: self.recovery_management.clone(),
            rpc: self.rpc.clone(),
            scheduler_service: self.scheduler_service.clone(),
            extra_deps: self.extra_deps.clone(),
        }
    }
}

impl<Ctx: WorkerCtx> All<Ctx> {
    pub fn new(
        active_workers: Arc<active_workers::ActiveWorkers<Ctx>>,
        engine: Arc<wasmtime::Engine>,
        linker: Arc<wasmtime::component::Linker<Ctx>>,
        runtime: Handle,
        template_service: Arc<dyn template::TemplateService + Send + Sync>,
        shard_manager_service: Arc<dyn shard_manager::ShardManagerService + Send + Sync>,
        worker_service: Arc<dyn worker::WorkerService + Send + Sync>,
        worker_enumeration_service: Arc<
            dyn worker_enumeration::WorkerEnumerationService + Send + Sync,
        >,
        running_worker_enumeration_service: Arc<
            dyn worker_enumeration::RunningWorkerEnumerationService + Send + Sync,
        >,
        promise_service: Arc<dyn promise::PromiseService + Send + Sync>,
        golem_config: Arc<golem_config::GolemConfig>,
        invocation_key_service: Arc<dyn invocation_key::InvocationKeyService + Send + Sync>,
        shard_service: Arc<dyn shard::ShardService + Send + Sync>,
        key_value_service: Arc<dyn key_value::KeyValueService + Send + Sync>,
        blob_store_service: Arc<dyn blob_store::BlobStoreService + Send + Sync>,
        oplog_service: Arc<dyn oplog::OplogService + Send + Sync>,
        recovery_management: Arc<dyn recovery::RecoveryManagement + Send + Sync>,
        rpc: Arc<dyn rpc::Rpc + Send + Sync>,
        scheduler_service: Arc<dyn scheduler::SchedulerService + Send + Sync>,
        extra_deps: Ctx::ExtraDeps,
    ) -> Self {
        Self {
            active_workers,
            engine,
            linker,
            runtime,
            template_service,
            shard_manager_service,
            worker_service,
            worker_enumeration_service,
            running_worker_enumeration_service,
            promise_service,
            golem_config,
            invocation_key_service,
            shard_service,
            key_value_service,
            blob_store_service,
            oplog_service,
            recovery_management,
            rpc,
            scheduler_service,
            extra_deps,
        }
    }

    #[cfg(any(feature = "mocks", test))]
    pub async fn mocked(mocked_extra_deps: Ctx::ExtraDeps) -> Self {
        let active_workers = Arc::new(active_workers::ActiveWorkers::bounded(
            100,
            0.01,
            Duration::from_secs(60),
        ));
        let engine = Arc::new(wasmtime::Engine::default());
        let linker = Arc::new(wasmtime::component::Linker::new(&engine));
        let runtime = Handle::current();
        let template_service = Arc::new(template::TemplateServiceMock::new());
        let worker_service = Arc::new(worker::WorkerServiceMock::new());
        let worker_enumeration_service =
            Arc::new(worker_enumeration::WorkerEnumerationServiceMock::new());
        let running_worker_enumeration_service =
            Arc::new(worker_enumeration::RunningWorkerEnumerationServiceMock::new());
        let promise_service = Arc::new(promise::PromiseServiceMock::new());
        let golem_config = Arc::new(golem_config::GolemConfig::default());
        let invocation_key_service =
            Arc::new(invocation_key::InvocationKeyServiceDefault::default());
        let shard_service = Arc::new(shard::ShardServiceDefault::new());
        let shard_manager_service = Arc::new(shard_manager::ShardManagerServiceSingleShard::new());
        let key_value_service = Arc::new(key_value::KeyValueServiceInMemory::new());
        let blob_store_service = Arc::new(blob_store::BlobStoreServiceInMemory::new());
        let oplog_service = Arc::new(oplog::OplogServiceMock::new());
        let recovery_management = Arc::new(recovery::RecoveryManagementMock::new());
        let rpc = Arc::new(rpc::RpcMock::new());
        let scheduler_service = Arc::new(scheduler::SchedulerServiceMock::new());
        Self {
            active_workers,
            engine,
            linker,
            runtime,
            template_service,
            shard_manager_service,
            worker_service,
            worker_enumeration_service,
            running_worker_enumeration_service,
            promise_service,
            golem_config,
            invocation_key_service,
            shard_service,
            key_value_service,
            blob_store_service,
            oplog_service,
            recovery_management,
            rpc,
            scheduler_service,
            extra_deps: mocked_extra_deps,
        }
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

impl<Ctx: WorkerCtx, T: UsesAllDeps<Ctx = Ctx>> HasTemplateService for T {
    fn template_service(&self) -> Arc<dyn template::TemplateService + Send + Sync> {
        self.all().template_service.clone()
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

impl<Ctx: WorkerCtx, T: UsesAllDeps<Ctx = Ctx>> HasInvocationKeyService for T {
    fn invocation_key_service(
        &self,
    ) -> Arc<dyn invocation_key::InvocationKeyService + Send + Sync> {
        self.all().invocation_key_service.clone()
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

impl<Ctx: WorkerCtx, T: UsesAllDeps<Ctx = Ctx>> HasRecoveryManagement for T {
    fn recovery_management(&self) -> Arc<dyn recovery::RecoveryManagement + Send + Sync> {
        self.all().recovery_management.clone()
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

impl<Ctx: WorkerCtx, T: UsesAllDeps<Ctx = Ctx>> HasExtraDeps<Ctx> for T {
    fn extra_deps(&self) -> Ctx::ExtraDeps {
        self.all().extra_deps.clone()
    }
}
