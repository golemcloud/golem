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

use crate::service::worker::WorkerServiceError;
use async_trait::async_trait;
use golem_api_grpc::proto::golem::workerexecutor::worker_executor_client::WorkerExecutorClient;
use golem_common::client::MultiTargetGrpcClient;
use golem_service_base::model::{GolemError, GolemErrorInvalidShardId, WorkerId};
use golem_service_base::routing_table::{HasRoutingTableService, RoutingTableError};
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
use tokio::task::JoinSet;
use tokio::time::sleep;
use tonic::transport::Channel;
use tonic::Status;
use tracing::{debug, info};

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
        G: Fn(Target::ResultOut) -> Result<R, GolemError> + Send;
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
}

pub trait HasWorkerExecutorClients {
    fn worker_executor_clients(&self) -> &MultiTargetGrpcClient<WorkerExecutorClient<Channel>>;
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
        G: Fn(Target::ResultOut) -> Result<R, GolemError> + Send,
    {
        loop {
            match target
                .call_on_worker_executor(self, remote_call.clone())
                .await
            {
                Ok(None) => {
                    info!("No active shards");
                    info!("Invalidating routing table");
                    self.routing_table_service()
                        .invalidate_routing_table()
                        .await;
                    sleep(Duration::from_secs(1)).await;
                }
                Ok(Some(out)) => match response_map(out) {
                    Ok(result) => break Ok(result),
                    Err(GolemError::InvalidShardId(GolemErrorInvalidShardId {
                        shard_id,
                        shard_ids,
                    })) => {
                        info!("InvalidShardId: {} not in {:?}", shard_id, shard_ids);
                        info!("Invalidating routing table");
                        self.routing_table_service()
                            .invalidate_routing_table()
                            .await;
                        sleep(Duration::from_secs(1)).await;
                    }
                    Err(other) => {
                        debug!("Got {:?}, not retrying", other);
                        break Err(WorkerServiceError::internal(other));
                    }
                },
                Err(GetWorkerExecutorClientError::FailedToGetRoutingTable(
                    RoutingTableError::Unexpected(details),
                )) if is_connection_failure(&details) => {
                    info!("Shard manager unavailable");
                    info!("Invalidating routing table and retrying in 1 seconds");
                    self.routing_table_service()
                        .invalidate_routing_table()
                        .await;
                    sleep(Duration::from_secs(1)).await;
                }
                Err(GetWorkerExecutorClientError::FailedToConnectToPod(status))
                    if is_connection_failure(&status.to_string()) =>
                {
                    info!("Worker executor unavailable");
                    info!("Invalidating routing table and retrying immediately");
                    self.routing_table_service()
                        .invalidate_routing_table()
                        .await;
                }
                Err(other) => {
                    debug!("Got {}, not retrying", other);
                    // let err = anyhow::Error::new(other);
                    break Err(WorkerServiceError::internal(other));
                }
            };
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
    message.contains("UNAVAILABLE")
        || message.contains("CHANNEL CLOSED")
        || message.contains("transport error")
        || message.contains("Connection refused")
}
