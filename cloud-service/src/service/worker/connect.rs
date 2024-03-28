use std::sync::Arc;

use futures::{Stream, StreamExt};
use golem_api_grpc::proto::golem::worker::LogEvent;
use golem_common::model::AccountId;
use golem_worker_service_base::service::worker::ConnectWorkerStream as BaseWorkerStream;
use tonic::Status;

use crate::repo::account_connections::AccountConnectionsRepo;

pub struct ConnectWorkerStream {
    stream: BaseWorkerStream,
    account_connections_repository: Arc<dyn AccountConnectionsRepo + Send + Sync>,
    account_id: AccountId,
}

impl ConnectWorkerStream {
    pub fn new(
        stream: BaseWorkerStream,
        account_connections_repository: Arc<dyn AccountConnectionsRepo + Send + Sync>,
        account_id: AccountId,
    ) -> Self {
        Self {
            stream,
            account_connections_repository,
            account_id,
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
        let account_connections_repository = self.account_connections_repository.clone();
        let account_id = self.account_id.clone();
        tokio::spawn(async move {
            let result = account_connections_repository.update(&account_id, -1).await;
            if let Err(error) = result {
                tracing::error!(
                    "Decrement active connections of account {account_id} failed {error}",
                );
            }
        });
    }
}
