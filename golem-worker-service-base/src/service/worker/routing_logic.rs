// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use std::collections::HashSet;
use std::fmt::Debug;
use std::future::Future;
use std::pin::Pin;

use async_trait::async_trait;
use tokio::task::JoinSet;
use tokio::time::{sleep, Instant};
use tonic::transport::Channel;
use tonic::Status;
use tracing::{debug, error, info, trace, warn, Instrument};

use golem_api_grpc::proto::golem::worker::v1::WorkerExecutionError;
use golem_api_grpc::proto::golem::workerexecutor::v1::worker_executor_client::WorkerExecutorClient;
use golem_common::client::MultiTargetGrpcClient;
use golem_common::model::error::{GolemError, GolemErrorInvalidShardId, GolemErrorUnknown};
use golem_common::model::RetryConfig;
use golem_common::model::{Pod, ShardId, TargetWorkerId, WorkerId};
use golem_common::retriable_error::IsRetriableError;
use golem_common::retries::get_delay;
use golem_common::SafeDisplay;
use golem_service_base::service::routing_table::{HasRoutingTableService, RoutingTableError};

use crate::service::worker::WorkerServiceError;

#[async_trait]
pub trait RoutingLogic {
    async fn call_worker_executor<Target, F, G, H, Out, R>(
        &self,
        target: Target,
        description: impl AsRef<str> + Send,
        remote_call: F,
        response_map: G,
        error_map: H,
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
        H: Fn(CallWorkerExecutorError) -> WorkerServiceError + Send + Sync;
}

#[async_trait]
pub trait CallOnExecutor<Out: Send + 'static> {
    type ResultOut: Send;

    async fn call_on_worker_executor<F>(
        &self,
        description: impl AsRef<str> + Send,
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

    fn tracing_kind(&self) -> &'static str;
}

#[async_trait]
impl<Out: Send + 'static> CallOnExecutor<Out> for WorkerId {
    type ResultOut = Out;

    async fn call_on_worker_executor<F>(
        &self,
        description: impl AsRef<str> + Send,
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
                        .call(description, pod.uri(), f)
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

    fn tracing_kind(&self) -> &'static str {
        "WorkerId"
    }
}

#[async_trait]
impl<Out: Send + 'static> CallOnExecutor<Out> for TargetWorkerId {
    type ResultOut = Out;

    async fn call_on_worker_executor<F>(
        &self,
        description: impl AsRef<str> + Send,
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
        if let Some(worker_id) = self.clone().try_into_worker_id() {
            // The TargetWorkerId had a worker name so we know which shard we need to call it on
            worker_id
                .call_on_worker_executor(description, context, f)
                .await
        } else {
            // The TargetWorkerId did not have a worker name specified so we can forward the call to a random
            // executor
            RandomExecutor
                .call_on_worker_executor(description, context, f)
                .await
        }
    }

    fn tracing_kind(&self) -> &'static str {
        if self.worker_name.is_none() {
            "RandomExecutor"
        } else {
            "WorkerId"
        }
    }
}

pub struct RandomExecutor;

#[async_trait]
impl<Out: Send + 'static> CallOnExecutor<Out> for RandomExecutor {
    type ResultOut = Out;

    async fn call_on_worker_executor<F>(
        &self,
        description: impl AsRef<str> + Send,
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
                        .call(description, pod.uri(), f)
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

    fn tracing_kind(&self) -> &'static str {
        "RandomExecutor"
    }
}

pub struct AllExecutors;

#[async_trait]
impl<Out: Send + 'static> CallOnExecutor<Out> for AllExecutors {
    type ResultOut = Vec<Out>;

    async fn call_on_worker_executor<F>(
        &self,
        description: impl AsRef<str> + Send,
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
        let description = description.as_ref().to_string();
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
                let _ = fibers.spawn(
                    {
                        let pod = pod.clone();
                        let f = f.clone();
                        let description = description.clone();
                        async move {
                            worker_executor_clients
                                .call(description, pod.uri(), f)
                                .await
                                .map_err(|err| (err, pod))
                        }
                    }
                    .in_current_span(),
                );
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

    fn tracing_kind(&self) -> &'static str {
        "AllExecutors"
    }
}

pub trait HasWorkerExecutorClients {
    fn worker_executor_clients(&self) -> &MultiTargetGrpcClient<WorkerExecutorClient<Channel>>;
    fn worker_executor_retry_config(&self) -> &RetryConfig;
}

#[derive(Debug)]
pub enum ResponseMapResult {
    InvalidShardId {
        shard_id: ShardId,
        shard_ids: HashSet<ShardId>,
    },
    ShardingNotReady,
    /// Error that is expected, not to be logged/counted as an error
    Expected(WorkerServiceError),
    /// Error that is unexpected, to be logged/counted as an error
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
            GolemError::ShardingNotReady(_) => ResponseMapResult::ShardingNotReady,
            GolemError::WorkerNotFound(_) | GolemError::WorkerAlreadyExists(_) => {
                ResponseMapResult::Expected(error.into())
            }
            other => ResponseMapResult::Other(other.into()),
        }
    }
}

impl From<&'static str> for ResponseMapResult {
    fn from(error: &'static str) -> Self {
        ResponseMapResult::Other(WorkerServiceError::Internal(error.to_string()))
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
        trace!(
            error = format!("{:?}", error),
            golem_error = golem_error.to_string(),
            response_map_result = format!("{:?}", response_map_result),
            "ResponseMapResult from WorkerExecutionError"
        );
        response_map_result
    }
}

impl From<WorkerServiceError> for ResponseMapResult {
    fn from(error: WorkerServiceError) -> Self {
        Self::Other(error)
    }
}

#[async_trait]
impl<T: HasRoutingTableService + HasWorkerExecutorClients + Send + Sync> RoutingLogic for T {
    async fn call_worker_executor<Target, F, G, H, Out, R>(
        &self,
        target: Target,
        description: impl AsRef<str> + Send,
        remote_call: F,
        response_map: G,
        error_map: H,
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
        H: Fn(CallWorkerExecutorError) -> WorkerServiceError + Send + Sync,
    {
        let mut retry = RetryState::new(self.worker_executor_retry_config(), description.as_ref());
        loop {
            let span = retry.start_attempt(Target::tracing_kind(&target));

            let worker_result = target
                .call_on_worker_executor(description.as_ref(), self, remote_call.clone())
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
                            Err(error @ ResponseMapResult::ShardingNotReady) => {
                                retry.retry(self, &error, &pod).await
                            }
                            Err(ResponseMapResult::Expected(error)) => {
                                retry.non_retryable_expected_error(error, &pod)
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
                            retry.non_retryable_error(error_map(error), &pod)
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
    #[error("Failed to get routing table: {0}")]
    FailedToGetRoutingTable(RoutingTableError),
    #[error("Failed to connect to pod: {} {}", .0.code(), .0.message())]
    FailedToConnectToPod(Status),
}

impl SafeDisplay for CallWorkerExecutorError {
    fn to_safe_string(&self) -> String {
        match self {
            CallWorkerExecutorError::FailedToGetRoutingTable(_) => self.to_string(),
            CallWorkerExecutorError::FailedToConnectToPod(_) => self.to_string(),
        }
    }
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

impl IsRetriableError for CallWorkerExecutorError {
    fn is_retriable(&self) -> bool {
        match self {
            CallWorkerExecutorError::FailedToGetRoutingTable(error) => error.is_retriable(),
            CallWorkerExecutorError::FailedToConnectToPod(status) => status.is_retriable(),
        }
    }

    fn as_loggable(&self) -> Option<String> {
        Some(self.to_string())
    }
}

struct RetryState<'a> {
    started_at: Instant,
    attempt: u64,
    retry_attempt: u64,
    retry_config: &'a RetryConfig,
    op: &'a str,
}

impl<'a> RetryState<'a> {
    fn new(retry_config: &'a RetryConfig, op: &'a str) -> Self {
        RetryState {
            started_at: Instant::now(),
            attempt: 0,
            retry_attempt: 0,
            retry_config,
            op,
        }
    }

    fn start_attempt(&mut self, executor_kind: &'static str) -> RetrySpan {
        self.attempt += 1;
        self.retry_attempt += 1;
        debug!(
            attempt = self.attempt,
            executor_kind = executor_kind,
            op = self.op,
            "Call on executor - start attempt"
        );
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
                    ?error,
                    ?pod,
                    delay_ms = delay.as_millis(),
                    op = self.op,
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
                    op = self.op,
                    "Call on executor - retry - resetting retry attempts"
                );
                Ok(None)
            }
        }
    }

    fn non_retryable_expected_error<T>(
        &self,
        error: WorkerServiceError,
        pod: &Option<Pod>,
    ) -> Result<Option<T>, WorkerServiceError> {
        trace!(
            error = error.to_string(),
            pod = format_pod(pod),
            op = self.op,
            "Call on executor - non retriable expected error"
        );
        Err(error)
    }

    fn non_retryable_error<T>(
        &self,
        error: WorkerServiceError,
        pod: &Option<Pod>,
    ) -> Result<Option<T>, WorkerServiceError> {
        error!(
            error = error.to_string(),
            pod = format_pod(pod),
            op = self.op,
            "Call on executor - non retriable error"
        );
        Err(error)
    }

    fn success(&self, pod: &Option<Pod>) {
        info!(
            duration_ms = self.started_at.elapsed().as_millis(),
            pod = format_pod(pod),
            op = self.op,
            "Call on executor - success"
        );
    }
}

fn format_pod(pod: &Option<Pod>) -> String {
    format!("{:?}", pod.as_ref().map(|p| p.uri()))
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
