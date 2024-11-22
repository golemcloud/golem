use cloud_common::auth::CloudNamespace;
use cloud_common::clients::limit::LimitService;
use futures::{Stream, StreamExt};
use golem_api_grpc::proto::golem::worker::LogEvent;
use golem_common::model::WorkerId;
use golem_worker_service_base::service::worker::WorkerStream;
use std::sync::Arc;
use tonic::Status;

pub struct ConnectWorkerStream {
    stream: WorkerStream<LogEvent>,
    worker_id: WorkerId,
    namespace: CloudNamespace,
    limit_service: Arc<dyn LimitService + Sync + Send>,
}

impl ConnectWorkerStream {
    pub fn new(
        stream: WorkerStream<LogEvent>,
        worker_id: WorkerId,
        namespace: CloudNamespace,
        limit_service: Arc<dyn LimitService + Sync + Send>,
    ) -> Self {
        Self {
            stream,
            worker_id,
            namespace,
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
            namespace = %self.namespace,
            "Dropping worker {} connections",
            self.worker_id
        );
        let limit_service = self.limit_service.clone();
        let namespace = self.namespace.clone();
        let worker_id = self.worker_id.clone();
        tokio::spawn(async move {
            let result = limit_service
                .update_worker_connection_limit(&namespace.account_id, &worker_id, -1)
                .await;
            if let Err(error) = result {
                tracing::error!(
                    namespace = %namespace,
                    "Decrement active connections failed {error}",
                );
            }
        });
    }
}
