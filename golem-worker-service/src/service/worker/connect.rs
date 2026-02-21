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

use super::WorkerStream;
use crate::service::limit::LimitService;
use futures::{Stream, StreamExt};
use golem_api_grpc::proto::golem::worker::LogEvent;
use golem_common::model::WorkerId;
use golem_common::model::account::AccountId;
use std::sync::Arc;
use tonic::Status;

pub struct ConnectWorkerStream {
    stream: WorkerStream<LogEvent>,
    worker_id: WorkerId,
    account_id: AccountId,
    limit_service: Arc<dyn LimitService>,
}

impl ConnectWorkerStream {
    pub fn new(
        stream: WorkerStream<LogEvent>,
        worker_id: WorkerId,
        account_id: AccountId,
        limit_service: Arc<dyn LimitService>,
    ) -> Self {
        Self {
            stream,
            worker_id,
            account_id,
            limit_service,
        }
    }
}

impl Stream for ConnectWorkerStream {
    type Item = Result<LogEvent, Status>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<LogEvent, Status>>> {
        self.stream.poll_next_unpin(cx)
    }
}

impl Drop for ConnectWorkerStream {
    fn drop(&mut self) {
        tracing::info!(
            account_id = %self.account_id,
            "Dropping worker {} connections",
            self.worker_id
        );
        let limit_service = self.limit_service.clone();
        let worker_id = self.worker_id.clone();
        let account_id = self.account_id;
        tokio::spawn(async move {
            let result = limit_service
                .update_worker_connection_limit(account_id, &worker_id, false)
                .await;

            if let Err(error) = result {
                tracing::error!(
                    account_id = %account_id,
                    "Decrement active connections failed {error}",
                );
            }
        });
    }
}
