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

use std::{
    pin::Pin,
    task::{Context, Poll},
};

use futures::{Stream, StreamExt};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tonic::{Status, Streaming};
use tracing::{error, Instrument};

use golem_common::metrics::api::{
    record_closed_grpc_api_active_stream, record_new_grpc_api_active_stream,
};

pub struct WorkerStream<T> {
    receiver: mpsc::Receiver<Result<T, Status>>,
    cancel: CancellationToken,
}

impl<T: Send + 'static> WorkerStream<T> {
    pub fn new(streaming: Streaming<T>) -> Self {
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
                            break;
                        }
                        message = streaming.next() => {
                            if let Some(message) = message {
                                if let Err(error) = sender.send(message).await {
                                    error!(
                                        error = error.to_string(),
                                        "Failed to forward WorkerStream"
                                    );
                                    break;
                                }
                            } else {
                                break;
                            }
                        }
                    }
                }

                drop(sender);
                record_closed_grpc_api_active_stream();
            }
            .in_current_span(),
        );

        Self { receiver, cancel }
    }
}

impl<T: Send + 'static> Stream for WorkerStream<T> {
    type Item = Result<T, Status>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<T, Status>>> {
        self.receiver.poll_recv(cx)
    }
}

impl<T> Drop for WorkerStream<T> {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
