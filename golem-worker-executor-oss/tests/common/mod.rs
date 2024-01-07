use std::collections::HashMap;
use std::env;
use std::path::Path;
use std::sync::Arc;

use golem_api_grpc::proto::golem::worker::{
    log_event, val, CallingConvention, LogEvent, StdOutLog, Val, ValList, ValRecord,
};
use golem_api_grpc::proto::golem::workerexecutor::worker_executor_client::WorkerExecutorClient;
use golem_api_grpc::proto::golem::workerexecutor::{
    create_worker_response, get_invocation_key_response, invoke_and_await_worker_response,
    ConnectWorkerRequest, CreateWorkerRequest, GetInvocationKeyRequest,
    InvokeAndAwaitWorkerRequest,
};
use golem_common::model::{AccountId, TemplateId, WorkerId};
use golem_worker_executor_base::services::golem_config::GolemConfig;
use golem_worker_executor_oss::run;
use golem_worker_executor_oss::services::config::AdditionalGolemConfig;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::task::JoinHandle;
use tonic::transport::Channel;
use tracing::{debug, error, info};
use uuid::Uuid;

pub struct TestWorkerExecutor {
    pub client: WorkerExecutorClient<Channel>,
    handle: JoinHandle<Result<(), String>>,
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
    ) -> Vec<Val> {
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
                calling_convention: CallingConvention::Component.into(),
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
            Some(invoke_and_await_worker_response::Result::Success(response)) => response.output,
            Some(invoke_and_await_worker_response::Result::Failure(error)) => {
                panic!("Failed to invoke worker: {error:?}")
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
}

impl Drop for TestWorkerExecutor {
    fn drop(&mut self) {
        self.handle.abort()
    }
}

pub async fn start() -> Result<TestWorkerExecutor, anyhow::Error> {
    tracing_subscriber::fmt().with_ansi(true).init();

    let prometheus = golem_worker_executor_base::metrics::register_all();
    let config = GolemConfig::from_file("config/worker-executor-local.toml");
    let additional_golem_config = Arc::new(AdditionalGolemConfig::from_file(
        "config/worker-executor-local.toml",
    ));
    let handle = tokio::runtime::Handle::current();

    let grpc_port = config.port;

    let server_handle = tokio::spawn(async move {
        let r = run(config, prometheus, handle, additional_golem_config)
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
                handle: server_handle,
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
