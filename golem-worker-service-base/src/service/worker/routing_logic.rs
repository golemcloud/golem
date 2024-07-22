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
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use anyhow::anyhow;
use async_trait::async_trait;
use tokio::task::JoinSet;
use tokio::time::sleep;
use tonic::transport::Channel;
use tonic::Status;
use tracing::{debug, error, info, Instrument};

use golem_api_grpc::proto::golem::worker::WorkerExecutionError;
use golem_api_grpc::proto::golem::workerexecutor::worker_executor_client::WorkerExecutorClient;
use golem_common::client::MultiTargetGrpcClient;
use golem_common::model::ShardId;
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
    ) -> Result<Option<Self::ResultOut>, GetWorkerExecutorClientError>
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
    ) -> Result<Option<Self::ResultOut>, GetWorkerExecutorClientError>
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
    ) -> Result<Option<Self::ResultOut>, GetWorkerExecutorClientError>
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
            .map_err(GetWorkerExecutorClientError::FailedToGetRoutingTable)?;

        match routing_table.lookup(self) {
            None => Ok(None),
            Some(pod) => Ok(Some(
                context
                    .worker_executor_clients()
                    .call(pod.uri_02(), f)
                    .await
                    .map_err(GetWorkerExecutorClientError::FailedToConnectToPod)?,
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
    ) -> Result<Option<Self::ResultOut>, GetWorkerExecutorClientError>
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
            .map_err(GetWorkerExecutorClientError::FailedToGetRoutingTable)?;

        match routing_table.random() {
            None => Ok(None),
            Some(pod) => Ok(Some(
                context
                    .worker_executor_clients()
                    .call(pod.uri_02(), f)
                    .await
                    .map_err(GetWorkerExecutorClientError::FailedToConnectToPod)?,
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
    ) -> Result<Option<Self::ResultOut>, GetWorkerExecutorClientError>
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
            .map_err(GetWorkerExecutorClientError::FailedToGetRoutingTable)?;

        let pods = routing_table.all();
        if pods.is_empty() {
            Ok(None)
        } else {
            let mut fibers = JoinSet::new();
            for pod in pods {
                let pod = pod.clone();
                let f_clone = f.clone();
                let worker_executor_clients = context.worker_executor_clients().clone();
                let _ = fibers.spawn(async move {
                    worker_executor_clients.call(pod.uri_02(), f_clone).await
                });
            }
            let mut results = Vec::new();
            while let Some(result) = fibers.join_next().await {
                results.push(result.expect("Join error"));
            }
            let results = results
                .into_iter()
                .collect::<Result<Vec<Out>, _>>()
                .map_err(GetWorkerExecutorClientError::FailedToConnectToPod)?;

            Ok(Some(results))
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
        let mut attempt = 0;
        loop {
            attempt += 1;
            let span = RetrySpan::new(Target::tracing_kind(), attempt);

            let worker_result = target
                .call_on_worker_executor(self, remote_call.clone())
                .await;

            let result = async {
                match worker_result {
                    Ok(None) => invalidate_routing_table_sleep(self, "NoActiveShards", None).await,
                    Ok(Some(out)) => match response_map(out) {
                        Ok(result) => Ok(Some(result)),
                        Err(ResponseMapResult::InvalidShardId {
                            shard_id,
                            shard_ids,
                        }) => {
                            debug!(
                                shard_id = shard_id.to_string(),
                                available_shard_ids = format!("{:?}", shard_ids),
                                "Invalid shard_id"
                            );
                            invalidate_routing_table_sleep(self, "InvalidShardID", None).await
                        }
                        Err(ResponseMapResult::Other(error)) => {
                            logged_non_retryable_error("WorkerExecutor", error)
                        }
                    },
                    Err(GetWorkerExecutorClientError::FailedToGetRoutingTable(
                        RoutingTableError::Unexpected(details),
                    )) if is_connection_failure(&details) => {
                        invalidate_routing_table_sleep(
                            self,
                            "FailedToGetRoutingTable",
                            Some(details.as_str()),
                        )
                        .await
                    }
                    Err(GetWorkerExecutorClientError::FailedToConnectToPod(status))
                        if is_connection_failure(&status.to_string()) =>
                    {
                        invalidate_routing_table_no_sleep(
                            self,
                            "FailedToConnectToPod",
                            Some(status.message()),
                        )
                        .await
                    }
                    Err(error) => {
                        logged_non_retryable_error("Routing", WorkerServiceError::internal(error))
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
pub enum GetWorkerExecutorClientError {
    // TODO: Change to display
    #[error("Failed to get routing table: {0:?}")]
    FailedToGetRoutingTable(RoutingTableError),
    #[error("Failed to connect to pod {0}")]
    FailedToConnectToPod(Status),
}

fn is_connection_failure(message: &str) -> bool {
    static CONNECTION_FAILURE_ERRORS: &[&str] = &[
        "CHANNEL CLOSED",
        "Connection refused",
        "UNAVAILABLE",
        "channel closed",
        "error trying to connect",
        "transport error",
    ];

    CONNECTION_FAILURE_ERRORS
        .iter()
        .any(|e| message.contains(e))
}

fn logged_non_retryable_error<T>(
    error_kind: &'static str,
    error: WorkerServiceError,
) -> Result<Option<T>, WorkerServiceError> {
    error!(
        error = error.to_string(),
        error_kind = error_kind,
        "Non retryable error"
    );
    Err(error)
}

async fn invalidate_routing_table_no_sleep<T: HasRoutingTableService, U>(
    context: &T,
    reason: &str,
    error: Option<&str>,
) -> Result<Option<U>, WorkerServiceError> {
    invalidate_routing_table(context, reason, error, false).await;
    Ok(None)
}

async fn invalidate_routing_table_sleep<T: HasRoutingTableService, U>(
    context: &T,
    reason: &str,
    error: Option<&str>,
) -> Result<Option<U>, WorkerServiceError> {
    invalidate_routing_table(context, reason, error, true).await;
    Ok(None)
}

async fn invalidate_routing_table<T: HasRoutingTableService>(
    context: &T,
    reason: &str,
    error: Option<&str>,
    should_sleep: bool,
) {
    info!(
        reason,
        error,
        sleep_after_invalidation = should_sleep,
        "Invalidating routing table"
    );

    context
        .routing_table_service()
        .invalidate_routing_table()
        .await;

    if should_sleep {
        sleep(Duration::from_secs(1)).await;
    }
}

struct RetrySpan {
    pub span: tracing::Span,
}

impl RetrySpan {
    fn new(call_on_executor_kind: &'static str, attempt: usize) -> Self {
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
