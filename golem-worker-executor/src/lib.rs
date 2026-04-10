// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

pub mod bootstrap;
pub mod config;
pub mod durable_host;
pub mod grpc;
pub mod metrics;
pub mod model;
pub mod preview2;
pub mod services;
pub mod storage;
pub mod wasi_host;
pub mod worker;
pub mod workerctx;

#[cfg(test)]
test_r::enable!();

use self::durable_host::{DurableWorkerCtx, DurableWorkerCtxView};
use self::services::agent_webhooks::AgentWebhooksService;
use self::services::environment_state::EnvironmentStateService;
use self::services::golem_config::EnvironmentStateServiceConfig;
use self::services::promise::LazyPromiseService;
use self::services::rdbms::RdbmsService;
use self::services::resource_limits::ResourceLimits;
use self::services::rpc::{DirectWorkerInvocationRpc, RemoteInvocationRpc};
use self::services::worker_fork::DefaultWorkerFork;
use self::wasi_host::create_linker;
use crate::grpc::WorkerExecutorImpl;
use crate::services::active_workers::ActiveWorkers;
use crate::services::agent_types::AgentTypesService;
use crate::services::blob_store::{BlobStoreService, DefaultBlobStoreService};
use crate::services::component::ComponentService;
use crate::services::events::Events;
use crate::services::golem_config::{
    EngineConfig, GolemConfig, HttpClientConfig, IndexedStorageConfig, KeyValueStorageConfig,
    KeyValueStorageInnerConfig,
};
use crate::services::key_value::{DefaultKeyValueService, KeyValueService};
use crate::services::oplog::plugin::{
    ForwardingOplogService, OplogProcessorPlugin, PerExecutorOplogProcessorPlugin,
};
use crate::services::oplog::{
    BlobOplogArchiveService, CompressedOplogArchiveService, MultiLayerOplogService,
    OplogArchiveService, OplogService, PrimaryOplogService,
};
use crate::services::promise::{DefaultPromiseService, DefaultPromiseWorkerAccess, PromiseService};
use crate::services::quota::QuotaService;
use crate::services::registry_event_subscriber::WorkerExecutorRegistryInvalidationHandler;
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
use crate::services::{
    All, HasActiveWorkers, HasAgentTypesService, HasComponentService, HasConfig,
    HasEnvironmentStateService, HasOplogService, HasWorkerActivator, HasWorkerService, rdbms,
};
use crate::storage::indexed::IndexedStorage;
use crate::storage::indexed::multi_sqlite::MultiSqliteIndexedStorage;
use crate::storage::indexed::postgres::PostgresIndexedStorage;
use crate::storage::indexed::redis::RedisIndexedStorage;
use crate::storage::indexed::sqlite::SqliteIndexedStorage;
use crate::storage::keyvalue::KeyValueStorage;
use crate::storage::keyvalue::memory::InMemoryKeyValueStorage;
use crate::storage::keyvalue::multi_sqlite::MultiSqliteKeyValueStorage;
use crate::storage::keyvalue::namespace_routed::NamespaceRoutedKeyValueStorage;
use crate::storage::keyvalue::postgres::PostgresKeyValueStorage;
use crate::storage::keyvalue::redis::RedisKeyValueStorage;
use crate::workerctx::WorkerCtx;
use anyhow::anyhow;
use async_trait::async_trait;
use futures::TryFutureExt;
use golem_api_grpc::proto;
use golem_api_grpc::proto::golem::workerexecutor::v1::worker_executor_server::WorkerExecutorServer;
use golem_common::redis::RedisPool;
use golem_service_base::clients::registry::{GrpcRegistryService, RegistryService};
use golem_service_base::config::BlobStorageConfig;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::grpc::server::GrpcServerTlsConfig;
use golem_service_base::service::initial_component_files::InitialComponentFilesService;
use golem_service_base::storage::blob::BlobStorage;
use golem_service_base::storage::blob::s3::S3BlobStorage;
use golem_service_base::storage::blob::sqlite::SqliteBlobStorage;
use humansize::{BINARY, ISizeFormatter};
use log::debug;
use nonempty_collections::NEVec;
use prometheus::Registry;
use services::file_loader::FileLoader;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use storage::keyvalue::sqlite::SqliteKeyValueStorage;
use tokio::net::TcpListener;
use tokio::runtime::Handle;
use tokio::task::JoinSet;
use tokio_stream::wrappers::TcpListenerStream;
use tokio_util::sync::CancellationToken;
use tonic::codec::CompressionEncoding;
use tonic::transport::Server;
use tonic_tracing_opentelemetry::middleware;
use tonic_tracing_opentelemetry::middleware::filters;
use tracing::{Instrument, info};
use wasmtime::component::{HasSelf, Linker};
use wasmtime::{Config, Engine, WasmBacktraceDetails};

pub struct RunDetails {
    pub http_port: u16,
    pub grpc_port: u16,
    pub epoch_thread: std::sync::Mutex<Option<std::thread::JoinHandle<()>>>,
    pub epoch_stop: Arc<AtomicBool>,
    /// Graph-wide shutdown signal. Cancelled in `Drop` before stopping the
    /// epoch thread so that all service background tasks exit promptly.
    pub shutdown: services::shutdown::Shutdown,
    /// Weak reference to a sentinel inside `All`. When `All` is properly
    /// deallocated, `upgrade()` returns `None`. Used by tests to detect leaks.
    pub leak_detector: std::sync::Weak<()>,
}

impl Drop for RunDetails {
    fn drop(&mut self) {
        self.shutdown.cancel();
        self.epoch_stop.store(true, Ordering::Release);
        if let Some(handle) = self.epoch_thread.lock().unwrap().take() {
            let _ = handle.join();
        }
    }
}

/// The Bootstrap trait should be implemented by all Worker Executors to customize the initialization
/// of its services.
/// With a valid `Bootstrap` implementation, the service can be started with the `run` method.
#[async_trait]
#[allow(clippy::too_many_arguments)]
pub trait Bootstrap<Ctx: WorkerCtx> {
    fn create_shard_manager_service(
        &self,
        shard_manager_client: Arc<dyn golem_service_base::clients::shard_manager::ShardManager>,
    ) -> Arc<dyn ShardManagerService> {
        Arc::new(crate::services::shard_manager::GrpcShardManagerService::new(shard_manager_client))
    }

    fn create_quota_service(
        &self,
        shard_manager_client: Arc<dyn golem_service_base::clients::shard_manager::ShardManager>,
        golem_config: &GolemConfig,
        shutdown_token: tokio_util::sync::CancellationToken,
    ) -> Arc<dyn QuotaService> {
        crate::services::quota::GrpcQuotaService::new(
            shard_manager_client,
            golem_config.grpc.port,
            shutdown_token,
            std::time::Duration::from_secs(10),
            std::time::Duration::from_secs(60),
        )
    }

    fn create_environment_state_service(
        &self,
        config: &EnvironmentStateServiceConfig,
        registry_service: Arc<dyn RegistryService>,
    ) -> Arc<dyn EnvironmentStateService> {
        Arc::new(
            crate::services::environment_state::GrpcEnvironmentStateService::new(
                registry_service,
                config.cache_capacity,
                config.cache_ttl,
                config.cache_eviction_interval,
            ),
        )
    }

    fn create_component_service(
        &self,
        golem_config: &GolemConfig,
        registry_service: Arc<dyn RegistryService>,
        blob_storage: Arc<dyn BlobStorage>,
    ) -> Arc<dyn ComponentService> {
        crate::services::component::configured(
            &golem_config.component_cache,
            &golem_config.compiled_component_service,
            registry_service.clone(),
            blob_storage,
        )
    }

    fn create_resource_limits(
        &self,
        golem_config: &GolemConfig,
        registry_service: Arc<dyn RegistryService>,
        shutdown_token: CancellationToken,
    ) -> Arc<dyn ResourceLimits> {
        crate::services::resource_limits::configured(
            &golem_config.resource_limits,
            registry_service.clone(),
            shutdown_token.clone(),
        )
    }

    fn create_worker_proxy(&self, golem_config: &GolemConfig) -> Arc<dyn WorkerProxy> {
        Arc::new(RemoteWorkerProxy::new(&golem_config.public_worker_api))
    }

    fn create_key_value_service(
        &self,
        key_value_storage: &Arc<dyn KeyValueStorage + Send + Sync>,
    ) -> Arc<dyn KeyValueService> {
        Arc::new(DefaultKeyValueService::new(key_value_storage.clone()))
    }

    fn create_blob_store_service(
        &self,
        blob_storage: &Arc<dyn BlobStorage>,
    ) -> Arc<dyn BlobStoreService> {
        Arc::new(DefaultBlobStoreService::new(blob_storage.clone()))
    }

    fn create_additional_deps(&self, registry_service: Arc<dyn RegistryService>) -> Ctx::ExtraDeps;

    fn create_rdbms_service(
        &self,
        golem_config: &GolemConfig,
        _additional_deps: &Ctx::ExtraDeps,
    ) -> Arc<dyn RdbmsService> {
        Arc::new(rdbms::RdbmsServiceDefault::new(golem_config.rdbms))
    }

    fn wrap_rpc(
        &self,
        rpc: Arc<dyn crate::services::rpc::Rpc>,
    ) -> Arc<dyn crate::services::rpc::Rpc> {
        rpc
    }

    async fn create_services(
        &self,
        active_workers: Arc<ActiveWorkers<Ctx>>,
        engine: Arc<Engine>,
        linker: Arc<Linker<Ctx>>,
        runtime: Handle,
        component_service: Arc<dyn ComponentService>,
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
        worker_activator: Arc<dyn WorkerActivator<Ctx>>,
        oplog_service: Arc<dyn OplogService>,
        scheduler_service: Arc<dyn SchedulerService>,
        worker_proxy: Arc<dyn WorkerProxy>,
        events: Arc<Events>,
        file_loader: Arc<FileLoader>,
        oplog_processor_plugin: Arc<dyn OplogProcessorPlugin>,
        agent_types_service: Arc<dyn AgentTypesService>,
        environment_state_service: Arc<dyn EnvironmentStateService>,
        agent_webhooks_service: Arc<AgentWebhooksService>,
        resource_limits: Arc<dyn ResourceLimits>,
        quota_service: Arc<dyn QuotaService>,
        additional_deps: Ctx::ExtraDeps,
        shutdown_token: tokio_util::sync::CancellationToken,
        http_connection_pool: Option<wasmtime_wasi_http::HttpConnectionPool>,
        websocket_connection_pool: crate::durable_host::websocket::WebSocketConnectionPool,
        leak_sentinel: Arc<()>,
    ) -> anyhow::Result<All<Ctx>> {
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
            quota_service.clone(),
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
            oplog_processor_plugin.clone(),
            resource_limits.clone(),
            environment_state_service.clone(),
            agent_types_service.clone(),
            agent_webhooks_service.clone(),
            shutdown_token.clone(),
            http_connection_pool.clone(),
            websocket_connection_pool.clone(),
            additional_deps.clone(),
            leak_sentinel.clone(),
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
            quota_service.clone(),
            key_value_service.clone(),
            blob_store_service.clone(),
            rdbms_service.clone(),
            oplog_service.clone(),
            scheduler_service.clone(),
            worker_activator.clone(),
            events.clone(),
            file_loader.clone(),
            oplog_processor_plugin.clone(),
            resource_limits.clone(),
            shutdown_token.clone(),
            environment_state_service.clone(),
            agent_types_service.clone(),
            agent_webhooks_service.clone(),
            http_connection_pool.clone(),
            websocket_connection_pool.clone(),
            additional_deps.clone(),
            leak_sentinel.clone(),
        ));
        let rpc = self.wrap_rpc(rpc);

        Ok(All::new(
            active_workers,
            agent_types_service,
            agent_webhooks_service,
            engine,
            linker,
            runtime.clone(),
            component_service,
            shard_manager_service,
            quota_service,
            worker_fork,
            worker_service,
            worker_enumeration_service,
            running_worker_enumeration_service,
            promise_service.clone(),
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
            oplog_processor_plugin.clone(),
            resource_limits,
            shutdown_token,
            http_connection_pool,
            websocket_connection_pool.clone(),
            environment_state_service.clone(),
            additional_deps,
            leak_sentinel,
        ))
    }

    /// Can be overridden to customize the wasmtime configuration
    fn create_wasmtime_config(&self, engine_config: &EngineConfig) -> Config {
        let mut config = Config::default();

        config.wasm_multi_value(true);
        config.wasm_component_model(true);
        config.epoch_interruption(true);
        config.consume_fuel(true);
        config.wasm_backtrace_details(WasmBacktraceDetails::Enable);

        if engine_config.enable_fs_cache {
            config.cache(Some(
                wasmtime::Cache::new(wasmtime::CacheConfig::new())
                    .expect("Failed to initialize cache"),
            ));
        }

        config
    }

    /// This method is responsible for linking all the host function implementations the worker
    /// executor supports.
    fn create_wasmtime_linker(&self, engine: &Engine) -> anyhow::Result<Linker<Ctx>> {
        let mut linker = create_linker(engine, DurableWorkerCtxView::durable_ctx_mut)?;
        crate::preview2::golem_api_1_x::host::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
            &mut linker,
            DurableWorkerCtxView::durable_ctx_mut,
        )?;
        crate::preview2::golem_api_1_x::retry::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
            &mut linker,
            DurableWorkerCtxView::durable_ctx_mut,
        )?;
        crate::preview2::golem_api_1_x::oplog::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
            &mut linker,
            DurableWorkerCtxView::durable_ctx_mut,
        )?;
        crate::preview2::golem_api_1_x::context::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
            &mut linker,
            DurableWorkerCtxView::durable_ctx_mut,
        )?;
        crate::preview2::golem_durability::durability::add_to_linker::<
            _,
            HasSelf<DurableWorkerCtx<Ctx>>,
        >(&mut linker, DurableWorkerCtxView::durable_ctx_mut)?;
        crate::preview2::golem::agent::host::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
            &mut linker,
            DurableWorkerCtxView::durable_ctx_mut,
        )?;
        golem_wasm::golem_core_1_5_x::types::add_to_linker::<_, HasSelf<DurableWorkerCtx<Ctx>>>(
            &mut linker,
            DurableWorkerCtxView::durable_ctx_mut,
        )?;
        Ok(linker)
    }
}

pub async fn create_worker_executor_impl<
    Ctx: WorkerCtx,
    BootstrapImpl: Bootstrap<Ctx> + ?Sized + Send + Sync,
>(
    golem_config: GolemConfig,
    bootstrap: &BootstrapImpl,
    runtime: Handle,
    lazy_worker_activator: &Arc<LazyWorkerActivator<Ctx>>,
    shutdown_token: tokio_util::sync::CancellationToken,
) -> Result<
    (
        All<Ctx>,
        std::thread::JoinHandle<()>,
        Arc<AtomicBool>,
        Arc<dyn RegistryService>,
    ),
    anyhow::Error,
> {
    let (redis, sqlite, key_value_storage): (
        Option<RedisPool>,
        Option<SqlitePool>,
        Arc<dyn KeyValueStorage + Send + Sync>,
    ) = match &golem_config.key_value_storage {
        KeyValueStorageConfig::Redis(redis) => {
            let pool = RedisPool::configured(redis)
                .await
                .map_err(|err| anyhow!(err))?;
            let key_value_storage: Arc<dyn KeyValueStorage + Send + Sync> =
                Arc::new(RedisKeyValueStorage::new(pool.clone()));
            (Some(pool), None, key_value_storage)
        }
        KeyValueStorageConfig::Postgres(postgres) => {
            let key_value_storage: Arc<dyn KeyValueStorage + Send + Sync> = Arc::new(
                PostgresKeyValueStorage::configured(postgres)
                    .await
                    .map_err(|err| anyhow!(err))?,
            );
            (None, None, key_value_storage)
        }
        KeyValueStorageConfig::NamespaceRouted(namespace_routed) => {
            let (cache_redis, cache_sqlite, cache_storage) =
                build_inner_key_value_storage(&namespace_routed.cache).await?;
            let (persistent_redis, persistent_sqlite, persistent_storage) =
                build_inner_key_value_storage(&namespace_routed.persistent).await?;

            let key_value_storage: Arc<dyn KeyValueStorage + Send + Sync> = Arc::new(
                NamespaceRoutedKeyValueStorage::new(cache_storage, persistent_storage),
            );

            (
                cache_redis.or(persistent_redis),
                cache_sqlite.or(persistent_sqlite),
                key_value_storage,
            )
        }
        KeyValueStorageConfig::InMemory(_) => {
            (None, None, Arc::new(InMemoryKeyValueStorage::new()))
        }
        KeyValueStorageConfig::Sqlite(sqlite) => {
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
        KeyValueStorageConfig::MultiSqlite(multi_sqlite) => {
            let key_value_storage: Arc<dyn KeyValueStorage + Send + Sync> =
                Arc::new(MultiSqliteKeyValueStorage::new(
                    &multi_sqlite.root_dir,
                    multi_sqlite.max_connections,
                    multi_sqlite.foreign_keys,
                ));
            (None, None, key_value_storage)
        }
    };

    let indexed_storage: Arc<dyn IndexedStorage + Send + Sync> = match &golem_config.indexed_storage
    {
        IndexedStorageConfig::KVStoreRedis(_) => {
            let redis = redis
                .expect("Redis must be configured as key-value storage when using KVStoreRedis");
            Arc::new(RedisIndexedStorage::new(redis.clone()))
        }
        IndexedStorageConfig::Redis(redis) => {
            let pool = RedisPool::configured(redis).await?;
            Arc::new(RedisIndexedStorage::new(pool.clone()))
        }
        IndexedStorageConfig::Postgres(postgres) => Arc::new(
            PostgresIndexedStorage::configured(postgres)
                .await
                .map_err(|err| anyhow!(err))?,
        ),
        IndexedStorageConfig::KVStoreSqlite(_) => {
            let sqlite = sqlite
                .clone()
                .expect("Sqlite must be configured as key-value storage when using KVStoreSqlite");
            Arc::new(
                SqliteIndexedStorage::new(sqlite.clone())
                    .await
                    .map_err(|err| anyhow!(err))?,
            )
        }
        IndexedStorageConfig::KVStoreMultiSqlite(_) => match &golem_config.key_value_storage {
            KeyValueStorageConfig::MultiSqlite(multi_sqlite) => {
                Arc::new(MultiSqliteIndexedStorage::new(
                    &multi_sqlite.root_dir,
                    multi_sqlite.max_connections,
                    multi_sqlite.foreign_keys,
                ))
            }
            _ => panic!(
                "Invalid configuration: multi-sqlite must be used as key-value storage when using KVStoreMultiSqlite"
            ),
        },
        IndexedStorageConfig::Sqlite(sqlite) => {
            let pool = SqlitePool::configured(sqlite)
                .await
                .map_err(|err| anyhow!(err))?;
            Arc::new(
                SqliteIndexedStorage::new(pool.clone())
                    .await
                    .map_err(|err| anyhow!(err))?,
            )
        }
        IndexedStorageConfig::MultiSqlite(multi_sqlite) => {
            Arc::new(MultiSqliteIndexedStorage::new(
                &multi_sqlite.root_dir,
                multi_sqlite.max_connections,
                multi_sqlite.foreign_keys,
            ))
        }
        IndexedStorageConfig::InMemory(_) => {
            Arc::new(storage::indexed::memory::InMemoryIndexedStorage::new())
        }
    };

    let blob_storage: Arc<dyn BlobStorage> = match &golem_config.blob_storage {
        BlobStorageConfig::S3(config) => Arc::new(S3BlobStorage::new(config.clone()).await),
        BlobStorageConfig::LocalFileSystem(config) => Arc::new(
            golem_service_base::storage::blob::fs::FileSystemBlobStorage::new(&config.root)
                .await
                .map_err(|err| anyhow!(err))?,
        ),
        BlobStorageConfig::KVStoreSqlite(_) => {
            let sqlite = sqlite
                .expect("Sqlite must be configured as key-value storage when using KVStoreSqlite");
            Arc::new(
                SqliteBlobStorage::new(sqlite.clone())
                    .await
                    .map_err(|err| anyhow!(err))?,
            )
        }
        BlobStorageConfig::Sqlite(sqlite) => {
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
            Arc::new(golem_service_base::storage::blob::memory::InMemoryBlobStorage::new())
        }
    };

    let initial_files_service = Arc::new(InitialComponentFilesService::new(blob_storage.clone()));

    let registry_service = Arc::new(GrpcRegistryService::new(&golem_config.registry_service));

    let component_service = bootstrap.create_component_service(
        &golem_config,
        registry_service.clone(),
        blob_storage.clone(),
    );

    let environment_state_service = bootstrap.create_environment_state_service(
        &golem_config.environment_state_service,
        registry_service.clone(),
    );

    let agent_webhooks_service = Arc::new(AgentWebhooksService::new(
        environment_state_service.clone(),
        golem_config
            .agent_webhooks_service
            .use_https_for_webhook_url,
        golem_config.agent_webhooks_service.hmac_key.0.clone(),
    ));

    let agent_type_service = services::agent_types::configured(
        &golem_config.agent_types_service,
        component_service.clone(),
        registry_service.clone(),
    );

    let http_connection_pool = match &golem_config.http_client {
        HttpClientConfig::Enabled(config) => Some(wasmtime_wasi_http::HttpConnectionPool::new(
            wasmtime_wasi_http::HttpConnectionPoolConfig {
                max_idle_per_host: config.max_idle_per_host,
                idle_timeout: config.idle_timeout,
                connect_timeout: config.connect_timeout,
                max_connections_per_host: config.max_connections_per_host,
                max_total_connections: config.max_total_connections,
                max_host_entries: config.max_host_entries,
            },
        )),
        HttpClientConfig::Disabled(_) => None,
    };
    let websocket_connection_pool = crate::durable_host::websocket::WebSocketConnectionPool::new(
        golem_config.max_websocket_connections,
    );
    let golem_config = Arc::new(golem_config);

    let shard_service = Arc::new(ShardServiceDefault::new());

    let mut oplog_archives: Vec<Arc<dyn OplogArchiveService>> = Vec::new();
    for idx in 1..golem_config.oplog.indexed_storage_layers {
        let svc: Arc<dyn OplogArchiveService> = Arc::new(CompressedOplogArchiveService::new(
            indexed_storage.clone(),
            idx,
        ));
        oplog_archives.push(svc);
    }
    for idx in 0..golem_config.oplog.blob_storage_layers {
        let svc: Arc<dyn OplogArchiveService> =
            Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), idx));
        oplog_archives.push(svc);
    }
    let oplog_archives = NEVec::try_from_vec(oplog_archives);

    let base_oplog_service: Arc<dyn OplogService> = match oplog_archives {
        None => Arc::new(
            PrimaryOplogService::new(
                indexed_storage.clone(),
                blob_storage.clone(),
                golem_config.oplog.max_operations_before_commit,
                golem_config.oplog.max_operations_before_commit_ephemeral,
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
                    golem_config.oplog.max_operations_before_commit_ephemeral,
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

    let active_workers = Arc::new(ActiveWorkers::<Ctx>::new(
        &golem_config.memory,
        &golem_config.filesystem_storage,
    ));

    let file_loader = Arc::new(FileLoader::new(
        initial_files_service.clone(),
        Some(active_workers.filesystem_storage_semaphore()),
    )?);

    let running_worker_enumeration_service = Arc::new(RunningWorkerEnumerationServiceDefault::new(
        active_workers.clone(),
    ));

    let shard_manager_client: Arc<dyn golem_service_base::clients::shard_manager::ShardManager> =
        Arc::new(
            golem_service_base::clients::shard_manager::GrpcShardManager::new(
                &golem_config.shard_manager,
            ),
        );

    let shard_manager_service =
        bootstrap.create_shard_manager_service(shard_manager_client.clone());

    let quota_service =
        bootstrap.create_quota_service(shard_manager_client, &golem_config, shutdown_token.clone());

    let config = bootstrap.create_wasmtime_config(&golem_config.engine);
    let engine = Arc::new(Engine::new(&config)?);
    let linker = bootstrap.create_wasmtime_linker(&engine)?;

    let engine_ref: Arc<Engine> = engine.clone();

    let epoch_interval = golem_config.limits.epoch_interval;
    let epoch_stop = Arc::new(AtomicBool::new(false));
    let epoch_stop_clone = epoch_stop.clone();
    let epoch_thread = std::thread::spawn(move || {
        while !epoch_stop_clone.load(Ordering::Acquire) {
            std::thread::sleep(epoch_interval);
            engine_ref.increment_epoch();
        }
    });

    let linker = Arc::new(linker);

    let key_value_service = bootstrap.create_key_value_service(&key_value_storage);

    let blob_store_service = bootstrap.create_blob_store_service(&blob_storage);

    let worker_proxy = bootstrap.create_worker_proxy(&golem_config);

    let events = Arc::new(Events::new(
        golem_config.limits.invocation_result_broadcast_capacity,
    ));

    let oplog_processor_plugin = Arc::new(PerExecutorOplogProcessorPlugin::new(
        component_service.clone(),
        shard_service.clone(),
        lazy_worker_activator.clone(),
        worker_proxy.clone(),
    ));

    let oplog_service: Arc<dyn OplogService> = Arc::new(ForwardingOplogService::new(
        base_oplog_service,
        oplog_processor_plugin.clone(),
        component_service.clone(),
        golem_config.oplog.plugin_max_commit_count,
        golem_config.oplog.plugin_max_elapsed_time,
    ));

    let worker_service = Arc::new(DefaultWorkerService::new(
        key_value_storage.clone(),
        shard_service.clone(),
        oplog_service.clone(),
        component_service.clone(),
        golem_config.clone(),
    ));
    let worker_enumeration_service = Arc::new(DefaultWorkerEnumerationService::new(
        worker_service.clone(),
        oplog_service.clone(),
        golem_config.clone(),
    ));

    let promise_service = Arc::new(LazyPromiseService::new());

    let scheduler_service = SchedulerServiceDefault::new(
        key_value_storage.clone(),
        shard_service.clone(),
        promise_service.clone(),
        Arc::new(lazy_worker_activator.clone() as Arc<dyn WorkerActivator<Ctx>>),
        oplog_service.clone(),
        worker_service.clone(),
        golem_config.scheduler.refresh_interval,
        shutdown_token.clone(),
    );

    let resource_limits = bootstrap.create_resource_limits(
        &golem_config,
        registry_service.clone(),
        shutdown_token.clone(),
    );

    let additional_deps = bootstrap.create_additional_deps(registry_service.clone());

    let rdbms_service = bootstrap.create_rdbms_service(&golem_config, &additional_deps);

    let leak_sentinel = Arc::new(());

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
            promise_service.clone(),
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
            oplog_processor_plugin,
            agent_type_service,
            environment_state_service,
            agent_webhooks_service,
            resource_limits,
            quota_service,
            additional_deps,
            shutdown_token,
            http_connection_pool,
            websocket_connection_pool,
            leak_sentinel,
        )
        .await?;

    let promise_worker_access = Arc::new(DefaultPromiseWorkerAccess::new(
        all.component_service(),
        all.worker_service(),
        all.active_workers(),
        all.oplog_service(),
        all.config(),
        all.worker_activator(),
    ));

    promise_service
        .set_implementation(DefaultPromiseService::new(
            key_value_storage.clone(),
            promise_worker_access,
        ))
        .await;

    Ok((all, epoch_thread, epoch_stop, registry_service))
}

/// Runs the worker executor
pub async fn bootstrap_and_run_worker_executor<
    Ctx: WorkerCtx,
    BootstrapImpl: Bootstrap<Ctx> + ?Sized + Send + Sync,
>(
    bootstrap: &BootstrapImpl,
    golem_config: GolemConfig,
    prometheus_registry: Registry,
    runtime: Handle,
    join_set: &mut JoinSet<Result<(), anyhow::Error>>,
    start_registry_invalidation_handler: bool,
) -> anyhow::Result<RunDetails> {
    debug!("Initializing worker executor");

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
    let shutdown = services::shutdown::Shutdown::new();

    let (worker_executor_impl, epoch_thread, epoch_stop, registry_service) =
        create_worker_executor_impl::<Ctx, BootstrapImpl>(
            golem_config.clone(),
            bootstrap,
            runtime.clone(),
            &lazy_worker_activator,
            shutdown.token(),
        )
        .await?;

    if start_registry_invalidation_handler {
        let registry_service = registry_service.clone();
        let environment_state_service = worker_executor_impl.environment_state_service();
        let agent_types_service = worker_executor_impl.agent_types();
        let shutdown_token = shutdown.token();
        join_set.spawn(async move {
            WorkerExecutorRegistryInvalidationHandler::run(
                registry_service,
                environment_state_service,
                agent_types_service,
                shutdown_token,
            )
            .await;
            Ok(())
        });
    };

    let leak_detector = worker_executor_impl.leak_detector();

    let grpc_port = run_grpc_server(worker_executor_impl, lazy_worker_activator, join_set).await?;

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
        epoch_stop,
        shutdown,
        leak_detector,
    })
}

pub async fn run_grpc_server<Ctx: WorkerCtx>(
    service_dependencies: All<Ctx>,
    lazy_worker_activator: Arc<LazyWorkerActivator<Ctx>>,
    join_set: &mut JoinSet<Result<(), anyhow::Error>>,
) -> anyhow::Result<u16> {
    let golem_config = service_dependencies.config();
    let (health_reporter, health_service) = tonic_health::server::health_reporter();
    health_reporter
        .set_serving::<WorkerExecutorServer<WorkerExecutorImpl<Ctx, All<Ctx>>>>()
        .await;

    let reflection_service = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(proto::FILE_DESCRIPTOR_SET)
        .build_v1()?;

    let addr = SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), golem_config.grpc.port);
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

    join_set.spawn({
        let mut server = Server::builder();

        if let GrpcServerTlsConfig::Enabled(tls) = &golem_config.grpc.tls {
            server = server.tls_config(tls.to_tonic())?;
        };

        server
            .layer(middleware::server::OtelGrpcLayer::default().filter(filters::reject_healthcheck))
            .max_concurrent_streams(Some(golem_config.limits.max_concurrent_streams))
            .add_service(reflection_service)
            .add_service(service)
            .add_service(health_service)
            .serve_with_incoming(TcpListenerStream::new(listener))
            .map_err(anyhow::Error::from)
            .in_current_span()
    });

    info!("Started worker service on ports: grpc: {grpc_port}");

    Ok(grpc_port)
}

async fn build_inner_key_value_storage(
    config: &KeyValueStorageInnerConfig,
) -> Result<
    (
        Option<RedisPool>,
        Option<SqlitePool>,
        Arc<dyn KeyValueStorage + Send + Sync>,
    ),
    anyhow::Error,
> {
    match config {
        KeyValueStorageInnerConfig::Redis(redis) => {
            let pool = RedisPool::configured(redis)
                .await
                .map_err(|err| anyhow!(err))?;
            let key_value_storage: Arc<dyn KeyValueStorage + Send + Sync> =
                Arc::new(RedisKeyValueStorage::new(pool.clone()));
            Ok((Some(pool), None, key_value_storage))
        }
        KeyValueStorageInnerConfig::Postgres(postgres) => {
            let key_value_storage: Arc<dyn KeyValueStorage + Send + Sync> = Arc::new(
                PostgresKeyValueStorage::configured(postgres)
                    .await
                    .map_err(|err| anyhow!(err))?,
            );
            Ok((None, None, key_value_storage))
        }
        KeyValueStorageInnerConfig::InMemory(_) => {
            let key_value_storage: Arc<dyn KeyValueStorage + Send + Sync> =
                Arc::new(InMemoryKeyValueStorage::new());
            Ok((None, None, key_value_storage))
        }
        KeyValueStorageInnerConfig::Sqlite(sqlite) => {
            let pool = SqlitePool::configured(sqlite)
                .await
                .map_err(|err| anyhow!(err))?;
            let key_value_storage: Arc<dyn KeyValueStorage + Send + Sync> = Arc::new(
                SqliteKeyValueStorage::new(pool.clone())
                    .await
                    .map_err(|err| anyhow!(err))?,
            );
            Ok((None, Some(pool), key_value_storage))
        }
        KeyValueStorageInnerConfig::MultiSqlite(multi_sqlite) => {
            let key_value_storage: Arc<dyn KeyValueStorage + Send + Sync> =
                Arc::new(MultiSqliteKeyValueStorage::new(
                    &multi_sqlite.root_dir,
                    multi_sqlite.max_connections,
                    multi_sqlite.foreign_keys,
                ));
            Ok((None, None, key_value_storage))
        }
    }
}
