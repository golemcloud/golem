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

use std::collections::HashSet;
use std::fmt::Debug;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use anyhow::anyhow;
use async_trait::async_trait;
use tokio::task::JoinSet;
use tokio::time::{sleep, Instant};
use tonic::transport::Channel;
use tonic::{Code, Status};
use tracing::{debug, error, info, warn, Instrument};

use golem_api_grpc::proto::golem::shardmanager::shard_manager_error::Error;
use golem_api_grpc::proto::golem::worker::WorkerExecutionError;
use golem_api_grpc::proto::golem::workerexecutor::worker_executor_client::WorkerExecutorClient;
use golem_common::client::MultiTargetGrpcClient;
use golem_common::config::RetryConfig;
use golem_common::model::{Pod, ShardId};
use golem_common::retries::get_delay;
use golem_service_base::model::{
    GolemError, GolemErrorInvalidShardId, GolemErrorUnknown, WorkerId,
};
use golem_service_base::routing_table::{HasRoutingTableService, RoutingTableError};

use crate::service::worker::WorkerServiceError;

#[async_trait]
pub trait RoutingLogic {
    async fn call_worker_executor<Target, F, G, Out, R>(
        &self,
        target: Target,
        remote_call: F,
        response_map: G,
    ) -> Result<R, WorkerServiceError>
    where
        Out: Send + 'static,
        R: Send,
        Target: CallOnExecutor<Out> + Send,
        F: for<'a> Fn(
                &'a mut WorkerExecutorClient<Channel>,
            )
                -> Pin<Box<dyn Future<Output = Result<Out, Status>> + 'a + Send>>
            + Send
            + Sync
            + Clone
            + 'static,
        G: Fn(Target::ResultOut) -> Result<R, ResponseMapResult> + Send + Sync;
}

#[async_trait]
pub trait CallOnExecutor<Out: Send + 'static> {
    type ResultOut: Send;

    async fn call_on_worker_executor<F>(
        &self,
        context: &(impl HasRoutingTableService + HasWorkerExecutorClients + Send + Sync),
        f: F,
    ) -> Result<(Option<Self::ResultOut>, Option<Pod>), CallWorkerExecutorErrorWithContext>
    where
        F: for<'a> Fn(
                &'a mut WorkerExecutorClient<Channel>,
            )
                -> Pin<Box<dyn Future<Output = Result<Out, Status>> + 'a + Send>>
            + Send
            + Sync
            + Clone
            + 'static;

    fn tracing_kind() -> &'static str;
}

// TODO; Delete the WorkerId in service-base in favour of WorkerId in golem-common
#[async_trait]
impl<Out: Send + 'static> CallOnExecutor<Out> for WorkerId {
    type ResultOut = Out;

    async fn call_on_worker_executor<F>(
        &self,
        context: &(impl HasRoutingTableService + HasWorkerExecutorClients + Send + Sync),
        f: F,
    ) -> Result<(Option<Self::ResultOut>, Option<Pod>), CallWorkerExecutorErrorWithContext>
    where
        F: for<'a> Fn(
                &'a mut WorkerExecutorClient<Channel>,
            )
                -> Pin<Box<dyn Future<Output = Result<Out, Status>> + 'a + Send>>
            + Send
            + Sync
            + Clone
            + 'static,
    {
        let worker_id = golem_common::model::WorkerId {
            component_id: self.component_id.clone(),
            worker_name: self.worker_name.to_string(),
        };
        worker_id.call_on_worker_executor(context, f).await
    }

    fn tracing_kind() -> &'static str {
        "WorkerId"
    }
}

#[async_trait]
impl<Out: Send + 'static> CallOnExecutor<Out> for golem_common::model::WorkerId {
    type ResultOut = Out;

    async fn call_on_worker_executor<F>(
        &self,
        context: &(impl HasRoutingTableService + HasWorkerExecutorClients + Send + Sync),
        f: F,
    ) -> Result<(Option<Self::ResultOut>, Option<Pod>), CallWorkerExecutorErrorWithContext>
    where
        F: for<'a> Fn(
                &'a mut WorkerExecutorClient<Channel>,
            )
                -> Pin<Box<dyn Future<Output = Result<Out, Status>> + 'a + Send>>
            + Send
            + Sync
            + Clone
            + 'static,
    {
        let routing_table = context
            .routing_table_service()
            .get_routing_table()
            .await
            .map_err(CallWorkerExecutorErrorWithContext::failed_to_get_routing_table)?;

        match routing_table.lookup(self) {
            None => Ok((None, None)),
            Some(pod) => Ok((
                Some(
                    context
                        .worker_executor_clients()
                        .call(pod.uri_02(), f)
                        .await
                        .map_err(|err| {
                            CallWorkerExecutorErrorWithContext::failed_to_connect_to_pod(
                                err,
                                pod.clone(),
                            )
                        })?,
                ),
                Some(pod.clone()),
            )),
        }
    }

    fn tracing_kind() -> &'static str {
        "WorkerId"
    }
}

pub struct RandomExecutor;

#[async_trait]
impl<Out: Send + 'static> CallOnExecutor<Out> for RandomExecutor {
    type ResultOut = Out;

    async fn call_on_worker_executor<F>(
        &self,
        context: &(impl HasRoutingTableService + HasWorkerExecutorClients + Send + Sync),
        f: F,
    ) -> Result<(Option<Self::ResultOut>, Option<Pod>), CallWorkerExecutorErrorWithContext>
    where
        F: for<'a> Fn(
                &'a mut WorkerExecutorClient<Channel>,
            )
                -> Pin<Box<dyn Future<Output = Result<Out, Status>> + 'a + Send>>
            + Send
            + Sync
            + Clone
            + 'static,
    {
        let routing_table = context
            .routing_table_service()
            .get_routing_table()
            .await
            .map_err(CallWorkerExecutorErrorWithContext::failed_to_get_routing_table)?;

        match routing_table.random() {
            None => Ok((None, None)),
            Some(pod) => Ok((
                Some(
                    context
                        .worker_executor_clients()
                        .call(pod.uri_02(), f)
                        .await
                        .map_err(|status| {
                            CallWorkerExecutorErrorWithContext::failed_to_connect_to_pod(
                                status,
                                pod.clone(),
                            )
                        })?,
                ),
                Some(pod.clone()),
            )),
        }
    }

    fn tracing_kind() -> &'static str {
        "RandomExecutor"
    }
}

pub struct AllExecutors;

#[async_trait]
impl<Out: Send + 'static> CallOnExecutor<Out> for AllExecutors {
    type ResultOut = Vec<Out>;

    async fn call_on_worker_executor<F>(
        &self,
        context: &(impl HasRoutingTableService + HasWorkerExecutorClients + Send + Sync),
        f: F,
    ) -> Result<(Option<Self::ResultOut>, Option<Pod>), CallWorkerExecutorErrorWithContext>
    where
        F: for<'a> Fn(
                &'a mut WorkerExecutorClient<Channel>,
            )
                -> Pin<Box<dyn Future<Output = Result<Out, Status>> + 'a + Send>>
            + Send
            + Sync
            + Clone
            + 'static + 'static,
    {
        let routing_table = context
            .routing_table_service()
            .get_routing_table()
            .await
            .map_err(CallWorkerExecutorErrorWithContext::failed_to_get_routing_table)?;

        let pods = routing_table.all();
        if pods.is_empty() {
            Ok((None, None))
        } else {
            let mut fibers = JoinSet::new();
            for pod in pods {
                let worker_executor_clients = context.worker_executor_clients().clone();
                let _ = fibers.spawn({
                    let pod = pod.clone();
                    let f = f.clone();
                    async move {
                        worker_executor_clients
                            .call(pod.uri_02(), f)
                            .await
                            .map_err(|err| (err, pod))
                    }
                });
            }
            let mut results = Vec::new();
            while let Some(result) = fibers.join_next().await {
                results.push(result.expect("Join error"));
            }
            let results = results
                .into_iter()
                .collect::<Result<Vec<Out>, _>>()
                .map_err(|(err, pod)| {
                    CallWorkerExecutorErrorWithContext::failed_to_connect_to_pod(err, pod)
                })?;

            Ok((Some(results), None))
        }
    }

    fn tracing_kind() -> &'static str {
        "AllExecutors"
    }
}

pub trait HasWorkerExecutorClients {
    fn worker_executor_clients(&self) -> &MultiTargetGrpcClient<WorkerExecutorClient<Channel>>;
}

#[derive(Debug)]
pub enum ResponseMapResult {
    InvalidShardId {
        shard_id: ShardId,
        shard_ids: HashSet<ShardId>,
    },
    Other(WorkerServiceError),
}

impl From<GolemError> for ResponseMapResult {
    fn from(error: GolemError) -> Self {
        match error {
            GolemError::InvalidShardId(GolemErrorInvalidShardId {
                shard_id,
                shard_ids,
            }) => ResponseMapResult::InvalidShardId {
                shard_id,
                shard_ids,
            },
            other => ResponseMapResult::Other(other.into()),
        }
    }
}

impl From<&'static str> for ResponseMapResult {
    fn from(error: &'static str) -> Self {
        ResponseMapResult::Other(WorkerServiceError::Internal(anyhow!(error)))
    }
}

impl From<WorkerExecutionError> for ResponseMapResult {
    fn from(error: WorkerExecutionError) -> Self {
        let golem_error = error.clone().try_into().unwrap_or_else(|_| {
            GolemError::Unknown(GolemErrorUnknown {
                details: "Unknown worker execution error".to_string(),
            })
        });
        let response_map_result = golem_error.clone().into();
        debug!(
            error = format!("{:?}", error),
            golem_error = golem_error.to_string(),
            response_map_result = format!("{:?}", response_map_result),
            "ResponseMapResult from WorkerExecutionError"
        );
        response_map_result
    }
}

#[async_trait]
impl<T: HasRoutingTableService + HasWorkerExecutorClients + Send + Sync> RoutingLogic for T {
    async fn call_worker_executor<Target, F, G, Out, R>(
        &self,
        target: Target,
        remote_call: F,
        response_map: G,
    ) -> Result<R, WorkerServiceError>
    where
        Out: Send + 'static,
        R: Send,
        Target: CallOnExecutor<Out> + Send,
        F: for<'a> Fn(
                &'a mut WorkerExecutorClient<Channel>,
            )
                -> Pin<Box<dyn Future<Output = Result<Out, Status>> + 'a + Send>>
            + Send
            + Sync
            + Clone
            + 'static,
        G: Fn(Target::ResultOut) -> Result<R, ResponseMapResult> + Send + Sync,
    {
        // TODO: extract to config
        let retry_config = &RetryConfig {
            max_attempts: 5,
            min_delay: Duration::from_millis(10),
            max_delay: Duration::from_secs(3),
            multiplier: 10.0,
            max_jitter_factor: Some(0.15),
        };

        let mut retry = RetryState::new(retry_config);
        loop {
            let span = retry.start_attempt(Target::tracing_kind());

            let worker_result = target
                .call_on_worker_executor(self, remote_call.clone())
                .await;

            let result = async {
                match worker_result {
                    Ok((result, pod)) => match result {
                        None => retry.retry(self, &"NoActiveShards", &pod).await,
                        Some(out) => match response_map(out) {
                            Ok(result) => {
                                retry.success(&pod);
                                Ok(Some(result))
                            }
                            Err(error @ ResponseMapResult::InvalidShardId { .. }) => {
                                retry.retry(self, &error, &pod).await
                            }
                            Err(ResponseMapResult::Other(error)) => {
                                retry.non_retryable_error(error, &pod)
                            }
                        },
                    },
                    Err(CallWorkerExecutorErrorWithContext { error, pod }) => {
                        if error.is_retriable() {
                            retry.retry(self, &error, &pod).await
                        } else {
                            retry.non_retryable_error(WorkerServiceError::internal(error), &pod)
                        }
                    }
                }
            };

            match result.instrument(span.span.clone()).await {
                Ok(Some(result)) => {
                    break Ok(result);
                }
                Ok(None) => {
                    // NOP, retry
                }
                Err(error) => {
                    break Err(error);
                }
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CallWorkerExecutorError {
    // TODO: Change to display
    #[error("Failed to get routing table: {0:?}")]
    FailedToGetRoutingTable(RoutingTableError),
    #[error("Failed to connect to pod: {0}")]
    FailedToConnectToPod(Status),
}

pub struct CallWorkerExecutorErrorWithContext {
    error: CallWorkerExecutorError,
    pod: Option<Pod>,
}

impl CallWorkerExecutorErrorWithContext {
    fn failed_to_get_routing_table(error: RoutingTableError) -> Self {
        CallWorkerExecutorErrorWithContext {
            error: CallWorkerExecutorError::FailedToGetRoutingTable(error),
            pod: None,
        }
    }

    fn failed_to_connect_to_pod(status: Status, pod: Pod) -> Self {
        CallWorkerExecutorErrorWithContext {
            error: CallWorkerExecutorError::FailedToConnectToPod(status),
            pod: Some(pod),
        }
    }
}

trait IsRetriableError {
    fn is_retriable(&self) -> bool;
}

impl IsRetriableError for Status {
    fn is_retriable(&self) -> bool {
        match self.code() {
            Code::Ok
            | Code::Cancelled
            | Code::InvalidArgument
            | Code::NotFound
            | Code::AlreadyExists
            | Code::PermissionDenied
            | Code::FailedPrecondition
            | Code::OutOfRange
            | Code::Unimplemented
            | Code::DataLoss
            | Code::Unauthenticated => false,
            Code::Unknown
            | Code::DeadlineExceeded
            | Code::ResourceExhausted
            | Code::Aborted
            | Code::Internal
            | Code::Unavailable => true,
        }
    }
}

impl IsRetriableError for RoutingTableError {
    fn is_retriable(&self) -> bool {
        match &self {
            RoutingTableError::ShardManagerGrpcError(status) => status.is_retriable(),
            RoutingTableError::ShardManagerError(error) => match &error.error {
                Some(error) => match error {
                    Error::InvalidRequest(_) => false,
                    Error::Timeout(_) => true,
                    Error::Unknown(_) => true,
                },
                None => true,
            },
            RoutingTableError::NoResult => true,
        }
    }
}

impl IsRetriableError for CallWorkerExecutorError {
    fn is_retriable(&self) -> bool {
        match self {
            CallWorkerExecutorError::FailedToGetRoutingTable(error) => error.is_retriable(),
            CallWorkerExecutorError::FailedToConnectToPod(status) => status.is_retriable(),
        }
    }
}

struct RetryState<'a> {
    started_at: Instant,
    attempt: u64,
    retry_attempt: u64,
    retry_config: &'a RetryConfig,
}

impl<'a> RetryState<'a> {
    fn new(retry_config: &'a RetryConfig) -> Self {
        RetryState {
            started_at: Instant::now(),
            attempt: 0,
            retry_attempt: 0,
            retry_config,
        }
    }

    fn start_attempt(&mut self, executor_kind: &'static str) -> RetrySpan {
        self.attempt += 1;
        self.retry_attempt += 1;
        RetrySpan::new(executor_kind, self.attempt)
    }

    async fn retry<T: HasRoutingTableService, U>(
        &mut self,
        context: &T,
        error: &impl Debug,
        pod: &Option<Pod>,
    ) -> Result<Option<U>, WorkerServiceError> {
        let invalidated = context
            .routing_table_service()
            .try_invalidate_routing_table()
            .await;

        match get_delay(self.retry_config, self.retry_attempt) {
            Some(delay) => {
                info!(
                    invalidated,
                    error = format!("{error:?}"),
                    pod = format!("{:?}", pod.as_ref().map(|p| p.uri_02())),
                    delay_ms = delay.as_millis(),
                    "Call on executor - retry"
                );
                sleep(delay).await;
                Ok(None)
            }
            None => {
                let delay = self.retry_config.max_delay;
                self.retry_attempt = 0;
                warn!(
                    invalidated,
                    error = format!("{error:?}"),
                    pod = format_pod(pod),
                    delay_ms = delay.as_millis(),
                    "Call on executor - retry - resetting retry attempts"
                );
                Ok(None)
            }
        }
    }

    fn non_retryable_error<T>(
        &self,
        error: WorkerServiceError,
        pod: &Option<Pod>,
    ) -> Result<Option<T>, WorkerServiceError> {
        error!(
            error = error.to_string(),
            pod = format_pod(pod),
            "Call on executor - non retriable error"
        );
        Err(error)
    }

    fn success(&self, pod: &Option<Pod>) {
        info!(
            duration_ms = self.started_at.elapsed().as_millis(),
            pod = format_pod(pod),
            "Call on executor - success"
        );
    }
}

fn format_pod(pod: &Option<Pod>) -> String {
    format!("{:?}", pod.as_ref().map(|p| p.uri_02()))
}

struct RetrySpan {
    pub span: tracing::Span,
}

impl RetrySpan {
    fn new(call_on_executor_kind: &'static str, attempt: u64) -> Self {
        Self {
            span: tracing::span!(
                tracing::Level::INFO,
                "call_on_executor_retry",
                executor_kind = call_on_executor_kind,
                attempt,
            ),
        }
    }
}
