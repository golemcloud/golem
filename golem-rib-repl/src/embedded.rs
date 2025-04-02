use crate::dependency_manager::{ReplDependencies, RibComponentMetadata, RibDependencyManager};
use crate::invoke::WorkerFunctionInvoke;
use anyhow::Error;
use async_trait::async_trait;
use golem_api_grpc::proto::golem::workerexecutor::v1::worker_executor_client::WorkerExecutorClient;
use golem_common::base_model::{ComponentId, PluginInstallationId};
use golem_common::model::invocation_context::{
    AttributeValue, InvocationContextSpan, InvocationContextStack, SpanId,
};
use golem_common::model::oplog::WorkerResourceId;
use golem_common::model::{
    AccountId, ComponentFilePath, ComponentType, ComponentVersion, IdempotencyKey, OwnedWorkerId,
    TargetWorkerId, WorkerId, WorkerMetadata, WorkerStatus, WorkerStatusRecord,
};
use golem_service_base::config::{BlobStorageConfig, LocalFileSystemBlobStorageConfig};
use golem_service_base::service::initial_component_files::InitialComponentFilesService;
use golem_service_base::service::plugin_wasm_files::PluginWasmFilesService;
use golem_service_base::storage::blob::fs::FileSystemBlobStorage;
use golem_service_base::storage::blob::BlobStorage;
use golem_test_framework::components::component_compilation_service::ComponentCompilationService;
use golem_test_framework::components::component_service::filesystem::FileSystemComponentService;
use golem_test_framework::components::component_service::ComponentService;
use golem_test_framework::components::rdb::Rdb;
use golem_test_framework::components::redis::Redis;
use golem_test_framework::components::redis_monitor::RedisMonitor;
use golem_test_framework::components::shard_manager::ShardManager;
use golem_test_framework::components::worker_executor::provided::ProvidedWorkerExecutor;
use golem_test_framework::components::worker_executor::WorkerExecutor;
use golem_test_framework::components::worker_executor_cluster::WorkerExecutorCluster;
use golem_test_framework::components::worker_service::forwarding::ForwardingWorkerService;
use golem_test_framework::components::worker_service::WorkerService;
use golem_test_framework::config::TestDependencies;
use golem_test_framework::dsl::TestDslUnsafe;
use golem_wasm_ast::analysis::AnalysedExport;
use golem_wasm_rpc::golem_rpc_0_2_x::types::{
    FutureInvokeResult, HostFutureInvokeResult, Pollable, WasmRpc,
};
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::wasmtime::ResourceStore;
use golem_wasm_rpc::{HostWasmRpc, RpcError, Uri, Value, ValueAndType, WitValue};
use golem_worker_executor_base::durable_host::{
    DurableWorkerCtx, DurableWorkerCtxView, PublicDurableWorkerState,
};
use golem_worker_executor_base::error::GolemError;
use golem_worker_executor_base::model::{
    CurrentResourceLimits, ExecutionStatus, InterruptKind, LastError, ListDirectoryResult,
    ReadFileResult, TrapType, WorkerConfig,
};
use golem_worker_executor_base::preview2::golem::durability;
use golem_worker_executor_base::preview2::golem_api_1_x;
use golem_worker_executor_base::services::active_workers::ActiveWorkers;
use golem_worker_executor_base::services::additional_config::{
    ComponentServiceConfig, ComponentServiceLocalConfig, DefaultAdditionalGolemConfig,
};
use golem_worker_executor_base::services::blob_store::BlobStoreService;
use golem_worker_executor_base::services::component::ComponentMetadata;
use golem_worker_executor_base::services::events::Events;
use golem_worker_executor_base::services::file_loader::FileLoader;
use golem_worker_executor_base::services::golem_config::{
    CompiledComponentServiceConfig, CompiledComponentServiceDisabledConfig, GolemConfig,
    IndexedStorageConfig, IndexedStorageInMemoryConfig, KeyValueStorageConfig,
    KeyValueStorageInMemoryConfig, ShardManagerServiceConfig, ShardManagerServiceSingleShardConfig,
};
use golem_worker_executor_base::services::key_value::KeyValueService;
use golem_worker_executor_base::services::oplog::plugin::OplogProcessorPlugin;
use golem_worker_executor_base::services::oplog::{Oplog, OplogService};
use golem_worker_executor_base::services::plugins::{Plugins, PluginsObservations};
use golem_worker_executor_base::services::promise::PromiseService;
use golem_worker_executor_base::services::rpc::{
    DirectWorkerInvocationRpc, RemoteInvocationRpc, Rpc,
};
use golem_worker_executor_base::services::scheduler::SchedulerService;
use golem_worker_executor_base::services::shard::ShardService;
use golem_worker_executor_base::services::shard_manager::ShardManagerService;
use golem_worker_executor_base::services::worker_activator::WorkerActivator;
use golem_worker_executor_base::services::worker_enumeration::{
    RunningWorkerEnumerationService, WorkerEnumerationService,
};
use golem_worker_executor_base::services::worker_event::WorkerEventService;
use golem_worker_executor_base::services::worker_fork::DefaultWorkerFork;
use golem_worker_executor_base::services::worker_proxy::WorkerProxy;
use golem_worker_executor_base::services::{
    component, plugins, rdbms, All, HasAll, HasConfig, HasOplogService,
};
use golem_worker_executor_base::wasi_host::create_linker;
use golem_worker_executor_base::worker::{RetryDecision, Worker};
use golem_worker_executor_base::workerctx::{
    DynamicLinking, ExternalOperations, FileSystemReading, FuelManagement, IndexedResourceStore,
    InvocationContextManagement, InvocationHooks, InvocationManagement, StatusManagement,
    UpdateManagement, WorkerCtx,
};
use golem_worker_executor_base::{Bootstrap, DefaultGolemTypes, RunDetails};
use prometheus::Registry;
use rib::{EvaluatedFnArgs, EvaluatedFqFn, EvaluatedWorkerName};
use std::collections::{HashMap, HashSet};
use std::fmt::{Debug, Formatter};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock, Weak};
use tokio::runtime::Handle;
use tokio::task::JoinSet;
use tracing::{debug, info};
use uuid::Uuid;
use wasmtime::component::{Component, Instance, Linker, Resource, ResourceAny};
use wasmtime::{AsContextMut, Engine, ResourceLimiterAsync};
use wasmtime_wasi::WasiView;
use wasmtime_wasi_http::WasiHttpView;

pub struct BootstrapDependencies {
    component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
    blob_storage: Arc<dyn BlobStorage + Send + Sync + 'static>,
    initial_component_files_service: Arc<InitialComponentFilesService>,
    plugin_wasm_files_service: Arc<PluginWasmFilesService>,
    component_directory: PathBuf,
}

impl Debug for BootstrapDependencies {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "WorkerExecutorLocalDependencies")
    }
}

impl BootstrapDependencies {
    pub async fn new() -> Self {
        let blob_storage = Arc::new(
            FileSystemBlobStorage::new(Path::new("data/blobs"))
                .await
                .unwrap(),
        );
        let initial_component_files_service =
            Arc::new(InitialComponentFilesService::new(blob_storage.clone()));

        let plugin_wasm_files_service = Arc::new(PluginWasmFilesService::new(blob_storage.clone()));

        let component_directory = Path::new("../test-components").to_path_buf();
        let component_service: Arc<dyn ComponentService + Send + Sync + 'static> = Arc::new(
            FileSystemComponentService::new(
                Path::new("data/components"),
                plugin_wasm_files_service.clone(),
            )
            .await,
        );

        Self {
            component_directory,
            component_service,
            blob_storage,
            initial_component_files_service,
            plugin_wasm_files_service,
        }
    }

    pub fn get_embedded_worker_executor_deps(
        &self,
        http_port: u16,
        grpc_port: u16,
    ) -> EmbeddedWorkerExecutorDependencies {
        // Connecting to the worker executor started in-process
        let worker_executor: Arc<dyn WorkerExecutor + Send + Sync + 'static> = Arc::new(
            ProvidedWorkerExecutor::new("localhost".to_string(), http_port, grpc_port, true),
        );
        // Fake worker service forwarding all requests to the worker executor directly
        let worker_service: Arc<dyn WorkerService + Send + Sync + 'static> = Arc::new(
            ForwardingWorkerService::new(worker_executor.clone(), self.component_service()),
        );

        EmbeddedWorkerExecutorDependencies {
            worker_service,
            component_service: self.component_service().clone(),
            component_directory: self.component_directory.clone(),
            blob_storage: self.blob_storage.clone(),
            initial_component_files_service: self.initial_component_files_service.clone(),
            plugin_wasm_files_service: self.plugin_wasm_files_service.clone(),
            _worker_executor: worker_executor,
        }
    }
}

#[async_trait]
impl TestDependencies for BootstrapDependencies {
    fn rdb(&self) -> Arc<dyn Rdb + Send + Sync + 'static> {
        panic!("Not supported")
    }

    fn redis(&self) -> Arc<dyn Redis + Send + Sync + 'static> {
        panic!("Redis is not used in embedded worker executor in REPL")
    }

    fn redis_monitor(&self) -> Arc<dyn RedisMonitor + Send + Sync + 'static> {
        panic!("Redis monitor is not used in embedded worker executor in REPL")
    }

    fn shard_manager(&self) -> Arc<dyn ShardManager + Send + Sync + 'static> {
        panic!("Shard manager is not used in embedded worker executor in REPL")
    }

    fn component_directory(&self) -> &Path {
        &self.component_directory
    }

    fn component_service(&self) -> Arc<dyn ComponentService + Send + Sync + 'static> {
        self.component_service.clone()
    }

    fn component_compilation_service(
        &self,
    ) -> Arc<dyn ComponentCompilationService + Send + Sync + 'static> {
        panic!("Not supported")
    }

    fn worker_service(&self) -> Arc<dyn WorkerService + Send + Sync + 'static> {
        panic!("Not supported")
    }

    fn worker_executor_cluster(&self) -> Arc<dyn WorkerExecutorCluster + Send + Sync + 'static> {
        panic!("Not supported")
    }

    fn initial_component_files_service(&self) -> Arc<InitialComponentFilesService> {
        self.initial_component_files_service.clone()
    }

    fn blob_storage(&self) -> Arc<dyn BlobStorage + Send + Sync + 'static> {
        self.blob_storage.clone()
    }

    fn plugin_wasm_files_service(&self) -> Arc<PluginWasmFilesService> {
        self.plugin_wasm_files_service.clone()
    }
}

pub async fn start(deps: &BootstrapDependencies) -> anyhow::Result<EmbeddedWorkerExecutor> {
    start_limited(deps).await
}

pub async fn start_limited(deps: &BootstrapDependencies) -> anyhow::Result<EmbeddedWorkerExecutor> {
    let prometheus = golem_worker_executor_base::metrics::register_all();
    let config = GolemConfig {
        key_value_storage: KeyValueStorageConfig::InMemory(KeyValueStorageInMemoryConfig {}),
        indexed_storage: IndexedStorageConfig::InMemory(IndexedStorageInMemoryConfig {}),
        blob_storage: BlobStorageConfig::LocalFileSystem(LocalFileSystemBlobStorageConfig {
            root: Path::new("data/blobs").to_path_buf(),
        }),
        port: 0,
        http_port: 0,

        compiled_component_service: CompiledComponentServiceConfig::Disabled(
            CompiledComponentServiceDisabledConfig {},
        ),
        shard_manager_service: ShardManagerServiceConfig::SingleShard(
            ShardManagerServiceSingleShardConfig {},
        ),
        ..Default::default()
    };

    let additional_config = DefaultAdditionalGolemConfig {
        component_service: ComponentServiceConfig::Local(ComponentServiceLocalConfig {
            root: Path::new("data/components").to_path_buf(),
        }),
        ..Default::default()
    };

    let handle = Handle::current();

    let mut join_set = JoinSet::new();

    let details = run(config, additional_config, prometheus, handle, &mut join_set).await?;
    let grpc_port = details.grpc_port;

    let start = std::time::Instant::now();
    loop {
        let client = WorkerExecutorClient::connect(format!("http://127.0.0.1:{grpc_port}")).await;
        if client.is_ok() {
            let deps = deps.get_embedded_worker_executor_deps(details.http_port, grpc_port);
            break Ok(EmbeddedWorkerExecutor {
                _join_set: Some(join_set),
                deps,
            });
        } else if start.elapsed().as_secs() > 10 {
            break Err(anyhow::anyhow!("Timeout waiting for server to start"));
        }
    }
}

#[derive(Clone)]
pub struct EmbeddedWorkerExecutorDependencies {
    worker_service: Arc<dyn WorkerService + Send + Sync + 'static>,
    component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
    blob_storage: Arc<dyn BlobStorage + Send + Sync + 'static>,
    initial_component_files_service: Arc<InitialComponentFilesService>,
    plugin_wasm_files_service: Arc<PluginWasmFilesService>,
    component_directory: PathBuf,
    _worker_executor: Arc<dyn WorkerExecutor + Send + Sync + 'static>,
}

#[async_trait]
impl TestDependencies for EmbeddedWorkerExecutorDependencies {
    fn rdb(&self) -> Arc<dyn Rdb + Send + Sync + 'static> {
        panic!("Not supported")
    }

    fn redis(&self) -> Arc<dyn Redis + Send + Sync + 'static> {
        panic!("Not supported")
    }

    fn redis_monitor(&self) -> Arc<dyn RedisMonitor + Send + Sync + 'static> {
        panic!("Not supported")
    }

    fn shard_manager(&self) -> Arc<dyn ShardManager + Send + Sync + 'static> {
        panic!("Not supported")
    }

    fn component_directory(&self) -> &Path {
        &self.component_directory
    }

    fn component_service(&self) -> Arc<dyn ComponentService + Send + Sync + 'static> {
        self.component_service.clone()
    }

    fn component_compilation_service(
        &self,
    ) -> Arc<dyn ComponentCompilationService + Send + Sync + 'static> {
        panic!("Not supported")
    }

    fn worker_service(&self) -> Arc<dyn WorkerService + Send + Sync + 'static> {
        self.worker_service.clone()
    }

    fn worker_executor_cluster(&self) -> Arc<dyn WorkerExecutorCluster + Send + Sync + 'static> {
        panic!("Not supported")
    }

    fn blob_storage(&self) -> Arc<dyn BlobStorage + Send + Sync + 'static> {
        self.blob_storage.clone()
    }

    fn initial_component_files_service(&self) -> Arc<InitialComponentFilesService> {
        self.initial_component_files_service.clone()
    }

    fn plugin_wasm_files_service(&self) -> Arc<PluginWasmFilesService> {
        self.plugin_wasm_files_service.clone()
    }
}

async fn run(
    golem_config: GolemConfig,
    additional_config: DefaultAdditionalGolemConfig,
    prometheus_registry: Registry,
    runtime: Handle,
    join_set: &mut JoinSet<Result<(), anyhow::Error>>,
) -> Result<RunDetails, anyhow::Error> {
    info!("Golem Worker Executor starting up...");

    ServerBootstrap { additional_config }
        .run(golem_config, prometheus_registry, runtime, join_set)
        .await
}

pub struct EmbeddedWorkerExecutor {
    deps: EmbeddedWorkerExecutorDependencies,
    _join_set: Option<JoinSet<anyhow::Result<()>>>,
}

struct ServerBootstrap {
    additional_config: DefaultAdditionalGolemConfig,
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
    ) -> Arc<dyn golem_worker_executor_base::services::component::ComponentService<DefaultGolemTypes>>
    {
        component::configured(
            &self.additional_config.component_service,
            &self.additional_config.component_cache,
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
        component_service: Arc<
            dyn golem_worker_executor_base::services::component::ComponentService<
                DefaultGolemTypes,
            >,
        >,
        shard_manager_service: Arc<dyn ShardManagerService + Send + Sync>,
        worker_service: Arc<
            dyn golem_worker_executor_base::services::worker::WorkerService + Send + Sync,
        >,
        worker_enumeration_service: Arc<dyn WorkerEnumerationService + Send + Sync>,
        running_worker_enumeration_service: Arc<dyn RunningWorkerEnumerationService + Send + Sync>,
        promise_service: Arc<dyn PromiseService + Send + Sync>,
        golem_config: Arc<GolemConfig>,
        shard_service: Arc<dyn ShardService + Send + Sync>,
        key_value_service: Arc<dyn KeyValueService + Send + Sync>,
        blob_store_service: Arc<dyn BlobStoreService + Send + Sync>,
        rdbms_service: Arc<dyn rdbms::RdbmsService + Send + Sync>,
        worker_activator: Arc<dyn WorkerActivator<TestWorkerCtx> + Send + Sync>,
        oplog_service: Arc<dyn OplogService + Send + Sync>,
        scheduler_service: Arc<dyn SchedulerService + Send + Sync>,
        worker_proxy: Arc<dyn WorkerProxy + Send + Sync>,
        events: Arc<Events>,
        file_loader: Arc<FileLoader>,
        plugins: Arc<dyn Plugins<DefaultGolemTypes>>,
        oplog_processor_plugin: Arc<dyn OplogProcessorPlugin + Send + Sync>,
    ) -> anyhow::Result<All<TestWorkerCtx>> {
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

#[async_trait]
impl WorkerCtx for TestWorkerCtx {
    type Types = DefaultGolemTypes;
    type PublicState = PublicDurableWorkerState<TestWorkerCtx>;

    async fn create(
        owned_worker_id: OwnedWorkerId,
        component_metadata: ComponentMetadata<DefaultGolemTypes>,
        promise_service: Arc<dyn PromiseService + Send + Sync>,
        worker_service: Arc<
            dyn golem_worker_executor_base::services::worker::WorkerService + Send + Sync,
        >,
        worker_enumeration_service: Arc<dyn WorkerEnumerationService + Send + Sync>,
        key_value_service: Arc<dyn KeyValueService + Send + Sync>,
        blob_store_service: Arc<dyn BlobStoreService + Send + Sync>,
        rdbms_service: Arc<dyn rdbms::RdbmsService + Send + Sync>,
        event_service: Arc<dyn WorkerEventService + Send + Sync>,
        _active_workers: Arc<ActiveWorkers<TestWorkerCtx>>,
        oplog_service: Arc<dyn OplogService + Send + Sync>,
        oplog: Arc<dyn Oplog + Send + Sync>,
        invocation_queue: Weak<Worker<TestWorkerCtx>>,
        scheduler_service: Arc<dyn SchedulerService + Send + Sync>,
        rpc: Arc<dyn Rpc + Send + Sync>,
        worker_proxy: Arc<dyn WorkerProxy + Send + Sync>,
        component_service: Arc<
            dyn golem_worker_executor_base::services::component::ComponentService<
                DefaultGolemTypes,
            >,
        >,
        _extra_deps: Self::ExtraDeps,
        config: Arc<GolemConfig>,
        worker_config: WorkerConfig,
        execution_status: Arc<RwLock<ExecutionStatus>>,
        file_loader: Arc<FileLoader>,
        plugins: Arc<dyn Plugins<DefaultGolemTypes>>,
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

    fn rpc(&self) -> Arc<dyn Rpc + Send + Sync> {
        self.durable_ctx.rpc()
    }

    fn worker_proxy(&self) -> Arc<dyn WorkerProxy + Send + Sync> {
        self.durable_ctx.worker_proxy()
    }

    fn component_service(&self) -> Arc<dyn component::ComponentService<Self::Types> + Send + Sync> {
        self.durable_ctx.component_service()
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

#[async_trait]
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

#[async_trait]
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
impl TestDependencies for EmbeddedWorkerExecutor {
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

    fn component_service(
        &self,
    ) -> Arc<
        dyn golem_test_framework::components::component_service::ComponentService
            + Send
            + Sync
            + 'static,
    > {
        self.deps.component_service()
    }

    fn component_compilation_service(
        &self,
    ) -> Arc<dyn ComponentCompilationService + Send + Sync + 'static> {
        self.deps.component_compilation_service()
    }

    fn worker_service(
        &self,
    ) -> Arc<
        dyn golem_test_framework::components::worker_service::WorkerService + Send + Sync + 'static,
    > {
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

// Embedded Dependency Manger

// A default Rib dependency manager is mainly allowing rib to be used standalone
// without the nuances of app manifest. This is mainly used for testing the REPL itself
pub struct EmbeddedDependencyManager {
    pub embedded_worker_executor: Arc<EmbeddedWorkerExecutor>,
}

impl EmbeddedDependencyManager {
    pub async fn new(
        embedded_worker_executor: Arc<EmbeddedWorkerExecutor>,
    ) -> Result<Self, String> {
        Ok(Self {
            embedded_worker_executor,
        })
    }
}

#[async_trait]
impl RibDependencyManager for EmbeddedDependencyManager {
    async fn get_dependencies(&self) -> Result<ReplDependencies, String> {
        Err("multiple components not supported in embedded mode".to_string())
    }

    async fn add_component(
        &self,
        source_path: &Path,
        component_name: String,
    ) -> Result<RibComponentMetadata, String> {
        let component_id = self
            .embedded_worker_executor
            .component(component_name.as_str())
            .store()
            .await;

        let result = self
            .embedded_worker_executor
            .component_service()
            .get_or_add_component(
                source_path,
                &component_name,
                ComponentType::Durable,
                &[],
                &HashMap::new(),
                false,
            )
            .await;

        Ok(RibComponentMetadata {
            component_id: component_id.0,
            metadata: result
                .metadata
                .map(|metadata| {
                    metadata
                        .exports
                        .iter()
                        .map(|m| AnalysedExport::try_from(m.clone()).unwrap())
                        .collect()
                })
                .unwrap_or_default(),
        })
    }
}

// Embedded RibFunctionInvoke implementation
pub struct EmbeddedWorkerFunctionInvoke {
    embedded_worker_executor: Arc<EmbeddedWorkerExecutor>,
}

impl EmbeddedWorkerFunctionInvoke {
    pub fn new(embedded_worker_executor: Arc<EmbeddedWorkerExecutor>) -> Self {
        Self {
            embedded_worker_executor,
        }
    }
}

#[async_trait]
impl WorkerFunctionInvoke for EmbeddedWorkerFunctionInvoke {
    async fn invoke(
        &self,
        component_id: Uuid,
        worker_name: Option<EvaluatedWorkerName>,
        function_name: EvaluatedFqFn,
        args: EvaluatedFnArgs,
    ) -> Result<ValueAndType, String> {
        let target_worker_id = worker_name
            .map(|w| TargetWorkerId {
                component_id: ComponentId(component_id),
                worker_name: Some(w.0),
            })
            .unwrap_or_else(|| TargetWorkerId {
                component_id: ComponentId(component_id),
                worker_name: None,
            });

        let function_name = function_name.0;

        self.embedded_worker_executor
            .invoke_and_await_typed(target_worker_id, function_name.as_str(), args.0)
            .await
            .map_err(|e| format!("Failed to invoke function: {:?}", e))
    }
}
