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
pub mod key_value;
pub mod promise;
pub mod shard;
pub mod shard_manager;
pub mod template;
pub mod worker;
pub mod worker_activator;
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

pub trait HasExtraDeps<Ctx: WorkerCtx> {
    fn extra_deps(&self) -> Ctx::ExtraDeps;
}

/// HasAll is a shortcut for requiring all available service dependencies
pub trait HasAll<Ctx: WorkerCtx>:
    HasActiveWorkers<Ctx>
    + HasTemplateService
    + HasConfig
    + HasWorkerService
    + HasInvocationKeyService
    + HasPromiseService
    + HasWasmtimeEngine<Ctx>
    + HasKeyValueService
    + HasBlobStoreService
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
            + HasInvocationKeyService
            + HasPromiseService
            + HasWasmtimeEngine<Ctx>
            + HasKeyValueService
            + HasBlobStoreService
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
    promise_service: Arc<dyn promise::PromiseService + Send + Sync>,
    golem_config: Arc<golem_config::GolemConfig>,
    invocation_key_service: Arc<dyn invocation_key::InvocationKeyService + Send + Sync>,
    shard_service: Arc<dyn shard::ShardService + Send + Sync>,
    key_value_service: Arc<dyn key_value::KeyValueService + Send + Sync>,
    blob_store_service: Arc<dyn blob_store::BlobStoreService + Send + Sync>,
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
            promise_service: self.promise_service.clone(),
            golem_config: self.golem_config.clone(),
            invocation_key_service: self.invocation_key_service.clone(),
            shard_service: self.shard_service.clone(),
            key_value_service: self.key_value_service.clone(),
            blob_store_service: self.blob_store_service.clone(),
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
        promise_service: Arc<dyn promise::PromiseService + Send + Sync>,
        golem_config: Arc<golem_config::GolemConfig>,
        invocation_key_service: Arc<dyn invocation_key::InvocationKeyService + Send + Sync>,
        shard_service: Arc<dyn shard::ShardService + Send + Sync>,
        key_value_service: Arc<dyn key_value::KeyValueService + Send + Sync>,
        blob_store_service: Arc<dyn blob_store::BlobStoreService + Send + Sync>,
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
            promise_service,
            golem_config,
            invocation_key_service,
            shard_service,
            key_value_service,
            blob_store_service,
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
        let promise_service = Arc::new(promise::PromiseServiceMock::new());
        let golem_config = Arc::new(golem_config::GolemConfig::default());
        let invocation_key_service = Arc::new(invocation_key::InvocationKeyServiceDefault::new());
        let shard_service = Arc::new(shard::ShardServiceDefault::new());
        let shard_manager_service = Arc::new(shard_manager::ShardManagerServiceSingleShard::new());
        let key_value_service = Arc::new(key_value::KeyValueServiceInMemory::new());
        let blob_store_service = Arc::new(blob_store::BlobStoreServiceInMemory::new());
        Self {
            active_workers,
            engine,
            linker,
            runtime,
            template_service,
            shard_manager_service,
            worker_service,
            promise_service,
            golem_config,
            invocation_key_service,
            shard_service,
            key_value_service,
            blob_store_service,
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

impl<Ctx: WorkerCtx, T: UsesAllDeps<Ctx = Ctx>> HasExtraDeps<Ctx> for T {
    fn extra_deps(&self) -> Ctx::ExtraDeps {
        self.all().extra_deps.clone()
    }
}
