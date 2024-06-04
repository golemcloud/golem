use crate::service::limit::LimitService;
use futures::{Stream, StreamExt};
use golem_api_grpc::proto::golem::worker::LogEvent;
use golem_common::model::AccountId;
use golem_service_base::model::WorkerId;
use golem_worker_service_base::service::worker::ConnectWorkerStream as BaseWorkerStream;
use std::sync::Arc;
use tonic::Status;

pub struct ConnectWorkerStream {
    stream: BaseWorkerStream,
    worker_id: WorkerId,
    account_id: AccountId,
    limit_service: Arc<dyn LimitService + Sync + Send>,
}

impl ConnectWorkerStream {
    pub fn new(
        stream: BaseWorkerStream,
        worker_id: WorkerId,
        account_id: AccountId,
        limit_service: Arc<dyn LimitService + Sync + Send>,
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
            "Dropping worker {} connections of account {}",
            self.worker_id,
            self.account_id
        );
        let limit_service = self.limit_service.clone();
        let account_id = self.account_id.clone();
        let worker_id = self.worker_id.clone();
        tokio::spawn(async move {
            let result = limit_service
                .update_worker_connection_limit(&account_id, &worker_id, -1)
                .await;
            if let Err(error) = result {
                tracing::error!(
                    "Decrement active connections of account {account_id} failed {error}",
                );
            }
        });
    }
}
