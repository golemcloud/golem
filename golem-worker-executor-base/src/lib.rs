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

pub mod durable_host;
pub mod error;
pub mod grpc;
pub mod metrics;
pub mod model;
pub mod preview2;
pub mod services;
pub mod storage;
pub mod virtual_export_compat;
pub mod wasi_host;
pub mod worker;
pub mod workerctx;

#[cfg(test)]
test_r::enable!();

use crate::grpc::WorkerExecutorImpl;
use crate::services::active_workers::ActiveWorkers;
use crate::services::blob_store::{BlobStoreService, DefaultBlobStoreService};
use crate::services::component::ComponentService;
use crate::services::events::Events;
use crate::services::golem_config::{GolemConfig, IndexedStorageConfig, KeyValueStorageConfig};
use crate::services::key_value::{DefaultKeyValueService, KeyValueService};
use crate::services::oplog::plugin::{
    ForwardingOplogService, OplogProcessorPlugin, PerExecutorOplogProcessorPlugin,
};
use crate::services::oplog::{
    BlobOplogArchiveService, CompressedOplogArchiveService, MultiLayerOplogService,
    OplogArchiveService, OplogService, PrimaryOplogService,
};
use crate::services::plugins::{Plugins, PluginsObservations};
use crate::services::promise::{DefaultPromiseService, PromiseService};
use crate::services::scheduler::{SchedulerService, SchedulerServiceDefault};
use crate::services::shard::{ShardService, ShardServiceDefault};
use crate::services::shard_manager::ShardManagerService;
use crate::services::worker::{DefaultWorkerService, WorkerService};
use crate::services::worker_activator::{LazyWorkerActivator, WorkerActivator};
use crate::services::worker_enumeration::{
    DefaultWorkerEnumerationService, RunningWorkerEnumerationService,
    RunningWorkerEnumerationServiceDefault, WorkerEnumerationService,
};
use crate::services::worker_proxy::{RemoteWorkerProxy, WorkerProxy};
use crate::services::{rdbms, shard_manager, All, HasConfig};
use crate::storage::indexed::redis::RedisIndexedStorage;
use crate::storage::indexed::sqlite::SqliteIndexedStorage;
use crate::storage::indexed::IndexedStorage;
use crate::storage::keyvalue::memory::InMemoryKeyValueStorage;
use crate::storage::keyvalue::redis::RedisKeyValueStorage;
use crate::storage::keyvalue::KeyValueStorage;
use crate::workerctx::WorkerCtx;
use anyhow::anyhow;
use async_trait::async_trait;
use golem_api_grpc::proto;
use golem_api_grpc::proto::golem::workerexecutor::v1::worker_executor_server::WorkerExecutorServer;
use golem_common::golem_version;
use golem_common::model::component::{ComponentOwner, DefaultComponentOwner};
use golem_common::model::plugin::{
    DefaultPluginOwner, DefaultPluginScope, PluginOwner, PluginScope,
};
use golem_common::redis::RedisPool;
use golem_service_base::config::BlobStorageConfig;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::service::initial_component_files::InitialComponentFilesService;
use golem_service_base::storage::blob::s3::S3BlobStorage;
use golem_service_base::storage::blob::sqlite::SqliteBlobStorage;
use golem_service_base::storage::blob::BlobStorage;
use humansize::{ISizeFormatter, BINARY};
use nonempty_collections::NEVec;
use prometheus::Registry;
use services::file_loader::FileLoader;
use std::sync::Arc;
use storage::keyvalue::sqlite::SqliteKeyValueStorage;
use tokio::net::TcpListener;
use tokio::runtime::Handle;
use tokio::task::JoinSet;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::codec::CompressionEncoding;
use tonic::transport::Server;
use tracing::{info, Instrument};
use uuid::Uuid;
use wasmtime::component::Linker;
use wasmtime::{Config, Engine, WasmBacktraceDetails};

const VERSION: &str = golem_version!();

pub struct RunDetails {
    pub http_port: u16,
    pub grpc_port: u16,
    pub epoch_thread: std::sync::Mutex<Option<std::thread::JoinHandle<()>>>,
}

/// The Bootstrap trait should be implemented by all Worker Executors to customize the initialization
/// of its services.
/// With a valid `Bootstrap` implementation the service can be started with the `run` method.
#[async_trait]
#[allow(clippy::too_many_arguments)]
pub trait Bootstrap<Ctx: WorkerCtx> {
    /// Allows customizing the `ActiveWorkers` service.
    fn create_active_workers(&self, golem_config: &GolemConfig) -> Arc<ActiveWorkers<Ctx>>;

    async fn run_grpc_server(
        &self,
        service_dependencies: All<Ctx>,
        lazy_worker_activator: Arc<LazyWorkerActivator<Ctx>>,
        join_set: &mut JoinSet<Result<(), anyhow::Error>>,
    ) -> anyhow::Result<u16> {
        let golem_config = service_dependencies.config();
        let (mut health_reporter, health_service) = tonic_health::server::health_reporter();
        health_reporter
            .set_serving::<WorkerExecutorServer<WorkerExecutorImpl<Ctx, All<Ctx>>>>()
            .await;

        let reflection_service = tonic_reflection::server::Builder::configure()
            .register_encoded_file_descriptor_set(proto::FILE_DESCRIPTOR_SET)
            .build_v1()?;

        let addr = golem_config.grpc_addr()?;

        let listener = TcpListener::bind(addr).await?;
        let grpc_port = listener.local_addr()?.port();

        let worker_impl = WorkerExecutorImpl::<Ctx, All<Ctx>>::new(
            service_dependencies,
            lazy_worker_activator,
            grpc_port,
        )
        .await?;

        let service = WorkerExecutorServer::new(worker_impl)
            .accept_compressed(CompressionEncoding::Gzip)
            .send_compressed(CompressionEncoding::Gzip);

        info!("Starting gRPC server on port {grpc_port}");

        join_set.spawn(
            async move {
                Server::builder()
                    .max_concurrent_streams(Some(golem_config.limits.max_concurrent_streams))
                    .add_service(reflection_service)
                    .add_service(service)
                    .add_service(health_service)
                    .serve_with_incoming(TcpListenerStream::new(listener))
                    .await
                    .map_err(|err| anyhow!(err))
            }
            .in_current_span(),
        );

        Ok(grpc_port)
    }

    #[allow(clippy::type_complexity)]
    fn create_plugins(
        &self,
        golem_config: &GolemConfig,
    ) -> (Arc<dyn Plugins<Ctx::Types>>, Arc<dyn PluginsObservations>);

    fn create_component_service(
        &self,
        golem_config: &GolemConfig,
        blob_storage: Arc<dyn BlobStorage + Send + Sync>,
        plugin_observations: Arc<dyn PluginsObservations>,
    ) -> Arc<dyn ComponentService<Ctx::Types>>;

    /// Allows customizing the `All` service.
    /// This is the place to initialize additional services and store them in `All`'s `extra_deps`
    /// field.
    #[allow(clippy::too_many_arguments)]
    async fn create_services(
        &self,
        active_workers: Arc<ActiveWorkers<Ctx>>,
        engine: Arc<Engine>,
        linker: Arc<Linker<Ctx>>,
        runtime: Handle,
        component_service: Arc<dyn ComponentService<Ctx::Types>>,
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
        worker_activator: Arc<dyn WorkerActivator<Ctx> + Send + Sync>,
        oplog_service: Arc<dyn OplogService + Send + Sync>,
        scheduler_service: Arc<dyn SchedulerService + Send + Sync>,
        worker_proxy: Arc<dyn WorkerProxy + Send + Sync>,
        events: Arc<Events>,
        file_loader: Arc<FileLoader>,
        plugins: Arc<dyn Plugins<Ctx::Types>>,
        oplog_processor_plugin: Arc<dyn OplogProcessorPlugin + Send + Sync>,
    ) -> anyhow::Result<All<Ctx>>;

    /// Can be overridden to customize the wasmtime configuration
    fn create_wasmtime_config(&self) -> Config {
        let mut config = Config::default();

        config.wasm_multi_value(true);
        config.async_support(true);
        config.wasm_component_model(true);
        config.epoch_interruption(true);
        config.consume_fuel(true);
        config.wasm_backtrace_details(WasmBacktraceDetails::Enable);

        config
    }

    /// This method is responsible for linking all the host function implementations the worker
    /// executor supports.
    fn create_wasmtime_linker(&self, engine: &Engine) -> anyhow::Result<Linker<Ctx>>;

    /// Runs the worker executor
    async fn run(
        &self,
        golem_config: GolemConfig,
        prometheus_registry: Registry,
        runtime: Handle,
        join_set: &mut JoinSet<Result<(), anyhow::Error>>,
    ) -> anyhow::Result<RunDetails> {
        info!("Golem Worker Executor starting up...");

        let total_system_memory = golem_config.memory.total_system_memory();
        let system_memory = golem_config.memory.system_memory();
        let worker_memory = golem_config.memory.worker_memory();
        info!(
            "Total system memory: {}, Available system memory: {}, Total memory available for workers: {}",
            ISizeFormatter::new(total_system_memory, BINARY),
            ISizeFormatter::new(system_memory, BINARY),
            ISizeFormatter::new(worker_memory, BINARY)
        );

        let lazy_worker_activator = Arc::new(LazyWorkerActivator::new());

        let (worker_executor_impl, epoch_thread) = create_worker_executor_impl::<Ctx, Self>(
            golem_config.clone(),
            self,
            runtime.clone(),
            &lazy_worker_activator,
        )
        .await?;

        let grpc_port = self
            .run_grpc_server(worker_executor_impl, lazy_worker_activator, join_set)
            .await?;

        let http_port = golem_service_base::observability::start_health_and_metrics_server(
            golem_config.http_addr()?,
            prometheus_registry,
            "Worker executor is running",
            join_set,
        )
        .await?;

        Ok(RunDetails {
            http_port,
            grpc_port,
            epoch_thread: std::sync::Mutex::new(Some(epoch_thread)),
        })
    }
}

async fn create_worker_executor_impl<Ctx: WorkerCtx, A: Bootstrap<Ctx> + ?Sized>(
    golem_config: GolemConfig,
    bootstrap: &A,
    runtime: Handle,
    lazy_worker_activator: &Arc<LazyWorkerActivator<Ctx>>,
) -> Result<(All<Ctx>, std::thread::JoinHandle<()>), anyhow::Error> {
    let (redis, sqlite, key_value_storage): (
        Option<RedisPool>,
        Option<SqlitePool>,
        Arc<dyn KeyValueStorage + Send + Sync>,
    ) = match &golem_config.key_value_storage {
        KeyValueStorageConfig::Redis(redis) => {
            info!("Using Redis for key-value storage at {}", redis.url());
            let pool = RedisPool::configured(redis)
                .await
                .map_err(|err| anyhow!(err))?;
            let key_value_storage: Arc<dyn KeyValueStorage + Send + Sync> =
                Arc::new(RedisKeyValueStorage::new(pool.clone()));
            (Some(pool), None, key_value_storage)
        }
        KeyValueStorageConfig::InMemory(_) => {
            info!("Using in-memory key-value storage");
            (None, None, Arc::new(InMemoryKeyValueStorage::new()))
        }
        KeyValueStorageConfig::Sqlite(sqlite) => {
            info!("Using Sqlite for key-value storage at {}", sqlite.database);
            let pool = SqlitePool::configured(sqlite)
                .await
                .map_err(|err| anyhow!(err))?;
            let key_value_storage: Arc<dyn KeyValueStorage + Send + Sync> = Arc::new(
                SqliteKeyValueStorage::new(pool.clone())
                    .await
                    .map_err(|err| anyhow!(err))?,
            );
            (None, Some(pool), key_value_storage)
        }
    };

    let indexed_storage: Arc<dyn IndexedStorage + Send + Sync> = match &golem_config.indexed_storage
    {
        IndexedStorageConfig::KVStoreRedis(_) => {
            info!("Using the same Redis for indexed-storage");
            let redis = redis
                .expect("Redis must be configured as key-value storage when using KVStoreRedis");
            Arc::new(RedisIndexedStorage::new(redis.clone()))
        }
        IndexedStorageConfig::Redis(redis) => {
            info!("Using Redis for indexed-storage at {}", redis.url());
            let pool = RedisPool::configured(redis).await?;
            Arc::new(RedisIndexedStorage::new(pool.clone()))
        }
        IndexedStorageConfig::KVStoreSqlite(_) => {
            info!("Using the same Sqlite for indexed-storage");
            let sqlite = sqlite
                .clone()
                .expect("Sqlite must be configured as key-value storage when using KVStoreSqlite");
            Arc::new(
                SqliteIndexedStorage::new(sqlite.clone())
                    .await
                    .map_err(|err| anyhow!(err))?,
            )
        }
        IndexedStorageConfig::Sqlite(sqlite) => {
            info!("Using Sqlite for indexed storage at {}", sqlite.database);
            let pool = SqlitePool::configured(sqlite)
                .await
                .map_err(|err| anyhow!(err))?;
            Arc::new(
                SqliteIndexedStorage::new(pool.clone())
                    .await
                    .map_err(|err| anyhow!(err))?,
            )
        }
        IndexedStorageConfig::InMemory(_) => {
            info!("Using in-memory indexed storage");
            Arc::new(storage::indexed::memory::InMemoryIndexedStorage::new())
        }
    };
    let blob_storage: Arc<dyn BlobStorage + Send + Sync> = match &golem_config.blob_storage {
        BlobStorageConfig::S3(config) => {
            info!("Using S3 for blob storage");
            Arc::new(S3BlobStorage::new(config.clone()).await)
        }
        BlobStorageConfig::LocalFileSystem(config) => {
            info!(
                "Using local file system for blob storage at {:?}",
                config.root
            );
            Arc::new(
                golem_service_base::storage::blob::fs::FileSystemBlobStorage::new(&config.root)
                    .await
                    .map_err(|err| anyhow!(err))?,
            )
        }
        BlobStorageConfig::KVStoreSqlite(_) => {
            info!("Using the same Sqlite for blob-storage");
            let sqlite = sqlite
                .expect("Sqlite must be configured as key-value storage when using KVStoreSqlite");
            Arc::new(
                SqliteBlobStorage::new(sqlite.clone())
                    .await
                    .map_err(|err| anyhow!(err))?,
            )
        }
        BlobStorageConfig::Sqlite(sqlite) => {
            info!("Using Sqlite for blob storage at {}", sqlite.database);
            let pool = SqlitePool::configured(sqlite)
                .await
                .map_err(|err| anyhow!(err))?;
            Arc::new(
                SqliteBlobStorage::new(pool.clone())
                    .await
                    .map_err(|err| anyhow!(err))?,
            )
        }
        BlobStorageConfig::InMemory(_) => {
            info!("Using in-memory blob storage");
            Arc::new(golem_service_base::storage::blob::memory::InMemoryBlobStorage::new())
        }
    };

    let initial_files_service = Arc::new(InitialComponentFilesService::new(blob_storage.clone()));

    let file_loader = Arc::new(FileLoader::new(initial_files_service.clone())?);
    let (plugins, plugins_observations) = bootstrap.create_plugins(&golem_config);

    let component_service = bootstrap.create_component_service(
        &golem_config,
        blob_storage.clone(),
        plugins_observations,
    );

    let golem_config = Arc::new(golem_config.clone());
    let promise_service: Arc<dyn PromiseService + Send + Sync> =
        Arc::new(DefaultPromiseService::new(key_value_storage.clone()));
    let shard_service = Arc::new(ShardServiceDefault::new());

    let mut oplog_archives: Vec<Arc<dyn OplogArchiveService + Send + Sync>> = Vec::new();
    for idx in 1..golem_config.oplog.indexed_storage_layers {
        let svc: Arc<dyn OplogArchiveService + Send + Sync> = Arc::new(
            CompressedOplogArchiveService::new(indexed_storage.clone(), idx),
        );
        oplog_archives.push(svc);
    }
    for idx in 0..golem_config.oplog.blob_storage_layers {
        let svc: Arc<dyn OplogArchiveService + Send + Sync> =
            Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), idx));
        oplog_archives.push(svc);
    }
    let oplog_archives = NEVec::try_from_vec(oplog_archives);

    let base_oplog_service: Arc<dyn OplogService + Send + Sync> = match oplog_archives {
        None => Arc::new(
            PrimaryOplogService::new(
                indexed_storage.clone(),
                blob_storage.clone(),
                golem_config.oplog.max_operations_before_commit,
                golem_config.oplog.max_payload_size,
            )
            .await,
        ),
        Some(oplog_archives) => {
            let primary = Arc::new(
                PrimaryOplogService::new(
                    indexed_storage.clone(),
                    blob_storage.clone(),
                    golem_config.oplog.max_operations_before_commit,
                    golem_config.oplog.max_payload_size,
                )
                .await,
            );

            Arc::new(MultiLayerOplogService::new(
                primary,
                oplog_archives,
                golem_config.oplog.entry_count_limit,
                golem_config.oplog.max_operations_before_commit_ephemeral,
            ))
        }
    };

    let active_workers = bootstrap.create_active_workers(&golem_config);

    let running_worker_enumeration_service = Arc::new(RunningWorkerEnumerationServiceDefault::new(
        active_workers.clone(),
    ));

    let shard_manager_service = shard_manager::configured(&golem_config.shard_manager_service);

    let config = bootstrap.create_wasmtime_config();
    let engine = Arc::new(Engine::new(&config)?);
    let linker = bootstrap.create_wasmtime_linker(&engine)?;

    let engine_ref: Arc<Engine> = engine.clone();

    let epoch_interval = golem_config.limits.epoch_interval;
    let epoch_thread = std::thread::spawn(move || loop {
        std::thread::sleep(epoch_interval);
        engine_ref.increment_epoch();
    });

    let linker = Arc::new(linker);

    let key_value_service = Arc::new(DefaultKeyValueService::new(key_value_storage.clone()));

    let blob_store_service = Arc::new(DefaultBlobStoreService::new(blob_storage.clone()));

    let worker_proxy: Arc<dyn WorkerProxy + Send + Sync> = Arc::new(RemoteWorkerProxy::new(
        golem_config.public_worker_api.uri(),
        golem_config
            .public_worker_api
            .access_token
            .parse::<Uuid>()
            .expect("Access token must be an UUID"),
        golem_config.public_worker_api.retries.clone(),
        golem_config.public_worker_api.connect_timeout,
    ));

    let rdbms_service: Arc<dyn rdbms::RdbmsService + Send + Sync> =
        Arc::new(rdbms::RdbmsServiceDefault::new(golem_config.rdbms));

    let events = Arc::new(Events::new(
        golem_config.limits.invocation_result_broadcast_capacity,
    ));

    let oplog_processor_plugin = Arc::new(PerExecutorOplogProcessorPlugin::new(
        component_service.clone(),
        shard_service.clone(),
        lazy_worker_activator.clone(),
        plugins.clone(),
    ));

    let oplog_service: Arc<dyn OplogService + Send + Sync> = Arc::new(ForwardingOplogService::new(
        base_oplog_service,
        oplog_processor_plugin.clone(),
        component_service.clone(),
        plugins.clone(),
    ));

    let worker_service = Arc::new(DefaultWorkerService::new(
        key_value_storage.clone(),
        shard_service.clone(),
        oplog_service.clone(),
    ));
    let worker_enumeration_service = Arc::new(DefaultWorkerEnumerationService::new(
        worker_service.clone(),
        oplog_service.clone(),
        golem_config.clone(),
    ));

    let scheduler_service = SchedulerServiceDefault::new(
        key_value_storage.clone(),
        shard_service.clone(),
        promise_service.clone(),
        Arc::new(lazy_worker_activator.clone() as Arc<dyn WorkerActivator<Ctx> + Send + Sync>),
        oplog_service.clone(),
        worker_service.clone(),
        golem_config.scheduler.refresh_interval,
    );

    let all = bootstrap
        .create_services(
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
            rdbms_service,
            lazy_worker_activator.clone(),
            oplog_service,
            scheduler_service,
            worker_proxy,
            events,
            file_loader,
            plugins,
            oplog_processor_plugin,
        )
        .await?;

    Ok((all, epoch_thread))
}

/// Trait to encapsulate different types that are used throughout the codebase (oss, cloud, testing, ...).
/// Implementating types should be fieldless structs.
///
/// Note: that deriving clauses put constraints on the type parameters. i.e.
///
/// ```
/// #[derive(Clone)]
/// struct Foo<T: GolemTypes> { owner: T::PluginOwner }
/// ```
///
/// becomes
/// ```
/// struct Foo<T: GolemTypes> { owner: T::PluginOwner }
///
/// impl <T: GolemTypes + Clone> Clone for Foo<T> { ... }
/// ```
///
/// To make this work better for deriving use the following structure for structs:
/// ```
/// #[derive(Clone)]
/// struct FooPoly<PluginOwner> { owner: PluginOwner }
/// type Foo<T: GolemTypes> = FooPoly<T::PluginOwner>
/// ```
pub trait GolemTypes: 'static {
    // TODO:
    // Optimally we would like to have a constraint on the associated type here:
    //
    // `type ComponentOwner: ComponentOwner<PluginOwner = Self::PluginOwner>;`
    //
    // This does currently now work nicely for two reasons:
    // * PluginOwner / PluginScope bring a lot of baggage. Especially the AuthCtx can make it difficult to move implementations to a central location
    // * cloud-worker-executor currently mixes Oss and Cloud types here. Optimally it would fully use cloud types.
    //
    // Once these two issues are addressed, introduce the contraint here.
    type ComponentOwner: ComponentOwner;

    type PluginOwner: PluginOwner;
    type PluginScope: PluginScope;
}

pub struct DefaultGolemTypes;

impl GolemTypes for DefaultGolemTypes {
    type ComponentOwner = DefaultComponentOwner;

    type PluginOwner = DefaultPluginOwner;
    type PluginScope = DefaultPluginScope;
}
