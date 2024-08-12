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

use std::{
    pin::Pin,
    task::{Context, Poll},
};

use futures::{Stream, StreamExt};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tonic::{Status, Streaming};
use tracing::{error, info, Instrument};

use golem_api_grpc::proto::golem::worker::LogEvent;
use golem_common::metrics::api::{
    record_closed_grpc_api_active_stream, record_new_grpc_api_active_stream,
};

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
        let cancel_clone = cancel.clone();

        tokio::spawn(
            async move {
                record_new_grpc_api_active_stream();

                loop {
                    tokio::select! {
                        _ = cancel_clone.cancelled() => {
                            info!("WorkerStream cancelled");
                            break;
                        }
                        message = streaming.next() => {
                            if let Some(message) = message {
                                info!("ConnectWorkerStream received message {message:?}");
                                if let Err(error) = sender.send(message).await {
                                    error!(
                                        error = error.to_string(),
                                        "Failed to forward WorkerStream"
                                    );
                                    break;
                                }
                            } else {
                                info!("WorkerStream finished");
                                break;
                            }
                        }
                    }
                }

                info!("WorkerStream dropping sender");
                drop(sender);
                info!("WorkerStream sender dropped");

                record_closed_grpc_api_active_stream();
            }
            .in_current_span(),
        );

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
