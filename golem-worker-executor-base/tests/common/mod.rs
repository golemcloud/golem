use anyhow::Error;
use async_trait::async_trait;
use prometheus::Registry;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, RwLock};
use std::{env, panic};

use crate::REDIS;
use golem_common::model::{AccountId, TemplateId, VersionedWorkerId, WorkerId};
use golem_common::proto::golem::workerexecutor::worker_executor_client::WorkerExecutorClient;
use golem_common::proto::golem::workerexecutor::{
    create_worker_response, get_invocation_key_response, interrupt_worker_response,
    invoke_and_await_worker_response, ConnectWorkerRequest, CreateWorkerRequest,
    GetInvocationKeyRequest, InterruptWorkerRequest, InterruptWorkerResponse,
    InvokeAndAwaitWorkerRequest,
};
use golem_common::proto::golem::{
    log_event, val, CallingConvention, LogEvent, StdOutLog, Val, ValList, ValRecord,
};
use golem_worker_executor_base::error::GolemError;
use golem_worker_executor_base::services::golem_config::{
    BlobStoreServiceConfig, BlobStoreServiceInMemoryConfig, CompiledTemplateServiceConfig,
    CompiledTemplateServiceLocalConfig, GolemConfig, KeyValueServiceConfig, PromisesConfig,
    ShardManagerServiceConfig, TemplateServiceConfig, TemplateServiceLocalConfig,
    WorkersServiceConfig,
};

use golem_worker_executor_base::golem_host::{GolemCtx, GolemPublicState, HasGolemCtx};
use golem_worker_executor_base::model::{ExecutionStatus, WorkerConfig};
use golem_worker_executor_base::services::active_workers::ActiveWorkers;
use golem_worker_executor_base::services::blob_store::BlobStoreService;
use golem_worker_executor_base::services::invocation_key::InvocationKeyService;
use golem_worker_executor_base::services::key_value::KeyValueService;
use golem_worker_executor_base::services::oplog::OplogService;
use golem_worker_executor_base::services::promise::PromiseService;
use golem_worker_executor_base::services::recovery::{
    RecoveryManagement, RecoveryManagementDefault,
};
use golem_worker_executor_base::services::scheduler::SchedulerService;
use golem_worker_executor_base::services::shard::ShardService;
use golem_worker_executor_base::services::shard_manager::ShardManagerService;
use golem_worker_executor_base::services::template::TemplateService;
use golem_worker_executor_base::services::worker::WorkerService;
use golem_worker_executor_base::services::worker_activator::WorkerActivator;
use golem_worker_executor_base::services::worker_event::WorkerEventService;
use golem_worker_executor_base::services::All;
use golem_worker_executor_base::wasi_host::create_linker;
use golem_worker_executor_base::workerctx::{FuelManagement, WorkerCtx};
use golem_worker_executor_base::{golem_host, Bootstrap};
use serde_json::Value;
use tokio::runtime::Handle;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::task::JoinHandle;
use tonic::transport::Channel;
use tracing::{debug, error, info};
use uuid::Uuid;
use wasmtime::component::Linker;
use wasmtime::{Engine, ResourceLimiterAsync};

pub struct TestWorkerExecutor {
    pub client: WorkerExecutorClient<Channel>,
    handle: Option<JoinHandle<Result<(), String>>>,
    grpc_port: u16,
}

impl TestWorkerExecutor {
    pub fn store_template(&self, source: &Path) -> TemplateId {
        let uuid = Uuid::new_v4();

        let cwd = env::current_dir().expect("Failed to get current directory");
        debug!("Current directory: {cwd:?}");

        let target_dir = cwd.join(Path::new("data/templates"));
        debug!("Local template store: {target_dir:?}");
        if !target_dir.exists() {
            std::fs::create_dir_all(&target_dir)
                .expect("Failed to create template store directory");
        }

        if !source.exists() {
            panic!("Source file does not exist: {source:?}");
        }

        let _ = std::fs::copy(source, target_dir.join(format!("{uuid}-0.wasm")))
            .expect("Failed to copy WASM to the local template store");

        TemplateId(uuid)
    }

    pub async fn start_worker(&mut self, template_id: &TemplateId, name: &str) -> WorkerId {
        let worker_id = WorkerId {
            template_id: template_id.clone(),
            worker_name: name.to_string(),
        };
        let response = self
            .client
            .create_worker(CreateWorkerRequest {
                worker_id: Some(worker_id.clone().into()),
                template_version: 0,
                args: vec![],
                env: HashMap::new(),
                account_id: Some(
                    AccountId {
                        value: "test-account".to_string(),
                    }
                    .into(),
                ),
                account_limits: None,
            })
            .await
            .expect("Failed to start worker")
            .into_inner();

        match response.result {
            None => panic!("No response from create_worker"),
            Some(create_worker_response::Result::Success(_)) => worker_id,
            Some(create_worker_response::Result::Failure(error)) => {
                panic!("Failed to start worker: {error:?}")
            }
        }
    }

    pub async fn invoke_and_await(
        &mut self,
        worker_id: &WorkerId,
        function_name: &str,
        params: Vec<Val>,
    ) -> Result<Vec<Val>, GolemError> {
        self.invoke_and_await_custom(
            worker_id,
            function_name,
            params,
            CallingConvention::Component,
        )
        .await
    }

    pub async fn invoke_and_await_stdio(
        &mut self,
        worker_id: &WorkerId,
        function_name: &str,
        params: Value,
    ) -> Result<Value, GolemError> {
        let json_string = params.to_string();
        self.invoke_and_await_custom(
            worker_id,
            function_name,
            vec![val_string(&json_string)],
            CallingConvention::Stdio,
        )
            .await
            .and_then(|vals| {
                if vals.len() == 1 {
                    let value_opt = &vals[0].val;

                    match value_opt {
                        Some(val::Val::String(s)) => {
                            if s.is_empty() {
                                Ok(Value::Null)
                            } else {
                                let result: Value = serde_json::from_str(s).unwrap_or(Value::String(s.to_string()));
                                Ok(result)
                            }
                        }
                        _ => Err(GolemError::ValueMismatch { details: "Expecting a single string as the result value when using stdio calling convention".to_string() }),
                    }
                } else {
                    Err(GolemError::ValueMismatch { details: "Expecting a single string as the result value when using stdio calling convention".to_string() })
                }
            })
    }

    pub async fn invoke_and_await_stdio_eventloop(
        &mut self,
        worker_id: &WorkerId,
        function_name: &str,
        params: Value,
    ) -> Result<Value, GolemError> {
        let json_string = params.to_string();
        self.invoke_and_await_custom(
            worker_id,
            function_name,
            vec![val_string(&json_string)],
            CallingConvention::StdioEventloop,
        )
            .await
            .and_then(|vals| {
                if vals.len() == 1 {
                    let value_opt = &vals[0].val;

                    match value_opt {
                        Some(val::Val::String(s)) => {
                            if s.is_empty() {
                                Ok(Value::Null)
                            } else {
                                let result: Value = serde_json::from_str(s).unwrap_or(Value::String(s.to_string()));
                                Ok(result)
                            }
                        }
                        _ => Err(GolemError::ValueMismatch { details: "Expecting a single string as the result value when using stdio calling convention".to_string() }),
                    }
                } else {
                    Err(GolemError::ValueMismatch { details: "Expecting a single string as the result value when using stdio calling convention".to_string() })
                }
            })
    }

    pub async fn invoke_and_await_custom(
        &mut self,
        worker_id: &WorkerId,
        function_name: &str,
        params: Vec<Val>,
        cc: CallingConvention,
    ) -> Result<Vec<Val>, GolemError> {
        let invocation_key = match self
            .client
            .get_invocation_key(GetInvocationKeyRequest {
                worker_id: Some(worker_id.clone().into()),
            })
            .await
            .expect("Failed to get invocation key")
            .into_inner()
            .result
            .expect("Invocation key response is empty")
        {
            get_invocation_key_response::Result::Success(response) => response
                .invocation_key
                .expect("Invocation key field is empty"),
            get_invocation_key_response::Result::Failure(error) => {
                panic!("Failed to get invocation key: {error:?}")
            }
        };
        let invoke_response = self
            .client
            .invoke_and_await_worker(InvokeAndAwaitWorkerRequest {
                worker_id: Some(worker_id.clone().into()),
                name: function_name.to_string(),
                input: params,
                invocation_key: Some(invocation_key),
                calling_convention: cc.into(),
                account_id: Some(
                    AccountId {
                        value: "test-account".to_string(),
                    }
                    .into(),
                ),
                account_limits: None,
            })
            .await
            .expect("Failed to invoke worker")
            .into_inner();

        match invoke_response.result {
            None => panic!("No response from invoke_and_await_worker"),
            Some(invoke_and_await_worker_response::Result::Success(response)) => {
                Ok(response.output)
            }
            Some(invoke_and_await_worker_response::Result::Failure(error)) => {
                Err(error.try_into().expect("Failed to convert error"))
            }
        }
    }

    pub async fn capture_output(&self, worker_id: &WorkerId) -> UnboundedReceiver<LogEvent> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let mut cloned_client = self.client.clone();
        let worker_id = worker_id.clone();
        tokio::spawn(async move {
            let mut response = cloned_client
                .connect_worker(ConnectWorkerRequest {
                    worker_id: Some(worker_id.clone().into()),
                    account_id: Some(
                        AccountId {
                            value: "test-account".to_string(),
                        }
                        .into(),
                    ),
                    account_limits: None,
                })
                .await
                .expect("Failed to connect worker")
                .into_inner();

            while let Some(event) = response.message().await.expect("Failed to get message") {
                debug!("Received event: {:?}", event);
                tx.send(event).expect("Failed to send event");
            }
        });

        rx
    }

    pub async fn interrupt(&mut self, worker_id: &WorkerId) {
        let response = self
            .client
            .interrupt_worker(InterruptWorkerRequest {
                worker_id: Some(worker_id.clone().into()),
                recover_immediately: false,
            })
            .await
            .expect("Failed to interrupt worker")
            .into_inner();

        match response {
            InterruptWorkerResponse {
                result: Some(interrupt_worker_response::Result::Success(_)),
            } => {}
            InterruptWorkerResponse {
                result: Some(interrupt_worker_response::Result::Failure(error)),
            } => panic!("Failed to interrupt worker: {error:?}"),
            _ => panic!("Failed to interrupt worker: unknown error"),
        }
    }

    pub async fn simulated_crash(&mut self, worker_id: &WorkerId) {
        let response = self
            .client
            .interrupt_worker(InterruptWorkerRequest {
                worker_id: Some(worker_id.clone().into()),
                recover_immediately: true,
            })
            .await
            .expect("Failed to crash worker")
            .into_inner();

        match response {
            InterruptWorkerResponse {
                result: Some(interrupt_worker_response::Result::Success(_)),
            } => {}
            InterruptWorkerResponse {
                result: Some(interrupt_worker_response::Result::Failure(error)),
            } => panic!("Failed to crash worker: {error:?}"),
            _ => panic!("Failed to crash worker: unknown error"),
        }
    }

    pub async fn async_clone(&self) -> Self {
        let new_client =
            WorkerExecutorClient::connect(format!("http://127.0.0.1:{}", self.grpc_port))
                .await
                .expect("Failed to connect to worker executor");
        Self {
            client: new_client,
            handle: None,
            grpc_port: self.grpc_port,
        }
    }
}

impl Drop for TestWorkerExecutor {
    fn drop(&mut self) {
        if let Some(handle) = &self.handle {
            handle.abort()
        }
    }
}

pub async fn start() -> Result<TestWorkerExecutor, anyhow::Error> {
    REDIS.assert_valid();
    println!("Using Redis on port {}", REDIS.port);

    let prometheus = golem_worker_executor_base::metrics::register_all();
    let config = GolemConfig {
        template_service: TemplateServiceConfig::Local(TemplateServiceLocalConfig {
            root: Path::new("data/templates").to_path_buf(),
        }),
        compiled_template_service: CompiledTemplateServiceConfig::Local(
            CompiledTemplateServiceLocalConfig {
                root: Path::new("data/templates").to_path_buf(),
            },
        ),
        blob_store_service: BlobStoreServiceConfig::InMemory(BlobStoreServiceInMemoryConfig {}),
        key_value_service: KeyValueServiceConfig::Redis,
        shard_manager_service: ShardManagerServiceConfig::SingleShard,
        promises: PromisesConfig::Redis,
        workers: WorkersServiceConfig::Redis,
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
        if let Ok(client) = client {
            break Ok(TestWorkerExecutor {
                client,
                handle: Some(server_handle),
                grpc_port,
            });
        } else if start.elapsed().as_secs() > 10 {
            break Err(anyhow::anyhow!("Timeout waiting for server to start"));
        }
    }
}

pub fn stdout_event(s: &str) -> LogEvent {
    LogEvent {
        event: Some(log_event::Event::Stdout(StdOutLog {
            message: s.to_string(),
        })),
    }
}

pub fn val_string(s: &str) -> Val {
    Val {
        val: Some(val::Val::String(s.to_string())),
    }
}

pub fn val_float32(f: f32) -> Val {
    Val {
        val: Some(val::Val::F32(f)),
    }
}

pub fn val_float64(f: f64) -> Val {
    Val {
        val: Some(val::Val::F64(f)),
    }
}

pub fn val_u32(i: u32) -> Val {
    Val {
        val: Some(val::Val::U32(i as i64)),
    }
}

pub fn val_record(items: Vec<Val>) -> Val {
    Val {
        val: Some(val::Val::Record(ValRecord { values: items })),
    }
}

pub fn val_list(items: Vec<Val>) -> Val {
    Val {
        val: Some(val::Val::List(ValList { values: items })),
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
    golem_ctx: GolemCtx<TestWorkerCtx>,
}

impl HasGolemCtx for TestWorkerCtx {
    type ExtraDeps = ();

    fn golem_ctx(&self) -> &GolemCtx<Self> {
        &self.golem_ctx
    }

    fn golem_ctx_mut(&mut self) -> &mut GolemCtx<Self> {
        &mut self.golem_ctx
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

    async fn return_fuel(&mut self, current_level: i64) -> Result<i64, GolemError> {
        Ok(current_level)
    }
}

struct ServerBootstrap {}

#[async_trait]
impl WorkerCtx for TestWorkerCtx {
    type PublicState = GolemPublicState;

    async fn create(
        worker_id: VersionedWorkerId,
        account_id: AccountId,
        promise_service: Arc<dyn PromiseService + Send + Sync>,
        invocation_key_service: Arc<dyn InvocationKeyService + Send + Sync>,
        worker_service: Arc<dyn WorkerService + Send + Sync>,
        key_value_service: Arc<dyn KeyValueService + Send + Sync>,
        blob_store_service: Arc<dyn BlobStoreService + Send + Sync>,
        event_service: Arc<dyn WorkerEventService + Send + Sync>,
        active_workers: Arc<ActiveWorkers<TestWorkerCtx>>,
        oplog_service: Arc<dyn OplogService + Send + Sync>,
        scheduler_service: Arc<dyn SchedulerService + Send + Sync>,
        recovery_management: Arc<dyn RecoveryManagement + Send + Sync>,
        _extra_deps: Self::ExtraDeps,
        config: Arc<GolemConfig>,
        worker_config: WorkerConfig,
        execution_status: Arc<RwLock<ExecutionStatus>>,
    ) -> Result<Self, GolemError> {
        let golem_ctx = GolemCtx::create(
            worker_id,
            account_id,
            promise_service,
            invocation_key_service,
            worker_service,
            key_value_service,
            blob_store_service,
            event_service,
            active_workers,
            oplog_service,
            scheduler_service,
            recovery_management,
            config,
            worker_config,
            execution_status,
        )
        .await?;
        Ok(Self { golem_ctx })
    }

    fn get_public_state(&self) -> &Self::PublicState {
        self.golem_ctx.get_public_state()
    }

    fn resource_limiter(&mut self) -> &mut dyn ResourceLimiterAsync {
        self
    }

    fn worker_id(&self) -> &VersionedWorkerId {
        self.golem_ctx.worker_id()
    }

    fn is_exit(error: &Error) -> Option<i32> {
        GolemCtx::<TestWorkerCtx>::is_exit(error)
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
            golem_config.limits.max_active_instances,
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
        template_service: Arc<dyn TemplateService + Send + Sync>,
        shard_manager_service: Arc<dyn ShardManagerService + Send + Sync>,
        worker_service: Arc<dyn WorkerService + Send + Sync>,
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
        let recovery_management = Arc::new(RecoveryManagementDefault::new(
            active_workers.clone(),
            engine.clone(),
            linker.clone(),
            runtime.clone(),
            template_service.clone(),
            worker_service.clone(),
            oplog_service.clone(),
            promise_service.clone(),
            scheduler_service.clone(),
            invocation_key_service.clone(),
            key_value_service.clone(),
            blob_store_service.clone(),
            golem_config.clone(),
            (),
        ));
        Ok(All::new(
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
            oplog_service,
            recovery_management,
            scheduler_service,
            (),
        ))
    }

    fn create_wasmtime_linker(&self, engine: &Engine) -> anyhow::Result<Linker<TestWorkerCtx>> {
        let mut linker =
            create_linker::<TestWorkerCtx, GolemCtx<TestWorkerCtx>>(engine, |x| &mut x.golem_ctx)?;
        golem_host::host::add_to_linker::<TestWorkerCtx, GolemCtx<TestWorkerCtx>>(
            &mut linker,
            |x| &mut x.golem_ctx,
        )?;
        Ok(linker)
    }
}
