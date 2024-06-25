// Copyright 2024 Golem Cloud
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

pub mod benchmark;

use crate::config::TestDependencies;
use async_trait::async_trait;
use golem_api_grpc::proto::golem::common::ErrorsBody;
use golem_api_grpc::proto::golem::worker::update_record::Update;
use golem_api_grpc::proto::golem::worker::worker_error::Error;
use golem_api_grpc::proto::golem::worker::{
    get_worker_metadata_response, get_workers_metadata_response, interrupt_worker_response,
    invoke_and_await_response, invoke_response, launch_new_worker_response, log_event,
    resume_worker_response, update_worker_response, worker_execution_error, CallingConvention,
    ConnectWorkerRequest, DeleteWorkerRequest, GetWorkerMetadataRequest, GetWorkersMetadataRequest,
    GetWorkersMetadataSuccessResponse, InterruptWorkerRequest, InterruptWorkerResponse,
    InvokeAndAwaitRequest, InvokeParameters, InvokeRequest, LaunchNewWorkerRequest, LogEvent,
    ResumeWorkerRequest, StdErrLog, StdOutLog, UpdateMode, UpdateWorkerRequest,
    UpdateWorkerResponse, WorkerError, WorkerExecutionError,
};
use golem_common::model::oplog::{OplogIndex, TimestampedUpdateDescription, UpdateDescription};
use golem_common::model::regions::DeletedRegions;
use golem_common::model::{
    ComponentId, ComponentVersion, FailedUpdateRecord, IdempotencyKey, ScanCursor,
    SuccessfulUpdateRecord, WorkerFilter, WorkerId, WorkerMetadata, WorkerStatusRecord,
};
use golem_wasm_ast::analysis::AnalysisContext;
use golem_wasm_ast::component::Component;
use golem_wasm_ast::IgnoreAllButMetadata;
use golem_wasm_rpc::Value;
use std::collections::HashMap;
use std::path::Path;
use tokio::select;
use tokio::sync::mpsc::UnboundedReceiver;
use tracing::{debug, info};
use uuid::Uuid;

#[async_trait]
pub trait TestDsl {
    async fn store_component(&self, name: &str) -> ComponentId;
    async fn store_unique_component(&self, name: &str) -> ComponentId;
    async fn store_component_unverified(&self, name: &str) -> ComponentId;
    async fn update_component(&self, component_id: &ComponentId, name: &str) -> ComponentVersion;

    async fn start_worker(&self, component_id: &ComponentId, name: &str) -> WorkerId;
    async fn try_start_worker(
        &self,
        component_id: &ComponentId,
        name: &str,
    ) -> Result<WorkerId, Error>;
    async fn start_worker_with(
        &self,
        component_id: &ComponentId,
        name: &str,
        args: Vec<String>,
        env: HashMap<String, String>,
    ) -> WorkerId;
    async fn try_start_worker_with(
        &self,
        component_id: &ComponentId,
        name: &str,
        args: Vec<String>,
        env: HashMap<String, String>,
    ) -> Result<WorkerId, Error>;
    async fn get_worker_metadata(&self, worker_id: &WorkerId) -> Option<WorkerMetadata>;
    async fn get_workers_metadata(
        &self,
        component_id: &ComponentId,
        filter: Option<WorkerFilter>,
        cursor: ScanCursor,
        count: u64,
        precise: bool,
    ) -> (Option<ScanCursor>, Vec<WorkerMetadata>);
    async fn delete_worker(&self, worker_id: &WorkerId);

    async fn invoke(
        &self,
        worker_id: &WorkerId,
        function_name: &str,
        params: Vec<Value>,
    ) -> Result<(), Error>;
    async fn invoke_with_key(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
        function_name: &str,
        params: Vec<Value>,
    ) -> Result<(), Error>;
    async fn invoke_and_await(
        &self,
        worker_id: &WorkerId,
        function_name: &str,
        params: Vec<Value>,
    ) -> Result<Vec<Value>, Error>;
    async fn invoke_and_await_with_key(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
        function_name: &str,
        params: Vec<Value>,
    ) -> Result<Vec<Value>, Error>;
    async fn invoke_and_await_stdio(
        &self,
        worker_id: &WorkerId,
        function_name: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, Error>;
    async fn invoke_and_await_custom(
        &self,
        worker_id: &WorkerId,
        function_name: &str,
        params: Vec<Value>,
        cc: CallingConvention,
    ) -> Result<Vec<Value>, Error>;
    async fn invoke_and_await_custom_with_key(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
        function_name: &str,
        params: Vec<Value>,
        cc: CallingConvention,
    ) -> Result<Vec<Value>, Error>;
    async fn capture_output(&self, worker_id: &WorkerId) -> UnboundedReceiver<LogEvent>;
    async fn capture_output_forever(
        &self,
        worker_id: &WorkerId,
    ) -> (
        UnboundedReceiver<Option<LogEvent>>,
        tokio::sync::oneshot::Sender<()>,
    );
    async fn capture_output_with_termination(
        &self,
        worker_id: &WorkerId,
    ) -> UnboundedReceiver<Option<LogEvent>>;
    async fn log_output(&self, worker_id: &WorkerId);
    async fn resume(&self, worker_id: &WorkerId);
    async fn interrupt(&self, worker_id: &WorkerId);
    async fn simulated_crash(&self, worker_id: &WorkerId);
    async fn auto_update_worker(&self, worker_id: &WorkerId, target_version: ComponentVersion);
    async fn manual_update_worker(&self, worker_id: &WorkerId, target_version: ComponentVersion);
}

#[async_trait]
impl<T: TestDependencies + Send + Sync> TestDsl for T {
    async fn store_component(&self, name: &str) -> ComponentId {
        let source_path = self.component_directory().join(format!("{name}.wasm"));
        dump_component_info(&source_path);
        self.component_service()
            .get_or_add_component(&source_path)
            .await
    }

    async fn store_unique_component(&self, name: &str) -> ComponentId {
        let source_path = self.component_directory().join(format!("{name}.wasm"));
        dump_component_info(&source_path);
        let uuid = Uuid::new_v4();
        let unique_name = format!("{name}-{uuid}");
        self.component_service()
            .add_component_with_name(&source_path, &unique_name)
            .await
            .expect("Failed to store unique component")
    }

    async fn store_component_unverified(&self, name: &str) -> ComponentId {
        let source_path = self.component_directory().join(format!("{name}.wasm"));
        self.component_service()
            .get_or_add_component(&source_path)
            .await
    }

    async fn update_component(&self, component_id: &ComponentId, name: &str) -> ComponentVersion {
        let source_path = self.component_directory().join(format!("{name}.wasm"));
        dump_component_info(&source_path);
        self.component_service()
            .update_component(component_id, &source_path)
            .await
    }

    async fn start_worker(&self, component_id: &ComponentId, name: &str) -> WorkerId {
        self.start_worker_with(component_id, name, vec![], HashMap::new())
            .await
    }

    async fn try_start_worker(
        &self,
        component_id: &ComponentId,
        name: &str,
    ) -> Result<WorkerId, Error> {
        self.try_start_worker_with(component_id, name, vec![], HashMap::new())
            .await
    }

    async fn start_worker_with(
        &self,
        component_id: &ComponentId,
        name: &str,
        args: Vec<String>,
        env: HashMap<String, String>,
    ) -> WorkerId {
        self.try_start_worker_with(component_id, name, args, env)
            .await
            .expect("Failed to start worker")
    }

    async fn try_start_worker_with(
        &self,
        component_id: &ComponentId,
        name: &str,
        args: Vec<String>,
        env: HashMap<String, String>,
    ) -> Result<WorkerId, Error> {
        let response = self
            .worker_service()
            .create_worker(LaunchNewWorkerRequest {
                component_id: Some(component_id.clone().into()),
                name: name.to_string(),
                args,
                env,
            })
            .await;

        match response.result {
            None => panic!("No response from create_worker"),
            Some(launch_new_worker_response::Result::Success(response)) => Ok(response
                .worker_id
                .expect("No worker id in response")
                .try_into()
                .expect("Failed to parse result worker id")),
            Some(launch_new_worker_response::Result::Error(WorkerError { error: Some(error) })) => {
                Err(error)
            }
            Some(launch_new_worker_response::Result::Error(_)) => {
                panic!("Error response without any details")
            }
        }
    }

    async fn get_worker_metadata(&self, worker_id: &WorkerId) -> Option<WorkerMetadata> {
        let worker_id: golem_api_grpc::proto::golem::worker::WorkerId = worker_id.clone().into();
        let response = self
            .worker_service()
            .get_worker_metadata(GetWorkerMetadataRequest {
                worker_id: Some(worker_id),
            })
            .await;

        match response.result {
            None => panic!("No response from connect_worker"),
            Some(get_worker_metadata_response::Result::Success(metadata)) => {
                Some(to_worker_metadata(&metadata))
            }
            Some(get_worker_metadata_response::Result::Error(WorkerError {
                error: Some(Error::NotFound { .. }),
            })) => None,
            Some(get_worker_metadata_response::Result::Error(WorkerError {
                error:
                    Some(Error::InternalError(WorkerExecutionError {
                        error: Some(worker_execution_error::Error::WorkerNotFound(_)),
                    })),
            })) => None,
            Some(get_worker_metadata_response::Result::Error(error)) => {
                panic!("Failed to get worker metadata: {error:?}")
            }
        }
    }

    async fn get_workers_metadata(
        &self,
        component_id: &ComponentId,
        filter: Option<WorkerFilter>,
        cursor: ScanCursor,
        count: u64,
        precise: bool,
    ) -> (Option<ScanCursor>, Vec<WorkerMetadata>) {
        let component_id: golem_api_grpc::proto::golem::component::ComponentId =
            component_id.clone().into();
        let response = self
            .worker_service()
            .get_workers_metadata(GetWorkersMetadataRequest {
                component_id: Some(component_id),
                filter: filter.map(|f| f.into()),
                cursor: Some(cursor.into()),
                count,
                precise,
            })
            .await;
        match response.result {
            None => panic!("No response from get_workers_metadata"),
            Some(get_workers_metadata_response::Result::Success(
                GetWorkersMetadataSuccessResponse { workers, cursor },
            )) => (
                cursor.map(|c| c.into()),
                workers.iter().map(to_worker_metadata).collect(),
            ),
            Some(get_workers_metadata_response::Result::Error(error)) => {
                panic!("Failed to get workers metadata: {error:?}")
            }
        }
    }

    async fn delete_worker(&self, worker_id: &WorkerId) {
        self.worker_service()
            .delete_worker(DeleteWorkerRequest {
                worker_id: Some(worker_id.clone().into()),
            })
            .await;
    }

    async fn invoke(
        &self,
        worker_id: &WorkerId,
        function_name: &str,
        params: Vec<Value>,
    ) -> Result<(), Error> {
        let invoke_response = self
            .worker_service()
            .invoke(InvokeRequest {
                worker_id: Some(worker_id.clone().into()),
                idempotency_key: None,
                function: function_name.to_string(),
                invoke_parameters: Some(InvokeParameters {
                    params: params.into_iter().map(|v| v.into()).collect(),
                }),
                context: None,
            })
            .await;

        match invoke_response.result {
            None => panic!("No response from invoke_worker"),
            Some(invoke_response::Result::Success(_)) => Ok(()),
            Some(invoke_response::Result::Error(WorkerError { error: Some(error) })) => Err(error),
            Some(invoke_response::Result::Error(_)) => {
                panic!("Empty error response from invoke_worker")
            }
        }
    }

    async fn invoke_with_key(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
        function_name: &str,
        params: Vec<Value>,
    ) -> Result<(), Error> {
        let invoke_response = self
            .worker_service()
            .invoke(InvokeRequest {
                worker_id: Some(worker_id.clone().into()),
                idempotency_key: Some(idempotency_key.clone().into()),
                function: function_name.to_string(),
                invoke_parameters: Some(InvokeParameters {
                    params: params.into_iter().map(|v| v.into()).collect(),
                }),
                context: None,
            })
            .await;

        match invoke_response.result {
            None => panic!("No response from invoke_worker"),
            Some(invoke_response::Result::Success(_)) => Ok(()),
            Some(invoke_response::Result::Error(WorkerError { error: Some(error) })) => Err(error),
            Some(invoke_response::Result::Error(_)) => {
                panic!("Empty error response from invoke_worker")
            }
        }
    }

    async fn invoke_and_await(
        &self,
        worker_id: &WorkerId,
        function_name: &str,
        params: Vec<Value>,
    ) -> Result<Vec<Value>, Error> {
        self.invoke_and_await_custom(
            worker_id,
            function_name,
            params,
            CallingConvention::Component,
        )
        .await
    }

    async fn invoke_and_await_with_key(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
        function_name: &str,
        params: Vec<Value>,
    ) -> Result<Vec<Value>, Error> {
        self.invoke_and_await_custom_with_key(
            worker_id,
            idempotency_key,
            function_name,
            params,
            CallingConvention::Component,
        )
        .await
    }

    async fn invoke_and_await_stdio(
        &self,
        worker_id: &WorkerId,
        function_name: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, Error> {
        let json_string = params.to_string();
        self.invoke_and_await_custom(
            worker_id,
            function_name,
            vec![Value::String(json_string)],
            CallingConvention::Stdio,
        )
            .await
            .and_then(|vals| {
                if vals.len() == 1 {
                    let value_opt = &vals[0];

                    match value_opt {
                        Value::String(s) => {
                            if s.is_empty() {
                                Ok(serde_json::Value::Null)
                            } else {
                                let result: serde_json::Value = serde_json::from_str(s).unwrap_or(serde_json::Value::String(s.to_string()));
                                Ok(result)
                            }
                        }
                        _ => Err(Error::BadRequest(
                            ErrorsBody { errors: vec!["Expecting a single string as the result value when using stdio calling convention".to_string()] }
                        )),
                    }
                } else {
                    Err(Error::BadRequest(
                        ErrorsBody { errors: vec!["Expecting a single string as the result value when using stdio calling convention".to_string()] }))
                }
            })
    }

    async fn invoke_and_await_custom(
        &self,
        worker_id: &WorkerId,
        function_name: &str,
        params: Vec<Value>,
        cc: CallingConvention,
    ) -> Result<Vec<Value>, Error> {
        let idempotency_key = IdempotencyKey::fresh();
        self.invoke_and_await_custom_with_key(
            worker_id,
            &idempotency_key,
            function_name,
            params,
            cc,
        )
        .await
    }

    async fn invoke_and_await_custom_with_key(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
        function_name: &str,
        params: Vec<Value>,
        cc: CallingConvention,
    ) -> Result<Vec<Value>, Error> {
        let invoke_response = self
            .worker_service()
            .invoke_and_await(InvokeAndAwaitRequest {
                worker_id: Some(worker_id.clone().into()),
                idempotency_key: Some(idempotency_key.clone().into()),
                function: function_name.to_string(),
                invoke_parameters: Some(InvokeParameters {
                    params: params.into_iter().map(|v| v.into()).collect(),
                }),
                calling_convention: cc.into(),
                context: None,
            })
            .await;

        match invoke_response.result {
            None => panic!("No response from invoke_and_await"),
            Some(invoke_and_await_response::Result::Success(response)) => Ok(response
                .result
                .into_iter()
                .map(|v| v.try_into())
                .collect::<Result<Vec<Value>, String>>()
                .expect("Invocation result had unexpected format")),
            Some(invoke_and_await_response::Result::Error(WorkerError { error: Some(error) })) => {
                Err(error)
            }
            Some(invoke_and_await_response::Result::Error(_)) => {
                panic!("Empty error response from invoke_and_await")
            }
        }
    }

    async fn capture_output(&self, worker_id: &WorkerId) -> UnboundedReceiver<LogEvent> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let cloned_service = self.worker_service().clone();
        let worker_id = worker_id.clone();
        tokio::spawn(async move {
            let mut response = cloned_service
                .connect_worker(ConnectWorkerRequest {
                    worker_id: Some(worker_id.clone().into()),
                })
                .await;

            while let Some(event) = response.message().await.expect("Failed to get message") {
                debug!("Received event: {:?}", event);
                tx.send(event).expect("Failed to send event");
            }

            debug!("Finished receiving events");
        });

        rx
    }

    async fn capture_output_forever(
        &self,
        worker_id: &WorkerId,
    ) -> (
        UnboundedReceiver<Option<LogEvent>>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let cloned_service = self.worker_service().clone();
        let worker_id = worker_id.clone();
        let (abort_tx, mut abort_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let mut abort = false;
            while !abort {
                let mut response = cloned_service
                    .connect_worker(ConnectWorkerRequest {
                        worker_id: Some(worker_id.clone().into()),
                    })
                    .await;

                loop {
                    select! {
                        msg = response.message() => {
                            match msg {
                                Ok(Some(event)) =>  {
                                    debug!("Received event: {:?}", event);
                                    tx.send(Some(event)).expect("Failed to send event");
                                }
                                Ok(None) => {
                                    break;
                                }
                                Err(e) => {
                                    panic!("Failed to get message: {:?}", e);
                                }
                            }
                        }
                        _ = (&mut abort_rx) => {
                            abort = true;
                            break;
                        }
                    }
                }
            }

            tx.send(None).expect("Failed to send event");
            debug!("Finished receiving events");
        });

        (rx, abort_tx)
    }

    async fn capture_output_with_termination(
        &self,
        worker_id: &WorkerId,
    ) -> UnboundedReceiver<Option<LogEvent>> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let cloned_service = self.worker_service().clone();
        let worker_id = worker_id.clone();
        tokio::spawn(async move {
            let mut response = cloned_service
                .connect_worker(ConnectWorkerRequest {
                    worker_id: Some(worker_id.clone().into()),
                })
                .await;

            while let Some(event) = response.message().await.expect("Failed to get message") {
                debug!("Received event: {:?}", event);
                tx.send(Some(event)).expect("Failed to send event");
            }

            debug!("Finished receiving events");
            tx.send(None).expect("Failed to send termination event");
        });

        rx
    }

    async fn log_output(&self, worker_id: &WorkerId) {
        let cloned_service = self.worker_service().clone();
        let worker_id = worker_id.clone();
        tokio::spawn(async move {
            let mut response = cloned_service
                .connect_worker(ConnectWorkerRequest {
                    worker_id: Some(worker_id.clone().into()),
                })
                .await;

            while let Some(event) = response.message().await.expect("Failed to get message") {
                info!("Received event: {:?}", event);
            }
        });
    }

    async fn resume(&self, worker_id: &WorkerId) {
        let response = self
            .worker_service()
            .resume_worker(ResumeWorkerRequest {
                worker_id: Some(worker_id.clone().into()),
            })
            .await;

        match response.result {
            None => panic!("No response from connect_worker"),
            Some(resume_worker_response::Result::Success(_)) => {}
            Some(resume_worker_response::Result::Error(error)) => {
                panic!("Failed to connect worker: {error:?}")
            }
        }
    }

    async fn interrupt(&self, worker_id: &WorkerId) {
        let response = self
            .worker_service()
            .interrupt_worker(InterruptWorkerRequest {
                worker_id: Some(worker_id.clone().into()),
                recover_immediately: false,
            })
            .await;

        match response {
            InterruptWorkerResponse {
                result: Some(interrupt_worker_response::Result::Success(_)),
            } => {}
            InterruptWorkerResponse {
                result: Some(interrupt_worker_response::Result::Error(error)),
            } => panic!("Failed to interrupt worker: {error:?}"),
            _ => panic!("Failed to interrupt worker: unknown error"),
        }
    }

    async fn simulated_crash(&self, worker_id: &WorkerId) {
        let response = self
            .worker_service()
            .interrupt_worker(InterruptWorkerRequest {
                worker_id: Some(worker_id.clone().into()),
                recover_immediately: true,
            })
            .await;

        match response {
            InterruptWorkerResponse {
                result: Some(interrupt_worker_response::Result::Success(_)),
            } => {}
            InterruptWorkerResponse {
                result: Some(interrupt_worker_response::Result::Error(error)),
            } => panic!("Failed to crash worker: {error:?}"),
            _ => panic!("Failed to crash worker: unknown error"),
        }
    }

    async fn auto_update_worker(&self, worker_id: &WorkerId, target_version: ComponentVersion) {
        let response = self
            .worker_service()
            .update_worker(UpdateWorkerRequest {
                worker_id: Some(worker_id.clone().into()),
                target_version,
                mode: UpdateMode::Automatic.into(),
            })
            .await;

        match response {
            UpdateWorkerResponse {
                result: Some(update_worker_response::Result::Success(_)),
            } => {}
            UpdateWorkerResponse {
                result: Some(update_worker_response::Result::Error(error)),
            } => panic!("Failed to update worker: {error:?}"),
            _ => panic!("Failed to update worker: unknown error"),
        }
    }

    async fn manual_update_worker(&self, worker_id: &WorkerId, target_version: ComponentVersion) {
        let response = self
            .worker_service()
            .update_worker(UpdateWorkerRequest {
                worker_id: Some(worker_id.clone().into()),
                target_version,
                mode: UpdateMode::Manual.into(),
            })
            .await;

        match response {
            UpdateWorkerResponse {
                result: Some(update_worker_response::Result::Success(_)),
            } => {}
            UpdateWorkerResponse {
                result: Some(update_worker_response::Result::Error(error)),
            } => panic!("Failed to update worker: {error:?}"),
            _ => panic!("Failed to update worker: unknown error"),
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

pub fn stdout_event_starting_with(event: &LogEvent, s: &str) -> bool {
    if let LogEvent {
        event: Some(log_event::Event::Stdout(StdOutLog { message })),
    } = event
    {
        message.starts_with(s)
    } else {
        false
    }
}

pub fn stderr_event(s: &str) -> LogEvent {
    LogEvent {
        event: Some(log_event::Event::Stderr(StdErrLog {
            message: s.to_string(),
        })),
    }
}

pub fn log_event_to_string(event: &LogEvent) -> String {
    match &event.event {
        Some(log_event::Event::Stdout(stdout)) => stdout.message.clone(),
        Some(log_event::Event::Stderr(stderr)) => stderr.message.clone(),
        Some(log_event::Event::Log(log)) => log.message.clone(),
        _ => std::panic!("Unexpected event type"),
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
        .map(log_event_to_string)
        .collect::<Vec<_>>()
        .join("");
    let lines = full_output
        .lines()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
    lines
}

pub fn is_worker_execution_error(got: &Error, expected: &worker_execution_error::Error) -> bool {
    matches!(got, Error::InternalError(error) if error.error.as_ref() == Some(expected))
}

pub fn worker_error_message(error: &Error) -> String {
    match error {
        Error::BadRequest(errors) => errors.errors.join(", "),
        Error::Unauthorized(error) => error.error.clone(),
        Error::LimitExceeded(error) => error.error.clone(),
        Error::NotFound(error) => error.error.clone(),
        Error::AlreadyExists(error) => error.error.clone(),
        Error::InternalError(error) => match &error.error {
            None => "Internal error".to_string(),
            Some(error) => match error {
                worker_execution_error::Error::InvalidRequest(error) => error.details.clone(),
                worker_execution_error::Error::WorkerAlreadyExists(error) => {
                    format!("Worker already exists: {:?}", error.worker_id)
                }
                worker_execution_error::Error::WorkerCreationFailed(error) => format!(
                    "Worker creation failed: {:?}: {}",
                    error.worker_id, error.details
                ),
                worker_execution_error::Error::FailedToResumeWorker(error) => {
                    format!("Failed to resume worker: {:?}", error.worker_id)
                }
                worker_execution_error::Error::ComponentDownloadFailed(error) => format!(
                    "Failed to download component: {:?} version {}: {}",
                    error.component_id, error.component_version, error.reason
                ),
                worker_execution_error::Error::ComponentParseFailed(error) => format!(
                    "Failed to parse component: {:?} version {}: {}",
                    error.component_id, error.component_version, error.reason
                ),
                worker_execution_error::Error::GetLatestVersionOfComponentFailed(error) => format!(
                    "Failed to get latest version of component: {:?}: {}",
                    error.component_id, error.reason
                ),
                worker_execution_error::Error::PromiseNotFound(error) => {
                    format!("Promise not found: {:?}", error.promise_id)
                }
                worker_execution_error::Error::PromiseDropped(error) => {
                    format!("Promise dropped: {:?}", error.promise_id)
                }
                worker_execution_error::Error::PromiseAlreadyCompleted(error) => {
                    format!("Promise already completed: {:?}", error.promise_id)
                }
                worker_execution_error::Error::Interrupted(error) => {
                    if error.recover_immediately {
                        "Simulated crash".to_string()
                    } else {
                        "Interrupted via the Golem API".to_string()
                    }
                }
                worker_execution_error::Error::ParamTypeMismatch(_error) => {
                    "Parameter type mismatch".to_string()
                }
                worker_execution_error::Error::NoValueInMessage(_error) => {
                    "No value in message".to_string()
                }
                worker_execution_error::Error::ValueMismatch(error) => {
                    format!("Value mismatch: {}", error.details)
                }
                worker_execution_error::Error::UnexpectedOplogEntry(error) => format!(
                    "Unexpected oplog entry; Expected: {}, got: {}",
                    error.expected, error.got
                ),
                worker_execution_error::Error::RuntimeError(error) => {
                    format!("Runtime error: {}", error.details)
                }
                worker_execution_error::Error::InvalidShardId(error) => format!(
                    "Invalid shard id: {:?}; ids: {:?}",
                    error.shard_id, error.shard_ids
                ),
                worker_execution_error::Error::PreviousInvocationFailed(error) => {
                    format!("Previous invocation failed: {}", error.details)
                }
                worker_execution_error::Error::Unknown(error) => {
                    format!("Unknown error: {}", error.details)
                }
                worker_execution_error::Error::PreviousInvocationExited(_error) => {
                    "Previous invocation exited".to_string()
                }
                worker_execution_error::Error::InvalidAccount(_error) => {
                    "Invalid account id".to_string()
                }
                worker_execution_error::Error::WorkerNotFound(error) => {
                    format!("Worker not found: {:?}", error.worker_id)
                }
            },
        },
    }
}

pub fn to_worker_metadata(
    metadata: &golem_api_grpc::proto::golem::worker::WorkerMetadata,
) -> WorkerMetadata {
    WorkerMetadata {
        worker_id: metadata
            .worker_id
            .clone()
            .expect("no worker_id")
            .clone()
            .try_into()
            .expect("invalid worker_id"),
        args: metadata.args.clone(),
        env: metadata
            .env
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<Vec<_>>(),
        account_id: metadata
            .account_id
            .clone()
            .expect("no account_id")
            .clone()
            .into(),
        created_at: metadata
            .created_at
            .as_ref()
            .expect("no created_at")
            .clone()
            .into(),
        last_known_status: WorkerStatusRecord {
            oplog_idx: OplogIndex::default(),
            status: metadata.status.try_into().expect("invalid status"),
            overridden_retry_config: None, // not passed through gRPC
            deleted_regions: DeletedRegions::new(),
            pending_invocations: vec![],
            pending_updates: metadata
                .updates
                .iter()
                .filter_map(|u| match &u.update {
                    Some(Update::Pending(_)) => Some(TimestampedUpdateDescription {
                        timestamp: u
                            .timestamp
                            .as_ref()
                            .expect("no timestamp on update record")
                            .clone()
                            .into(),
                        oplog_index: OplogIndex::from_u64(0),
                        description: UpdateDescription::Automatic {
                            target_version: u.target_version,
                        },
                    }),
                    _ => None,
                })
                .collect(),
            failed_updates: metadata
                .updates
                .iter()
                .filter_map(|u| match &u.update {
                    Some(Update::Failed(failed_update)) => Some(FailedUpdateRecord {
                        timestamp: u
                            .timestamp
                            .as_ref()
                            .expect("no timestamp on update record")
                            .clone()
                            .into(),
                        target_version: u.target_version,
                        details: failed_update.details.clone(),
                    }),
                    _ => None,
                })
                .collect(),
            successful_updates: metadata
                .updates
                .iter()
                .filter_map(|u| match &u.update {
                    Some(Update::Successful(_)) => Some(SuccessfulUpdateRecord {
                        timestamp: u
                            .timestamp
                            .as_ref()
                            .expect("no timestamp on update record")
                            .clone()
                            .into(),
                        target_version: u.target_version,
                    }),
                    _ => None,
                })
                .collect(),
            invocation_results: HashMap::new(),
            current_idempotency_key: None,
            component_version: metadata.component_version,
            component_size: metadata.component_size,
            total_linear_memory_size: metadata.total_linear_memory_size,
        },
        parent: None,
    }
}

fn dump_component_info(path: &Path) {
    let data = std::fs::read(path).unwrap();
    let component = Component::<IgnoreAllButMetadata>::from_bytes(&data).unwrap();

    let state = AnalysisContext::new(component);
    let exports = state.get_top_level_exports();
    let mems = state.get_all_memories();

    info!("Exports of {path:?}: {exports:?}");
    info!("Linear memories of {path:?}: {mems:?}");
    let _ = exports.unwrap();
}
