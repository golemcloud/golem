use crate::{LastUniqueId, WorkerExecutorPerTestDependencies, WorkerExecutorTestDependencies};
use anyhow::Error;
use async_trait::async_trait;
use bytes::Bytes;
use dashmap::DashMap;
use golem_api_grpc::proto::golem::workerexecutor::v1::worker_executor_client::WorkerExecutorClient;
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    get_running_workers_metadata_response, GetRunningWorkersMetadataRequest,
    GetRunningWorkersMetadataSuccessResponse,
};
use golem_common::config::RedisConfig;
use golem_common::model::agent::AgentId;
use golem_common::model::invocation_context::{
    AttributeValue, InvocationContextSpan, InvocationContextStack, SpanId,
};
use golem_common::model::oplog::{
    OplogEntry, OplogPayload, TimestampedUpdateDescription, UpdateDescription,
};
use golem_common::model::{
    AccountId, ComponentFilePath, ComponentId, ComponentVersion, GetFileSystemNodeResult,
    IdempotencyKey, OplogIndex, OwnedWorkerId, PluginInstallationId, ProjectId, RetryConfig,
    TransactionId, WorkerFilter, WorkerId, WorkerMetadata, WorkerStatusRecord,
};
use golem_service_base::config::{BlobStorageConfig, LocalFileSystemBlobStorageConfig};
use golem_service_base::error::worker_executor::{InterruptKind, WorkerExecutorError};
use golem_service_base::service::initial_component_files::InitialComponentFilesService;
use golem_service_base::service::plugin_wasm_files::PluginWasmFilesService;
use golem_service_base::storage::blob::BlobStorage;
use golem_test_framework::components::cloud_service::CloudService;
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
use golem_wasm_rpc::wasmtime::{ResourceStore, ResourceTypeId};
use golem_wasm_rpc::{HostWasmRpc, RpcError, Uri, Value, ValueAndType, WitValue};
use golem_worker_executor::durable_host::{
    DurableWorkerCtx, DurableWorkerCtxView, PublicDurableWorkerState,
};
use golem_worker_executor::model::{
    CurrentResourceLimits, ExecutionStatus, LastError, ReadFileResult, TrapType, WorkerConfig,
};
use golem_worker_executor::preview2::golem::durability;
use golem_worker_executor::preview2::{golem_agent, golem_api_1_x};
use golem_worker_executor::services::active_workers::ActiveWorkers;
use golem_worker_executor::services::agent_types::AgentTypesService;
use golem_worker_executor::services::blob_store::BlobStoreService;
use golem_worker_executor::services::component::ComponentService;
use golem_worker_executor::services::events::Events;
use golem_worker_executor::services::file_loader::FileLoader;
use golem_worker_executor::services::golem_config::{
    AgentTypesServiceConfig, AgentTypesServiceLocalConfig, CompiledComponentServiceConfig,
    CompiledComponentServiceEnabledConfig, ComponentServiceConfig, ComponentServiceLocalConfig,
    GolemConfig, IndexedStorageConfig, IndexedStorageKVStoreRedisConfig, KeyValueStorageConfig,
    MemoryConfig, ProjectServiceConfig, ProjectServiceDisabledConfig, ShardManagerServiceConfig,
    ShardManagerServiceSingleShardConfig,
};
use golem_worker_executor::services::key_value::KeyValueService;
use golem_worker_executor::services::oplog::plugin::OplogProcessorPlugin;
use golem_worker_executor::services::oplog::{CommitLevel, Oplog, OplogService};
use golem_worker_executor::services::plugins::{Plugins, PluginsObservations};
use golem_worker_executor::services::projects::ProjectService;
use golem_worker_executor::services::promise::PromiseService;
use golem_worker_executor::services::rdbms::mysql::MysqlType;
use golem_worker_executor::services::rdbms::postgres::PostgresType;
use golem_worker_executor::services::rdbms::{
    DbResult, DbResultStream, DbTransaction, Rdbms, RdbmsPoolKey, RdbmsStatus,
    RdbmsTransactionStatus, RdbmsType,
};
use golem_worker_executor::services::resource_limits::ResourceLimits;
use golem_worker_executor::services::rpc::{DirectWorkerInvocationRpc, RemoteInvocationRpc, Rpc};
use golem_worker_executor::services::scheduler::SchedulerService;
use golem_worker_executor::services::shard::ShardService;
use golem_worker_executor::services::shard_manager::ShardManagerService;
use golem_worker_executor::services::worker::WorkerService;
use golem_worker_executor::services::worker_activator::WorkerActivator;
use golem_worker_executor::services::worker_enumeration::{
    RunningWorkerEnumerationService, WorkerEnumerationService,
};
use golem_worker_executor::services::worker_event::WorkerEventService;
use golem_worker_executor::services::worker_fork::{DefaultWorkerFork, WorkerForkService};
use golem_worker_executor::services::worker_proxy::WorkerProxy;
use golem_worker_executor::services::{rdbms, resource_limits, All, HasAll};
use golem_worker_executor::wasi_host::create_linker;
use golem_worker_executor::worker::{RetryDecision, Worker};
use golem_worker_executor::workerctx::{
    DynamicLinking, ExternalOperations, FileSystemReading, FuelManagement, HasWasiConfigVars,
    InvocationContextManagement, InvocationHooks, InvocationManagement, LogEventEmitBehaviour,
    StatusManagement, UpdateManagement, WorkerCtx,
};
use golem_worker_executor::{Bootstrap, RunDetails};
use prometheus::Registry;
use regex::Regex;
use std::collections::{BTreeMap, HashSet};
use std::fmt::{Debug, Formatter};
use std::future::Future;
use std::ops::Deref;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::{Arc, RwLock, Weak};
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::task::JoinSet;
use tonic::transport::Channel;
use tracing::{debug, info};
use uuid::Uuid;
use wasmtime::component::{Component, Instance, Linker, Resource, ResourceAny};
use wasmtime::{AsContextMut, Engine, ResourceLimiterAsync};
use wasmtime_wasi::p2::WasiView;
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
    fn rdb(&self) -> Arc<dyn Rdb> {
        panic!("Not supported")
    }

    fn redis(&self) -> Arc<dyn Redis> {
        self.deps.redis.clone()
    }

    fn blob_storage(&self) -> Arc<dyn BlobStorage> {
        self.deps.blob_storage.clone()
    }

    fn redis_monitor(&self) -> Arc<dyn RedisMonitor> {
        self.deps.redis_monitor.clone()
    }

    fn shard_manager(&self) -> Arc<dyn ShardManager> {
        panic!("Not supported")
    }

    fn component_directory(&self) -> &Path {
        &self.deps.component_directory
    }

    fn component_temp_directory(&self) -> &Path {
        self.deps.component_temp_directory.path()
    }

    fn component_service(
        &self,
    ) -> Arc<dyn golem_test_framework::components::component_service::ComponentService> {
        self.deps.component_service.clone()
    }

    fn component_compilation_service(&self) -> Arc<dyn ComponentCompilationService> {
        panic!("Not supported")
    }

    fn worker_service(
        &self,
    ) -> Arc<dyn golem_test_framework::components::worker_service::WorkerService> {
        self.deps.worker_service.clone()
    }

    fn worker_executor_cluster(&self) -> Arc<dyn WorkerExecutorCluster> {
        panic!("Not supported")
    }

    fn initial_component_files_service(&self) -> Arc<InitialComponentFilesService> {
        self.deps.initial_component_files_service.clone()
    }

    fn plugin_wasm_files_service(&self) -> Arc<PluginWasmFilesService> {
        self.deps.plugin_wasm_files_service.clone()
    }

    fn cloud_service(&self) -> Arc<dyn CloudService> {
        self.deps.cloud_service.clone()
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
    start_customized(deps, context, None, None).await
}

pub async fn start_customized(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    system_memory_override: Option<u64>,
    retry_override: Option<RetryConfig>,
) -> anyhow::Result<TestWorkerExecutor> {
    let redis = deps.redis.clone();
    let redis_monitor = deps.redis_monitor.clone();
    redis.assert_valid();
    redis_monitor.assert_valid();
    info!("Using Redis on port {}", redis.public_port());

    let prometheus = golem_worker_executor::metrics::register_all();
    let admin_account_id = deps.cloud_service.admin_account_id();
    let admin_project_id = deps
        .cloud_service
        .get_default_project(&deps.cloud_service.admin_token())
        .await?;
    let admin_project_name = deps
        .cloud_service
        .get_project_name(&admin_project_id)
        .await?;
    let mut config = GolemConfig {
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
        project_service: ProjectServiceConfig::Disabled(ProjectServiceDisabledConfig {
            account_id: admin_account_id,
            project_id: admin_project_id,
            project_name: admin_project_name,
        }),
        agent_types_service: AgentTypesServiceConfig::Local(AgentTypesServiceLocalConfig {}),
        ..Default::default()
    };
    if let Some(retry) = retry_override {
        config.retry = retry;
    }

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

impl HasWasiConfigVars for TestWorkerCtx {
    fn wasi_config_vars(&self) -> BTreeMap<String, String> {
        self.durable_ctx.wasi_config_vars()
    }
}

impl wasmtime_wasi::p2::bindings::cli::environment::Host for TestWorkerCtx {
    fn get_environment(
        &mut self,
    ) -> impl Future<Output = anyhow::Result<Vec<(String, String)>>> + Send {
        wasmtime_wasi::p2::bindings::cli::environment::Host::get_environment(&mut self.durable_ctx)
    }

    fn get_arguments(&mut self) -> impl Future<Output = anyhow::Result<Vec<String>>> + Send {
        wasmtime_wasi::p2::bindings::cli::environment::Host::get_arguments(&mut self.durable_ctx)
    }

    fn initial_cwd(&mut self) -> impl Future<Output = anyhow::Result<Option<String>>> + Send {
        wasmtime_wasi::p2::bindings::cli::environment::Host::initial_cwd(&mut self.durable_ctx)
    }
}

#[async_trait]
impl FuelManagement for TestWorkerCtx {
    fn is_out_of_fuel(&self, _current_level: i64) -> bool {
        false
    }

    async fn borrow_fuel(&mut self) -> Result<(), WorkerExecutorError> {
        Ok(())
    }

    fn borrow_fuel_sync(&mut self) {}

    async fn return_fuel(&mut self, _current_level: i64) -> Result<i64, WorkerExecutorError> {
        Ok(0)
    }
}

#[async_trait]
impl ExternalOperations<TestWorkerCtx> for TestWorkerCtx {
    type ExtraDeps = AdditionalTestDeps;

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

    async fn resume_replay(
        store: &mut (impl AsContextMut<Data = TestWorkerCtx> + Send),
        instance: &Instance,
        refresh_replay_target: bool,
    ) -> Result<RetryDecision, WorkerExecutorError> {
        DurableWorkerCtx::<TestWorkerCtx>::resume_replay(store, instance, refresh_replay_target)
            .await
    }

    async fn prepare_instance(
        worker_id: &WorkerId,
        instance: &Instance,
        store: &mut (impl AsContextMut<Data = TestWorkerCtx> + Send),
    ) -> Result<RetryDecision, WorkerExecutorError> {
        DurableWorkerCtx::<TestWorkerCtx>::prepare_instance(worker_id, instance, store).await
    }

    async fn record_last_known_limits<T: HasAll<TestWorkerCtx> + Send + Sync>(
        this: &T,
        project_id: &ProjectId,
        last_known_limits: &CurrentResourceLimits,
    ) -> Result<(), WorkerExecutorError> {
        DurableWorkerCtx::<TestWorkerCtx>::record_last_known_limits(
            this,
            project_id,
            last_known_limits,
        )
        .await
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
    async fn on_exported_function_invoked(
        &mut self,
        full_function_name: &str,
        function_input: &Vec<Value>,
    ) -> Result<(), WorkerExecutorError> {
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
        output: Option<ValueAndType>,
    ) -> Result<(), WorkerExecutorError> {
        self.durable_ctx
            .on_invocation_success(full_function_name, function_input, consumed_fuel, output)
            .await
    }

    async fn get_current_retry_point(&self) -> OplogIndex {
        self.durable_ctx.get_current_retry_point().await
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
        target_version: ComponentVersion,
        details: Option<String>,
    ) {
        self.durable_ctx
            .on_worker_update_failed(target_version, details)
            .await
    }

    async fn on_worker_update_succeeded(
        &self,
        update: &UpdateDescription,
        new_component_size: u64,
        new_active_plugins: HashSet<PluginInstallationId>,
    ) {
        self.durable_ctx
            .on_worker_update_succeeded(update, new_component_size, new_active_plugins)
            .await
    }
}

struct ServerBootstrap {}

#[async_trait]
impl WorkerCtx for TestWorkerCtx {
    type PublicState = PublicDurableWorkerState<TestWorkerCtx>;

    const LOG_EVENT_EMIT_BEHAVIOUR: LogEventEmitBehaviour = LogEventEmitBehaviour::LiveOnly;

    async fn create(
        _account_id: AccountId,
        owned_worker_id: OwnedWorkerId,
        agent_id: Option<AgentId>,
        promise_service: Arc<dyn PromiseService>,
        worker_service: Arc<dyn WorkerService>,
        worker_enumeration_service: Arc<dyn WorkerEnumerationService>,
        key_value_service: Arc<dyn KeyValueService>,
        blob_store_service: Arc<dyn BlobStoreService>,
        rdbms_service: Arc<dyn rdbms::RdbmsService>,
        event_service: Arc<dyn WorkerEventService>,
        _active_workers: Arc<ActiveWorkers<TestWorkerCtx>>,
        oplog_service: Arc<dyn OplogService>,
        oplog: Arc<dyn Oplog>,
        invocation_queue: Weak<Worker<TestWorkerCtx>>,
        scheduler_service: Arc<dyn SchedulerService>,
        rpc: Arc<dyn Rpc>,
        worker_proxy: Arc<dyn WorkerProxy>,
        component_service: Arc<dyn ComponentService>,
        extra_deps: Self::ExtraDeps,
        config: Arc<GolemConfig>,
        worker_config: WorkerConfig,
        execution_status: Arc<RwLock<ExecutionStatus>>,
        file_loader: Arc<FileLoader>,
        plugins: Arc<dyn Plugins>,
        worker_fork: Arc<dyn WorkerForkService>,
        _resource_limits: Arc<dyn ResourceLimits>,
        project_service: Arc<dyn ProjectService>,
        agent_types_service: Arc<dyn AgentTypesService>,
        shard_service: Arc<dyn ShardService>,
        pending_update: Option<TimestampedUpdateDescription>,
    ) -> Result<Self, WorkerExecutorError> {
        let oplog = Arc::new(TestOplog::new(
            owned_worker_id.clone(),
            oplog.clone(),
            extra_deps,
        ));

        let durable_ctx = DurableWorkerCtx::create(
            owned_worker_id,
            agent_id,
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
            project_service,
            agent_types_service,
            shard_service,
            pending_update,
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

    fn agent_id(&self) -> Option<AgentId> {
        self.durable_ctx.agent_id()
    }

    fn created_by(&self) -> &AccountId {
        self.durable_ctx.created_by()
    }

    fn component_metadata(&self) -> &golem_service_base::model::Component {
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
            self.durable_ctx.increase_memory(delta).await?;
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
    async fn get_file_system_node(
        &self,
        path: &ComponentFilePath,
    ) -> Result<GetFileSystemNodeResult, WorkerExecutorError> {
        self.durable_ctx.get_file_system_node(path).await
    }

    async fn read_file(
        &self,
        path: &ComponentFilePath,
    ) -> Result<ReadFileResult, WorkerExecutorError> {
        self.durable_ctx.read_file(path).await
    }
}

impl HostWasmRpc for TestWorkerCtx {
    async fn new(
        &mut self,
        worker_id: golem_wasm_rpc::AgentId,
    ) -> anyhow::Result<Resource<WasmRpc>> {
        self.durable_ctx.new(worker_id).await
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
        component_metadata: &golem_service_base::model::Component,
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
    ) -> (Arc<dyn Plugins>, Arc<dyn PluginsObservations>) {
        let plugins =
            golem_worker_executor::services::plugins::configured(&golem_config.plugin_service);
        (plugins.clone(), plugins)
    }

    fn create_component_service(
        &self,
        golem_config: &GolemConfig,
        blob_storage: Arc<dyn BlobStorage>,
        plugin_observations: Arc<dyn PluginsObservations>,
        project_service: Arc<dyn ProjectService>,
    ) -> Arc<dyn ComponentService> {
        golem_worker_executor::services::component::configured(
            &golem_config.component_service,
            &golem_config.component_cache,
            &golem_config.compiled_component_service,
            blob_storage,
            plugin_observations,
            project_service,
        )
    }

    async fn create_services(
        &self,
        active_workers: Arc<ActiveWorkers<TestWorkerCtx>>,
        engine: Arc<Engine>,
        linker: Arc<Linker<TestWorkerCtx>>,
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
        worker_activator: Arc<dyn WorkerActivator<TestWorkerCtx>>,
        oplog_service: Arc<dyn OplogService>,
        scheduler_service: Arc<dyn SchedulerService>,
        worker_proxy: Arc<dyn WorkerProxy>,
        events: Arc<Events>,
        file_loader: Arc<FileLoader>,
        plugins: Arc<dyn Plugins>,
        oplog_processor_plugin: Arc<dyn OplogProcessorPlugin>,
        project_service: Arc<dyn ProjectService>,
        agent_types_service: Arc<dyn AgentTypesService>,
    ) -> anyhow::Result<All<TestWorkerCtx>> {
        let resource_limits = resource_limits::configured(&golem_config.resource_limits);
        let extra_deps = AdditionalTestDeps::new();
        let rdbms_service: Arc<dyn rdbms::RdbmsService> = Arc::new(TestRdmsService::new(
            rdbms_service.clone(),
            extra_deps.clone(),
        ));
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
            project_service.clone(),
            agent_types_service.clone(),
            extra_deps.clone(),
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
            project_service.clone(),
            agent_types_service.clone(),
            extra_deps.clone(),
        ));
        Ok(All::new(
            active_workers,
            agent_types_service,
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
            project_service,
            extra_deps.clone(),
        ))
    }

    fn create_wasmtime_linker(&self, engine: &Engine) -> anyhow::Result<Linker<TestWorkerCtx>> {
        let mut linker = create_linker(engine, get_durable_ctx)?;
        golem_api_1_x::host::add_to_linker_get_host(&mut linker, get_durable_ctx)?;
        golem_api_1_x::oplog::add_to_linker_get_host(&mut linker, get_durable_ctx)?;
        golem_api_1_x::context::add_to_linker_get_host(&mut linker, get_durable_ctx)?;
        durability::durability::add_to_linker_get_host(&mut linker, get_durable_ctx)?;
        golem_agent::host::add_to_linker_get_host(&mut linker, get_durable_ctx)?;
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

#[derive(Clone)]
struct TestOplog {
    owned_worker_id: OwnedWorkerId,
    oplog: Arc<dyn Oplog>,
    additional_test_deps: AdditionalTestDeps,
}

impl TestOplog {
    fn new(
        owned_worker_id: OwnedWorkerId,
        oplog: Arc<dyn Oplog>,
        additional_test_deps: AdditionalTestDeps,
    ) -> Self {
        Self {
            owned_worker_id,
            oplog,
            additional_test_deps,
        }
    }

    fn check_oplog_add(&self, entry: &OplogEntry) -> Result<(), String> {
        let entry_name = match entry {
            OplogEntry::BeginRemoteTransaction { .. } => "BeginRemoteTransaction",
            OplogEntry::PreRollbackRemoteTransaction { .. } => "PreRollbackRemoteTransaction",
            OplogEntry::PreCommitRemoteTransaction { .. } => "PreCommitRemoteTransaction",
            OplogEntry::CommittedRemoteTransaction { .. } => "CommittedRemoteTransaction",
            OplogEntry::RolledBackRemoteTransaction { .. } => "RolledBackRemoteTransaction",
            OplogEntry::BeginRemoteWrite { .. } => "BeginRemoteWrite",
            OplogEntry::EndRemoteWrite { .. } => "EndRemoteWrite",
            _ => "Other",
        };

        // FailOplogAdd{times}On{entry}
        let re = Regex::new(r"FailOplogAdd(\d+)On([A-Za-z]+)").unwrap();

        let worker_name = self.owned_worker_id.worker_id.worker_name.as_str();
        if let Some(captures) = re.captures(worker_name) {
            let times = &captures[1].parse::<usize>().unwrap_or_default();
            let entry = &captures[2];
            if entry == entry_name {
                let failed_before = self.additional_test_deps.get_oplog_failures_count(
                    self.owned_worker_id.worker_id.clone(),
                    entry_name.to_string(),
                );

                if failed_before >= *times {
                    Ok(())
                } else {
                    self.additional_test_deps.add_oplog_failure(
                        self.owned_worker_id.worker_id.clone(),
                        entry_name.to_string(),
                    );

                    tracing::info!("Failing worker as it hit marked oplog entry");

                    Err(format!(
                        "worker {worker_name} failed on {entry_name} {} times",
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

    async fn add_safe(&self, entry: OplogEntry) -> Result<(), String> {
        self.check_oplog_add(&entry)?;
        self.oplog.add_safe(entry).await
    }

    async fn drop_prefix(&self, last_dropped_id: OplogIndex) {
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

    async fn length(&self) -> u64 {
        self.oplog.length().await
    }

    async fn upload_payload(&self, data: &[u8]) -> Result<OplogPayload, String> {
        self.oplog.upload_payload(data).await
    }

    async fn download_payload(&self, payload: &OplogPayload) -> Result<Bytes, String> {
        self.oplog.download_payload(payload).await
    }
}

impl Debug for TestOplog {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.oplog)
    }
}

#[derive(Clone)]
struct TestRdmsService {
    mysql: Arc<dyn Rdbms<MysqlType> + Send + Sync>,
    postgres: Arc<dyn Rdbms<PostgresType> + Send + Sync>,
}

impl TestRdmsService {
    fn new(rdbms: Arc<dyn rdbms::RdbmsService>, additional_test_deps: AdditionalTestDeps) -> Self {
        let mysql: Arc<dyn Rdbms<MysqlType> + Send + Sync> =
            Arc::new(TestRdms::new(rdbms.mysql(), additional_test_deps.clone()));
        let postgres: Arc<dyn Rdbms<PostgresType> + Send + Sync> = Arc::new(TestRdms::new(
            rdbms.postgres(),
            additional_test_deps.clone(),
        ));
        Self { mysql, postgres }
    }
}

impl rdbms::RdbmsService for TestRdmsService {
    fn mysql(&self) -> Arc<dyn Rdbms<MysqlType> + Send + Sync> {
        self.mysql.clone()
    }

    fn postgres(&self) -> Arc<dyn Rdbms<PostgresType> + Send + Sync> {
        self.postgres.clone()
    }
}

#[derive(Clone)]
struct TestRdms<T: rdbms::RdbmsType> {
    rdbms: Arc<dyn rdbms::Rdbms<T> + Send + Sync>,
    additional_test_deps: AdditionalTestDeps,
}

impl<T: rdbms::RdbmsType> TestRdms<T> {
    fn new(
        rdbms: Arc<dyn rdbms::Rdbms<T> + Send + Sync>,
        additional_test_deps: AdditionalTestDeps,
    ) -> Self {
        Self {
            rdbms,
            additional_test_deps,
        }
    }

    fn check_rdbms_tx(&self, worker_id: &WorkerId, entry_name: &str) -> Result<(), rdbms::Error> {
        // FailRdbmsTx{times}On{entry}
        let re = Regex::new(r"FailRdbmsTx(\d+)On([A-Za-z]+)").unwrap();

        let worker_name = worker_id.worker_name.as_str();
        if let Some(captures) = re.captures(worker_name) {
            let times = &captures[1].parse::<usize>().unwrap_or_default();
            let entry = &captures[2];
            if entry == entry_name {
                let failed_before = self
                    .additional_test_deps
                    .get_rdbms_tx_failures_count(worker_id.clone(), entry_name.to_string());

                if failed_before >= *times {
                    Ok(())
                } else {
                    self.additional_test_deps
                        .add_rdbms_tx_failure(worker_id.clone(), entry_name.to_string());
                    Err(rdbms::Error::Other(format!(
                        "worker {} failed on {} {} times",
                        worker_name,
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
impl<T: rdbms::RdbmsType> rdbms::Rdbms<T> for TestRdms<T> {
    async fn create(
        &self,
        address: &str,
        worker_id: &WorkerId,
    ) -> Result<RdbmsPoolKey, rdbms::Error> {
        self.rdbms.deref().create(address, worker_id).await
    }

    fn exists(&self, key: &RdbmsPoolKey, worker_id: &WorkerId) -> bool {
        self.rdbms.deref().exists(key, worker_id)
    }

    fn remove(&self, key: &RdbmsPoolKey, worker_id: &WorkerId) -> bool {
        self.rdbms.deref().remove(key, worker_id)
    }

    async fn execute(
        &self,
        key: &RdbmsPoolKey,
        worker_id: &WorkerId,
        statement: &str,
        params: Vec<T::DbValue>,
    ) -> Result<u64, rdbms::Error>
    where
        <T as RdbmsType>::DbValue: 'async_trait,
    {
        self.rdbms
            .deref()
            .execute(key, worker_id, statement, params)
            .await
    }

    async fn query_stream(
        &self,
        key: &RdbmsPoolKey,
        worker_id: &WorkerId,
        statement: &str,
        params: Vec<T::DbValue>,
    ) -> Result<Arc<dyn DbResultStream<T> + Send + Sync>, rdbms::Error>
    where
        <T as RdbmsType>::DbValue: 'async_trait,
    {
        self.rdbms
            .deref()
            .query_stream(key, worker_id, statement, params)
            .await
    }

    async fn query(
        &self,
        key: &RdbmsPoolKey,
        worker_id: &WorkerId,
        statement: &str,
        params: Vec<T::DbValue>,
    ) -> Result<DbResult<T>, rdbms::Error>
    where
        <T as RdbmsType>::DbValue: 'async_trait,
    {
        self.rdbms
            .deref()
            .query(key, worker_id, statement, params)
            .await
    }

    async fn begin_transaction(
        &self,
        key: &RdbmsPoolKey,
        worker_id: &WorkerId,
    ) -> Result<Arc<dyn DbTransaction<T> + Send + Sync>, rdbms::Error> {
        self.check_rdbms_tx(worker_id, "BeginTransaction")?;
        self.rdbms.deref().begin_transaction(key, worker_id).await
    }

    async fn get_transaction_status(
        &self,
        key: &RdbmsPoolKey,
        worker_id: &WorkerId,
        transaction_id: &TransactionId,
    ) -> Result<RdbmsTransactionStatus, rdbms::Error> {
        let r = self.check_rdbms_tx(worker_id, "GetTransactionStatusNotFound");
        if r.is_err() {
            Ok(RdbmsTransactionStatus::NotFound)
        } else {
            self.rdbms
                .deref()
                .get_transaction_status(key, worker_id, transaction_id)
                .await
        }
    }

    async fn cleanup_transaction(
        &self,
        key: &RdbmsPoolKey,
        worker_id: &WorkerId,
        transaction_id: &TransactionId,
    ) -> Result<(), rdbms::Error> {
        self.check_rdbms_tx(worker_id, "CleanupTransaction")?;
        self.rdbms
            .deref()
            .cleanup_transaction(key, worker_id, transaction_id)
            .await
    }

    fn status(&self) -> RdbmsStatus {
        self.rdbms.deref().status()
    }
}

#[derive(Clone)]
pub struct AdditionalTestDeps {
    oplog_failures: Arc<DashMap<WorkerId, DashMap<String, usize>>>,
    rdbms_tx_failures: Arc<DashMap<WorkerId, DashMap<String, usize>>>,
}

impl AdditionalTestDeps {
    pub fn new() -> Self {
        let oplog_failures = Arc::new(DashMap::new());
        let rdbms_tx_failures = Arc::new(DashMap::new());
        Self {
            oplog_failures,
            rdbms_tx_failures,
        }
    }

    pub fn get_oplog_failures_count(&self, worker_id: WorkerId, entry: String) -> usize {
        let v = self
            .oplog_failures
            .get(&worker_id)
            .and_then(|v| v.get(&entry).map(|v| *v.value()));
        v.unwrap_or_default()
    }

    pub fn add_oplog_failure(&self, worker_id: WorkerId, entry: String) {
        *self
            .oplog_failures
            .entry(worker_id)
            .or_default()
            .entry(entry)
            .or_default()
            .value_mut() += 1;
    }

    pub fn get_rdbms_tx_failures_count(&self, worker_id: WorkerId, entry: String) -> usize {
        let v = self
            .rdbms_tx_failures
            .get(&worker_id)
            .and_then(|v| v.get(&entry).map(|v| *v.value()));
        v.unwrap_or_default()
    }

    pub fn add_rdbms_tx_failure(&self, worker_id: WorkerId, entry: String) {
        *self
            .rdbms_tx_failures
            .entry(worker_id)
            .or_default()
            .entry(entry)
            .or_default()
            .value_mut() += 1;
    }
}
