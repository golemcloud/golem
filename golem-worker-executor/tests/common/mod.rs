use anyhow::Error;
use async_trait::async_trait;
use golem_service_base::service::plugin_wasm_files::PluginWasmFilesService;
use std::collections::HashSet;

use golem_service_base::service::initial_component_files::InitialComponentFilesService;
use golem_service_base::storage::blob::BlobStorage;
use golem_wasm_rpc::wasmtime::ResourceStore;
use golem_wasm_rpc::{HostWasmRpc, RpcError, Uri, Value, WitValue};
use golem_worker_executor::services::file_loader::FileLoader;
use prometheus::Registry;

use crate::{LastUniqueId, WorkerExecutorPerTestDependencies, WorkerExecutorTestDependencies};
use golem_api_grpc::proto::golem::workerexecutor::v1::worker_executor_client::WorkerExecutorClient;
use golem_common::model::{
    AccountId, ComponentFilePath, ComponentId, ComponentVersion, IdempotencyKey, OwnedWorkerId,
    PluginInstallationId, ScanCursor, TargetWorkerId, WorkerFilter, WorkerId, WorkerMetadata,
    WorkerStatus, WorkerStatusRecord,
};
use golem_service_base::config::{BlobStorageConfig, LocalFileSystemBlobStorageConfig};
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_worker_executor::error::GolemError;
use golem_worker_executor::services::golem_config::{
    CompiledComponentServiceConfig, CompiledComponentServiceEnabledConfig, ComponentServiceConfig,
    ComponentServiceLocalConfig, GolemConfig, IndexedStorageConfig,
    IndexedStorageKVStoreRedisConfig, KeyValueStorageConfig, MemoryConfig,
    ShardManagerServiceConfig, ShardManagerServiceSingleShardConfig,
};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::{Arc, RwLock, Weak};

use golem_worker_executor::durable_host::{
    DurableWorkerCtx, DurableWorkerCtxView, PublicDurableWorkerState,
};
use golem_worker_executor::model::{
    CurrentResourceLimits, ExecutionStatus, InterruptKind, LastError, ListDirectoryResult,
    ReadFileResult, TrapType, WorkerConfig,
};
use golem_worker_executor::services::active_workers::ActiveWorkers;
use golem_worker_executor::services::blob_store::BlobStoreService;
use golem_worker_executor::services::component::{ComponentMetadata, ComponentService};
use golem_worker_executor::services::key_value::KeyValueService;
use golem_worker_executor::services::oplog::{Oplog, OplogService};
use golem_worker_executor::services::promise::PromiseService;
use golem_worker_executor::services::scheduler::SchedulerService;
use golem_worker_executor::services::shard::ShardService;
use golem_worker_executor::services::shard_manager::ShardManagerService;
use golem_worker_executor::services::worker::WorkerService;
use golem_worker_executor::services::worker_activator::WorkerActivator;
use golem_worker_executor::services::worker_event::WorkerEventService;
use golem_worker_executor::services::{
    plugins, rdbms, resource_limits, All, HasAll, HasConfig, HasOplogService,
};
use golem_worker_executor::wasi_host::create_linker;
use golem_worker_executor::workerctx::{
    DynamicLinking, ExternalOperations, FileSystemReading, FuelManagement, IndexedResourceStore,
    InvocationContextManagement, InvocationHooks, InvocationManagement, StatusManagement,
    UpdateManagement, WorkerCtx,
};
use golem_worker_executor::{Bootstrap, DefaultGolemTypes, RunDetails};

use tokio::runtime::Handle;

use tokio::task::JoinSet;

use golem_common::config::RedisConfig;

use golem_api_grpc::proto::golem::workerexecutor::v1::{
    get_running_workers_metadata_response, get_workers_metadata_response,
    GetRunningWorkersMetadataRequest, GetRunningWorkersMetadataSuccessResponse,
    GetWorkersMetadataRequest, GetWorkersMetadataSuccessResponse,
};
use golem_common::model::invocation_context::{
    AttributeValue, InvocationContextSpan, InvocationContextStack, SpanId,
};
use golem_common::model::oplog::WorkerResourceId;
use golem_test_framework::components::component_compilation_service::ComponentCompilationService;
use golem_test_framework::components::rdb::Rdb;
use golem_test_framework::components::redis::Redis;
use golem_test_framework::components::redis_monitor::RedisMonitor;
use golem_test_framework::components::shard_manager::ShardManager;
use golem_test_framework::components::worker_executor_cluster::WorkerExecutorCluster;
use golem_test_framework::config::TestDependencies;
use golem_test_framework::dsl::to_worker_metadata;
use golem_wasm_rpc::golem_rpc_0_2_x::types::{FutureInvokeResult, WasmRpc};
use golem_wasm_rpc::golem_rpc_0_2_x::types::{HostFutureInvokeResult, Pollable};
use golem_worker_executor::preview2::golem::durability;
use golem_worker_executor::preview2::golem_api_1_x;
use golem_worker_executor::services::component;
use golem_worker_executor::services::events::Events;
use golem_worker_executor::services::oplog::plugin::OplogProcessorPlugin;
use golem_worker_executor::services::plugins::{Plugins, PluginsObservations};
use golem_worker_executor::services::resource_limits::ResourceLimits;
use golem_worker_executor::services::rpc::{DirectWorkerInvocationRpc, RemoteInvocationRpc, Rpc};
use golem_worker_executor::services::worker_enumeration::{
    RunningWorkerEnumerationService, WorkerEnumerationService,
};
use golem_worker_executor::services::worker_fork::{DefaultWorkerFork, WorkerForkService};
use golem_worker_executor::services::worker_proxy::WorkerProxy;
use golem_worker_executor::worker::{RetryDecision, Worker};
use tonic::transport::Channel;
use tracing::{debug, info};
use uuid::Uuid;
use wasmtime::component::{Component, Instance, Linker, Resource, ResourceAny};
use wasmtime::{AsContextMut, Engine, ResourceLimiterAsync};
use wasmtime_wasi::WasiView;
use wasmtime_wasi_http::WasiHttpView;

pub struct TestWorkerExecutor {
    _join_set: Option<JoinSet<anyhow::Result<()>>>,
    deps: WorkerExecutorPerTestDependencies,
}

impl TestWorkerExecutor {
    pub async fn client(&self) -> golem_test_framework::Result<WorkerExecutorClient<Channel>> {
        self.deps.worker_executor.client().await
    }

    pub async fn get_running_workers_metadata(
        &self,
        component_id: &ComponentId,
        filter: Option<WorkerFilter>,
    ) -> Vec<(WorkerMetadata, Option<String>)> {
        let component_id: golem_api_grpc::proto::golem::component::ComponentId =
            component_id.clone().into();
        let response = self
            .client()
            .await
            .expect("Failed to get client")
            .get_running_workers_metadata(GetRunningWorkersMetadataRequest {
                component_id: Some(component_id),
                filter: filter.map(|f| f.into()),
            })
            .await
            .expect("Failed to get running workers metadata")
            .into_inner();

        match response.result {
            None => panic!("No response from get_running_workers_metadata"),
            Some(get_running_workers_metadata_response::Result::Success(
                GetRunningWorkersMetadataSuccessResponse { workers },
            )) => workers.iter().map(to_worker_metadata).collect(),

            Some(get_running_workers_metadata_response::Result::Failure(error)) => {
                panic!("Failed to get worker metadata: {error:?}")
            }
        }
    }

    pub async fn get_workers_metadata(
        &self,
        component_id: &ComponentId,
        filter: Option<WorkerFilter>,
        cursor: ScanCursor,
        count: u64,
        precise: bool,
    ) -> (Option<ScanCursor>, Vec<(WorkerMetadata, Option<String>)>) {
        let component_id: golem_api_grpc::proto::golem::component::ComponentId =
            component_id.clone().into();
        let response = self
            .client()
            .await
            .expect("Failed to get client")
            .get_workers_metadata(GetWorkersMetadataRequest {
                component_id: Some(component_id),
                filter: filter.map(|f| f.into()),
                cursor: Some(cursor.into()),
                count,
                precise,
                account_id: Some(
                    AccountId {
                        value: "test-account".to_string(),
                    }
                    .into(),
                ),
            })
            .await
            .expect("Failed to get workers metadata")
            .into_inner();

        match response.result {
            None => panic!("No response from get_workers_metadata"),
            Some(get_workers_metadata_response::Result::Success(
                GetWorkersMetadataSuccessResponse { workers, cursor },
            )) => (
                cursor.map(|c| c.into()),
                workers.iter().map(to_worker_metadata).collect(),
            ),
            Some(get_workers_metadata_response::Result::Failure(error)) => {
                panic!("Failed to get workers metadata: {error:?}")
            }
        }
    }
}

impl Clone for TestWorkerExecutor {
    fn clone(&self) -> Self {
        Self {
            _join_set: None,
            deps: self.deps.clone(),
        }
    }
}

#[async_trait]
impl TestDependencies for TestWorkerExecutor {
    fn rdb(&self) -> Arc<dyn Rdb + Send + Sync + 'static> {
        self.deps.rdb()
    }

    fn redis(&self) -> Arc<dyn Redis + Send + Sync + 'static> {
        self.deps.redis()
    }

    fn blob_storage(&self) -> Arc<dyn BlobStorage + Send + Sync + 'static> {
        self.deps.blob_storage()
    }

    fn redis_monitor(&self) -> Arc<dyn RedisMonitor + Send + Sync + 'static> {
        self.deps.redis_monitor()
    }

    fn shard_manager(&self) -> Arc<dyn ShardManager + Send + Sync + 'static> {
        self.deps.shard_manager()
    }

    fn component_directory(&self) -> &Path {
        self.deps.component_directory()
    }

    fn component_temp_directory(&self) -> &Path {
        self.deps.component_temp_directory()
    }

    fn component_service(
        &self,
    ) -> Arc<dyn golem_test_framework::components::component_service::ComponentService> {
        self.deps.component_service()
    }

    fn component_compilation_service(
        &self,
    ) -> Arc<dyn ComponentCompilationService + Send + Sync + 'static> {
        self.deps.component_compilation_service()
    }

    fn worker_service(
        &self,
    ) -> Arc<dyn golem_test_framework::components::worker_service::WorkerService + 'static> {
        self.deps.worker_service()
    }

    fn worker_executor_cluster(&self) -> Arc<dyn WorkerExecutorCluster + Send + Sync + 'static> {
        self.deps.worker_executor_cluster()
    }

    fn initial_component_files_service(&self) -> Arc<InitialComponentFilesService> {
        self.deps.initial_component_files_service()
    }

    fn plugin_wasm_files_service(&self) -> Arc<PluginWasmFilesService> {
        self.deps.plugin_wasm_files_service()
    }
}

pub struct TestContext {
    base_prefix: String,
    unique_id: u16,
}

impl TestContext {
    pub fn new(last_unique_id: &LastUniqueId) -> Self {
        let base_prefix = Uuid::new_v4().to_string();
        let unique_id = last_unique_id.id.fetch_add(1, Ordering::Relaxed);
        Self {
            base_prefix,
            unique_id,
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
    start_limited(deps, context, None).await
}

pub async fn start_limited(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    system_memory_override: Option<u64>,
) -> anyhow::Result<TestWorkerExecutor> {
    let redis = deps.redis();
    let redis_monitor = deps.redis_monitor();
    redis.assert_valid();
    redis_monitor.assert_valid();
    info!("Using Redis on port {}", redis.public_port());

    let prometheus = golem_worker_executor::metrics::register_all();
    let config = GolemConfig {
        key_value_storage: KeyValueStorageConfig::Redis(RedisConfig {
            port: redis.public_port(),
            key_prefix: context.redis_prefix(),
            ..Default::default()
        }),
        indexed_storage: IndexedStorageConfig::KVStoreRedis(IndexedStorageKVStoreRedisConfig {}),
        blob_storage: BlobStorageConfig::LocalFileSystem(LocalFileSystemBlobStorageConfig {
            root: Path::new("data/blobs").to_path_buf(),
        }),
        port: 0,
        http_port: 0,

        compiled_component_service: CompiledComponentServiceConfig::Enabled(
            CompiledComponentServiceEnabledConfig {},
        ),
        shard_manager_service: ShardManagerServiceConfig::SingleShard(
            ShardManagerServiceSingleShardConfig {},
        ),
        memory: MemoryConfig {
            system_memory_override,
            ..Default::default()
        },
        component_service: ComponentServiceConfig::Local(ComponentServiceLocalConfig {
            root: Path::new("data/components").to_path_buf(),
        }),
        ..Default::default()
    };

    let handle = Handle::current();

    let mut join_set = JoinSet::new();

    let details = run(config, prometheus, handle, &mut join_set).await?;
    let grpc_port = details.grpc_port;

    let start = std::time::Instant::now();
    loop {
        info!("Waiting for worker-executor to be reachable on port {grpc_port}");
        let client = WorkerExecutorClient::connect(format!("http://127.0.0.1:{grpc_port}")).await;
        if client.is_ok() {
            let deps = deps.per_test(&context.redis_prefix(), details.http_port, grpc_port);
            break Ok(TestWorkerExecutor {
                _join_set: Some(join_set),
                deps,
            });
        } else if start.elapsed().as_secs() > 10 {
            break Err(anyhow::anyhow!("Timeout waiting for server to start"));
        }
    }
}

async fn run(
    golem_config: GolemConfig,
    prometheus_registry: Registry,
    runtime: Handle,
    join_set: &mut JoinSet<Result<(), Error>>,
) -> Result<RunDetails, Error> {
    info!("Golem Worker Executor starting up...");

    ServerBootstrap {}
        .run(golem_config, prometheus_registry, runtime, join_set)
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

#[async_trait]
impl FuelManagement for TestWorkerCtx {
    fn is_out_of_fuel(&self, _current_level: i64) -> bool {
        false
    }

    async fn borrow_fuel(&mut self) -> Result<(), GolemError> {
        Ok(())
    }

    fn borrow_fuel_sync(&mut self) {}

    async fn return_fuel(&mut self, _current_level: i64) -> Result<i64, GolemError> {
        Ok(0)
    }
}

#[async_trait]
impl IndexedResourceStore for TestWorkerCtx {
    fn get_indexed_resource(
        &self,
        resource_name: &str,
        resource_params: &[String],
    ) -> Option<WorkerResourceId> {
        self.durable_ctx
            .get_indexed_resource(resource_name, resource_params)
    }

    async fn store_indexed_resource(
        &mut self,
        resource_name: &str,
        resource_params: &[String],
        resource: WorkerResourceId,
    ) {
        self.durable_ctx
            .store_indexed_resource(resource_name, resource_params, resource)
            .await
    }

    fn drop_indexed_resource(&mut self, resource_name: &str, resource_params: &[String]) {
        self.durable_ctx
            .drop_indexed_resource(resource_name, resource_params)
    }
}

#[async_trait]
impl ExternalOperations<TestWorkerCtx> for TestWorkerCtx {
    type ExtraDeps = ();

    async fn get_last_error_and_retry_count<T: HasAll<TestWorkerCtx> + Send + Sync>(
        this: &T,
        owned_worker_id: &OwnedWorkerId,
        latest_worker_status: &WorkerStatusRecord,
    ) -> Option<LastError> {
        DurableWorkerCtx::<TestWorkerCtx>::get_last_error_and_retry_count(
            this,
            owned_worker_id,
            latest_worker_status,
        )
        .await
    }

    async fn compute_latest_worker_status<T: HasOplogService + HasConfig + Send + Sync>(
        this: &T,
        owned_worker_id: &OwnedWorkerId,
        metadata: &Option<WorkerMetadata>,
    ) -> Result<WorkerStatusRecord, GolemError> {
        DurableWorkerCtx::<TestWorkerCtx>::compute_latest_worker_status(
            this,
            owned_worker_id,
            metadata,
        )
        .await
    }

    async fn resume_replay(
        store: &mut (impl AsContextMut<Data = TestWorkerCtx> + Send),
        instance: &Instance,
    ) -> Result<RetryDecision, GolemError> {
        DurableWorkerCtx::<TestWorkerCtx>::resume_replay(store, instance).await
    }

    async fn prepare_instance(
        worker_id: &WorkerId,
        instance: &Instance,
        store: &mut (impl AsContextMut<Data = TestWorkerCtx> + Send),
    ) -> Result<RetryDecision, GolemError> {
        DurableWorkerCtx::<TestWorkerCtx>::prepare_instance(worker_id, instance, store).await
    }

    async fn record_last_known_limits<T: HasAll<TestWorkerCtx> + Send + Sync>(
        this: &T,
        account_id: &AccountId,
        last_known_limits: &CurrentResourceLimits,
    ) -> Result<(), GolemError> {
        DurableWorkerCtx::<TestWorkerCtx>::record_last_known_limits(
            this,
            account_id,
            last_known_limits,
        )
        .await
    }

    async fn on_worker_deleted<T: HasAll<TestWorkerCtx> + Send + Sync>(
        this: &T,
        worker_id: &WorkerId,
    ) -> Result<(), GolemError> {
        DurableWorkerCtx::<TestWorkerCtx>::on_worker_deleted(this, worker_id).await
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
    ) -> Result<(), GolemError> {
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

    async fn set_suspended(&self) -> Result<(), GolemError> {
        self.durable_ctx.set_suspended().await
    }

    fn set_running(&self) {
        self.durable_ctx.set_running()
    }

    async fn get_worker_status(&self) -> WorkerStatus {
        self.durable_ctx.get_worker_status().await
    }

    async fn store_worker_status(&self, status: WorkerStatus) {
        self.durable_ctx.store_worker_status(status).await
    }

    async fn update_pending_invocations(&self) {
        self.durable_ctx.update_pending_invocations().await
    }

    async fn update_pending_updates(&self) {
        self.durable_ctx.update_pending_updates().await
    }
}

#[async_trait]
impl InvocationHooks for TestWorkerCtx {
    async fn on_exported_function_invoked(
        &mut self,
        full_function_name: &str,
        function_input: &Vec<Value>,
    ) -> Result<(), GolemError> {
        self.durable_ctx
            .on_exported_function_invoked(full_function_name, function_input)
            .await
    }

    async fn on_invocation_failure(&mut self, trap_type: &TrapType) -> RetryDecision {
        self.durable_ctx.on_invocation_failure(trap_type).await
    }

    async fn on_invocation_success(
        &mut self,
        full_function_name: &str,
        function_input: &Vec<Value>,
        consumed_fuel: i64,
        output: TypeAnnotatedValue,
    ) -> Result<(), GolemError> {
        self.durable_ctx
            .on_invocation_success(full_function_name, function_input, consumed_fuel, output)
            .await
    }
}

#[async_trait]
impl ResourceStore for TestWorkerCtx {
    fn self_uri(&self) -> Uri {
        self.durable_ctx.self_uri()
    }

    async fn add(&mut self, resource: ResourceAny) -> u64 {
        self.durable_ctx.add(resource).await
    }

    async fn get(&mut self, resource_id: u64) -> Option<ResourceAny> {
        ResourceStore::get(&mut self.durable_ctx, resource_id).await
    }

    async fn borrow(&self, resource_id: u64) -> Option<ResourceAny> {
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
        target_version: ComponentVersion,
        details: Option<String>,
    ) {
        self.durable_ctx
            .on_worker_update_failed(target_version, details)
            .await
    }

    async fn on_worker_update_succeeded(
        &self,
        target_version: ComponentVersion,
        new_component_size: u64,
        new_active_plugins: HashSet<PluginInstallationId>,
    ) {
        self.durable_ctx
            .on_worker_update_succeeded(target_version, new_component_size, new_active_plugins)
            .await
    }
}

struct ServerBootstrap {}

#[async_trait]
impl WorkerCtx for TestWorkerCtx {
    type Types = DefaultGolemTypes;
    type PublicState = PublicDurableWorkerState<TestWorkerCtx>;

    async fn create(
        owned_worker_id: OwnedWorkerId,
        component_metadata: ComponentMetadata<DefaultGolemTypes>,
        promise_service: Arc<dyn PromiseService>,
        worker_service: Arc<dyn WorkerService>,
        worker_enumeration_service: Arc<dyn WorkerEnumerationService>,
        key_value_service: Arc<dyn KeyValueService>,
        blob_store_service: Arc<dyn BlobStoreService>,
        rdbms_service: Arc<dyn rdbms::RdbmsService>,
        event_service: Arc<dyn WorkerEventService + Send + Sync>,
        _active_workers: Arc<ActiveWorkers<TestWorkerCtx>>,
        oplog_service: Arc<dyn OplogService>,
        oplog: Arc<dyn Oplog>,
        invocation_queue: Weak<Worker<TestWorkerCtx>>,
        scheduler_service: Arc<dyn SchedulerService>,
        rpc: Arc<dyn Rpc>,
        worker_proxy: Arc<dyn WorkerProxy>,
        component_service: Arc<dyn ComponentService<DefaultGolemTypes>>,
        _extra_deps: Self::ExtraDeps,
        config: Arc<GolemConfig>,
        worker_config: WorkerConfig,
        execution_status: Arc<RwLock<ExecutionStatus>>,
        file_loader: Arc<FileLoader>,
        plugins: Arc<dyn Plugins<DefaultGolemTypes>>,
        worker_fork: Arc<dyn WorkerForkService>,
        _resource_limits: Arc<dyn ResourceLimits>,
    ) -> Result<Self, GolemError> {
        let durable_ctx = DurableWorkerCtx::create(
            owned_worker_id,
            component_metadata,
            promise_service,
            worker_service,
            worker_enumeration_service,
            key_value_service,
            blob_store_service,
            rdbms_service,
            event_service,
            oplog_service,
            oplog,
            invocation_queue,
            scheduler_service,
            rpc,
            worker_proxy,
            component_service,
            config,
            worker_config,
            execution_status,
            file_loader,
            plugins,
            worker_fork,
        )
        .await?;
        Ok(Self { durable_ctx })
    }

    fn as_wasi_view(&mut self) -> impl WasiView {
        self.durable_ctx.as_wasi_view()
    }

    fn as_wasi_http_view(&mut self) -> impl WasiHttpView {
        self.durable_ctx.as_wasi_http_view()
    }

    fn get_public_state(&self) -> &Self::PublicState {
        &self.durable_ctx.public_state
    }

    fn resource_limiter(&mut self) -> &mut dyn ResourceLimiterAsync {
        self
    }

    fn worker_id(&self) -> &WorkerId {
        self.durable_ctx.worker_id()
    }

    fn owned_worker_id(&self) -> &OwnedWorkerId {
        self.durable_ctx.owned_worker_id()
    }

    fn component_metadata(&self) -> &ComponentMetadata<DefaultGolemTypes> {
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

    fn component_service(&self) -> Arc<dyn ComponentService<Self::Types> + Send + Sync> {
        self.durable_ctx.component_service()
    }

    fn worker_fork(&self) -> Arc<dyn WorkerForkService> {
        self.durable_ctx.worker_fork()
    }

    async fn generate_unique_local_worker_id(
        &mut self,
        remote_worker_id: TargetWorkerId,
    ) -> Result<WorkerId, GolemError> {
        self.durable_ctx
            .generate_unique_local_worker_id(remote_worker_id)
            .await
    }
}

#[async_trait]
impl ResourceLimiterAsync for TestWorkerCtx {
    async fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> anyhow::Result<bool> {
        debug!(
            "Memory growing for {}: current: {}, desired: {}",
            self.worker_id(),
            current,
            desired
        );
        let current_known = self.durable_ctx.total_linear_memory_size();
        let delta = (desired as u64).saturating_sub(current_known);
        if delta > 0 {
            debug!("CURRENT KNOWN: {current_known} DESIRED: {desired} DELTA: {delta}");
            Ok(self.durable_ctx.increase_memory(delta).await?)
        } else {
            Ok(true)
        }
    }

    async fn table_growing(
        &mut self,
        current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> anyhow::Result<bool> {
        debug!(
            "Table growing for {}: current: {}, desired: {}",
            self.worker_id(),
            current,
            desired
        );
        Ok(true)
    }
}

#[async_trait]
impl FileSystemReading for TestWorkerCtx {
    async fn list_directory(
        &self,
        path: &ComponentFilePath,
    ) -> Result<ListDirectoryResult, GolemError> {
        self.durable_ctx.list_directory(path).await
    }

    async fn read_file(&self, path: &ComponentFilePath) -> Result<ReadFileResult, GolemError> {
        self.durable_ctx.read_file(path).await
    }
}

impl HostWasmRpc for TestWorkerCtx {
    async fn new(
        &mut self,
        worker_id: golem_wasm_rpc::WorkerId,
    ) -> anyhow::Result<Resource<WasmRpc>> {
        self.durable_ctx.new(worker_id).await
    }

    async fn ephemeral(
        &mut self,
        component_id: golem_wasm_rpc::ComponentId,
    ) -> anyhow::Result<Resource<WasmRpc>> {
        self.durable_ctx.ephemeral(component_id).await
    }

    async fn invoke_and_await(
        &mut self,
        self_: Resource<WasmRpc>,
        function_name: String,
        function_params: Vec<WitValue>,
    ) -> anyhow::Result<Result<WitValue, RpcError>> {
        self.durable_ctx
            .invoke_and_await(self_, function_name, function_params)
            .await
    }

    async fn invoke(
        &mut self,
        self_: Resource<WasmRpc>,
        function_name: String,
        function_params: Vec<WitValue>,
    ) -> anyhow::Result<Result<(), RpcError>> {
        self.durable_ctx
            .invoke(self_, function_name, function_params)
            .await
    }

    async fn async_invoke_and_await(
        &mut self,
        self_: Resource<WasmRpc>,
        function_name: String,
        function_params: Vec<WitValue>,
    ) -> anyhow::Result<Resource<FutureInvokeResult>> {
        self.durable_ctx
            .async_invoke_and_await(self_, function_name, function_params)
            .await
    }

    async fn schedule_invocation(
        &mut self,
        self_: Resource<WasmRpc>,
        datetime: golem_wasm_rpc::wasi::clocks::wall_clock::Datetime,
        function_name: String,
        function_params: Vec<WitValue>,
    ) -> anyhow::Result<()> {
        self.durable_ctx
            .schedule_invocation(self_, datetime, function_name, function_params)
            .await
    }

    async fn schedule_cancelable_invocation(
        &mut self,
        self_: Resource<WasmRpc>,
        datetime: golem_wasm_rpc::wasi::clocks::wall_clock::Datetime,
        function_name: String,
        function_params: Vec<WitValue>,
    ) -> anyhow::Result<Resource<golem_wasm_rpc::golem_rpc_0_2_x::types::CancellationToken>> {
        self.durable_ctx
            .schedule_cancelable_invocation(self_, datetime, function_name, function_params)
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
    ) -> anyhow::Result<Resource<Pollable>> {
        HostFutureInvokeResult::subscribe(&mut self.durable_ctx, self_).await
    }

    async fn get(
        &mut self,
        self_: Resource<FutureInvokeResult>,
    ) -> anyhow::Result<Option<Result<WitValue, RpcError>>> {
        HostFutureInvokeResult::get(&mut self.durable_ctx, self_).await
    }

    async fn drop(&mut self, rep: Resource<FutureInvokeResult>) -> anyhow::Result<()> {
        HostFutureInvokeResult::drop(&mut self.durable_ctx, rep).await
    }
}

#[async_trait]
impl DynamicLinking<TestWorkerCtx> for TestWorkerCtx {
    fn link(
        &mut self,
        engine: &Engine,
        linker: &mut Linker<TestWorkerCtx>,
        component: &Component,
        component_metadata: &ComponentMetadata<DefaultGolemTypes>,
    ) -> anyhow::Result<()> {
        self.durable_ctx
            .link(engine, linker, component, component_metadata)
    }
}

#[async_trait]
impl InvocationContextManagement for TestWorkerCtx {
    async fn start_span(
        &mut self,
        initial_attributes: &[(String, AttributeValue)],
    ) -> Result<Arc<InvocationContextSpan>, GolemError> {
        self.durable_ctx.start_span(initial_attributes).await
    }

    async fn start_child_span(
        &mut self,
        parent: &SpanId,
        initial_attributes: &[(String, AttributeValue)],
    ) -> Result<Arc<InvocationContextSpan>, GolemError> {
        self.durable_ctx
            .start_child_span(parent, initial_attributes)
            .await
    }

    fn remove_span(&mut self, span_id: &SpanId) -> Result<(), GolemError> {
        self.durable_ctx.remove_span(span_id)
    }

    async fn finish_span(&mut self, span_id: &SpanId) -> Result<(), GolemError> {
        self.durable_ctx.finish_span(span_id).await
    }

    async fn set_span_attribute(
        &mut self,
        span_id: &SpanId,
        key: &str,
        value: AttributeValue,
    ) -> Result<(), GolemError> {
        self.durable_ctx
            .set_span_attribute(span_id, key, value)
            .await
    }
}

#[async_trait]
impl Bootstrap<TestWorkerCtx> for ServerBootstrap {
    fn create_active_workers(
        &self,
        golem_config: &GolemConfig,
    ) -> Arc<ActiveWorkers<TestWorkerCtx>> {
        Arc::new(ActiveWorkers::<TestWorkerCtx>::new(&golem_config.memory))
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
        active_workers: Arc<ActiveWorkers<TestWorkerCtx>>,
        engine: Arc<Engine>,
        linker: Arc<Linker<TestWorkerCtx>>,
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
        worker_activator: Arc<dyn WorkerActivator<TestWorkerCtx>>,
        oplog_service: Arc<dyn OplogService>,
        scheduler_service: Arc<dyn SchedulerService>,
        worker_proxy: Arc<dyn WorkerProxy>,
        events: Arc<Events>,
        file_loader: Arc<FileLoader>,
        plugins: Arc<dyn Plugins<DefaultGolemTypes>>,
        oplog_processor_plugin: Arc<dyn OplogProcessorPlugin>,
    ) -> anyhow::Result<All<TestWorkerCtx>> {
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
            (),
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
            (),
        ));
        Ok(All::new(
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
            rdbms_service,
            oplog_service,
            rpc,
            scheduler_service,
            worker_activator,
            worker_proxy,
            events,
            file_loader,
            plugins,
            oplog_processor_plugin,
            resource_limits,
            (),
        ))
    }

    fn create_wasmtime_linker(&self, engine: &Engine) -> anyhow::Result<Linker<TestWorkerCtx>> {
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

fn get_durable_ctx(ctx: &mut TestWorkerCtx) -> &mut DurableWorkerCtx<TestWorkerCtx> {
    &mut ctx.durable_ctx
}
