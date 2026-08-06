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

use std::{
    pin::Pin,
    task::{Context, Poll},
};

use futures::{Stream, StreamExt};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tonic::{Status, Streaming};
use tracing::{Instrument, Level, error};

use golem_common::metrics::api::{
    record_closed_grpc_api_active_stream, record_new_grpc_api_active_stream,
};
use golem_common::related_span;
use golem_common::tracing::TraceOrigin;

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

        // The pump lives as long as the stream does - a `connect` websocket can stay
        // open for hours - so it links back to the request that opened it rather than
        // running inside its span, which would keep that span open just as long.
        //
        // `new` is reached from `WorkerService::connect` and from the file-read path
        // (`get_file_contents`), both of which the API layer runs under a request
        // span, so a span is current here. See `TraceOrigin::capture_current` for the
        // rule.
        let origin = TraceOrigin::capture_current();

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
            .instrument(related_span!(origin, Level::INFO, "worker_stream_pump")),
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
