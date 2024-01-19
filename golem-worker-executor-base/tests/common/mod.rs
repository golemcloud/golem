use anyhow::Error;
use async_trait::async_trait;
use prometheus::Registry;
use std::collections::HashMap;
use std::path::Path;
use std::string::FromUtf8Error;
use std::sync::{Arc, RwLock};
use std::{env, panic};

use crate::{common, REDIS};
use golem_api_grpc::proto::golem::worker::{
    log_event, val, worker_execution_error, CallingConvention, LogEvent, StdOutLog, Val, ValFlags,
    ValList, ValOption, ValRecord, ValResult, ValTuple, WorkerExecutionError,
};
use golem_api_grpc::proto::golem::workerexecutor::worker_executor_client::WorkerExecutorClient;
use golem_api_grpc::proto::golem::workerexecutor::{
    create_worker_response, get_invocation_key_response, get_worker_metadata_response,
    interrupt_worker_response, invoke_and_await_worker_response, invoke_worker_response,
    resume_worker_response, ConnectWorkerRequest, CreateWorkerRequest, GetInvocationKeyRequest,
    InterruptWorkerRequest, InterruptWorkerResponse, InvokeAndAwaitWorkerRequest,
    InvokeWorkerRequest, ResumeWorkerRequest,
};
use golem_common::model::{
    AccountId, InvocationKey, TemplateId, VersionedWorkerId, WorkerId, WorkerMetadata, WorkerStatus,
};
use golem_worker_executor_base::error::GolemError;
use golem_worker_executor_base::services::golem_config::{
    BlobStoreServiceConfig, BlobStoreServiceInMemoryConfig, CompiledTemplateServiceConfig,
    CompiledTemplateServiceLocalConfig, GolemConfig, KeyValueServiceConfig, PromisesConfig,
    ShardManagerServiceConfig, TemplateServiceConfig, TemplateServiceLocalConfig,
    WorkersServiceConfig,
};

use golem_worker_executor_base::durable_host::{
    DurableWorkerCtx, DurableWorkerCtxView, PublicDurableWorkerState,
};
use golem_worker_executor_base::model::{
    CurrentResourceLimits, ExecutionStatus, InterruptKind, WorkerConfig,
};
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
use golem_worker_executor_base::services::{All, HasAll};
use golem_worker_executor_base::wasi_host::create_linker;
use golem_worker_executor_base::workerctx::{
    ExternalOperations, FuelManagement, InvocationHooks, InvocationManagement, IoCapturing,
    StatusManagement, WorkerCtx,
};
use golem_worker_executor_base::{durable_host, Bootstrap};
use serde_json::Value;
use tokio::runtime::Handle;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::task::JoinHandle;

use tonic::transport::Channel;
use tracing::{debug, error, info};
use uuid::Uuid;
use wasmtime::component::{Instance, Linker};
use wasmtime::{AsContextMut, Engine, ResourceLimiterAsync};

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

    pub fn update_template(&self, template_id: &TemplateId, source: &Path) -> i32 {
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

        let template_id_str = template_id.to_string();
        let mut versions = std::fs::read_dir(&target_dir)
            .expect("Failed to read template store directory")
            .filter_map(|entry| {
                let entry = entry.unwrap();
                let path = entry.path();
                let file_name = path.file_name().unwrap().to_str().unwrap();

                if file_name.starts_with(&template_id_str) && file_name.ends_with(".wasm") {
                    let version_part = file_name.split('-').last().unwrap();
                    let version_part = version_part[..version_part.len() - 5].to_string();
                    version_part.parse::<i32>().ok()
                } else {
                    None
                }
            })
            .collect::<Vec<i32>>();
        versions.sort();
        let new_version = versions.last().unwrap_or(&-1) + 1;
        let target = target_dir.join(format!("{template_id}-{new_version}.wasm"));

        let _ =
            std::fs::copy(source, target).expect("Failed to copy WASM to the local template store");

        new_version
    }

    pub async fn start_worker(&mut self, template_id: &TemplateId, name: &str) -> WorkerId {
        self.start_worker_versioned(template_id, 0, name).await
    }

    pub async fn try_start_worker(
        &mut self,
        template_id: &TemplateId,
        name: &str,
    ) -> Result<WorkerId, worker_execution_error::Error> {
        self.try_start_worker_versioned(template_id, 0, name, vec![], HashMap::new())
            .await
    }

    pub async fn start_worker_versioned(
        &mut self,
        template_id: &TemplateId,
        template_version: i32,
        name: &str,
    ) -> WorkerId {
        self.try_start_worker_versioned(template_id, template_version, name, vec![], HashMap::new())
            .await
            .expect("Failed to start worker")
    }

    pub async fn try_start_worker_versioned(
        &mut self,
        template_id: &TemplateId,
        template_version: i32,
        name: &str,
        args: Vec<String>,
        env: HashMap<String, String>,
    ) -> Result<WorkerId, worker_execution_error::Error> {
        let worker_id = WorkerId {
            template_id: template_id.clone(),
            worker_name: name.to_string(),
        };
        let response = self
            .client
            .create_worker(CreateWorkerRequest {
                worker_id: Some(worker_id.clone().into()),
                template_version,
                args,
                env,
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
            Some(create_worker_response::Result::Success(_)) => Ok(worker_id),
            Some(create_worker_response::Result::Failure(WorkerExecutionError {
                error: Some(error),
            })) => Err(error),
            Some(create_worker_response::Result::Failure(error)) => {
                panic!("Failed to start worker: {error:?}")
            }
        }
    }

    pub async fn get_worker_metadata(&mut self, worker_id: &WorkerId) -> Option<WorkerMetadata> {
        let worker_id: golem_api_grpc::proto::golem::worker::WorkerId = worker_id.clone().into();
        let response = self
            .client
            .get_worker_metadata(worker_id)
            .await
            .expect("Failed to get worker metadata")
            .into_inner();

        match response.result {
            None => panic!("No response from connect_worker"),
            Some(get_worker_metadata_response::Result::Success(metadata)) => {
                Some(metadata.try_into().unwrap())
            }
            Some(get_worker_metadata_response::Result::Failure(WorkerExecutionError {
                error: Some(worker_execution_error::Error::WorkerNotFound(_)),
            })) => None,
            Some(get_worker_metadata_response::Result::Failure(error)) => {
                panic!("Failed to get worker metadata: {error:?}")
            }
        }
    }

    pub async fn delete_worker(&mut self, worker_id: &WorkerId) {
        let worker_id: golem_api_grpc::proto::golem::worker::WorkerId = worker_id.clone().into();
        self.client.delete_worker(worker_id).await.unwrap();
    }

    pub async fn get_invocation_key(&mut self, worker_id: &WorkerId) -> InvocationKey {
        match self
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
        }
        .into()
    }

    pub async fn invoke(
        &mut self,
        worker_id: &WorkerId,
        function_name: &str,
        params: Vec<Val>,
    ) -> Result<(), GolemError> {
        let invoke_response = self
            .client
            .invoke_worker(InvokeWorkerRequest {
                worker_id: Some(worker_id.clone().into()),
                name: function_name.to_string(),
                input: params,
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
            Some(invoke_worker_response::Result::Success(_)) => Ok(()),
            Some(invoke_worker_response::Result::Failure(error)) => {
                Err(error.try_into().expect("Failed to convert error"))
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

    pub async fn invoke_and_await_with_key(
        &mut self,
        worker_id: &WorkerId,
        invocation_key: &InvocationKey,
        function_name: &str,
        params: Vec<Val>,
    ) -> Result<Vec<Val>, GolemError> {
        self.invoke_and_await_custom_with_key(
            worker_id,
            invocation_key,
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
        let invocation_key = self.get_invocation_key(worker_id).await;
        self.invoke_and_await_custom_with_key(worker_id, &invocation_key, function_name, params, cc)
            .await
    }

    pub async fn invoke_and_await_custom_with_key(
        &mut self,
        worker_id: &WorkerId,
        invocation_key: &InvocationKey,
        function_name: &str,
        params: Vec<Val>,
        cc: CallingConvention,
    ) -> Result<Vec<Val>, GolemError> {
        let invoke_response = self
            .client
            .invoke_and_await_worker(InvokeAndAwaitWorkerRequest {
                worker_id: Some(worker_id.clone().into()),
                name: function_name.to_string(),
                input: params,
                invocation_key: Some(invocation_key.clone().into()),
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

            debug!("Finished receiving events");
        });

        rx
    }

    pub async fn capture_output_with_termination(
        &self,
        worker_id: &WorkerId,
    ) -> UnboundedReceiver<Option<LogEvent>> {
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
                tx.send(Some(event)).expect("Failed to send event");
            }

            debug!("Finished receiving events");
            tx.send(None).expect("Failed to send termination event");
        });

        rx
    }

    pub async fn log_output(&self, worker_id: &WorkerId) {
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
                info!("Received event: {:?}", event);
            }
        });
    }

    pub async fn resume(&mut self, worker_id: &WorkerId) {
        let response = self
            .client
            .resume_worker(ResumeWorkerRequest {
                worker_id: Some(worker_id.clone().into()),
                name: "".to_string(),
            })
            .await
            .expect("Failed to resume worker")
            .into_inner();

        match response.result {
            None => panic!("No response from connect_worker"),
            Some(resume_worker_response::Result::Success(_)) => {}
            Some(resume_worker_response::Result::Failure(error)) => {
                panic!("Failed to connect worker: {error:?}")
            }
        }
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
        let clone_info = self.clone_info();
        Self::from_clone_info(clone_info).await
    }

    pub fn clone_info(&self) -> TestWorkerExecutorClone {
        TestWorkerExecutorClone {
            grpc_port: self.grpc_port,
        }
    }

    pub async fn from_clone_info(clone_info: TestWorkerExecutorClone) -> Self {
        let new_client =
            WorkerExecutorClient::connect(format!("http://127.0.0.1:{}", clone_info.grpc_port))
                .await
                .expect("Failed to connect to worker executor");
        Self {
            client: new_client,
            handle: None,
            grpc_port: clone_info.grpc_port,
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

pub fn log_event_to_string(event: &LogEvent) -> String {
    match &event.event {
        Some(log_event::Event::Stdout(stdout)) => stdout.message.clone(),
        Some(log_event::Event::Stderr(stderr)) => stderr.message.clone(),
        Some(log_event::Event::Log(log)) => log.message.clone(),
        _ => panic!("Unexpected event type"),
    }
}

pub async fn drain_connection(rx: UnboundedReceiver<Option<LogEvent>>) -> Vec<Option<LogEvent>> {
    let mut rx = rx;
    let mut events = vec![];
    rx.recv_many(&mut events, 100).await;

    if !events.contains(&None) {
        loop {
            match rx.recv().await {
                Some(Some(event)) => events.push(Some(event)),
                Some(None) => break,
                None => break,
            }
        }
    }
    events
}

pub async fn events_to_lines(rx: &mut UnboundedReceiver<LogEvent>) -> Vec<String> {
    let mut events = vec![];
    rx.recv_many(&mut events, 100).await;
    let full_output = events
        .iter()
        .map(common::log_event_to_string)
        .collect::<Vec<_>>()
        .join("");
    let lines = full_output
        .lines()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
    lines
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

pub fn val_bool(b: bool) -> Val {
    Val {
        val: Some(val::Val::Bool(b)),
    }
}

pub fn val_u8(i: u8) -> Val {
    Val {
        val: Some(val::Val::U8(i as i32)),
    }
}

pub fn val_i32(i: i32) -> Val {
    Val {
        val: Some(val::Val::S32(i)),
    }
}

pub fn val_u32(i: u32) -> Val {
    Val {
        val: Some(val::Val::U32(i as i64)),
    }
}

pub fn val_u64(i: u64) -> Val {
    Val {
        val: Some(val::Val::U64(i as i64)),
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

pub fn val_flags(count: i32, indexes: &[i32]) -> Val {
    Val {
        val: Some(val::Val::Flags(ValFlags {
            count,
            value: indexes.to_vec(),
        })),
    }
}

pub fn val_result(value: Result<Val, Val>) -> Val {
    Val {
        val: Some(val::Val::Result(Box::new(match value {
            Ok(ok) => ValResult {
                discriminant: 0,
                value: Some(Box::new(ok)),
            },
            Err(err) => ValResult {
                discriminant: 1,
                value: Some(Box::new(err)),
            },
        }))),
    }
}

pub fn val_option(value: Option<Val>) -> Val {
    Val {
        val: Some(val::Val::Option(Box::new(match value {
            Some(some) => ValOption {
                discriminant: 1,
                value: Some(Box::new(some)),
            },
            None => ValOption {
                discriminant: 0,
                value: None,
            },
        }))),
    }
}

pub fn val_pair(first: Val, second: Val) -> Val {
    Val {
        val: Some(val::Val::Tuple(ValTuple {
            values: vec![first, second],
        })),
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

    async fn return_fuel(&mut self, current_level: i64) -> Result<i64, GolemError> {
        Ok(current_level)
    }
}

#[async_trait]
impl ExternalOperations<TestWorkerCtx> for TestWorkerCtx {
    type ExtraDeps = ();

    async fn set_worker_status<T: HasAll<TestWorkerCtx> + Send + Sync>(
        this: &T,
        worker_id: &WorkerId,
        status: WorkerStatus,
    ) {
        DurableWorkerCtx::<TestWorkerCtx>::set_worker_status(this, worker_id, status).await
    }

    async fn get_worker_retry_count<T: HasAll<TestWorkerCtx> + Send + Sync>(
        this: &T,
        worker_id: &WorkerId,
    ) -> u32 {
        DurableWorkerCtx::<TestWorkerCtx>::get_worker_retry_count(this, worker_id).await
    }

    async fn get_assumed_worker_status<T: HasAll<TestWorkerCtx> + Send + Sync>(
        this: &T,
        worker_id: &WorkerId,
        metadata: &Option<WorkerMetadata>,
    ) -> WorkerStatus {
        DurableWorkerCtx::<TestWorkerCtx>::get_assumed_worker_status(this, worker_id, metadata)
            .await
    }

    async fn prepare_instance(
        worker_id: &VersionedWorkerId,
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
    async fn set_current_invocation_key(&mut self, invocation_key: Option<InvocationKey>) {
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
        vals: Result<Vec<Val>, GolemError>,
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

    async fn deactivate(&self) {
        self.durable_ctx.deactivate().await
    }
}

#[async_trait]
impl InvocationHooks for TestWorkerCtx {
    async fn on_exported_function_invoked(
        &mut self,
        full_function_name: &str,
        function_input: &Vec<Val>,
        calling_convention: Option<&golem_common::model::CallingConvention>,
    ) -> anyhow::Result<()> {
        self.durable_ctx
            .on_exported_function_invoked(full_function_name, function_input, calling_convention)
            .await
    }

    async fn on_invocation_failure(&mut self, error: &Error) -> Result<(), Error> {
        self.durable_ctx.on_invocation_failure(error).await
    }

    async fn on_invocation_failure_deactivated(
        &mut self,
        error: &Error,
    ) -> Result<WorkerStatus, Error> {
        self.durable_ctx
            .on_invocation_failure_deactivated(error)
            .await
    }

    async fn on_invocation_success(
        &mut self,
        full_function_name: &str,
        function_input: &Vec<Val>,
        consumed_fuel: i64,
        output: Vec<Val>,
    ) -> Result<Option<Vec<Val>>, Error> {
        self.durable_ctx
            .on_invocation_success(full_function_name, function_input, consumed_fuel, output)
            .await
    }
}

struct ServerBootstrap {}

#[async_trait]
impl WorkerCtx for TestWorkerCtx {
    type PublicState = PublicDurableWorkerState;

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
        let durable_ctx = DurableWorkerCtx::create(
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
        Ok(Self { durable_ctx })
    }

    fn get_public_state(&self) -> &Self::PublicState {
        self.durable_ctx.get_public_state()
    }

    fn resource_limiter(&mut self) -> &mut dyn ResourceLimiterAsync {
        self
    }

    fn worker_id(&self) -> &VersionedWorkerId {
        self.durable_ctx.worker_id()
    }

    fn is_exit(error: &Error) -> Option<i32> {
        DurableWorkerCtx::<TestWorkerCtx>::is_exit(error)
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
            create_linker::<TestWorkerCtx, DurableWorkerCtx<TestWorkerCtx>>(engine, |x| {
                &mut x.durable_ctx
            })?;
        durable_host::host::add_to_linker::<TestWorkerCtx, DurableWorkerCtx<TestWorkerCtx>>(
            &mut linker,
            |x| &mut x.durable_ctx,
        )?;
        Ok(linker)
    }
}

#[derive(Copy, Clone)]
pub struct TestWorkerExecutorClone {
    grpc_port: u16,
}
