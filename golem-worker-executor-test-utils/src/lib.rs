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

pub mod agent_deployments_service;
pub mod component_service;
pub mod component_writer;
pub mod dsl_impl;

use self::agent_deployments_service::{
    ConfiguredRetryPoliciesEnvironmentStateService, DisabledEnvironmentStateService,
};
use self::component_writer::FileSystemComponentWriter;
use crate::component_service::ComponentServiceLocalFileSystem;
use anyhow::{Error, anyhow};
use async_trait::async_trait;
use golem_api_grpc::proto::golem::workerexecutor::v1::worker_executor_client::WorkerExecutorClient;
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    GetRunningWorkersMetadataRequest, get_running_workers_metadata_response,
};
use golem_common::base_model::environment_plugin_grant::EnvironmentPluginGrantId;
use golem_common::config::{DbSqliteConfig, RedisConfig};
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::agent::{AgentMode, ParsedAgentId};
use golem_common::model::application::ApplicationId;
use golem_common::model::auth::{AccountRole, TokenSecret};
use golem_common::model::component::ComponentRevision;
use golem_common::model::component::{CanonicalFilePath, ComponentId};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::{
    AttributeValue, InvocationContextSpan, InvocationContextStack, SpanId,
};
use golem_common::model::oplog::{
    AgentError, OplogEntry, PayloadId, PersistenceLevel, RawOplogPayload,
    TimestampedUpdateDescription, types::ObjectMetadata,
};
use golem_common::model::plan::PlanId;
use golem_common::model::retry_policy::NamedRetryPolicy;
use golem_common::model::worker::{AgentConfigEntryDto, AgentMetadataDto};
use golem_common::model::{
    AgentFilter, AgentId, AgentInvocation, AgentInvocationOutput, AgentStatusRecord,
    IdempotencyKey, OplogIndex, OwnedAgentId, RdbmsPoolKey, RetryConfig, TransactionId,
};
use golem_common::resource_runtime::Uri;
use golem_common::resource_runtime::{ResourceStore, ResourceTypeId};
use golem_common::schema::SchemaValue;
use golem_service_base::clients::registry::RegistryService;
use golem_service_base::config::{BlobStorageConfig, LocalFileSystemBlobStorageConfig};
use golem_service_base::error::worker_executor::{InterruptKind, WorkerExecutorError};
use golem_service_base::grpc::server::GrpcServerTlsConfig;
use golem_service_base::model::GetFileSystemNodeResult;
use golem_service_base::model::auth::{AuthCtx, UserAuthCtx};
use golem_service_base::model::component::Component;
use golem_service_base::service::compiled_component::{
    CompiledComponentServiceConfig, CompiledComponentServiceEnabledConfig,
    DefaultCompiledComponentService,
};
use golem_service_base::service::initial_agent_files::InitialAgentFilesService;
use golem_service_base::storage::blob::BlobStorage;
use golem_service_base::storage::blob::fs::FileSystemBlobStorage;
use golem_test_framework::components::redis::Redis;
use golem_test_framework::components::redis::spawned::SpawnedRedis;
use golem_test_framework::components::redis_monitor::RedisMonitor;
use golem_test_framework::components::redis_monitor::spawned::SpawnedRedisMonitor;
use golem_worker_executor::durable_host::{
    DurableWorkerCtx, DurableWorkerCtxView, PublicDurableWorkerState,
};
use golem_worker_executor::model::{
    AgentConfig, ExecutionStatus, LastError, ReadFileResult, TrapType,
};
use golem_worker_executor::preview2::golem::agent::host::{
    CancellationToken, FutureInvokeResult, HostFutureInvokeResult, HostWasmRpc, RpcError, WasmRpc,
};
use golem_worker_executor::preview2::{golem_api_1_x, golem_durability};
use golem_worker_executor::services::active_workers::ActiveWorkers;
use golem_worker_executor::services::active_workers::memory_probe::FixedProbe;
use golem_worker_executor::services::agent_types::AgentTypesService;
use golem_worker_executor::services::agent_webhooks::AgentWebhooksService;
use golem_worker_executor::services::blob_store::{
    BlobStoreError, BlobStoreService, DefaultBlobStoreService,
};
use golem_worker_executor::services::component::ComponentService;
use golem_worker_executor::services::direct_invocation_auth::{
    DirectInvocationAuthService, NoOpDirectInvocationAuthService,
};
use golem_worker_executor::services::environment_state::EnvironmentStateService;
use golem_worker_executor::services::file_loader::FileLoader;
use golem_worker_executor::services::golem_config::{
    AgentTypesServiceConfig, AgentTypesServiceLocalConfig, EngineConfig,
    EnvironmentStateServiceConfig, FilesystemStorageConfig, GolemConfig, GrpcApiConfig,
    HttpClientConfig, IndexedStorageConfig, IndexedStorageKVStoreRedisConfig,
    IndexedStorageKVStoreSqliteConfig, KeyValueStorageConfig, KeyValueStorageInnerConfig,
    KeyValueStorageNamespaceRoutedConfig, MemoryConfig, OplogConfig, ResourceLimitsConfig,
    ResourceLimitsDisabledConfig, SchedulerStorageConfig, SnapshotPolicy,
};
use golem_worker_executor::services::key_value::{DefaultKeyValueService, KeyValueService};
use golem_worker_executor::services::oplog::{CommitLevel, Oplog, OplogService};
use golem_worker_executor::services::promise::PromiseService;
use golem_worker_executor::services::quota::QuotaService;
use golem_worker_executor::services::rdbms::ignite::IgniteType;
use golem_worker_executor::services::rdbms::mysql::MysqlType;
use golem_worker_executor::services::rdbms::postgres::PostgresType;
use golem_worker_executor::services::rdbms::{
    DbResult, DbResultStream, DbTransaction, Rdbms, RdbmsService, RdbmsStatus,
    RdbmsTransactionStatus, RdbmsType,
};
use golem_worker_executor::services::resource_limits::{
    AtomicResourceEntry, ResourceLimits, ResourceLimitsDisabled,
};
use golem_worker_executor::services::rpc::{Rpc, RpcDemand, RpcError as ServiceRpcError};
use golem_worker_executor::services::scheduler::SchedulerService;
use golem_worker_executor::services::shard::ShardService;
use golem_worker_executor::services::worker::WorkerService;
use golem_worker_executor::services::worker_enumeration::WorkerEnumerationService;
use golem_worker_executor::services::worker_event::WorkerEventService;
use golem_worker_executor::services::worker_fork::WorkerForkService;
use golem_worker_executor::services::worker_proxy::WorkerProxy;
use golem_worker_executor::services::{HasAll, NoAdditionalDeps, rdbms};
use golem_worker_executor::storage::keyvalue::KeyValueStorage;
use golem_worker_executor::worker::{RetryDecision, Worker};
use golem_worker_executor::workerctx::{
    CallCountManagement, ExternalOperations, FileSystemReading, FuelManagement,
    InvocationContextManagement, InvocationHooks, InvocationManagement, LogEventEmitBehaviour,
    StatusManagement, UpdateManagement, WorkerCtx,
};
use golem_worker_executor::{Bootstrap, RunDetails, bootstrap_and_run_worker_executor};
use prometheus::Registry;
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt::{Debug, Formatter};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, RwLock, Weak};
use std::time::Duration;
use tempfile::TempDir;
use tokio::runtime::Handle;
use tokio::task::JoinSet;
use tonic::transport::Channel;
use tonic_tracing_opentelemetry::middleware::client::OtelGrpcService;
use tower::ServiceBuilder;
use tracing::{Level, debug, info};
use uuid::Uuid;
use wasmtime::component::{HasSelf, Instance, Linker, Resource, ResourceAny};
use wasmtime::{AsContextMut, Engine, ResourceLimiterAsync};
use wasmtime_wasi::WasiView;

#[cfg(test)]
test_r::enable!();

pub use golem_test_framework::dsl::PrecompiledComponent;

/// A handle to either an owned `TempDir` (parent process) or a borrowed
/// on-disk path (worker process).
///
/// Phase 3.4 makes [`WorkerExecutorTestDependencies`] a `Hosted` test-dep:
/// the parent owns the live `TempDir`s and ships only their paths over to
/// worker subprocesses through the `HostedDep` descriptor. Workers must
/// **not** delete the parent's directories on drop. This wrapper exposes
/// the same `path()` API in both cases while constraining `Drop` to the
/// owner.
pub enum TestTempDir {
    /// Parent-owned: dropping this also removes the underlying directory.
    Owned(TempDir),
    /// Worker-borrowed: dropping this does NOT remove the underlying
    /// directory. Lifetime is controlled by whichever process holds the
    /// matching `Owned` value.
    Borrowed(PathBuf),
}

impl TestTempDir {
    pub fn path(&self) -> &Path {
        match self {
            Self::Owned(td) => td.path(),
            Self::Borrowed(p) => p.as_path(),
        }
    }
}

impl Debug for TestTempDir {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "TestTempDir({:?})", self.path())
    }
}

/// Defines a `#[test_dep(scope = Shared)]` function that pre-warms the analysis cache for a
/// test component during test-r dependency initialization.
///
/// Usage: `test_component!(function_name, "tag_name", "wasm_file_name", "package:name");`
///
/// Each invocation must use a unique `tag_name` because test-r identifies deps
/// of the same type by their tag. The tag is also used in test function parameters
/// via `#[tagged_as("tag_name")] param: &PrecompiledComponent`.
#[macro_export]
macro_rules! test_component {
    ($fn_name:ident, $tag:expr, $wasm_name:expr, $package_name:expr) => {
        #[test_dep(scope = Shared, tagged_as = $tag)]
        pub async fn $fn_name(deps: &WorkerExecutorTestDependencies) -> PrecompiledComponent {
            tracing::info!(
                "Pre-compiling test component '{}' (package: '{}')",
                $wasm_name,
                $package_name
            );
            let wasm_path = deps
                .component_directory
                .join(format!("{}.wasm", $wasm_name));
            deps.component_writer
                .warm_cache(&wasm_path)
                .await
                .expect(concat!("Failed to warm cache for component ", $wasm_name));
            tracing::info!(
                "Pre-compiled test component '{}' (package: '{}') successfully",
                $wasm_name,
                $package_name
            );
            PrecompiledComponent::new($wasm_name, $package_name)
        }
    };
}

#[derive(Clone)]
pub struct WorkerExecutorTestDependencies {
    pub redis: Arc<dyn Redis>,
    pub redis_monitor: Arc<dyn RedisMonitor>,
    pub component_writer: Arc<FileSystemComponentWriter>,
    pub initial_agent_files_service: Arc<InitialAgentFilesService>,
    pub component_directory: PathBuf,
    pub component_temp_directory: Arc<TestTempDir>,
    pub component_service_directory: PathBuf,
    data_dir: Arc<TestTempDir>,
}

impl Debug for WorkerExecutorTestDependencies {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "WorkerExecutorTestDependencies")
    }
}

/// Wire format for the [`HostedDep`] descriptor of
/// [`WorkerExecutorTestDependencies`].
///
/// The parent serialises this once after constructing the live owner;
/// each spawned worker subprocess deserialises it and reconstructs an
/// equivalent struct whose handles attach to the parent's resources
/// (already-running Redis, on-disk caches, etc.) instead of spawning new
/// ones. See [`WorkerExecutorTestDependencies::from_descriptor`] for the
/// reconstruction logic.
#[derive(serde::Serialize, serde::Deserialize)]
struct WorkerExecutorTestDependenciesDescriptor {
    redis_host: String,
    redis_port: u16,
    redis_prefix: String,
    blob_storage_root: PathBuf,
    component_directory: PathBuf,
    component_service_directory: PathBuf,
    component_temp_directory: PathBuf,
    data_dir_path: PathBuf,
}

impl WorkerExecutorTestDependencies {
    pub fn blob_storage_root(&self) -> PathBuf {
        self.data_dir.path().join("blobs")
    }

    pub async fn new() -> Self {
        // The AWS SDK crates transitively pull in both aws-lc-rs and ring as rustls backends,
        // so rustls cannot pick one automatically. Install ring as the process default; ignore
        // the error if another test in the same process already installed a provider.
        let _ = rustls::crypto::ring::default_provider().install_default();

        let redis: Arc<dyn Redis> = Arc::new(SpawnedRedis::new(
            6379,
            "".to_string(),
            Level::INFO,
            Level::ERROR,
        ));
        let redis_monitor: Arc<dyn RedisMonitor> = Arc::new(SpawnedRedisMonitor::new(
            redis.clone(),
            Level::TRACE,
            Level::ERROR,
        ));

        let data_dir = TempDir::new().unwrap();
        let blob_storage_root = data_dir.path().join("blobs");
        let component_service_directory = data_dir.path().join("components");

        let blob_storage = Arc::new(
            FileSystemBlobStorage::new(&blob_storage_root)
                .await
                .unwrap(),
        );

        let initial_agent_files_service =
            Arc::new(InitialAgentFilesService::new(blob_storage.clone()));

        let component_directory = Path::new("../test-components").to_path_buf();

        let component_writer: Arc<FileSystemComponentWriter> =
            Arc::new(FileSystemComponentWriter::new(&component_service_directory).await);

        // `FileSystemComponentWriter::new` `remove_dir_all`s the root and
        // then only re-creates subdirectories lazily on the first component
        // write. The `HostedDep::descriptor` impl below eagerly
        // canonicalises `component_service_directory`, so we materialise
        // the empty root here to keep `descriptor()` valid even before any
        // test has written a component.
        tokio::fs::create_dir_all(&component_service_directory)
            .await
            .unwrap();

        Self {
            redis,
            redis_monitor,
            component_directory,
            component_service_directory,
            component_writer,
            initial_agent_files_service: initial_agent_files_service.clone(),
            component_temp_directory: Arc::new(TestTempDir::Owned(TempDir::new().unwrap())),
            data_dir: Arc::new(TestTempDir::Owned(data_dir)),
        }
    }
}

impl test_r::core::HostedDep for WorkerExecutorTestDependencies {
    fn descriptor(&self) -> Vec<u8> {
        // Canonicalize every on-disk path before shipping it to workers:
        // worker subprocesses may run with a different cwd than the parent
        // (e.g. the runner's stamping), so the descriptor must not contain
        // relative paths like `../test-components` (Phase 3.4 hardening
        // from the oracle review).
        fn canonical(path: &Path) -> PathBuf {
            std::fs::canonicalize(path).unwrap_or_else(|e| {
                panic!(
                    "WorkerExecutorTestDependencies::descriptor: \
                     cannot canonicalize {path:?}: {e}"
                )
            })
        }

        let descriptor = WorkerExecutorTestDependenciesDescriptor {
            redis_host: self.redis.private_host().to_string(),
            redis_port: self.redis.private_port(),
            redis_prefix: self.redis.prefix().to_string(),
            blob_storage_root: canonical(&self.blob_storage_root()),
            component_directory: canonical(&self.component_directory),
            component_service_directory: canonical(&self.component_service_directory),
            component_temp_directory: canonical(self.component_temp_directory.path()),
            data_dir_path: canonical(self.data_dir.path()),
        };
        serde_json::to_vec(&descriptor)
            .expect("serializing WorkerExecutorTestDependenciesDescriptor must not fail")
    }

    fn from_descriptor(bytes: &[u8]) -> Self {
        // `new()` installs the rustls CryptoProvider on the parent, but worker
        // subprocesses created by test-r reconstruct the dep via
        // `from_descriptor` and so never run `new()`. Without a process-level
        // CryptoProvider, `HttpConnectionPool::new` (called from the executor
        // bootstrap in every test) panics because the AWS SDK transitively
        // enables both `aws-lc-rs` and `ring` rustls backends and rustls
        // refuses to auto-pick one. Install ring here as well; the result is
        // ignored if another `from_descriptor` call has already installed it
        // in this worker process.
        let _ = rustls::crypto::ring::default_provider().install_default();

        let descriptor: WorkerExecutorTestDependenciesDescriptor = serde_json::from_slice(bytes)
            .expect("deserializing WorkerExecutorTestDependenciesDescriptor must not fail");

        let redis: Arc<dyn Redis> = Arc::new(
            golem_test_framework::components::redis::provided::ProvidedRedis::new(
                descriptor.redis_host,
                descriptor.redis_port,
                descriptor.redis_prefix,
            ),
        );
        let redis_monitor: Arc<dyn RedisMonitor> = Arc::new(
            golem_test_framework::components::redis_monitor::provided::ProvidedRedisMonitor::new(),
        );

        // Worker side: attach to the parent's on-disk blob storage without
        // touching its layout. `FileSystemBlobStorage::attach_existing` is
        // sync (the parent already created the directory tree).
        let blob_storage = Arc::new(
            FileSystemBlobStorage::attach_existing(&descriptor.blob_storage_root)
                .expect("attach to parent-prepared blob storage"),
        );
        let initial_agent_files_service =
            Arc::new(InitialAgentFilesService::new(blob_storage.clone()));

        // Worker side: do NOT call `FileSystemComponentWriter::new` here —
        // it would `remove_dir_all` the parent's already-warmed component
        // store. Use `attach_existing` to reuse the parent's on-disk cache.
        let component_writer: Arc<FileSystemComponentWriter> = Arc::new(
            FileSystemComponentWriter::attach_existing(&descriptor.component_service_directory),
        );

        Self {
            redis,
            redis_monitor,
            component_directory: descriptor.component_directory,
            component_service_directory: descriptor.component_service_directory,
            component_writer,
            initial_agent_files_service,
            component_temp_directory: Arc::new(TestTempDir::Borrowed(
                descriptor.component_temp_directory,
            )),
            data_dir: Arc::new(TestTempDir::Borrowed(descriptor.data_dir_path)),
        }
    }
}

#[cfg(test)]
mod hosted_descriptor_tests {
    use super::*;
    use test_r::core::HostedDep;
    use test_r::test;

    /// Phase 3.4 regression: when a worker reconstructs
    /// `WorkerExecutorTestDependencies` via `HostedDep::from_descriptor`
    /// and the resulting struct is dropped, the parent's TempDirs must
    /// survive. Dropping a borrowed `TestTempDir` is a no-op by
    /// construction; this test pins that invariant.
    #[test]
    fn worker_side_drop_does_not_delete_parent_temp_dirs() {
        // Stand in for the parent's data + component-temp directories.
        let parent_data = TempDir::new().unwrap();
        let parent_component_temp = TempDir::new().unwrap();
        let blob_storage_root = parent_data.path().join("blobs");
        let component_service_directory = parent_data.path().join("components");
        std::fs::create_dir_all(&blob_storage_root).unwrap();
        std::fs::create_dir_all(&component_service_directory).unwrap();

        let descriptor = WorkerExecutorTestDependenciesDescriptor {
            redis_host: "localhost".to_string(),
            redis_port: 6379,
            redis_prefix: "".to_string(),
            blob_storage_root: blob_storage_root.clone(),
            component_directory: Path::new("../test-components").to_path_buf(),
            component_service_directory: component_service_directory.clone(),
            component_temp_directory: parent_component_temp.path().to_path_buf(),
            data_dir_path: parent_data.path().to_path_buf(),
        };
        let bytes = serde_json::to_vec(&descriptor).unwrap();

        // Worker reconstructs and immediately drops.
        {
            let worker = WorkerExecutorTestDependencies::from_descriptor(&bytes);
            // Borrowed temp-dir wrappers must point at the parent paths.
            assert_eq!(worker.data_dir.path(), parent_data.path());
            assert_eq!(
                worker.component_temp_directory.path(),
                parent_component_temp.path()
            );
            assert_eq!(worker.blob_storage_root(), blob_storage_root);
            drop(worker);
        }

        // After the worker handle drops, the parent's directories must
        // still exist — Borrowed must not delete on drop.
        assert!(parent_data.path().exists(), "parent data_dir was removed");
        assert!(
            parent_component_temp.path().exists(),
            "parent component_temp_directory was removed"
        );
        assert!(
            blob_storage_root.exists(),
            "parent blob storage root was removed"
        );
        assert!(
            component_service_directory.exists(),
            "parent component service directory was removed"
        );
    }

    /// Phase 3.4 regression: `from_descriptor` must NOT call
    /// `FileSystemComponentWriter::new()` because that would
    /// `remove_dir_all` the parent's already-warmed component cache.
    #[test]
    fn worker_side_attach_does_not_destroy_component_service_directory() {
        let parent_data = TempDir::new().unwrap();
        let blob_storage_root = parent_data.path().join("blobs");
        let component_service_directory = parent_data.path().join("components");
        std::fs::create_dir_all(&blob_storage_root).unwrap();
        std::fs::create_dir_all(&component_service_directory).unwrap();

        // Sentinel file written by the "parent" warm-cache step.
        let sentinel = component_service_directory.join("warmed.txt");
        std::fs::write(&sentinel, b"warmed").unwrap();

        let descriptor = WorkerExecutorTestDependenciesDescriptor {
            redis_host: "localhost".to_string(),
            redis_port: 6379,
            redis_prefix: "".to_string(),
            blob_storage_root,
            component_directory: Path::new("../test-components").to_path_buf(),
            component_service_directory: component_service_directory.clone(),
            component_temp_directory: parent_data.path().join("ctemp"),
            data_dir_path: parent_data.path().to_path_buf(),
        };
        std::fs::create_dir_all(&descriptor.component_temp_directory).unwrap();
        let bytes = serde_json::to_vec(&descriptor).unwrap();

        let _worker = WorkerExecutorTestDependencies::from_descriptor(&bytes);
        // Sentinel must survive worker-side attach. If `from_descriptor`
        // ever switches back to calling `FileSystemComponentWriter::new`,
        // this assertion will fire because `new` deletes the root.
        assert!(
            sentinel.exists(),
            "worker-side attach destroyed the parent's component cache"
        );
    }

    /// Phase 3.4 hardening (oracle review): the worker-side
    /// `FileSystemComponentWriter::attach_existing` must fail fast if the
    /// parent-prepared directory does not exist, instead of silently
    /// creating a fresh per-worker store on first write.
    #[test]
    fn attach_existing_fails_fast_when_component_dir_missing() {
        use crate::component_writer::FileSystemComponentWriter;

        let parent_data = TempDir::new().unwrap();
        let missing = parent_data.path().join("never-prepared");

        let result = std::panic::catch_unwind(|| {
            FileSystemComponentWriter::attach_existing(&missing);
        });
        assert!(
            result.is_err(),
            "attach_existing must panic when {missing:?} does not exist"
        );
    }
}

#[derive(Clone)]
pub struct TestWorkerExecutor {
    _join_set: Arc<JoinSet<anyhow::Result<()>>>,
    /// Holds the `RunDetails` to keep the shutdown token (and epoch thread)
    /// alive for the duration of the test. Dropping this triggers the
    /// graph-wide shutdown signal.
    _run_details: Arc<RunDetails>,
    pub deps: WorkerExecutorTestDependencies,
    pub client: WorkerExecutorClient<OtelGrpcService<Channel>>,
    pub context: TestContext,
    /// Same `AdditionalTestDeps` instance that the worker context received via
    /// `Bootstrap::create_additional_deps`. Tests use it to evict a worker's
    /// wasmtime instance while keeping the `Worker` shell (and its read-only
    /// cache) alive, and to read per-agent instance load counts.
    additional_test_deps: AdditionalTestDeps,
    leak_detector: std::sync::Weak<()>,
}

impl TestWorkerExecutor {
    /// Returns a weak reference that can be used to verify that the
    /// service graph (`All`) was properly deallocated after the executor
    /// is dropped. If `upgrade()` returns `Some`, services have leaked.
    pub fn leak_detector(&self) -> std::sync::Weak<()> {
        self.leak_detector.clone()
    }

    pub fn auth_ctx(&self) -> AuthCtx {
        AuthCtx::User(UserAuthCtx {
            account_id: self.context.account_id,
            account_email: golem_common::model::account::AccountEmail::new("test@golem"),
            account_plan_id: self.context.account_plan_id,
            account_roles: self.context.account_roles.clone(),
            effective_surface: golem_common::model::card::EffectiveSurface {
                source_card_ids: Vec::new(),
                lower: Vec::new(),
                upper: Vec::new(),
            },
        })
    }

    pub async fn store_component_with_id(
        &self,
        name: &str,
        component_id: &ComponentId,
        environment_id: &EnvironmentId,
    ) -> anyhow::Result<Component> {
        let source_path = self.deps.component_directory.join(format!("{name}.wasm"));
        self.deps
            .component_writer
            .add_component_with_id(
                &source_path,
                component_id,
                name,
                *environment_id,
                self.context.application_id,
                self.context.account_id,
            )
            .await
    }

    /// Returns `true` iff the `Worker` shell for `owned_agent_id` is currently
    /// registered in `ActiveWorkers` *and* has a loaded wasmtime instance.
    /// Returns `false` when:
    ///   - no `Worker` shell is currently in `ActiveWorkers` for this id, or
    ///   - the shell is present but the wasmtime instance has been unloaded
    ///     (e.g. after memory-pressure eviction).
    ///
    /// Used by the read-only cache eviction-survival test (#3393 T5).
    pub async fn worker_is_loaded(&self, owned_agent_id: &OwnedAgentId) -> bool {
        match self
            .additional_test_deps
            .try_get_worker(owned_agent_id)
            .await
        {
            Some(worker) => worker.is_loaded().await,
            None => false,
        }
    }

    /// Returns the current eviction classification for the worker shell
    /// registered in `ActiveWorkers`, or `None` if the worker is missing or
    /// non-evictable. Used by tests to wait until the worker is `LoadedIdle`
    /// before triggering memory-pressure eviction.
    pub async fn worker_eviction_class(
        &self,
        owned_agent_id: &OwnedAgentId,
    ) -> Option<golem_worker_executor::worker::EvictionClass> {
        let worker = self
            .additional_test_deps
            .try_get_worker(owned_agent_id)
            .await?;
        worker.eviction_class().await
    }

    /// Returns the per-worker memory requirement that the executor uses when
    /// reserving from the worker memory semaphore. Lets tests sanity-check that
    /// they have constrained the memory budget tightly enough to force
    /// eviction.
    pub async fn worker_memory_requirement(
        &self,
        owned_agent_id: &OwnedAgentId,
    ) -> anyhow::Result<u64> {
        let worker = self
            .additional_test_deps
            .try_get_worker(owned_agent_id)
            .await
            .ok_or_else(|| anyhow!("worker {owned_agent_id} is not currently in ActiveWorkers"))?;
        Ok(worker.memory_requirement().await?)
    }

    pub async fn get_running_workers_metadata(
        &self,
        component_id: &ComponentId,
        filter: Option<AgentFilter>,
    ) -> anyhow::Result<Vec<AgentMetadataDto>> {
        let response = self
            .client
            .clone()
            .get_running_workers_metadata(GetRunningWorkersMetadataRequest {
                component_id: Some((*component_id).into()),
                filter: filter.map(|f| f.into()),
                auth_ctx: Some(self.auth_ctx().into()),
            })
            .await
            .expect("Failed to get running workers metadata")
            .into_inner();

        match response.result {
            None => panic!("No response from get_running_workers_metadata"),
            Some(get_running_workers_metadata_response::Result::Success(success)) => Ok(success
                .workers
                .into_iter()
                .map(|w| w.try_into())
                .collect::<Result<_, _>>()
                .map_err(|e| anyhow!("Failed converting worker metadata: {e}"))?),
            Some(get_running_workers_metadata_response::Result::Failure(error)) => {
                Err(anyhow!("Failed to get worker metadata: {error:?}"))
            }
        }
    }
}

/// A single global, monotonically-increasing id allocator shared by
/// every test-r worker in the suite.
///
/// Workers parameterise tests on [`LastUniqueId`] (a type alias for the
/// auto-generated `UniqueIdsStub`); each call to `next()` round-trips via
/// the IPC HostedRpc transport to the parent-hosted [`LastUniqueIdOwner`]
/// and returns a fresh id. There is no per-worker partitioning anymore —
/// uniqueness is enforced by the single shared `AtomicU64` in the parent.
#[test_r::hosted_rpc]
pub trait UniqueIds {
    fn next(&self) -> u64;
}

/// Parent-side owner: lives in the top-level test process and serves
/// [`UniqueIds::next`] calls from every worker subprocess.
///
/// Intentionally **does not** derive [`Default`]: the derived zero-value
/// default would silently re-introduce `0` as the first id and break the
/// "never returns 0" contract preserved from the previous
/// `AtomicU16`-based shape. The manual `Default` impl below forwards to
/// [`LastUniqueIdOwner::new`] so callers using either entry point get a
/// counter that starts at `1`.
#[derive(Debug)]
pub struct LastUniqueIdOwner {
    next_id: AtomicU64,
}

impl LastUniqueIdOwner {
    pub fn new() -> Self {
        Self {
            next_id: AtomicU64::new(1),
        }
    }
}

impl Default for LastUniqueIdOwner {
    fn default() -> Self {
        Self::new()
    }
}

impl UniqueIds for LastUniqueIdOwner {
    fn next(&self) -> u64 {
        // Uniqueness is the only requirement — `Relaxed` is sufficient
        // because every distinct `fetch_add` necessarily yields a
        // distinct value, and HostedRpc dispatch is already serialised
        // per-dep at the runtime level.
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }
}

impl test_r::core::HostedRpcDep for LastUniqueIdOwner {
    type Stub = UniqueIdsStub;

    fn dispatch(&mut self, method_idx: u32, args: &[u8]) -> Result<Vec<u8>, String> {
        UniqueIdsDispatch::dispatch_unique_ids(self, method_idx, args)
    }

    fn build_stub(channel: test_r::core::HostedRpcChannel) -> Self::Stub {
        UniqueIdsStub::new(channel)
    }
}

/// Public name kept stable for downstream tests; internally this is the
/// macro-generated worker-side stub for the [`UniqueIds`] trait.
///
/// This is a type alias (not a wrapper), so:
/// - `&LastUniqueId` parameters and `inherit_test_dep!(LastUniqueId)`
///   downstream sites continue to compile unchanged.
/// - `LastUniqueId::next` resolves through the [`UniqueIds`] trait, so
///   callers in **other** crates that want to invoke it directly need
///   `use golem_worker_executor_test_utils::UniqueIds;` in scope.
/// - `Debug` output and panic messages will refer to the value as a
///   `UniqueIdsStub`, not as `LastUniqueId`.
pub type LastUniqueId = UniqueIdsStub;

#[cfg(test)]
mod last_unique_id_owner_tests {
    use super::{LastUniqueIdOwner, UniqueIds};
    use test_r::test;

    /// Pin the "never returns 0" contract preserved from the previous
    /// `AtomicU16`-based shape: the first id allocated by a fresh
    /// owner must be `1`, not `0`.
    #[test]
    fn new_starts_at_one() {
        let owner = LastUniqueIdOwner::new();
        assert_eq!(
            owner.next(),
            1,
            "first id from LastUniqueIdOwner::new() must be 1 to preserve the never-returns-0 contract"
        );
    }

    /// Pin that the manual `Default` impl forwards to `new()`. If a
    /// future refactor lets the derived `Default` slip back in, this
    /// test fires because the derived default would yield `0` first.
    #[test]
    fn default_starts_at_one() {
        let owner = <LastUniqueIdOwner as Default>::default();
        assert_eq!(
            owner.next(),
            1,
            "first id from LastUniqueIdOwner::default() must be 1; \
             a derived Default would re-introduce 0 here"
        );
    }

    /// Subsequent calls must produce distinct, monotonically increasing
    /// ids. This is the core uniqueness contract.
    #[test]
    fn next_is_strictly_monotonic_and_unique() {
        let owner = LastUniqueIdOwner::new();
        let a = owner.next();
        let b = owner.next();
        let c = owner.next();
        assert!(
            a < b && b < c,
            "ids must be strictly increasing: got {a}, {b}, {c}"
        );
        assert_ne!(a, 0, "no id may be 0");
    }
}

#[derive(Debug, Clone)]
pub struct TestContext {
    base_prefix: String,
    unique_id: u64,

    // account id to use during tests
    pub account_id: AccountId,
    // plan of the account id to use
    pub account_plan_id: PlanId,
    // roles of the account plan
    pub account_roles: BTreeSet<AccountRole>,
    // tokens of account to use
    pub account_token: TokenSecret,
    // application id to use during tests
    pub application_id: ApplicationId,
    // default environment id to use during tests
    pub default_environment_id: EnvironmentId,
}

impl TestContext {
    pub fn new(last_unique_id: &LastUniqueId) -> Self {
        let base_prefix = Uuid::new_v4().to_string();
        let unique_id = last_unique_id.next();

        let account_id = AccountId::new();
        let account_plan_id = PlanId::new();
        let account_roles = BTreeSet::new();
        let application_id = ApplicationId::new();
        let default_environment_id = EnvironmentId::new();
        let account_token = TokenSecret::new();

        Self {
            base_prefix,
            unique_id,
            account_id,
            account_plan_id,
            account_roles,
            account_token,
            application_id,
            default_environment_id,
        }
    }

    pub fn redis_prefix(&self) -> String {
        format!("test-{}-{}:", self.base_prefix, self.unique_id)
    }
}

pub async fn start(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
) -> anyhow::Result<TestWorkerExecutor> {
    start_customized(deps, context, None, None, None, None, None, None).await
}

pub async fn start_with_snapshot_policy(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    snapshot_policy: SnapshotPolicy,
) -> anyhow::Result<TestWorkerExecutor> {
    start_customized(
        deps,
        context,
        None,
        None,
        None,
        Some(snapshot_policy),
        None,
        None,
    )
    .await
}

pub async fn start_with_http_client_config(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    http_client: HttpClientConfig,
) -> anyhow::Result<TestWorkerExecutor> {
    start_customized(
        deps,
        context,
        None,
        None,
        None,
        None,
        Some(http_client),
        None,
    )
    .await
}

pub async fn start_with_oplog_config(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    oplog_config_override: Option<OplogConfig>,
) -> anyhow::Result<TestWorkerExecutor> {
    start_customized(
        deps,
        context,
        None,
        None,
        None,
        None,
        None,
        oplog_config_override,
    )
    .await
}

pub async fn start_with_redis_storage(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
) -> anyhow::Result<TestWorkerExecutor> {
    start_with_redis_oplog_config(deps, context, None).await
}

pub async fn start_with_redis_oplog_config(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    oplog_config_override: Option<OplogConfig>,
) -> anyhow::Result<TestWorkerExecutor> {
    let redis = deps.redis.clone();
    let redis_monitor = deps.redis_monitor.clone();
    redis.assert_valid();
    redis_monitor.assert_valid();
    info!("Using Redis on port {}", redis.public_port());

    let mut config = make_base_test_config(deps);
    apply_redis_storage_config(&mut config, deps, context);
    if let Some(oplog_config) = oplog_config_override {
        config.oplog = oplog_config;
    }

    start_executor_with_config(deps, context, config, TestExecutorOverrides::default()).await
}

/// Overrides for customizing the test executor. Allows wrapping services with
/// failure-injecting wrappers and modifying the GolemConfig.
type ConfigureFn = dyn Fn(&mut GolemConfig) + Send + Sync;
type WrapKeyValueServiceFn =
    dyn Fn(Arc<dyn KeyValueService>) -> Arc<dyn KeyValueService> + Send + Sync;
type WrapBlobStoreServiceFn =
    dyn Fn(Arc<dyn BlobStoreService>) -> Arc<dyn BlobStoreService> + Send + Sync;
type WrapRpcFn = dyn Fn(Arc<dyn Rpc>) -> Arc<dyn Rpc> + Send + Sync;
type CreateDirectInvocationAuthFn = dyn Fn() -> Arc<dyn DirectInvocationAuthService> + Send + Sync;

#[derive(Clone, Default)]
pub struct TestExecutorOverrides {
    pub configure: Option<Arc<ConfigureFn>>,
    pub wrap_key_value_service: Option<Arc<WrapKeyValueServiceFn>>,
    pub wrap_blob_store_service: Option<Arc<WrapBlobStoreServiceFn>>,
    pub wrap_rpc: Option<Arc<WrapRpcFn>>,
    pub create_direct_invocation_auth: Option<Arc<CreateDirectInvocationAuthFn>>,
    /// Named retry policies that the executor's `EnvironmentStateService`
    /// should expose to running agents (mirrors `retryPolicyDefaults` in
    /// `golem.yaml`).  When `None`, an empty policy list is used.
    pub retry_policies: Option<Vec<NamedRetryPolicy>>,
}

fn make_base_test_config(deps: &WorkerExecutorTestDependencies) -> GolemConfig {
    GolemConfig {
        blob_storage: BlobStorageConfig::LocalFileSystem(LocalFileSystemBlobStorageConfig {
            root: deps.data_dir.path().join("blobs"),
        }),
        http_port: 0,
        grpc: GrpcApiConfig {
            port: 0,
            tls: GrpcServerTlsConfig::disabled(),
            ..Default::default()
        },
        compiled_component_service: CompiledComponentServiceConfig::Enabled(
            CompiledComponentServiceEnabledConfig {},
        ),
        agent_types_service: AgentTypesServiceConfig::Local(AgentTypesServiceLocalConfig {}),
        engine: EngineConfig {
            enable_fs_cache: true,
        },
        // Use Disabled resource limits so Worker::new() can call initialize_account
        // without attempting a gRPC connection to a registry service that does
        // not exist in this test setup.
        resource_limits: ResourceLimitsConfig::Disabled(ResourceLimitsDisabledConfig {}),
        ..Default::default()
    }
}

pub fn sqlite_storage_config(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
) -> DbSqliteConfig {
    let database = deps
        .data_dir
        .path()
        .join(format!(
            "worker-executor-{}.db",
            context.redis_prefix().replace(':', "_")
        ))
        .to_string_lossy()
        .into_owned();

    DbSqliteConfig {
        database,
        max_connections: 8,
        foreign_keys: false,
    }
}

pub fn scheduler_sqlite_storage_config(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
) -> DbSqliteConfig {
    let database = deps
        .data_dir
        .path()
        .join(format!(
            "worker-executor-scheduler-{}.db",
            context.redis_prefix().replace(':', "_")
        ))
        .to_string_lossy()
        .into_owned();

    DbSqliteConfig {
        database,
        max_connections: 8,
        foreign_keys: false,
    }
}

fn apply_sqlite_storage_config(
    config: &mut GolemConfig,
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
) {
    config.key_value_storage = KeyValueStorageConfig::Sqlite(sqlite_storage_config(deps, context));
    config.indexed_storage =
        IndexedStorageConfig::KVStoreSqlite(IndexedStorageKVStoreSqliteConfig {});
    config.scheduler_storage =
        SchedulerStorageConfig::Sqlite(scheduler_sqlite_storage_config(deps, context));
}

fn apply_redis_storage_config(
    config: &mut GolemConfig,
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
) {
    config.key_value_storage =
        KeyValueStorageConfig::NamespaceRouted(KeyValueStorageNamespaceRoutedConfig {
            cache: KeyValueStorageInnerConfig::Redis(RedisConfig {
                port: deps.redis.public_port(),
                key_prefix: context.redis_prefix(),
                ..Default::default()
            }),
            persistent: KeyValueStorageInnerConfig::Sqlite(sqlite_storage_config(deps, context)),
        });
    config.indexed_storage =
        IndexedStorageConfig::KVStoreRedis(IndexedStorageKVStoreRedisConfig {});
    config.scheduler_storage =
        SchedulerStorageConfig::Sqlite(scheduler_sqlite_storage_config(deps, context));
}

async fn start_executor_with_config(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    config: GolemConfig,
    overrides: TestExecutorOverrides,
) -> anyhow::Result<TestWorkerExecutor> {
    let prometheus = golem_worker_executor::metrics::register_all();

    let handle = Handle::current();
    let mut join_set = JoinSet::new();

    // Allocate the AdditionalTestDeps here so that the same instance is shared
    // between the bootstrap (via `create_additional_deps`) and the
    // `TestWorkerExecutor` returned to the test (so tests can observe and
    // mutate per-worker test-only state, e.g. eviction).
    let additional_test_deps = AdditionalTestDeps::new();

    let details = run(
        config,
        prometheus,
        handle,
        deps.component_service_directory.clone(),
        overrides,
        additional_test_deps.clone(),
        &mut join_set,
    )
    .await?;
    let grpc_port = details.grpc_port;
    let leak_detector = details.leak_detector.clone();
    let details = Arc::new(details);

    let start = std::time::Instant::now();
    loop {
        info!("Waiting for worker-executor to be reachable on port {grpc_port}");
        let channel = Channel::from_shared(format!("http://127.0.0.1:{grpc_port}"))
            .expect("Valid URI")
            .connect()
            .await;

        if let Ok(channel) = channel {
            let otel_channel = ServiceBuilder::new()
                .layer(tonic_tracing_opentelemetry::middleware::client::OtelGrpcLayer)
                .service(channel);
            let client = WorkerExecutorClient::new(otel_channel)
                .max_decoding_message_size(32 * 1024 * 1024)
                .max_encoding_message_size(32 * 1024 * 1024);
            break Ok(TestWorkerExecutor {
                _join_set: Arc::new(join_set),
                _run_details: details,
                deps: deps.clone(),
                client,
                context: context.clone(),
                additional_test_deps,
                leak_detector,
            });
        } else if start.elapsed().as_secs() > 10 {
            break Err(anyhow::anyhow!("Timeout waiting for server to start"));
        }
    }
}

pub async fn start_with_overrides(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    overrides: TestExecutorOverrides,
) -> anyhow::Result<TestWorkerExecutor> {
    let mut config = make_base_test_config(deps);
    apply_sqlite_storage_config(&mut config, deps, context);
    config.memory = MemoryConfig {
        ..Default::default()
    };

    if let Some(configure) = &overrides.configure {
        configure(&mut config);
    }

    start_executor_with_config(deps, context, config, overrides).await
}

pub async fn start_customized(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    system_memory_override: Option<u64>,
    system_storage_override: Option<u64>,
    retry_override: Option<RetryConfig>,
    snapshot_policy_override: Option<SnapshotPolicy>,
    http_client_override: Option<HttpClientConfig>,
    oplog_config_override: Option<OplogConfig>,
) -> anyhow::Result<TestWorkerExecutor> {
    let mut config = make_base_test_config(deps);
    apply_sqlite_storage_config(&mut config, deps, context);
    config.memory = MemoryConfig {
        system_memory_override,
        ..Default::default()
    };
    config.filesystem_storage = FilesystemStorageConfig {
        total_worker_filesystem_storage_bytes: system_storage_override,
        ..Default::default()
    };
    if let Some(retry) = retry_override {
        config.retry = retry;
    }
    if let Some(snapshot_policy) = snapshot_policy_override {
        config.oplog.default_snapshotting = snapshot_policy;
    }
    if let Some(http_client) = http_client_override {
        config.http_client = http_client;
    }
    if let Some(oplog_config) = oplog_config_override {
        config.oplog = oplog_config;
    }

    start_executor_with_config(deps, context, config, TestExecutorOverrides::default()).await
}

async fn run(
    golem_config: GolemConfig,
    prometheus_registry: Registry,
    runtime: Handle,
    component_service_directory: PathBuf,
    overrides: TestExecutorOverrides,
    additional_test_deps: AdditionalTestDeps,
    join_set: &mut JoinSet<Result<(), Error>>,
) -> Result<RunDetails, Error> {
    info!("Golem Worker Executor starting up...");

    bootstrap_and_run_worker_executor(
        &TestServerBootstrap {
            component_service_directory,
            overrides,
            additional_test_deps,
        },
        golem_config,
        prometheus_registry,
        runtime,
        join_set,
        false,
    )
    .await
}

struct TestWorkerCtx {
    durable_ctx: DurableWorkerCtx<TestWorkerCtx>,
}

impl DurableWorkerCtxView<TestWorkerCtx> for TestWorkerCtx {
    fn durable_ctx(&self) -> &DurableWorkerCtx<TestWorkerCtx> {
        &self.durable_ctx
    }

    fn durable_ctx_mut(&mut self) -> &mut DurableWorkerCtx<TestWorkerCtx> {
        &mut self.durable_ctx
    }
}

impl wasmtime_wasi::p2::bindings::cli::environment::Host for TestWorkerCtx {
    fn get_environment(
        &mut self,
    ) -> impl Future<Output = wasmtime::Result<Vec<(String, String)>>> + Send {
        wasmtime_wasi::p2::bindings::cli::environment::Host::get_environment(&mut self.durable_ctx)
    }

    fn get_arguments(&mut self) -> impl Future<Output = wasmtime::Result<Vec<String>>> + Send {
        wasmtime_wasi::p2::bindings::cli::environment::Host::get_arguments(&mut self.durable_ctx)
    }

    fn initial_cwd(&mut self) -> impl Future<Output = wasmtime::Result<Option<String>>> + Send {
        wasmtime_wasi::p2::bindings::cli::environment::Host::initial_cwd(&mut self.durable_ctx)
    }
}

#[async_trait]
impl FuelManagement for TestWorkerCtx {
    fn ensure_fuel(&mut self, _current_level: u64) -> Result<(), AgentError> {
        Ok(())
    }

    fn return_fuel(&mut self, _current_level: u64) -> u64 {
        0
    }
}

impl CallCountManagement for TestWorkerCtx {
    fn reset_invocation_call_counts(&mut self) {
        self.durable_ctx.reset_invocation_call_counts();
    }

    fn record_monthly_http_call(&mut self) -> anyhow::Result<()> {
        Ok(()) // test context: monthly limits are always unlimited
    }

    fn record_monthly_rpc_call(&mut self) -> anyhow::Result<()> {
        Ok(()) // test context: monthly limits are always unlimited
    }
}

#[async_trait]
impl ExternalOperations<TestWorkerCtx> for TestWorkerCtx {
    type ExtraDeps = AdditionalTestDeps;

    async fn get_last_error_and_retry_count<T: HasAll<TestWorkerCtx> + Send + Sync>(
        this: &T,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        latest_worker_status: &AgentStatusRecord,
    ) -> Option<LastError> {
        DurableWorkerCtx::<TestWorkerCtx>::get_last_error_and_retry_count(
            this,
            owned_agent_id,
            agent_mode,
            latest_worker_status,
        )
        .await
    }

    async fn resume_replay(
        store: &mut (impl AsContextMut<Data = TestWorkerCtx> + Send),
        instance: &Instance,
        refresh_replay_target: bool,
    ) -> Result<Option<RetryDecision>, WorkerExecutorError> {
        DurableWorkerCtx::<TestWorkerCtx>::resume_replay(store, instance, refresh_replay_target)
            .await
    }

    async fn prepare_instance(
        agent_id: &AgentId,
        instance: &Instance,
        store: &mut (impl AsContextMut<Data = TestWorkerCtx> + Send),
    ) -> Result<Option<RetryDecision>, WorkerExecutorError> {
        DurableWorkerCtx::<TestWorkerCtx>::prepare_instance(agent_id, instance, store).await
    }

    async fn on_shard_assignment_changed<T: HasAll<TestWorkerCtx> + Send + Sync + 'static>(
        this: &T,
    ) -> Result<(), Error> {
        DurableWorkerCtx::<TestWorkerCtx>::on_shard_assignment_changed(this).await
    }
}

#[async_trait]
impl InvocationManagement for TestWorkerCtx {
    async fn set_current_idempotency_key(&mut self, key: IdempotencyKey) {
        self.durable_ctx.set_current_idempotency_key(key).await
    }

    async fn get_current_idempotency_key(&self) -> Option<IdempotencyKey> {
        self.durable_ctx.get_current_idempotency_key().await
    }

    async fn set_current_invocation_context(
        &mut self,
        invocation_context: InvocationContextStack,
    ) -> Result<(), WorkerExecutorError> {
        self.durable_ctx
            .set_current_invocation_context(invocation_context)
            .await
    }

    async fn get_current_invocation_context(&self) -> InvocationContextStack {
        self.durable_ctx.get_current_invocation_context().await
    }

    fn is_live(&self) -> bool {
        self.durable_ctx.is_live()
    }

    fn is_replay(&self) -> bool {
        self.durable_ctx.is_replay()
    }
}

#[async_trait]
impl StatusManagement for TestWorkerCtx {
    fn check_interrupt(&self) -> Option<InterruptKind> {
        self.durable_ctx.check_interrupt()
    }

    fn set_suspended(&self) {
        self.durable_ctx.set_suspended()
    }

    fn set_running(&self) {
        self.durable_ctx.set_running()
    }
}

#[async_trait]
impl InvocationHooks for TestWorkerCtx {
    async fn on_agent_invocation_started(
        &mut self,
        invocation: AgentInvocation,
    ) -> Result<(), WorkerExecutorError> {
        self.durable_ctx
            .on_agent_invocation_started(invocation)
            .await
    }

    async fn on_invocation_failure(
        &mut self,
        full_function_name: &str,
        trap_type: &TrapType,
    ) -> RetryDecision {
        self.durable_ctx
            .on_invocation_failure(full_function_name, trap_type)
            .await
    }

    async fn on_agent_invocation_success(
        &mut self,
        full_function_name: &str,
        consumed_fuel: u64,
        output: &mut AgentInvocationOutput,
    ) -> Result<(), WorkerExecutorError> {
        self.durable_ctx
            .on_agent_invocation_success(full_function_name, consumed_fuel, output)
            .await
    }

    async fn get_current_retry_point(&self) -> OplogIndex {
        self.durable_ctx.get_current_retry_point().await
    }

    fn enter_read_only_mode(&mut self, method_name: String) {
        self.durable_ctx.enter_read_only_mode(method_name)
    }

    fn exit_read_only_mode(&mut self) {
        self.durable_ctx.exit_read_only_mode()
    }
}

#[async_trait]
impl ResourceStore for TestWorkerCtx {
    fn self_uri(&self) -> Uri {
        self.durable_ctx.self_uri()
    }

    async fn add(&mut self, resource: ResourceAny, name: ResourceTypeId) -> u64 {
        self.durable_ctx.add(resource, name).await
    }

    async fn get(&mut self, resource_id: u64) -> Option<(ResourceTypeId, ResourceAny)> {
        ResourceStore::get(&mut self.durable_ctx, resource_id).await
    }

    async fn borrow(&self, resource_id: u64) -> Option<(ResourceTypeId, ResourceAny)> {
        self.durable_ctx.borrow(resource_id).await
    }
}

#[async_trait]
impl UpdateManagement for TestWorkerCtx {
    fn begin_call_snapshotting_function(&mut self) {
        self.durable_ctx.begin_call_snapshotting_function()
    }

    fn end_call_snapshotting_function(&mut self) {
        self.durable_ctx.end_call_snapshotting_function()
    }

    async fn on_worker_update_failed(
        &self,
        target_revision: ComponentRevision,
        details: Option<String>,
    ) {
        self.durable_ctx
            .on_worker_update_failed(target_revision, details)
            .await
    }

    async fn on_worker_update_succeeded(
        &self,
        target_revision: ComponentRevision,
        new_component_size: u64,
        new_active_plugins: HashSet<EnvironmentPluginGrantId>,
    ) {
        self.durable_ctx
            .on_worker_update_succeeded(target_revision, new_component_size, new_active_plugins)
            .await
    }
}

struct TestServerBootstrap {
    component_service_directory: PathBuf,
    overrides: TestExecutorOverrides,
    /// The `AdditionalTestDeps` instance that the worker context will receive
    /// from `create_additional_deps`. Shared with `TestWorkerExecutor` so tests
    /// can observe (and mutate) per-worker test-only state.
    additional_test_deps: AdditionalTestDeps,
}

#[async_trait]
impl WorkerCtx for TestWorkerCtx {
    type PublicState = PublicDurableWorkerState<TestWorkerCtx>;

    const LOG_EVENT_EMIT_BEHAVIOUR: LogEventEmitBehaviour = LogEventEmitBehaviour::LiveOnly;

    async fn create(
        _account_id: AccountId,
        owned_agent_id: OwnedAgentId,
        agent_id: Option<ParsedAgentId>,
        promise_service: Arc<dyn PromiseService>,
        worker_service: Arc<dyn WorkerService>,
        worker_enumeration_service: Arc<dyn WorkerEnumerationService>,
        key_value_service: Arc<dyn KeyValueService>,
        blob_store_service: Arc<dyn BlobStoreService>,
        rdbms_service: Arc<dyn rdbms::RdbmsService>,
        quota_service: Arc<dyn QuotaService>,
        event_service: Arc<dyn WorkerEventService>,
        active_workers: Arc<ActiveWorkers<TestWorkerCtx>>,
        oplog_service: Arc<dyn OplogService>,
        oplog: Arc<dyn Oplog>,
        invocation_queue: Weak<Worker<TestWorkerCtx>>,
        scheduler_service: Arc<dyn SchedulerService>,
        rpc: Arc<dyn Rpc>,
        worker_proxy: Arc<dyn WorkerProxy>,
        component_service: Arc<dyn ComponentService>,
        extra_deps: Self::ExtraDeps,
        config: Arc<GolemConfig>,
        worker_config: AgentConfig,
        execution_status: Arc<RwLock<ExecutionStatus>>,
        file_loader: Arc<FileLoader>,
        worker_fork: Arc<dyn WorkerForkService>,
        _resource_limits: Arc<dyn ResourceLimits>,
        agent_types_service: Arc<dyn AgentTypesService>,
        environment_state_service: Arc<dyn EnvironmentStateService>,
        agent_webhooks_service: Arc<AgentWebhooksService>,
        shard_service: Arc<dyn ShardService>,
        http_connection_pool: Option<wasmtime_wasi_http::HttpConnectionPool>,
        websocket_connection_pool: golem_worker_executor::durable_host::websocket::WebSocketConnectionPool,
        pending_update: Option<TimestampedUpdateDescription>,
        original_phantom_id: Option<Uuid>,
    ) -> Result<Self, WorkerExecutorError> {
        // Capture the executor's ActiveWorkers handle the first time we see
        // it, so test helpers (e.g. `worker_is_loaded`) can observe worker
        // shells under memory-pressure eviction (#3393 T5).
        extra_deps.set_active_workers(active_workers.clone());

        let oplog = Arc::new(TestOplog::new(
            owned_agent_id.clone(),
            oplog.clone(),
            extra_deps,
        ));
        let account_resource_limits = Arc::new(AtomicResourceEntry::new(
            u64::MAX,
            usize::MAX,
            usize::MAX,
            u64::MAX,
            u64::MAX,
        ));

        let durable_ctx = DurableWorkerCtx::create(
            owned_agent_id,
            agent_id,
            promise_service,
            worker_service,
            worker_enumeration_service,
            key_value_service,
            blob_store_service,
            rdbms_service,
            quota_service,
            event_service,
            oplog_service,
            oplog,
            invocation_queue,
            scheduler_service,
            rpc,
            worker_proxy,
            component_service,
            account_resource_limits,
            config,
            worker_config,
            execution_status,
            file_loader,
            worker_fork,
            agent_types_service,
            environment_state_service,
            agent_webhooks_service,
            shard_service,
            http_connection_pool,
            websocket_connection_pool,
            pending_update,
            original_phantom_id,
            u64::MAX,
            u64::MAX,
        )
        .await?;
        Ok(Self { durable_ctx })
    }

    fn as_wasi_view(&mut self) -> impl WasiView {
        self.durable_ctx.as_wasi_view()
    }

    fn as_wasi_http_view(&mut self) -> wasmtime_wasi_http::p2::WasiHttpCtxView<'_> {
        self.durable_ctx.as_wasi_http_view()
    }

    fn get_public_state(&self) -> &Self::PublicState {
        &self.durable_ctx.public_state
    }

    fn resource_limiter(&mut self) -> &mut dyn ResourceLimiterAsync {
        self
    }

    fn agent_id(&self) -> &AgentId {
        self.durable_ctx.agent_id()
    }

    fn owned_agent_id(&self) -> &OwnedAgentId {
        self.durable_ctx.owned_agent_id()
    }

    fn parsed_agent_id(&self) -> Option<ParsedAgentId> {
        self.durable_ctx.parsed_agent_id()
    }

    fn agent_type_provision_config(
        &self,
    ) -> Option<&golem_common::base_model::component_metadata::AgentTypeProvisionConfig> {
        self.durable_ctx.agent_type_provision_config()
    }

    fn agent_mode(&self) -> AgentMode {
        self.durable_ctx.agent_mode()
    }

    fn created_by(&self) -> AccountId {
        self.durable_ctx.created_by()
    }

    fn created_by_email(&self) -> &AccountEmail {
        self.durable_ctx.created_by_email()
    }

    fn component_metadata(&self) -> &Component {
        self.durable_ctx.component_metadata()
    }

    fn is_exit(error: &Error) -> Option<i32> {
        DurableWorkerCtx::<TestWorkerCtx>::is_exit(error)
    }

    fn rpc(&self) -> Arc<dyn Rpc> {
        self.durable_ctx.rpc()
    }

    fn worker_proxy(&self) -> Arc<dyn WorkerProxy> {
        self.durable_ctx.worker_proxy()
    }

    fn component_service(&self) -> Arc<dyn ComponentService> {
        self.durable_ctx.component_service()
    }

    fn worker_fork(&self) -> Arc<dyn WorkerForkService> {
        self.durable_ctx.worker_fork()
    }

    fn max_disk_space(&self) -> u64 {
        u64::MAX // no plan limit enforcement in tests by default
    }
}

#[async_trait]
impl ResourceLimiterAsync for TestWorkerCtx {
    async fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        debug!(
            "Memory growing for {}: current: {}, desired: {}",
            self.agent_id(),
            current,
            desired
        );
        let current_known = self.durable_ctx.total_linear_memory_size();
        let delta = (desired as u64).saturating_sub(current_known);
        if delta > 0 {
            self.durable_ctx
                .increase_memory(delta)
                .await
                .map_err(wasmtime::Error::from_anyhow)?;
            Ok(true)
        } else {
            Ok(true)
        }
    }

    async fn table_growing(
        &mut self,
        current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        debug!(
            "Table growing for {}: current: {}, desired: {}",
            self.agent_id(),
            current,
            desired
        );
        Ok(true)
    }
}

#[async_trait]
impl FileSystemReading for TestWorkerCtx {
    async fn get_file_system_node(
        &self,
        path: &CanonicalFilePath,
    ) -> Result<GetFileSystemNodeResult, WorkerExecutorError> {
        self.durable_ctx.get_file_system_node(path).await
    }

    async fn read_file(
        &self,
        path: &CanonicalFilePath,
    ) -> Result<ReadFileResult, WorkerExecutorError> {
        self.durable_ctx.read_file(path).await
    }
}

impl HostWasmRpc for TestWorkerCtx {
    async fn new(
        &mut self,
        agent_type_name: String,
        constructor: golem_schema::schema::wit::wire::SchemaValueTree,
        phantom_id: Option<golem_schema::schema::wit::wire::Uuid>,
        config: Vec<
            golem_common::schema::agent::bindings::golem::agent::common::TypedAgentConfigValue,
        >,
    ) -> anyhow::Result<Resource<WasmRpc>> {
        self.durable_ctx
            .new(agent_type_name, constructor, phantom_id, config)
            .await
    }

    async fn invoke_and_await(
        &mut self,
        self_: Resource<WasmRpc>,
        method_name: String,
        input: golem_schema::schema::wit::wire::SchemaValueTree,
    ) -> anyhow::Result<Result<Option<golem_schema::schema::wit::wire::SchemaValueTree>, RpcError>>
    {
        self.durable_ctx
            .invoke_and_await(self_, method_name, input)
            .await
    }

    async fn invoke(
        &mut self,
        self_: Resource<WasmRpc>,
        method_name: String,
        input: golem_schema::schema::wit::wire::SchemaValueTree,
    ) -> anyhow::Result<Result<(), RpcError>> {
        self.durable_ctx.invoke(self_, method_name, input).await
    }

    async fn async_invoke_and_await(
        &mut self,
        self_: Resource<WasmRpc>,
        method_name: String,
        input: golem_schema::schema::wit::wire::SchemaValueTree,
    ) -> anyhow::Result<Resource<FutureInvokeResult>> {
        self.durable_ctx
            .async_invoke_and_await(self_, method_name, input)
            .await
    }

    async fn schedule_invocation(
        &mut self,
        self_: Resource<WasmRpc>,
        scheduled_time: wasmtime_wasi::p2::bindings::clocks::wall_clock::Datetime,
        method_name: String,
        input: golem_schema::schema::wit::wire::SchemaValueTree,
    ) -> anyhow::Result<()> {
        self.durable_ctx
            .schedule_invocation(self_, scheduled_time, method_name, input)
            .await
    }

    async fn schedule_cancelable_invocation(
        &mut self,
        self_: Resource<WasmRpc>,
        scheduled_time: wasmtime_wasi::p2::bindings::clocks::wall_clock::Datetime,
        method_name: String,
        input: golem_schema::schema::wit::wire::SchemaValueTree,
    ) -> anyhow::Result<Resource<CancellationToken>> {
        self.durable_ctx
            .schedule_cancelable_invocation(self_, scheduled_time, method_name, input)
            .await
    }

    async fn drop(&mut self, rep: Resource<WasmRpc>) -> anyhow::Result<()> {
        HostWasmRpc::drop(&mut self.durable_ctx, rep).await
    }
}

impl HostFutureInvokeResult for TestWorkerCtx {
    async fn subscribe(
        &mut self,
        self_: Resource<FutureInvokeResult>,
    ) -> anyhow::Result<Resource<wasmtime_wasi::p2::DynPollable>> {
        HostFutureInvokeResult::subscribe(&mut self.durable_ctx, self_).await
    }

    async fn get(
        &mut self,
        self_: Resource<FutureInvokeResult>,
    ) -> anyhow::Result<
        Option<Result<Option<golem_schema::schema::wit::wire::SchemaValueTree>, RpcError>>,
    > {
        HostFutureInvokeResult::get(&mut self.durable_ctx, self_).await
    }

    async fn cancel(&mut self, self_: Resource<FutureInvokeResult>) -> anyhow::Result<()> {
        HostFutureInvokeResult::cancel(&mut self.durable_ctx, self_).await
    }

    async fn drop(&mut self, rep: Resource<FutureInvokeResult>) -> anyhow::Result<()> {
        HostFutureInvokeResult::drop(&mut self.durable_ctx, rep).await
    }
}

#[async_trait]
impl InvocationContextManagement for TestWorkerCtx {
    async fn start_span(
        &mut self,
        initial_attributes: &[(String, AttributeValue)],
        activate: bool,
    ) -> Result<Arc<InvocationContextSpan>, WorkerExecutorError> {
        self.durable_ctx
            .start_span(initial_attributes, activate)
            .await
    }

    async fn start_child_span(
        &mut self,
        parent: &SpanId,
        initial_attributes: &[(String, AttributeValue)],
    ) -> Result<Arc<InvocationContextSpan>, WorkerExecutorError> {
        self.durable_ctx
            .start_child_span(parent, initial_attributes)
            .await
    }

    fn remove_span(&mut self, span_id: &SpanId) -> Result<(), WorkerExecutorError> {
        self.durable_ctx.remove_span(span_id)
    }

    async fn finish_span(&mut self, span_id: &SpanId) -> Result<(), WorkerExecutorError> {
        self.durable_ctx.finish_span(span_id).await
    }

    async fn set_span_attribute(
        &mut self,
        span_id: &SpanId,
        key: &str,
        value: AttributeValue,
    ) -> Result<(), WorkerExecutorError> {
        self.durable_ctx
            .set_span_attribute(span_id, key, value)
            .await
    }

    fn clone_as_inherited_stack(&self, current_span_id: &SpanId) -> InvocationContextStack {
        self.durable_ctx.clone_as_inherited_stack(current_span_id)
    }
}

#[async_trait]
impl Bootstrap<TestWorkerCtx> for TestServerBootstrap {
    fn create_active_workers(
        &self,
        golem_config: &GolemConfig,
        shutdown_token: tokio_util::sync::CancellationToken,
    ) -> Arc<ActiveWorkers<TestWorkerCtx>> {
        // The in-process test harness shares its process (and RSS) with the test
        // framework and other services, so a process-RSS probe cannot isolate
        // this executor's footprint. When a test pins a memory limit via
        // system_memory_override, give the gate a fixed probe reporting that
        // limit with zero current usage, so admission is decided solely on the
        // granted accounting (exact and process-isolated) against the pinned
        // limit. The usable_ratio (worker_memory_ratio) still applies, matching
        // the pre-gate semaphore pool size of system_memory_override * ratio.
        match golem_config.memory.system_memory_override {
            Some(limit) => Arc::new(ActiveWorkers::new_with_probe(
                Box::new(FixedProbe::new(limit, 0)),
                &golem_config.memory,
                &golem_config.filesystem_storage,
                &golem_config.agent_status_flush,
                shutdown_token,
            )),
            None => Arc::new(ActiveWorkers::new(
                &golem_config.memory,
                &golem_config.filesystem_storage,
                &golem_config.agent_status_flush,
                shutdown_token,
            )),
        }
    }

    fn create_shard_manager_service(
        &self,
        _shard_manager_client: Arc<dyn golem_service_base::clients::shard_manager::ShardManager>,
    ) -> Arc<dyn golem_worker_executor::services::shard_manager::ShardManagerService> {
        Arc::new(golem_worker_executor::services::shard_manager::ShardManagerServiceSingleShard)
    }

    fn create_quota_service(
        &self,
        _shard_manager_client: Arc<dyn golem_service_base::clients::shard_manager::ShardManager>,
        _config: &golem_worker_executor::services::golem_config::QuotaServiceConfig,
        _shutdown_token: tokio_util::sync::CancellationToken,
    ) -> Arc<dyn golem_worker_executor::services::quota::QuotaService> {
        Arc::new(golem_worker_executor::services::quota::UnlimitedQuotaService)
    }

    fn create_environment_state_service(
        &self,
        _config: &EnvironmentStateServiceConfig,
        _registry_service: Arc<dyn RegistryService>,
    ) -> Arc<dyn EnvironmentStateService> {
        match &self.overrides.retry_policies {
            Some(policies) => Arc::new(ConfiguredRetryPoliciesEnvironmentStateService {
                policies: policies.clone(),
            }),
            None => Arc::new(DisabledEnvironmentStateService),
        }
    }

    fn create_component_service(
        &self,
        _golem_config: &GolemConfig,
        _registry_service: Arc<dyn RegistryService>,
        blob_storage: Arc<dyn BlobStorage>,
    ) -> Arc<dyn ComponentService> {
        Arc::new(ComponentServiceLocalFileSystem::new(
            &self.component_service_directory,
            10000,
            Duration::from_secs(3600),
            Arc::new(DefaultCompiledComponentService::new(blob_storage)),
        ))
    }

    fn create_additional_deps(
        &self,
        _registry_service: Arc<dyn RegistryService>,
    ) -> AdditionalTestDeps {
        self.additional_test_deps.clone()
    }

    fn create_direct_invocation_auth_service(
        &self,
        _registry_service: Arc<dyn RegistryService>,
        _golem_config: &GolemConfig,
    ) -> Arc<dyn DirectInvocationAuthService> {
        if let Some(create) = &self.overrides.create_direct_invocation_auth {
            create()
        } else {
            Arc::new(NoOpDirectInvocationAuthService)
        }
    }

    fn create_key_value_service(
        &self,
        key_value_storage: &Arc<dyn KeyValueStorage + Send + Sync>,
    ) -> Arc<dyn KeyValueService> {
        let key_value_service = Arc::new(DefaultKeyValueService::new(key_value_storage.clone()));

        if let Some(wrap) = &self.overrides.wrap_key_value_service {
            wrap(key_value_service)
        } else {
            key_value_service
        }
    }

    fn create_blob_store_service(
        &self,
        blob_storage: &Arc<dyn BlobStorage>,
    ) -> Arc<dyn BlobStoreService> {
        let blob_store_service = Arc::new(DefaultBlobStoreService::new(blob_storage.clone()));

        if let Some(wrap) = &self.overrides.wrap_blob_store_service {
            wrap(blob_store_service)
        } else {
            blob_store_service
        }
    }

    fn create_rdbms_service(
        &self,
        golem_config: &GolemConfig,
        additional_deps: &AdditionalTestDeps,
    ) -> Arc<dyn RdbmsService> {
        Arc::new(TestRdmsService::new(
            Arc::new(rdbms::RdbmsServiceDefault::new(golem_config.rdbms)),
            additional_deps.clone(),
        ))
    }

    fn wrap_rpc(&self, rpc: Arc<dyn Rpc>) -> Arc<dyn Rpc> {
        if let Some(wrap) = &self.overrides.wrap_rpc {
            wrap(rpc)
        } else {
            rpc
        }
    }
}

// -------------------------------------------------------------------------
// Production-Context bootstrap — uses the production Context which has real
// fuel management via FuelTracker, and real ResourceLimiterAsync enforcement
// (memory_growing / table_growing checks against AtomicResourceEntry).
// Used by start_with_fuel_tracking and start_with_table_limit.
// -------------------------------------------------------------------------

struct ProductionContextTestServerBootstrap {
    component_service_directory: PathBuf,
    resource_limits: Arc<dyn ResourceLimits>,
}

#[async_trait]
impl Bootstrap<golem_worker_executor::workerctx::default::Context>
    for ProductionContextTestServerBootstrap
{
    fn create_shard_manager_service(
        &self,
        _shard_manager_client: Arc<dyn golem_service_base::clients::shard_manager::ShardManager>,
    ) -> Arc<dyn golem_worker_executor::services::shard_manager::ShardManagerService> {
        Arc::new(golem_worker_executor::services::shard_manager::ShardManagerServiceSingleShard)
    }

    fn create_quota_service(
        &self,
        _shard_manager_client: Arc<dyn golem_service_base::clients::shard_manager::ShardManager>,
        _config: &golem_worker_executor::services::golem_config::QuotaServiceConfig,
        _shutdown_token: tokio_util::sync::CancellationToken,
    ) -> Arc<dyn golem_worker_executor::services::quota::QuotaService> {
        Arc::new(golem_worker_executor::services::quota::UnlimitedQuotaService)
    }

    fn create_environment_state_service(
        &self,
        _config: &EnvironmentStateServiceConfig,
        _registry_service: Arc<dyn RegistryService>,
    ) -> Arc<dyn EnvironmentStateService> {
        Arc::new(DisabledEnvironmentStateService)
    }

    fn create_component_service(
        &self,
        _golem_config: &GolemConfig,
        _registry_service: Arc<dyn RegistryService>,
        blob_storage: Arc<dyn BlobStorage>,
    ) -> Arc<dyn ComponentService> {
        Arc::new(ComponentServiceLocalFileSystem::new(
            &self.component_service_directory,
            10000,
            Duration::from_secs(3600),
            Arc::new(DefaultCompiledComponentService::new(blob_storage)),
        ))
    }

    fn create_resource_limits(
        &self,
        _golem_config: &GolemConfig,
        _registry_service: Arc<dyn RegistryService>,
        _shutdown_token: tokio_util::sync::CancellationToken,
    ) -> Arc<dyn ResourceLimits> {
        self.resource_limits.clone()
    }

    fn create_additional_deps(
        &self,
        _registry_service: Arc<dyn RegistryService>,
    ) -> NoAdditionalDeps {
        NoAdditionalDeps {}
    }

    fn create_direct_invocation_auth_service(
        &self,
        _registry_service: Arc<dyn RegistryService>,
        _golem_config: &GolemConfig,
    ) -> Arc<dyn DirectInvocationAuthService> {
        Arc::new(NoOpDirectInvocationAuthService)
    }

    fn create_wasmtime_linker(
        &self,
        engine: &Engine,
    ) -> anyhow::Result<Linker<golem_worker_executor::workerctx::default::Context>> {
        use golem_worker_executor::workerctx::default::Context;
        let mut linker = golem_worker_executor::wasi_host::create_linker(
            engine,
            <Context as DurableWorkerCtxView<Context>>::durable_ctx_mut,
        )?;
        golem_api_1_x::host::add_to_linker::<_, HasSelf<DurableWorkerCtx<Context>>>(
            &mut linker,
            <Context as DurableWorkerCtxView<Context>>::durable_ctx_mut,
        )?;
        golem_api_1_x::retry::add_to_linker::<_, HasSelf<DurableWorkerCtx<Context>>>(
            &mut linker,
            <Context as DurableWorkerCtxView<Context>>::durable_ctx_mut,
        )?;
        golem_api_1_x::oplog::add_to_linker::<_, HasSelf<DurableWorkerCtx<Context>>>(
            &mut linker,
            <Context as DurableWorkerCtxView<Context>>::durable_ctx_mut,
        )?;
        golem_api_1_x::context::add_to_linker::<_, HasSelf<DurableWorkerCtx<Context>>>(
            &mut linker,
            <Context as DurableWorkerCtxView<Context>>::durable_ctx_mut,
        )?;
        golem_durability::durability::add_to_linker::<_, HasSelf<DurableWorkerCtx<Context>>>(
            &mut linker,
            <Context as DurableWorkerCtxView<Context>>::durable_ctx_mut,
        )?;
        golem_worker_executor::preview2::golem::agent::host::add_to_linker::<
            _,
            HasSelf<DurableWorkerCtx<Context>>,
        >(
            &mut linker,
            <Context as DurableWorkerCtxView<Context>>::durable_ctx_mut,
        )?;
        golem_schema::schema::wit::wire::add_to_linker::<_, HasSelf<DurableWorkerCtx<Context>>>(
            &mut linker,
            <Context as DurableWorkerCtxView<Context>>::durable_ctx_mut,
        )?;
        Ok(linker)
    }
}

/// A `ResourceLimits` implementation that provides a fixed table element limit
/// while keeping fuel and memory unlimited. Used by table-limit tests.
struct FixedTableLimitResourceLimits {
    max_table_elements: usize,
}

#[async_trait]
impl ResourceLimits for FixedTableLimitResourceLimits {
    async fn initialize_account(
        &self,
        _account_id: golem_common::model::account::AccountId,
    ) -> Result<
        Arc<AtomicResourceEntry>,
        golem_service_base::error::worker_executor::WorkerExecutorError,
    > {
        Ok(Arc::new(AtomicResourceEntry::new(
            u64::MAX,
            usize::MAX,
            self.max_table_elements,
            u64::MAX,
            u64::MAX,
        )))
    }
}

fn make_production_context_config(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
) -> GolemConfig {
    let mut config = make_base_test_config(deps);
    apply_sqlite_storage_config(&mut config, deps, context);
    config
}

async fn run_production_context_bootstrap(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    resource_limits: Arc<dyn ResourceLimits>,
    timeout_msg: &'static str,
) -> anyhow::Result<TestWorkerExecutor> {
    let prometheus = golem_worker_executor::metrics::register_all();
    let config = make_production_context_config(deps, context);

    let handle = tokio::runtime::Handle::current();
    let mut join_set = tokio::task::JoinSet::new();

    let details = bootstrap_and_run_worker_executor(
        &ProductionContextTestServerBootstrap {
            component_service_directory: deps.component_service_directory.clone(),
            resource_limits,
        },
        config,
        prometheus,
        handle,
        &mut join_set,
        false,
    )
    .await?;

    let grpc_port = details.grpc_port;
    let leak_detector = details.leak_detector.clone();
    let details = Arc::new(details);

    let start = std::time::Instant::now();
    loop {
        let channel =
            tonic::transport::Channel::from_shared(format!("http://127.0.0.1:{grpc_port}"))
                .expect("Valid URI")
                .connect()
                .await;

        if let Ok(channel) = channel {
            let otel_channel = tower::ServiceBuilder::new()
                .layer(tonic_tracing_opentelemetry::middleware::client::OtelGrpcLayer)
                .service(channel);
            let client = WorkerExecutorClient::new(otel_channel)
                .max_decoding_message_size(32 * 1024 * 1024)
                .max_encoding_message_size(32 * 1024 * 1024);
            return Ok(TestWorkerExecutor {
                _join_set: Arc::new(join_set),
                _run_details: details,
                deps: deps.clone(),
                client,
                context: context.clone(),
                // Production-context bootstrap path uses the real `NoAdditionalDeps`
                // worker context, not `TestWorkerCtx`, so the worker-inspection
                // helpers do not apply here. We hand the executor a fresh, empty
                // `AdditionalTestDeps` purely to satisfy the field; calling
                // `worker_is_loaded` / `worker_eviction_class` / `worker_memory_requirement`
                // on this path will report "no worker" because no `ActiveWorkers`
                // handle was ever captured.
                additional_test_deps: AdditionalTestDeps::new(),
                leak_detector,
            });
        } else if start.elapsed().as_secs() > 10 {
            return Err(anyhow::anyhow!(timeout_msg));
        }
    }
}

/// Starts a worker executor that uses the production [`Context`] worker context,
/// which has real fuel management via [`FuelTracker`] and [`AtomicResourceEntry`].
/// Use this in tests that need to verify actual fuel consumption behaviour.
pub async fn start_with_fuel_tracking(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
) -> anyhow::Result<TestWorkerExecutor> {
    // ResourceLimitsDisabled gives unlimited fuel budget so workers are never
    // suspended, allowing fuel consumption to be measured freely.
    start_with_resource_limits(deps, context, Arc::new(ResourceLimitsDisabled)).await
}

/// Starts a worker executor that uses the production [`Context`] with a custom
/// [`ResourceLimits`] implementation. Useful for tests that need to observe
/// per-account resource initialization behaviour.
pub async fn start_with_resource_limits(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    resource_limits: Arc<dyn ResourceLimits>,
) -> anyhow::Result<TestWorkerExecutor> {
    run_production_context_bootstrap(
        deps,
        context,
        resource_limits,
        "Timeout waiting for custom-resource-limits server to start",
    )
    .await
}

/// Starts a worker executor that uses the production [`Context`] worker context,
/// with a specific function table element limit enforced via `table_growing`.
///
/// The resource limiter is configured with `max_table_elements` as the limit;
/// fuel and memory remain unlimited (using `u64::MAX` / `usize::MAX`).
pub async fn start_with_table_limit(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    max_table_elements: usize,
) -> anyhow::Result<TestWorkerExecutor> {
    run_production_context_bootstrap(
        deps,
        context,
        Arc::new(FixedTableLimitResourceLimits { max_table_elements }),
        "Timeout waiting for table-limit server to start",
    )
    .await
}

/// A `ResourceLimits` implementation that provides a fixed per-executor
/// concurrent agent limit while keeping all other limits unlimited.
/// Used by concurrent agent limit tests.
struct FixedConcurrentAgentLimitResourceLimits {
    max_concurrent_agents_per_executor: u64,
}

#[async_trait]
impl ResourceLimits for FixedConcurrentAgentLimitResourceLimits {
    async fn initialize_account(
        &self,
        _account_id: golem_common::model::account::AccountId,
    ) -> Result<
        Arc<AtomicResourceEntry>,
        golem_service_base::error::worker_executor::WorkerExecutorError,
    > {
        Ok(Arc::new(AtomicResourceEntry::new(
            u64::MAX,
            usize::MAX,
            usize::MAX,
            u64::MAX,
            self.max_concurrent_agents_per_executor,
        )))
    }
}

/// Starts a worker executor with a per-executor concurrent agent limit.
///
/// All agents running on this executor count against the per-account limit.
/// When the limit is reached, new agents wait until a running agent stops
/// or an idle agent is evicted.
pub async fn start_with_concurrent_agent_limit(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    max_concurrent_agents: u64,
) -> anyhow::Result<TestWorkerExecutor> {
    run_production_context_bootstrap(
        deps,
        context,
        Arc::new(FixedConcurrentAgentLimitResourceLimits {
            max_concurrent_agents_per_executor: max_concurrent_agents,
        }),
        "Timeout waiting for concurrent-agent-limit server to start",
    )
    .await
}

/// A `ResourceLimits` implementation that provides a fixed per-worker disk
/// space limit while keeping fuel, memory, and table elements unlimited.
/// Used by storage quota tests.
struct FixedFilesystemStorageQuotaResourceLimits {
    max_disk_space_bytes: u64,
}

#[async_trait]
impl ResourceLimits for FixedFilesystemStorageQuotaResourceLimits {
    async fn initialize_account(
        &self,
        _account_id: golem_common::model::account::AccountId,
    ) -> Result<
        Arc<AtomicResourceEntry>,
        golem_service_base::error::worker_executor::WorkerExecutorError,
    > {
        Ok(Arc::new(AtomicResourceEntry::new(
            u64::MAX,
            usize::MAX,
            usize::MAX,
            self.max_disk_space_bytes,
            u64::MAX,
        )))
    }
}

/// Starts a worker executor with a per-agent plan-level storage limit.
///
/// Uses the production [`Context`] so that `check_filesystem_quota` enforces
/// `max_disk_space_bytes` against each agent's `current_filesystem_storage_usage`.
/// Exceeding it returns `WorkerAgentExceededFilesystemStorageLimit` (permanent, not retried).
/// The executor-wide semaphore pool is left unlimited (10 GB default).
pub async fn start_with_agent_storage_quota(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    max_disk_space_bytes: u64,
) -> anyhow::Result<TestWorkerExecutor> {
    run_production_context_bootstrap(
        deps,
        context,
        Arc::new(FixedFilesystemStorageQuotaResourceLimits {
            max_disk_space_bytes,
        }),
        "Timeout waiting for agent-storage-quota server to start",
    )
    .await
}

/// A `ResourceLimits` implementation that enforces fixed per-invocation HTTP and RPC
/// call limits while keeping fuel, memory, and table elements unlimited.
/// Used by per-invocation call-limit tests.
struct FixedInvocationLimitResourceLimits {
    per_invocation_http_call_limit: u64,
    per_invocation_rpc_call_limit: u64,
}

#[async_trait]
impl ResourceLimits for FixedInvocationLimitResourceLimits {
    async fn initialize_account(
        &self,
        _account_id: golem_common::model::account::AccountId,
    ) -> Result<
        Arc<AtomicResourceEntry>,
        golem_service_base::error::worker_executor::WorkerExecutorError,
    > {
        Ok(Arc::new(AtomicResourceEntry::new_with_invocation_limits(
            u64::MAX,
            usize::MAX,
            usize::MAX,
            u64::MAX,
            self.per_invocation_http_call_limit,
            self.per_invocation_rpc_call_limit,
        )))
    }
}

/// Starts a worker executor that uses the production [`Context`] worker context,
/// with specific per-invocation HTTP and RPC call limits enforced.
///
/// Fuel, memory, and table elements remain unlimited.
pub async fn start_with_invocation_limits(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    per_invocation_http_call_limit: u64,
    per_invocation_rpc_call_limit: u64,
) -> anyhow::Result<TestWorkerExecutor> {
    run_production_context_bootstrap(
        deps,
        context,
        Arc::new(FixedInvocationLimitResourceLimits {
            per_invocation_http_call_limit,
            per_invocation_rpc_call_limit,
        }),
        "Timeout waiting for invocation-limit server to start",
    )
    .await
}

/// A `ResourceLimits` implementation that enforces fixed monthly account-level
/// HTTP and RPC call budgets while keeping everything else unlimited.
struct FixedMonthlyCallLimitResourceLimits {
    monthly_http_calls: u64,
    monthly_rpc_calls: u64,
}

#[async_trait]
impl ResourceLimits for FixedMonthlyCallLimitResourceLimits {
    async fn initialize_account(
        &self,
        _account_id: golem_common::model::account::AccountId,
    ) -> Result<
        Arc<AtomicResourceEntry>,
        golem_service_base::error::worker_executor::WorkerExecutorError,
    > {
        Ok(Arc::new(AtomicResourceEntry::new_with_all_limits(
            u64::MAX,
            usize::MAX,
            usize::MAX,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            self.monthly_http_calls,
            self.monthly_rpc_calls,
            AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
            AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
        )))
    }
}

/// Starts a worker executor that uses the production [`Context`] worker context,
/// with specific monthly account-level HTTP and RPC call budgets enforced.
///
/// When the budget is exhausted the worker is suspended (not trapped) — the same
/// mechanism as fuel exhaustion. Per-invocation limits, fuel, memory, and disk
/// remain unlimited.
pub async fn start_with_monthly_call_limits(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    monthly_http_calls: u64,
    monthly_rpc_calls: u64,
) -> anyhow::Result<TestWorkerExecutor> {
    run_production_context_bootstrap(
        deps,
        context,
        Arc::new(FixedMonthlyCallLimitResourceLimits {
            monthly_http_calls,
            monthly_rpc_calls,
        }),
        "Timeout waiting for monthly-call-limit server to start",
    )
    .await
}

/// Starts a worker executor with a constrained executor-wide storage pool.
///
/// The pool is shared across all agents on the node. Uses `TestWorkerCtx`
/// (no per-agent plan limit). Exhausting the pool returns `NodeOutOfFilesystemStorage`
/// (retriable). Use this to test node-level storage pressure and eviction.
pub async fn start_with_executor_storage_pool(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    pool_bytes: u64,
) -> anyhow::Result<TestWorkerExecutor> {
    start_customized(
        deps,
        context,
        None,
        Some(pool_bytes),
        None,
        None,
        None,
        None,
    )
    .await
}

#[derive(Clone)]
struct TestOplog {
    owned_agent_id: OwnedAgentId,
    oplog: Arc<dyn Oplog>,
    additional_test_deps: AdditionalTestDeps,
}

impl TestOplog {
    fn new(
        owned_agent_id: OwnedAgentId,
        oplog: Arc<dyn Oplog>,
        additional_test_deps: AdditionalTestDeps,
    ) -> Self {
        Self {
            owned_agent_id,
            oplog,
            additional_test_deps,
        }
    }

    async fn check_oplog_add(&self, entry: &OplogEntry) -> Result<(), String> {
        let entry_name = match entry {
            OplogEntry::BeginRemoteTransaction { .. } => "BeginRemoteTransaction",
            OplogEntry::PreRollbackRemoteTransaction { .. } => "PreRollbackRemoteTransaction",
            OplogEntry::PreCommitRemoteTransaction { .. } => "PreCommitRemoteTransaction",
            OplogEntry::CommittedRemoteTransaction { .. } => "CommittedRemoteTransaction",
            OplogEntry::RolledBackRemoteTransaction { .. } => "RolledBackRemoteTransaction",
            OplogEntry::Start { .. } => "Start",
            OplogEntry::End { .. } => "End",
            OplogEntry::Cancelled { .. } => "Cancelled",
            _ => "Other",
        };

        // FailOplogAdd{times}On{entry}
        let re = Regex::new(r"FailOplogAdd(\d+)On([A-Za-z]+)").unwrap();

        let agent_name = self.owned_agent_id.agent_id.agent_id.as_str();
        if let Some(captures) = re.captures(agent_name) {
            let times = &captures[1].parse::<usize>().unwrap_or_default();
            let entry = &captures[2];
            if entry == entry_name {
                let failed_before = self
                    .additional_test_deps
                    .get_oplog_failures_count(
                        self.owned_agent_id.agent_id.clone(),
                        entry_name.to_string(),
                    )
                    .await;

                if failed_before >= *times {
                    Ok(())
                } else {
                    self.additional_test_deps
                        .add_oplog_failure(
                            self.owned_agent_id.agent_id.clone(),
                            entry_name.to_string(),
                        )
                        .await;

                    info!("Failing worker as it hit marked oplog entry");

                    Err(format!(
                        "worker {agent_name} failed on {entry_name} {} times",
                        failed_before + 1
                    ))
                }
            } else {
                Ok(())
            }
        } else {
            Ok(())
        }
    }
}

#[async_trait]
impl Oplog for TestOplog {
    async fn add(&self, entry: OplogEntry) -> OplogIndex {
        self.oplog.add(entry).await
    }

    async fn fallible_add(&self, entry: OplogEntry) -> Result<(), String> {
        self.check_oplog_add(&entry).await?;
        self.oplog.fallible_add(entry).await
    }

    async fn fallible_add_pair(
        &self,
        first: OplogEntry,
        second: OplogEntry,
    ) -> Result<(OplogIndex, OplogIndex), String> {
        self.check_oplog_add(&first).await?;
        self.check_oplog_add(&second).await?;
        self.oplog.fallible_add_pair(first, second).await
    }

    async fn drop_prefix(&self, last_dropped_id: OplogIndex) -> u64 {
        self.oplog.drop_prefix(last_dropped_id).await
    }

    async fn commit(&self, level: CommitLevel) -> BTreeMap<OplogIndex, OplogEntry> {
        self.oplog.commit(level).await
    }

    async fn current_oplog_index(&self) -> OplogIndex {
        self.oplog.current_oplog_index().await
    }

    async fn last_added_non_hint_entry(&self) -> Option<OplogIndex> {
        self.oplog.last_added_non_hint_entry().await
    }

    async fn wait_for_replicas(&self, replicas: u8, timeout: Duration) -> bool {
        self.oplog.wait_for_replicas(replicas, timeout).await
    }

    async fn read(&self, oplog_index: OplogIndex) -> OplogEntry {
        self.oplog.read(oplog_index).await
    }

    async fn read_many(&self, oplog_index: OplogIndex, n: u64) -> BTreeMap<OplogIndex, OplogEntry> {
        self.oplog.read_many(oplog_index, n).await
    }

    async fn length(&self) -> u64 {
        self.oplog.length().await
    }

    async fn upload_raw_payload(&self, data: Vec<u8>) -> Result<RawOplogPayload, String> {
        self.oplog.upload_raw_payload(data).await
    }

    async fn download_raw_payload(
        &self,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        self.oplog.download_raw_payload(payload_id, md5_hash).await
    }

    async fn switch_persistence_level(&self, mode: PersistenceLevel) {
        self.oplog.switch_persistence_level(mode).await;
    }

    async fn add_pair(
        &self,
        start: OplogEntry,
        make_second: Box<dyn FnOnce(OplogIndex) -> OplogEntry + Send>,
    ) -> (OplogIndex, OplogIndex) {
        self.oplog.add_pair(start, make_second).await
    }

    fn inner(&self) -> Option<Arc<dyn Oplog>> {
        Some(self.oplog.clone())
    }
}

impl Debug for TestOplog {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.oplog)
    }
}

#[derive(Clone)]
struct TestRdmsService {
    ignite: Arc<dyn Rdbms<IgniteType> + Send + Sync>,
    mysql: Arc<dyn Rdbms<MysqlType> + Send + Sync>,
    postgres: Arc<dyn Rdbms<PostgresType> + Send + Sync>,
}

impl TestRdmsService {
    fn new(rdbms: Arc<dyn rdbms::RdbmsService>, additional_test_deps: AdditionalTestDeps) -> Self {
        let ignite: Arc<dyn Rdbms<IgniteType> + Send + Sync> =
            Arc::new(TestRdms::new(rdbms.ignite(), additional_test_deps.clone()));
        let mysql: Arc<dyn Rdbms<MysqlType> + Send + Sync> =
            Arc::new(TestRdms::new(rdbms.mysql(), additional_test_deps.clone()));
        let postgres: Arc<dyn Rdbms<PostgresType> + Send + Sync> = Arc::new(TestRdms::new(
            rdbms.postgres(),
            additional_test_deps.clone(),
        ));
        Self {
            ignite,
            mysql,
            postgres,
        }
    }
}

impl rdbms::RdbmsService for TestRdmsService {
    fn ignite(&self) -> Arc<dyn Rdbms<IgniteType>> {
        self.ignite.clone()
    }

    fn mysql(&self) -> Arc<dyn Rdbms<MysqlType>> {
        self.mysql.clone()
    }

    fn postgres(&self) -> Arc<dyn Rdbms<PostgresType>> {
        self.postgres.clone()
    }
}

#[derive(Clone)]
struct TestRdms<T: RdbmsType> {
    rdbms: Arc<dyn Rdbms<T>>,
    additional_test_deps: AdditionalTestDeps,
}

impl<T: RdbmsType> TestRdms<T> {
    fn new(rdbms: Arc<dyn Rdbms<T>>, additional_test_deps: AdditionalTestDeps) -> Self {
        Self {
            rdbms,
            additional_test_deps,
        }
    }

    async fn check_rdbms_tx(
        &self,
        agent_id: &AgentId,
        entry_name: &str,
    ) -> Result<(), rdbms::RdbmsError> {
        // FailRdbmsTx{times}On{entry}
        let re = Regex::new(r"FailRdbmsTx(\d+)On([A-Za-z]+)").unwrap();

        let agent_name = agent_id.agent_id.as_str();
        if let Some(captures) = re.captures(agent_name) {
            let times = &captures[1].parse::<usize>().unwrap_or_default();
            let entry = &captures[2];
            if entry == entry_name {
                let failed_before = self
                    .additional_test_deps
                    .get_rdbms_tx_failures_count(agent_id.clone(), entry_name.to_string())
                    .await;

                if failed_before >= *times {
                    Ok(())
                } else {
                    self.additional_test_deps
                        .add_rdbms_tx_failure(agent_id.clone(), entry_name.to_string())
                        .await;
                    Err(rdbms::RdbmsError::Other(format!(
                        "worker {} failed on {} {} times",
                        agent_name,
                        entry_name,
                        failed_before + 1
                    )))
                }
            } else {
                Ok(())
            }
        } else {
            Ok(())
        }
    }
}

#[async_trait]
impl<T: RdbmsType> Rdbms<T> for TestRdms<T> {
    async fn create(
        &self,
        address: &str,
        agent_id: &AgentId,
    ) -> Result<RdbmsPoolKey, rdbms::RdbmsError> {
        self.rdbms.create(address, agent_id).await
    }

    async fn exists(&self, key: &RdbmsPoolKey, agent_id: &AgentId) -> bool {
        self.rdbms.exists(key, agent_id).await
    }

    async fn remove(&self, key: &RdbmsPoolKey, agent_id: &AgentId) -> bool {
        self.rdbms.remove(key, agent_id).await
    }

    async fn execute(
        &self,
        key: &RdbmsPoolKey,
        agent_id: &AgentId,
        statement: &str,
        params: Vec<T::DbValue>,
    ) -> Result<u64, rdbms::RdbmsError>
    where
        <T as RdbmsType>::DbValue: 'async_trait,
    {
        self.rdbms.execute(key, agent_id, statement, params).await
    }

    async fn query_stream(
        &self,
        key: &RdbmsPoolKey,
        agent_id: &AgentId,
        statement: &str,
        params: Vec<T::DbValue>,
    ) -> Result<Arc<dyn DbResultStream<T> + Send + Sync>, rdbms::RdbmsError>
    where
        <T as RdbmsType>::DbValue: 'async_trait,
    {
        self.rdbms
            .query_stream(key, agent_id, statement, params)
            .await
    }

    async fn query(
        &self,
        key: &RdbmsPoolKey,
        agent_id: &AgentId,
        statement: &str,
        params: Vec<T::DbValue>,
    ) -> Result<DbResult<T>, rdbms::RdbmsError>
    where
        <T as RdbmsType>::DbValue: 'async_trait,
    {
        self.rdbms.query(key, agent_id, statement, params).await
    }

    async fn begin_transaction(
        &self,
        key: &RdbmsPoolKey,
        agent_id: &AgentId,
    ) -> Result<Arc<dyn DbTransaction<T> + Send + Sync>, rdbms::RdbmsError> {
        self.check_rdbms_tx(agent_id, "BeginTransaction").await?;
        self.rdbms.begin_transaction(key, agent_id).await
    }

    async fn get_transaction_status(
        &self,
        key: &RdbmsPoolKey,
        agent_id: &AgentId,
        transaction_id: &TransactionId,
    ) -> Result<RdbmsTransactionStatus, rdbms::RdbmsError> {
        let r = self
            .check_rdbms_tx(agent_id, "GetTransactionStatusNotFound")
            .await;
        if r.is_err() {
            Ok(RdbmsTransactionStatus::NotFound)
        } else {
            self.rdbms
                .get_transaction_status(key, agent_id, transaction_id)
                .await
        }
    }

    async fn cleanup_transaction(
        &self,
        key: &RdbmsPoolKey,
        agent_id: &AgentId,
        transaction_id: &TransactionId,
    ) -> Result<(), rdbms::RdbmsError> {
        self.check_rdbms_tx(agent_id, "CleanupTransaction").await?;
        self.rdbms
            .cleanup_transaction(key, agent_id, transaction_id)
            .await
    }

    async fn status(&self) -> RdbmsStatus {
        self.rdbms.status().await
    }
}

#[derive(Clone)]
pub struct AdditionalTestDeps {
    oplog_failures: Arc<scc::HashMap<AgentId, scc::HashMap<String, usize>>>,
    rdbms_tx_failures: Arc<scc::HashMap<AgentId, scc::HashMap<String, usize>>>,
    /// Captured once on first call to [`TestWorkerCtx::create`]. Used by the
    /// read-only test helpers (`worker_is_loaded`,
    /// `worker_eviction_class`, `worker_memory_requirement`) to observe
    /// live `Worker` state for the memory-pressure-driven eviction test
    /// (issue #3393 T5).
    active_workers: Arc<std::sync::OnceLock<Arc<ActiveWorkers<TestWorkerCtx>>>>,
}

impl Default for AdditionalTestDeps {
    fn default() -> Self {
        Self::new()
    }
}

impl AdditionalTestDeps {
    pub fn new() -> Self {
        let oplog_failures = Arc::new(scc::HashMap::new());
        let rdbms_tx_failures = Arc::new(scc::HashMap::new());
        Self {
            oplog_failures,
            rdbms_tx_failures,
            active_workers: Arc::new(std::sync::OnceLock::new()),
        }
    }

    /// Stores the executor's `ActiveWorkers` registry on first call. Subsequent
    /// calls are no-ops because they all carry the same `Arc`.
    pub(crate) fn set_active_workers(&self, workers: Arc<ActiveWorkers<TestWorkerCtx>>) {
        let _ = self.active_workers.set(workers);
    }

    /// Look up a `Worker` shell currently registered in `ActiveWorkers`.
    /// Returns `None` if the executor has not loaded any worker yet (handle
    /// not captured), or if no `Worker` for `owned_agent_id` is currently
    /// resident.
    pub(crate) async fn try_get_worker(
        &self,
        owned_agent_id: &OwnedAgentId,
    ) -> Option<Arc<Worker<TestWorkerCtx>>> {
        self.active_workers.get()?.try_get(owned_agent_id).await
    }

    pub async fn get_oplog_failures_count(&self, agent_id: AgentId, entry: String) -> usize {
        let inner = self.oplog_failures.get_async(&agent_id).await;
        if let Some(inner) = inner {
            inner
                .read_async(&entry, |_, v| *v)
                .await
                .unwrap_or_default()
        } else {
            0
        }
    }

    pub async fn add_oplog_failure(&self, agent_id: AgentId, entry: String) {
        let inner = self.oplog_failures.entry_async(agent_id).await.or_default();

        *inner.entry_async(entry).await.or_default().get_mut() += 1;
    }

    pub async fn get_rdbms_tx_failures_count(&self, agent_id: AgentId, entry: String) -> usize {
        let inner = self.rdbms_tx_failures.get_async(&agent_id).await;

        if let Some(inner) = inner {
            inner
                .read_async(&entry, |_, v| *v)
                .await
                .unwrap_or_default()
        } else {
            0
        }
    }

    pub async fn add_rdbms_tx_failure(&self, agent_id: AgentId, entry: String) {
        let inner = self
            .rdbms_tx_failures
            .entry_async(agent_id)
            .await
            .or_default();

        *inner.entry_async(entry).await.or_default().get_mut() += 1;
    }
}

pub struct FailingKeyValueService {
    inner: Arc<dyn KeyValueService>,
    remaining_failures: AtomicU32,
    remaining_set_failures: AtomicU32,
}

impl FailingKeyValueService {
    pub fn new(inner: Arc<dyn KeyValueService>, failure_count: u32) -> Self {
        Self {
            inner,
            remaining_failures: AtomicU32::new(failure_count),
            remaining_set_failures: AtomicU32::new(0),
        }
    }

    pub fn with_set_failures(inner: Arc<dyn KeyValueService>, set_failure_count: u32) -> Self {
        Self {
            inner,
            remaining_failures: AtomicU32::new(0),
            remaining_set_failures: AtomicU32::new(set_failure_count),
        }
    }
}

#[async_trait]
impl KeyValueService for FailingKeyValueService {
    async fn delete(
        &self,
        environment_id: EnvironmentId,
        bucket: String,
        key: String,
    ) -> anyhow::Result<()> {
        self.inner.delete(environment_id, bucket, key).await
    }

    async fn delete_many(
        &self,
        environment_id: EnvironmentId,
        bucket: String,
        keys: Vec<String>,
    ) -> anyhow::Result<()> {
        self.inner.delete_many(environment_id, bucket, keys).await
    }

    async fn exists(
        &self,
        environment_id: EnvironmentId,
        bucket: String,
        key: String,
    ) -> anyhow::Result<bool> {
        self.inner.exists(environment_id, bucket, key).await
    }

    async fn get(
        &self,
        environment_id: EnvironmentId,
        bucket: String,
        key: String,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        if self
            .remaining_failures
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
        {
            Err(anyhow!("transient test failure"))
        } else {
            self.inner.get(environment_id, bucket, key).await
        }
    }

    async fn get_keys(
        &self,
        environment_id: EnvironmentId,
        bucket: String,
    ) -> anyhow::Result<Vec<String>> {
        self.inner.get_keys(environment_id, bucket).await
    }

    async fn get_many(
        &self,
        environment_id: EnvironmentId,
        bucket: String,
        keys: Vec<String>,
    ) -> anyhow::Result<Vec<Option<Vec<u8>>>> {
        self.inner.get_many(environment_id, bucket, keys).await
    }

    async fn set(
        &self,
        environment_id: EnvironmentId,
        bucket: String,
        key: String,
        outgoing_value: Vec<u8>,
    ) -> anyhow::Result<()> {
        if self
            .remaining_set_failures
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
        {
            Err(anyhow!("transient test failure"))
        } else {
            self.inner
                .set(environment_id, bucket, key, outgoing_value)
                .await
        }
    }

    async fn set_many(
        &self,
        environment_id: EnvironmentId,
        bucket: String,
        key_values: Vec<(String, Vec<u8>)>,
    ) -> anyhow::Result<()> {
        self.inner
            .set_many(environment_id, bucket, key_values)
            .await
    }
}

pub struct FailingBlobStoreService {
    inner: Arc<dyn BlobStoreService>,
    remaining_failures: AtomicU32,
}

impl FailingBlobStoreService {
    pub fn new(inner: Arc<dyn BlobStoreService>, failure_count: u32) -> Self {
        Self {
            inner,
            remaining_failures: AtomicU32::new(failure_count),
        }
    }
}

#[async_trait]
impl BlobStoreService for FailingBlobStoreService {
    async fn clear(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<(), BlobStoreError> {
        self.inner.clear(environment_id, container_name).await
    }

    async fn container_exists(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<bool, BlobStoreError> {
        self.inner
            .container_exists(environment_id, container_name)
            .await
    }

    async fn copy_object(
        &self,
        environment_id: EnvironmentId,
        source_container_name: String,
        source_object_name: String,
        destination_container_name: String,
        destination_object_name: String,
    ) -> Result<(), BlobStoreError> {
        self.inner
            .copy_object(
                environment_id,
                source_container_name,
                source_object_name,
                destination_container_name,
                destination_object_name,
            )
            .await
    }

    async fn create_container(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<(), BlobStoreError> {
        self.inner
            .create_container(environment_id, container_name)
            .await
    }

    async fn delete_container(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<(), BlobStoreError> {
        self.inner
            .delete_container(environment_id, container_name)
            .await
    }

    async fn delete_object(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
        object_name: String,
    ) -> Result<(), BlobStoreError> {
        self.inner
            .delete_object(environment_id, container_name, object_name)
            .await
    }

    async fn delete_objects(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
        object_names: Vec<String>,
    ) -> Result<(), BlobStoreError> {
        self.inner
            .delete_objects(environment_id, container_name, object_names)
            .await
    }

    async fn get_container(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<Option<u64>, BlobStoreError> {
        self.inner
            .get_container(environment_id, container_name)
            .await
    }

    async fn get_data(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
        object_name: String,
        start: u64,
        end: u64,
    ) -> Result<Vec<u8>, BlobStoreError> {
        if self
            .remaining_failures
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
        {
            Err(BlobStoreError::TransientBackend(
                "transient test failure".to_string(),
            ))
        } else {
            self.inner
                .get_data(environment_id, container_name, object_name, start, end)
                .await
        }
    }

    async fn has_object(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
        object_name: String,
    ) -> Result<bool, BlobStoreError> {
        self.inner
            .has_object(environment_id, container_name, object_name)
            .await
    }

    async fn list_objects(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<Vec<String>, BlobStoreError> {
        self.inner
            .list_objects(environment_id, container_name)
            .await
    }

    async fn move_object(
        &self,
        environment_id: EnvironmentId,
        source_container_name: String,
        source_object_name: String,
        destination_container_name: String,
        destination_object_name: String,
    ) -> Result<(), BlobStoreError> {
        self.inner
            .move_object(
                environment_id,
                source_container_name,
                source_object_name,
                destination_container_name,
                destination_object_name,
            )
            .await
    }

    async fn object_info(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
        object_name: String,
    ) -> Result<ObjectMetadata, BlobStoreError> {
        self.inner
            .object_info(environment_id, container_name, object_name)
            .await
    }

    async fn write_data(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
        object_name: String,
        data: Vec<u8>,
    ) -> Result<(), BlobStoreError> {
        self.inner
            .write_data(environment_id, container_name, object_name, data)
            .await
    }
}

pub struct FailingRpc {
    inner: Arc<dyn Rpc>,
    remaining_failures: AtomicU32,
}

impl FailingRpc {
    pub fn new(inner: Arc<dyn Rpc>, failure_count: u32) -> Self {
        Self {
            inner,
            remaining_failures: AtomicU32::new(failure_count),
        }
    }
}

#[async_trait]
impl Rpc for FailingRpc {
    async fn create_demand(
        &self,
        owned_agent_id: &OwnedAgentId,
        self_created_by: AccountId,
        self_agent_id: &AgentId,
        self_env: &[(String, String)],
        self_stack: InvocationContextStack,
        config: Vec<AgentConfigEntryDto>,
        auth_ctx: &AuthCtx,
    ) -> Result<Box<dyn RpcDemand>, ServiceRpcError> {
        self.inner
            .create_demand(
                owned_agent_id,
                self_created_by,
                self_agent_id,
                self_env,
                self_stack,
                config,
                auth_ctx,
            )
            .await
    }

    async fn invoke_and_await(
        &self,
        owned_agent_id: &OwnedAgentId,
        idempotency_key: Option<IdempotencyKey>,
        method_name: String,
        method_parameters: SchemaValue,
        self_created_by: AccountId,
        self_agent_id: &AgentId,
        self_env: &[(String, String)],
        self_stack: InvocationContextStack,
        auth_ctx: &AuthCtx,
    ) -> Result<SchemaValue, ServiceRpcError> {
        if self
            .remaining_failures
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
        {
            Err(ServiceRpcError::RemoteInternalError {
                details: "transient test failure".to_string(),
            })
        } else {
            self.inner
                .invoke_and_await(
                    owned_agent_id,
                    idempotency_key,
                    method_name,
                    method_parameters,
                    self_created_by,
                    self_agent_id,
                    self_env,
                    self_stack,
                    auth_ctx,
                )
                .await
        }
    }

    async fn invoke(
        &self,
        owned_agent_id: &OwnedAgentId,
        idempotency_key: Option<IdempotencyKey>,
        method_name: String,
        method_parameters: SchemaValue,
        self_created_by: AccountId,
        self_agent_id: &AgentId,
        self_env: &[(String, String)],
        self_stack: InvocationContextStack,
        auth_ctx: &AuthCtx,
    ) -> Result<(), ServiceRpcError> {
        self.inner
            .invoke(
                owned_agent_id,
                idempotency_key,
                method_name,
                method_parameters,
                self_created_by,
                self_agent_id,
                self_env,
                self_stack,
                auth_ctx,
            )
            .await
    }
}
