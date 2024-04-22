use anyhow::Error;
use async_trait::async_trait;
use ctor::ctor;

use golem_wasm_rpc::wasmtime::ResourceStore;
use golem_wasm_rpc::{Uri, Value};
use prometheus::Registry;

use std::path::{Path, PathBuf};
use std::string::FromUtf8Error;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, RwLock};

use crate::{WorkerExecutorPerTestDependencies, BASE_DEPS};

use golem_api_grpc::proto::golem::workerexecutor::worker_executor_client::WorkerExecutorClient;

use golem_common::model::{
    AccountId, InvocationKey, ComponentId, WorkerFilter, WorkerId, WorkerMetadata, WorkerStatus,
    WorkerStatusRecord,
};
use golem_worker_executor_base::error::GolemError;
use golem_worker_executor_base::services::golem_config::{
    BlobStoreServiceConfig, BlobStoreServiceInMemoryConfig, CompiledComponentServiceConfig,
    CompiledComponentServiceLocalConfig, GolemConfig, KeyValueServiceConfig, PromisesConfig,
    ShardManagerServiceConfig, ComponentServiceConfig, ComponentServiceLocalConfig,
    WorkerServiceGrpcConfig, WorkersServiceConfig,
};

use golem_worker_executor_base::durable_host::{
    DurableWorkerCtx, DurableWorkerCtxView, PublicDurableWorkerState,
};
use golem_worker_executor_base::model::{
    CurrentResourceLimits, ExecutionStatus, InterruptKind, LastError, TrapType, WorkerConfig,
};
use golem_worker_executor_base::services::active_workers::ActiveWorkers;
use golem_worker_executor_base::services::blob_store::BlobStoreService;
use golem_worker_executor_base::services::invocation_key::InvocationKeyService;
use golem_worker_executor_base::services::key_value::KeyValueService;
use golem_worker_executor_base::services::oplog::{Oplog, OplogService};
use golem_worker_executor_base::services::promise::PromiseService;
use golem_worker_executor_base::services::recovery::{
    RecoveryManagement, RecoveryManagementDefault,
};
use golem_worker_executor_base::services::scheduler::SchedulerService;
use golem_worker_executor_base::services::shard::ShardService;
use golem_worker_executor_base::services::shard_manager::ShardManagerService;
use golem_worker_executor_base::services::component::ComponentService;
use golem_worker_executor_base::services::worker::WorkerService;
use golem_worker_executor_base::services::worker_activator::WorkerActivator;
use golem_worker_executor_base::services::worker_event::WorkerEventService;
use golem_worker_executor_base::services::{worker_enumeration, All, HasAll};
use golem_worker_executor_base::wasi_host::create_linker;
use golem_worker_executor_base::workerctx::{
    ExternalOperations, FuelManagement, InvocationHooks, InvocationManagement, IoCapturing,
    StatusManagement, WorkerCtx,
};
use golem_worker_executor_base::Bootstrap;

use tokio::runtime::Handle;

use tokio::task::JoinHandle;

use golem::api;
use golem_common::config::RedisConfig;

use golem_api_grpc::proto::golem::workerexecutor::{
    get_running_workers_metadata_response, get_workers_metadata_response,
    GetRunningWorkersMetadataRequest, GetRunningWorkersMetadataSuccessResponse,
    GetWorkersMetadataRequest, GetWorkersMetadataSuccessResponse,
};
use golem_test_framework::components::rdb::Rdb;
use golem_test_framework::components::redis::Redis;
use golem_test_framework::components::redis_monitor::RedisMonitor;
use golem_test_framework::components::shard_manager::ShardManager;
use golem_test_framework::components::worker_executor_cluster::WorkerExecutorCluster;
use golem_test_framework::config::TestDependencies;
use golem_test_framework::dsl::to_worker_metadata;
use golem_worker_executor_base::preview2::golem;
use golem_worker_executor_base::services::invocation_queue::InvocationQueue;
use golem_worker_executor_base::services::rpc::{
    DirectWorkerInvocationRpc, RemoteInvocationRpc, Rpc,
};
use golem_worker_executor_base::services::worker_enumeration::{
    RunningWorkerEnumerationService, WorkerEnumerationService,
};
use tonic::transport::Channel;
use tracing::{error, info};
use uuid::Uuid;
use wasmtime::component::{Instance, Linker, ResourceAny};
use wasmtime::{AsContextMut, Engine, ResourceLimiterAsync};

pub struct TestWorkerExecutor {
    handle: Option<JoinHandle<Result<(), String>>>,
    deps: WorkerExecutorPerTestDependencies,
}

impl TestWorkerExecutor {
    pub async fn client(&self) -> WorkerExecutorClient<Channel> {
        self.deps.worker_executor.client().await
    }

    pub async fn get_running_workers_metadata(
        &self,
        component_id: &ComponentId,
        filter: Option<WorkerFilter>,
    ) -> Vec<WorkerMetadata> {
        let component_id: golem_api_grpc::proto::golem::component::ComponentId =
            component_id.clone().into();
        let response = self
            .client()
            .await
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
        cursor: u64,
        count: u64,
        precise: bool,
    ) -> (Option<u64>, Vec<WorkerMetadata>) {
        let component_id: golem_api_grpc::proto::golem::component::ComponentId =
            component_id.clone().into();
        let response = self
            .client()
            .await
            .get_workers_metadata(GetWorkersMetadataRequest {
                component_id: Some(component_id),
                filter: filter.map(|f| f.into()),
                cursor,
                count,
                precise,
            })
            .await
            .expect("Failed to get workers metadata")
            .into_inner();

        match response.result {
            None => panic!("No response from get_workers_metadata"),
            Some(get_workers_metadata_response::Result::Success(
                GetWorkersMetadataSuccessResponse { workers, cursor },
            )) => (cursor, workers.iter().map(to_worker_metadata).collect()),
            Some(get_workers_metadata_response::Result::Failure(error)) => {
                panic!("Failed to get workers metadata: {error:?}")
            }
        }
    }
}

impl Clone for TestWorkerExecutor {
    fn clone(&self) -> Self {
        Self {
            handle: None,
            deps: self.deps.clone(),
        }
    }
}

impl TestDependencies for TestWorkerExecutor {
    fn rdb(&self) -> Arc<dyn Rdb + Send + Sync + 'static> {
        self.deps.rdb()
    }

    fn redis(&self) -> Arc<dyn Redis + Send + Sync + 'static> {
        self.deps.redis()
    }

    fn redis_monitor(&self) -> Arc<dyn RedisMonitor + Send + Sync + 'static> {
        self.deps.redis_monitor()
    }

    fn shard_manager(&self) -> Arc<dyn ShardManager + Send + Sync + 'static> {
        self.deps.shard_manager()
    }

    fn component_directory(&self) -> PathBuf {
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
}

impl Drop for TestWorkerExecutor {
    fn drop(&mut self) {
        if let Some(handle) = &self.handle {
            handle.abort()
        }
    }
}

pub struct TestContext {
    unique_id: u16,
}

#[ctor]
pub static LAST_UNIQUE_ID: AtomicU16 = AtomicU16::new(0);

impl TestContext {
    pub fn new() -> Self {
        let unique_id = LAST_UNIQUE_ID.fetch_add(1, Ordering::Relaxed);
        Self { unique_id }
    }

    pub fn redis_prefix(&self) -> String {
        format!("test-{}:", self.unique_id)
    }

    pub fn grpc_port(&self) -> u16 {
        9000 + (self.unique_id * 3)
    }

    pub fn http_port(&self) -> u16 {
        9001 + (self.unique_id * 3)
    }

    pub fn host_http_port(&self) -> u16 {
        9002 + (self.unique_id * 3)
    }
}

pub async fn start(context: &TestContext) -> anyhow::Result<TestWorkerExecutor> {
    let redis = BASE_DEPS.redis();
    let redis_monitor = BASE_DEPS.redis_monitor();
    redis.assert_valid();
    redis_monitor.assert_valid();
    println!("Using Redis on port {}", redis.public_port());

    let prometheus = golem_worker_executor_base::metrics::register_all();
    let config = GolemConfig {
        port: context.grpc_port(),
        http_port: context.http_port(),
        component_service: ComponentServiceConfig::Local(ComponentServiceLocalConfig {
            root: Path::new("data/components").to_path_buf(),
        }),
        compiled_component_service: CompiledComponentServiceConfig::Local(
            CompiledComponentServiceLocalConfig {
                root: Path::new("data/components").to_path_buf(),
            },
        ),
        blob_store_service: BlobStoreServiceConfig::InMemory(BlobStoreServiceInMemoryConfig {}),
        key_value_service: KeyValueServiceConfig::Redis,
        shard_manager_service: ShardManagerServiceConfig::SingleShard,
        promises: PromisesConfig::Redis,
        workers: WorkersServiceConfig::Redis,
        redis: RedisConfig {
            port: redis.public_port(),
            key_prefix: context.redis_prefix(),
            ..Default::default()
        },
        public_worker_api: WorkerServiceGrpcConfig {
            host: "localhost".to_string(),
            port: context.grpc_port(),
            access_token: "03494299-B515-4427-8C37-4C1C915679B7".to_string(),
        },
        ..Default::default()
    };

    let handle = Handle::current();

    let grpc_port = config.port;

    let server_handle = tokio::spawn(async move {
        let r = run(config, prometheus, handle)
            .await
            .map_err(|e| format!("{e}"));
        match &r {
            Ok(_) => info!("Server finished successfully"),
            Err(e) => error!("Server finished with error: {e}"),
        }
        r
    });

    let start = std::time::Instant::now();
    loop {
        let client = WorkerExecutorClient::connect(format!("http://127.0.0.1:{grpc_port}")).await;
        if client.is_ok() {
            let deps = BASE_DEPS.per_test(
                &context.redis_prefix(),
                context.http_port(),
                context.grpc_port(),
            );
            break Ok(TestWorkerExecutor {
                handle: Some(server_handle),
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
) -> Result<(), Box<dyn std::error::Error>> {
    info!("Golem Worker Executor starting up...");
    Ok(ServerBootstrap {}
        .run(golem_config, prometheus_registry, runtime)
        .await?)
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
impl ExternalOperations<TestWorkerCtx> for TestWorkerCtx {
    type ExtraDeps = ();

    async fn set_worker_status<T: HasAll<TestWorkerCtx> + Send + Sync>(
        this: &T,
        worker_id: &WorkerId,
        status: WorkerStatus,
    ) -> Result<(), GolemError> {
        DurableWorkerCtx::<TestWorkerCtx>::set_worker_status(this, worker_id, status).await
    }

    async fn get_last_error_and_retry_count<T: HasAll<TestWorkerCtx> + Send + Sync>(
        this: &T,
        worker_id: &WorkerId,
    ) -> Option<LastError> {
        DurableWorkerCtx::<TestWorkerCtx>::get_last_error_and_retry_count(this, worker_id).await
    }

    async fn compute_latest_worker_status<T: HasAll<TestWorkerCtx> + Send + Sync>(
        this: &T,
        worker_id: &WorkerId,
        metadata: &Option<WorkerMetadata>,
    ) -> Result<WorkerStatusRecord, GolemError> {
        DurableWorkerCtx::<TestWorkerCtx>::compute_latest_worker_status(this, worker_id, metadata)
            .await
    }

    async fn prepare_instance(
        worker_id: &WorkerId,
        instance: &Instance,
        store: &mut (impl AsContextMut<Data = TestWorkerCtx> + Send),
    ) -> Result<(), GolemError> {
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

    async fn on_shard_assignment_changed<T: HasAll<TestWorkerCtx> + Send + Sync>(
        this: &T,
    ) -> Result<(), Error> {
        DurableWorkerCtx::<TestWorkerCtx>::on_shard_assignment_changed(this).await
    }
}

#[async_trait]
impl InvocationManagement for TestWorkerCtx {
    async fn set_current_invocation_key(&mut self, invocation_key: InvocationKey) {
        self.durable_ctx
            .set_current_invocation_key(invocation_key)
            .await
    }

    async fn get_current_invocation_key(&self) -> Option<InvocationKey> {
        self.durable_ctx.get_current_invocation_key().await
    }

    async fn interrupt_invocation_key(&mut self, key: &InvocationKey) {
        self.durable_ctx.interrupt_invocation_key(key).await
    }

    async fn resume_invocation_key(&mut self, key: &InvocationKey) {
        self.durable_ctx.resume_invocation_key(key).await
    }

    async fn confirm_invocation_key(
        &mut self,
        key: &InvocationKey,
        vals: Result<Vec<Value>, GolemError>,
    ) {
        self.durable_ctx.confirm_invocation_key(key, vals).await
    }
}

#[async_trait]
impl IoCapturing for TestWorkerCtx {
    async fn start_capturing_stdout(&mut self, provided_stdin: String) {
        self.durable_ctx
            .start_capturing_stdout(provided_stdin)
            .await
    }

    async fn finish_capturing_stdout(&mut self) -> Result<String, FromUtf8Error> {
        self.durable_ctx.finish_capturing_stdout().await
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

    async fn get_worker_status(&self) -> WorkerStatus {
        self.durable_ctx.get_worker_status().await
    }

    async fn store_worker_status(&self, status: WorkerStatus) {
        self.durable_ctx.store_worker_status(status).await
    }

    async fn update_pending_invocations(&self) {
        self.durable_ctx.update_pending_invocations().await
    }

    async fn deactivate(&self) {
        self.durable_ctx.deactivate().await
    }
}

#[async_trait]
impl InvocationHooks for TestWorkerCtx {
    async fn on_exported_function_invoked(
        &mut self,
        full_function_name: &str,
        function_input: &Vec<Value>,
        calling_convention: Option<golem_common::model::CallingConvention>,
    ) -> anyhow::Result<()> {
        self.durable_ctx
            .on_exported_function_invoked(full_function_name, function_input, calling_convention)
            .await
    }

    async fn on_invocation_failure(&mut self, trap_type: &TrapType) -> Result<(), Error> {
        self.durable_ctx.on_invocation_failure(trap_type).await
    }

    async fn on_invocation_failure_deactivated(
        &mut self,
        trap_type: &TrapType,
    ) -> Result<WorkerStatus, Error> {
        self.durable_ctx
            .on_invocation_failure_deactivated(trap_type)
            .await
    }

    async fn on_invocation_success(
        &mut self,
        full_function_name: &str,
        function_input: &Vec<Value>,
        consumed_fuel: i64,
        output: Vec<Value>,
    ) -> Result<Option<Vec<Value>>, Error> {
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

    fn add(&mut self, resource: ResourceAny) -> u64 {
        self.durable_ctx.add(resource)
    }

    fn get(&mut self, resource_id: u64) -> Option<ResourceAny> {
        self.durable_ctx.get(resource_id)
    }

    fn borrow(&self, resource_id: u64) -> Option<ResourceAny> {
        self.durable_ctx.borrow(resource_id)
    }
}

struct ServerBootstrap {}

#[async_trait]
impl WorkerCtx for TestWorkerCtx {
    type PublicState = PublicDurableWorkerState<TestWorkerCtx>;

    async fn create(
        worker_id: WorkerId,
        account_id: AccountId,
        promise_service: Arc<dyn PromiseService + Send + Sync>,
        invocation_key_service: Arc<dyn InvocationKeyService + Send + Sync>,
        worker_service: Arc<dyn WorkerService + Send + Sync>,
        worker_enumeration_service: Arc<
            dyn worker_enumeration::WorkerEnumerationService + Send + Sync,
        >,
        key_value_service: Arc<dyn KeyValueService + Send + Sync>,
        blob_store_service: Arc<dyn BlobStoreService + Send + Sync>,
        event_service: Arc<dyn WorkerEventService + Send + Sync>,
        active_workers: Arc<ActiveWorkers<TestWorkerCtx>>,
        oplog_service: Arc<dyn OplogService + Send + Sync>,
        oplog: Arc<dyn Oplog + Send + Sync>,
        invocation_queue: Arc<InvocationQueue<TestWorkerCtx>>,
        scheduler_service: Arc<dyn SchedulerService + Send + Sync>,
        recovery_management: Arc<dyn RecoveryManagement + Send + Sync>,
        rpc: Arc<dyn Rpc + Send + Sync>,
        _extra_deps: Self::ExtraDeps,
        config: Arc<GolemConfig>,
        worker_config: WorkerConfig,
        execution_status: Arc<RwLock<ExecutionStatus>>,
    ) -> Result<Self, GolemError> {
        let durable_ctx = DurableWorkerCtx::create(
            worker_id,
            account_id,
            promise_service,
            invocation_key_service,
            worker_service,
            worker_enumeration_service,
            key_value_service,
            blob_store_service,
            event_service,
            active_workers,
            oplog_service,
            oplog,
            invocation_queue,
            scheduler_service,
            recovery_management,
            rpc,
            config,
            worker_config,
            execution_status,
        )
        .await?;
        Ok(Self { durable_ctx })
    }

    fn get_public_state(&self) -> &Self::PublicState {
        self.durable_ctx.get_public_state()
    }

    fn resource_limiter(&mut self) -> &mut dyn ResourceLimiterAsync {
        self
    }

    fn worker_id(&self) -> &WorkerId {
        self.durable_ctx.worker_id()
    }

    fn is_exit(error: &Error) -> Option<i32> {
        DurableWorkerCtx::<TestWorkerCtx>::is_exit(error)
    }

    fn rpc(&self) -> Arc<dyn Rpc + Send + Sync> {
        self.durable_ctx.rpc()
    }
}

#[async_trait]
impl ResourceLimiterAsync for TestWorkerCtx {
    async fn memory_growing(
        &mut self,
        _current: usize,
        _desired: usize,
        _maximum: Option<usize>,
    ) -> anyhow::Result<bool> {
        Ok(true)
    }

    async fn table_growing(
        &mut self,
        _current: u32,
        _desired: u32,
        _maximum: Option<u32>,
    ) -> anyhow::Result<bool> {
        Ok(true)
    }
}

#[async_trait]
impl Bootstrap<TestWorkerCtx> for ServerBootstrap {
    fn create_active_workers(
        &self,
        golem_config: &GolemConfig,
    ) -> Arc<ActiveWorkers<TestWorkerCtx>> {
        Arc::new(ActiveWorkers::<TestWorkerCtx>::bounded(
            golem_config.limits.max_active_workers,
            golem_config.active_workers.drop_when_full,
            golem_config.active_workers.ttl,
        ))
    }

    async fn create_services(
        &self,
        active_workers: Arc<ActiveWorkers<TestWorkerCtx>>,
        engine: Arc<Engine>,
        linker: Arc<Linker<TestWorkerCtx>>,
        runtime: Handle,
        component_service: Arc<dyn ComponentService + Send + Sync>,
        shard_manager_service: Arc<dyn ShardManagerService + Send + Sync>,
        worker_service: Arc<dyn WorkerService + Send + Sync>,
        worker_enumeration_service: Arc<dyn WorkerEnumerationService + Send + Sync>,
        running_worker_enumeration_service: Arc<dyn RunningWorkerEnumerationService + Send + Sync>,
        promise_service: Arc<dyn PromiseService + Send + Sync>,
        golem_config: Arc<GolemConfig>,
        invocation_key_service: Arc<dyn InvocationKeyService + Send + Sync>,
        shard_service: Arc<dyn ShardService + Send + Sync>,
        key_value_service: Arc<dyn KeyValueService + Send + Sync>,
        blob_store_service: Arc<dyn BlobStoreService + Send + Sync>,
        _worker_activator: Arc<dyn WorkerActivator + Send + Sync>,
        oplog_service: Arc<dyn OplogService + Send + Sync>,
        scheduler_service: Arc<dyn SchedulerService + Send + Sync>,
    ) -> anyhow::Result<All<TestWorkerCtx>> {
        let rpc = Arc::new(DirectWorkerInvocationRpc::new(
            Arc::new(RemoteInvocationRpc::new(
                golem_config.public_worker_api.uri(),
                golem_config
                    .public_worker_api
                    .access_token
                    .parse::<Uuid>()
                    .expect("Access token must be an UUID"),
            )),
            active_workers.clone(),
            engine.clone(),
            linker.clone(),
            runtime.clone(),
            component_service.clone(),
            worker_service.clone(),
            worker_enumeration_service.clone(),
            running_worker_enumeration_service.clone(),
            promise_service.clone(),
            golem_config.clone(),
            invocation_key_service.clone(),
            shard_service.clone(),
            shard_manager_service.clone(),
            key_value_service.clone(),
            blob_store_service.clone(),
            oplog_service.clone(),
            scheduler_service.clone(),
            (),
        ));
        let recovery_management = Arc::new(RecoveryManagementDefault::new(
            active_workers.clone(),
            engine.clone(),
            linker.clone(),
            runtime.clone(),
            component_service.clone(),
            worker_service.clone(),
            worker_enumeration_service.clone(),
            running_worker_enumeration_service.clone(),
            oplog_service.clone(),
            promise_service.clone(),
            scheduler_service.clone(),
            invocation_key_service.clone(),
            key_value_service.clone(),
            blob_store_service.clone(),
            rpc.clone(),
            golem_config.clone(),
            (),
        ));
        rpc.set_recovery_management(recovery_management.clone());

        Ok(All::new(
            active_workers,
            engine,
            linker,
            runtime,
            component_service,
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
            (),
        ))
    }

    fn create_wasmtime_linker(&self, engine: &Engine) -> anyhow::Result<Linker<TestWorkerCtx>> {
        let mut linker =
            create_linker::<TestWorkerCtx, DurableWorkerCtx<TestWorkerCtx>>(engine, |x| {
                &mut x.durable_ctx
            })?;
        api::host::add_to_linker::<TestWorkerCtx, DurableWorkerCtx<TestWorkerCtx>>(
            &mut linker,
            |x| &mut x.durable_ctx,
        )?;
        golem_wasm_rpc::golem::rpc::types::add_to_linker::<
            TestWorkerCtx,
            DurableWorkerCtx<TestWorkerCtx>,
        >(&mut linker, |x| &mut x.durable_ctx)?;
        Ok(linker)
    }
}
