use std::{
    pin::Pin,
    task::{Context, Poll},
};

use futures::{Stream, StreamExt};
use golem_api_grpc::proto::golem::worker::LogEvent;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tonic::{Status, Streaming};

pub struct ConnectWorkerStream {
    receiver: mpsc::Receiver<Result<LogEvent, Status>>,
    cancel: CancellationToken,
}

impl ConnectWorkerStream {
    pub fn new(streaming: Streaming<LogEvent>) -> Self {
        // Create a channel which is Send and Sync.
        // Streaming is not Sync.
        let (sender, receiver) = mpsc::channel(32);
        let mut streaming = streaming;

        let cancel = CancellationToken::new();

        tokio::spawn({
            let cancel = cancel.clone();

            let forward_loop = {
                let sender = sender.clone();
                async move {
                    while let Some(message) = streaming.next().await {
                        if let Err(error) = sender.send(message).await {
                            tracing::info!("Failed to forward WorkerStream: {error}");
                            break;
                        }
                    }
                }
            };

            async move {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        tracing::info!("WorkerStream cancelled");
                    }
                    _ = forward_loop => {
                        tracing::info!("WorkerStream forward loop finished");
                    }
                };
                sender.closed().await;
            }
        });

        Self { receiver, cancel }
    }
}

impl Stream for ConnectWorkerStream {
    type Item = Result<LogEvent, Status>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<LogEvent, Status>>> {
        self.receiver.poll_recv(cx)
    }
}

impl Drop for ConnectWorkerStream {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
