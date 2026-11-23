// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use crate::service::worker::WorkerServiceError;
use async_trait::async_trait;
use golem_api_grpc::proto::golem::worker::v1::WorkerExecutionError;
use golem_api_grpc::proto::golem::workerexecutor::v1::worker_executor_client::WorkerExecutorClient;
use golem_common::SafeDisplay;
use golem_common::model::RetryConfig;
use golem_common::model::{AgentId, Pod, ShardId};
use golem_common::retriable_error::{IsRetriableError, is_grpc_message_size_limit};
use golem_common::retries::get_delay;
use golem_service_base::clients::shard_manager::ShardManagerError;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::grpc::client::MultiTargetGrpcClient;
use golem_service_base::service::routing_table::HasRoutingTableService;
use std::collections::HashSet;
use std::fmt::Debug;
use std::future::Future;
use std::pin::Pin;
use tokio::task::JoinSet;
use tokio::time::{Instant, sleep};
use tonic::Status;
use tonic::transport::Channel;
use tonic_tracing_opentelemetry::middleware::client::OtelGrpcService;
use tracing::{Instrument, debug, info, trace, warn};

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
        <Target as CallOnExecutor<Out>>::ResultOut: Debug,
        F: for<'a> Fn(
                &'a mut WorkerExecutorClient<OtelGrpcService<Channel>>,
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
                &'a mut WorkerExecutorClient<OtelGrpcService<Channel>>,
            )
                -> Pin<Box<dyn Future<Output = Result<Out, Status>> + 'a + Send>>
            + Send
            + Sync
            + Clone
            + 'static;

    fn tracing_kind(&self) -> &'static str;

    /// Whether the routing retries the call when it found no executor, the executor refused the
    /// call before it did work, or the connection failed in a way that can be retried. A call that
    /// is not retried fails at its first such outcome.
    fn retried(&self) -> bool {
        true
    }
}

#[async_trait]
impl<Out: Send + 'static> CallOnExecutor<Out> for AgentId {
    type ResultOut = Out;

    async fn call_on_worker_executor<F>(
        &self,
        description: impl AsRef<str> + Send,
        context: &(impl HasRoutingTableService + HasWorkerExecutorClients + Send + Sync),
        f: F,
    ) -> Result<(Option<Self::ResultOut>, Option<Pod>), CallWorkerExecutorErrorWithContext>
    where
        F: for<'a> Fn(
                &'a mut WorkerExecutorClient<OtelGrpcService<Channel>>,
            )
                -> Pin<Box<dyn Future<Output = Result<Out, Status>> + 'a + Send>>
            + Send
            + Sync
            + Clone
            + 'static,
    {
        call_agent_on_worker_executor(self, true, description, context, f).await
    }

    fn tracing_kind(&self) -> &'static str {
        "AgentId"
    }
}

/// A call on an agent that the connection and the routing replay only when `replayable`. A call
/// that failed after the worker service sent it can have had an effect on the executor that no
/// response reports; such a call is not `replayable`. Without a replay, the connection still
/// retires a dead connection, and the routing still retries when it found no executor or the
/// executor refused the call before it did work. The status of a failed call keeps its message,
/// and gets a code that the routing does not retry.
pub struct AgentCall {
    pub agent_id: AgentId,
    pub replayable: bool,
}

#[async_trait]
impl<Out: Send + 'static> CallOnExecutor<Out> for AgentCall {
    type ResultOut = Out;

    async fn call_on_worker_executor<F>(
        &self,
        description: impl AsRef<str> + Send,
        context: &(impl HasRoutingTableService + HasWorkerExecutorClients + Send + Sync),
        f: F,
    ) -> Result<(Option<Self::ResultOut>, Option<Pod>), CallWorkerExecutorErrorWithContext>
    where
        F: for<'a> Fn(
                &'a mut WorkerExecutorClient<OtelGrpcService<Channel>>,
            )
                -> Pin<Box<dyn Future<Output = Result<Out, Status>> + 'a + Send>>
            + Send
            + Sync
            + Clone
            + 'static,
    {
        call_agent_on_worker_executor(&self.agent_id, self.replayable, description, context, f)
            .await
    }

    fn tracing_kind(&self) -> &'static str {
        "AgentId"
    }
}

/// The status of a call that is not replayed: a status that the routing retries gets the code
/// `FAILED_PRECONDITION`, and keeps its code and message in its message.
fn not_replayed(status: Status) -> Status {
    if CallWorkerExecutorError::FailedToConnectToPod(status.clone()).is_retriable() {
        Status::failed_precondition(format!(
            "a call that cannot be replayed failed after it was sent ({}): {}",
            status.code(),
            status.message()
        ))
    } else {
        status
    }
}

async fn call_agent_on_worker_executor<Out, F>(
    agent_id: &AgentId,
    replayable: bool,
    description: impl AsRef<str> + Send,
    context: &(impl HasRoutingTableService + HasWorkerExecutorClients + Send + Sync),
    f: F,
) -> Result<(Option<Out>, Option<Pod>), CallWorkerExecutorErrorWithContext>
where
    Out: Send + 'static,
    F: for<'a> Fn(
            &'a mut WorkerExecutorClient<OtelGrpcService<Channel>>,
        ) -> Pin<Box<dyn Future<Output = Result<Out, Status>> + 'a + Send>>
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

    tracing::debug!("Routing agent call for {agent_id}");

    match routing_table.lookup(agent_id) {
        None => Ok((None, None)),
        Some(pod) => {
            let clients = context.worker_executor_clients();

            tracing::debug!("Calling agent {agent_id} on pod: {pod:?}");

            let uri = pod.uri(clients.uses_tls());
            let result = if replayable {
                clients.call(description, uri, f).await
            } else {
                clients
                    .call_without_retry(description, uri, f)
                    .await
                    .map_err(not_replayed)
            };
            Ok((
                Some(result.map_err(|err| {
                    CallWorkerExecutorErrorWithContext::failed_to_connect_to_pod(err, *pod)
                })?),
                Some(*pod),
            ))
        }
    }
}

/// A call on any executor. A call that is not `replayable` is neither replayed by the connection
/// nor retried by the routing: it makes one attempt, as a call that must not spend more work than
/// one call of the executor needs. A call that failed after the worker service sent it gets the
/// status of [`not_replayed`].
pub struct RandomExecutor {
    pub replayable: bool,
}

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
                &'a mut WorkerExecutorClient<OtelGrpcService<Channel>>,
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
            Some(pod) => {
                let clients = context.worker_executor_clients();

                let uri = pod.uri(clients.uses_tls());
                let result = if self.replayable {
                    clients.call(description, uri, f).await
                } else {
                    clients
                        .call_without_retry(description, uri, f)
                        .await
                        .map_err(not_replayed)
                };
                Ok((
                    Some(result.map_err(|status| {
                        CallWorkerExecutorErrorWithContext::failed_to_connect_to_pod(status, *pod)
                    })?),
                    Some(*pod),
                ))
            }
        }
    }

    fn tracing_kind(&self) -> &'static str {
        "RandomExecutor"
    }

    fn retried(&self) -> bool {
        self.replayable
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
                &'a mut WorkerExecutorClient<OtelGrpcService<Channel>>,
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
                        let pod = *pod;
                        let f = f.clone();
                        let description = description.clone();
                        async move {
                            worker_executor_clients
                                .call(description, pod.uri(worker_executor_clients.uses_tls()), f)
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
    fn worker_executor_clients(
        &self,
    ) -> &MultiTargetGrpcClient<WorkerExecutorClient<OtelGrpcService<Channel>>>;
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

impl From<WorkerExecutorError> for ResponseMapResult {
    fn from(error: WorkerExecutorError) -> Self {
        match error {
            WorkerExecutorError::InvalidShardId {
                shard_id,
                shard_ids,
            } => ResponseMapResult::InvalidShardId {
                shard_id,
                shard_ids: HashSet::from_iter(shard_ids),
            },
            WorkerExecutorError::ShardingNotReady => ResponseMapResult::ShardingNotReady,
            WorkerExecutorError::AgentNotFound { .. }
            | WorkerExecutorError::AgentAlreadyExists { .. }
            | WorkerExecutorError::CreateIdempotencyKeyConflict { .. }
            | WorkerExecutorError::InvocationCancelled
            | WorkerExecutorError::CannotCancelRunningInvocation
            | WorkerExecutorError::ComponentNotFound { .. } => {
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
        let golem_error = error
            .clone()
            .try_into()
            .unwrap_or_else(|_| WorkerExecutorError::unknown("Unknown worker execution error"));
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
        <Target as CallOnExecutor<Out>>::ResultOut: Debug,
        F: for<'a> Fn(
                &'a mut WorkerExecutorClient<OtelGrpcService<Channel>>,
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
        let retried = target.retried();
        loop {
            let span = retry.start_attempt(Target::tracing_kind(&target));

            let worker_result = target
                .call_on_worker_executor(description.as_ref(), self, remote_call.clone())
                .await;

            let result = async {
                match worker_result {
                    Ok((result, pod)) => match result {
                        None if retried => retry.retry(self, &"NoActiveShards", &pod).await,
                        None => retry.not_retried(&"NoActiveShards", &pod),
                        Some(out) => match response_map(out) {
                            Ok(result) => {
                                retry.success(&pod);
                                Ok(Some(result))
                            }
                            Err(
                                error @ (ResponseMapResult::InvalidShardId { .. }
                                | ResponseMapResult::ShardingNotReady),
                            ) => {
                                if retried {
                                    retry.retry(self, &error, &pod).await
                                } else {
                                    retry.not_retried(&error, &pod)
                                }
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
                        if retried && error.is_retriable() {
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
    FailedToGetRoutingTable(ShardManagerError),
    #[error("Failed to connect to pod: {} {}", .0.code(), .0.message())]
    FailedToConnectToPod(Status),
    #[error("gRPC message exceeded the configured size limit: {}", .0.message())]
    MessageTooLarge(Status),
}

impl SafeDisplay for CallWorkerExecutorError {
    fn to_safe_string(&self) -> String {
        match self {
            CallWorkerExecutorError::FailedToGetRoutingTable(_) => self.to_string(),
            CallWorkerExecutorError::FailedToConnectToPod(_) => self.to_string(),
            CallWorkerExecutorError::MessageTooLarge(_) => self.to_string(),
        }
    }
}

#[derive(Debug)]
pub struct CallWorkerExecutorErrorWithContext {
    error: CallWorkerExecutorError,
    pod: Option<Pod>,
}

impl CallWorkerExecutorErrorWithContext {
    fn failed_to_get_routing_table(error: ShardManagerError) -> Self {
        CallWorkerExecutorErrorWithContext {
            error: CallWorkerExecutorError::FailedToGetRoutingTable(error),
            pod: None,
        }
    }

    fn failed_to_connect_to_pod(status: Status, pod: Pod) -> Self {
        let error = if is_grpc_message_size_limit(&status) {
            CallWorkerExecutorError::MessageTooLarge(status)
        } else {
            CallWorkerExecutorError::FailedToConnectToPod(status)
        };
        CallWorkerExecutorErrorWithContext {
            error,
            pod: Some(pod),
        }
    }
}

impl IsRetriableError for CallWorkerExecutorError {
    fn is_retriable(&self) -> bool {
        match self {
            CallWorkerExecutorError::FailedToGetRoutingTable(error) => error.is_retriable(),
            CallWorkerExecutorError::FailedToConnectToPod(status) => {
                status.is_retriable() || status.code() == tonic::Code::Cancelled
            }
            CallWorkerExecutorError::MessageTooLarge(_) => false,
        }
    }

    fn as_loggable(&self) -> Option<String> {
        Some(self.to_string())
    }
}

struct RetryState<'a> {
    started_at: Instant,
    attempt: u64,
    retry_attempt: u32,
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
            "Attempting to call executor"
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
                    attempt = self.attempt,
                    delay_ms = delay.as_millis(),
                    op = self.op,
                    "Retry calling executor after delay"
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
                    "Retry calling executor reached the maximum attempts"
                );
                // The attempt counter restarts and the loop goes on, so the wait has to happen
                // here too, or an executor that keeps refusing is retried as fast as the network
                // allows.
                sleep(delay).await;
                Ok(None)
            }
        }
    }

    /// Fails a call that is not retried at an outcome that the routing retries for other calls.
    fn not_retried<T>(
        &self,
        error: &impl Debug,
        pod: &Option<Pod>,
    ) -> Result<Option<T>, WorkerServiceError> {
        info!(
            error = format!("{error:?}"),
            pod = format_pod(pod),
            op = self.op,
            "Executor call that is not retried failed"
        );
        Err(WorkerServiceError::Internal(format!(
            "The call to the executor was not retried: {error:?}"
        )))
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
            "Expected executor error, not retrying"
        );
        Err(error)
    }

    fn non_retryable_error<T>(
        &self,
        error: WorkerServiceError,
        pod: &Option<Pod>,
    ) -> Result<Option<T>, WorkerServiceError> {
        info!(
            error = error.to_string(),
            pod = format_pod(pod),
            op = self.op,
            "Non retryable executor error"
        );
        Err(error)
    }

    fn success(&self, pod: &Option<Pod>) {
        info!(
            duration_ms = self.started_at.elapsed().as_millis(),
            pod = format_pod(pod),
            op = self.op,
            "Call to executor succeeded"
        );
    }
}

fn format_pod(pod: &Option<Pod>) -> String {
    format!("{:?}", pod.as_ref())
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

#[cfg(test)]
mod tests {
    use super::{CallWorkerExecutorError, not_replayed};
    use golem_common::retriable_error::IsRetriableError;
    use test_r::test;
    use tonic::{Code, Status};

    #[test]
    fn a_call_that_cannot_be_replayed_fails_with_a_status_the_routing_does_not_retry() {
        for code in [
            Code::Unavailable,
            Code::Unknown,
            Code::Cancelled,
            Code::DeadlineExceeded,
            Code::Internal,
            Code::Aborted,
        ] {
            let status = not_replayed(Status::new(code, "connection reset"));
            assert!(
                !CallWorkerExecutorError::FailedToConnectToPod(status.clone()).is_retriable(),
                "{code:?}"
            );
            assert_eq!(status.code(), Code::FailedPrecondition, "{code:?}");
            assert!(status.message().contains("connection reset"), "{code:?}");
        }
        // A status that the routing does not retry keeps its code.
        let status = not_replayed(Status::not_found("no agent"));
        assert_eq!(status.code(), Code::NotFound);
        assert_eq!(status.message(), "no agent");
    }
}
