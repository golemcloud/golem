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

use super::{
    FileInspection, FilesystemGenerationHandle, ReadRange, open_file_for_inspection, read_file,
};
use crate::sandbox_filesystem::SandboxFilesystemAdapter;
use bytes::Bytes;
use futures::Stream;
use futures::task::AtomicWaker;
use golem_common::model::filesystem::{
    FILE_READ_CHUNK_SIZE, FileByteSelection, FileReadError, FileReadExtent, FileReadHead,
};
use golem_service_base::model::FileReadResponse;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use tokio::sync::{Notify, mpsc, oneshot};
use tokio::time::{Instant, sleep_until};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Streaming,
    AllQueued,
    ConsumerEof,
    Cancelled,
    Aborted(FileReadError),
}

struct Completion {
    state: Mutex<State>,
    producer: Notify,
    consumer: AtomicWaker,
}

impl Completion {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State::Streaming),
            producer: Notify::new(),
            consumer: AtomicWaker::new(),
        })
    }

    fn finish(&self, terminal: State) {
        let mut state = self.state.lock().unwrap();
        if matches!(*state, State::Streaming | State::AllQueued) {
            *state = terminal;
        }
        drop(state);
        self.producer.notify_one();
        self.consumer.wake();
    }

    async fn wait_for_consumer(&self) {
        loop {
            let notified = self.producer.notified();
            if !matches!(
                *self.state.lock().unwrap(),
                State::Streaming | State::AllQueued
            ) {
                return;
            }
            notified.await;
        }
    }
}

struct ReadBody {
    receiver: mpsc::Receiver<Bytes>,
    completion: Arc<Completion>,
    deadline: Instant,
    done: bool,
}

struct ProducerGuard(Arc<Completion>);

impl Drop for ProducerGuard {
    fn drop(&mut self) {
        self.0.finish(State::Aborted(FileReadError::Lifecycle));
    }
}

impl Stream for ReadBody {
    type Item = Result<Bytes, FileReadError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.done {
            return Poll::Ready(None);
        }
        let completion = self.completion.clone();
        completion.consumer.register(cx.waker());
        let mut state = completion.state.lock().unwrap();
        if Instant::now() >= self.deadline && matches!(*state, State::Streaming | State::AllQueued)
        {
            *state = State::Aborted(FileReadError::DeadlineExceeded);
        }
        // Serialize delivery with abort and AllQueued publication, including the EOF poll.
        if let State::Aborted(error) = *state {
            self.done = true;
            self.receiver.close();
            drop(state);
            completion.producer.notify_one();
            return Poll::Ready(Some(Err(error)));
        }
        match self.receiver.poll_recv(cx) {
            Poll::Ready(Some(bytes)) => Poll::Ready(Some(Ok(bytes))),
            Poll::Ready(None) => {
                self.done = true;
                let result = if *state == State::AllQueued {
                    *state = State::ConsumerEof;
                    Poll::Ready(None)
                } else {
                    *state = State::Aborted(FileReadError::Lifecycle);
                    Poll::Ready(Some(Err(FileReadError::Lifecycle)))
                };
                drop(state);
                completion.producer.notify_one();
                result
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for ReadBody {
    fn drop(&mut self) {
        if !self.done {
            self.completion.finish(State::Cancelled);
        }
    }
}

/// The invocation loop awaits this while holding the resident Store and exclusive OwnerLane.
/// The future does not finish on last-byte enqueue: it keeps that serialization scope until
/// the consumer polls EOF, drops the body, or the arrival deadline or an I/O failure wins.
/// No producer task is spawned, and file length is never converted to a host-sized allocation.
pub(crate) async fn produce_file_read<Adapter: SandboxFilesystemAdapter>(
    handle: &FilesystemGenerationHandle<Adapter>,
    path: &str,
    selection: FileByteSelection,
    deadline: Instant,
    mut response: oneshot::Sender<Result<FileReadResponse, FileReadError>>,
) {
    if Instant::now() >= deadline {
        let _ = response.send(Err(FileReadError::DeadlineExceeded));
        return;
    }
    let inspection = tokio::select! {
        biased;
        _ = sleep_until(deadline) => Err(FileReadError::DeadlineExceeded),
        _ = response.closed() => return,
        result = open_file_for_inspection(handle, path, selection) => result,
    };
    if Instant::now() >= deadline {
        let _ = response.send(Err(FileReadError::DeadlineExceeded));
        return;
    }
    let (mut file, metadata) = match inspection {
        Err(error) => {
            let _ = response.send(Err(error));
            return;
        }
        Ok(FileInspection::Rejected(head)) => {
            let _ = response.send(Ok(FileReadResponse {
                head,
                body: Box::pin(futures::stream::empty()),
            }));
            return;
        }
        Ok(FileInspection::Opened { file, metadata }) => (file, metadata),
    };
    let (mut offset, mut remaining) = match metadata.selection {
        FileReadExtent::Selected { offset, length } => (offset, length),
        FileReadExtent::Unsatisfiable => (0, 0),
    };
    if remaining == 0 {
        let _ = response.send(Ok(FileReadResponse {
            head: FileReadHead::File(metadata),
            body: Box::pin(futures::stream::empty()),
        }));
        return;
    }
    let completion = Completion::new();
    let _producer = ProducerGuard(completion.clone());
    let (sender, receiver) = mpsc::channel(1);
    let body = ReadBody {
        receiver,
        completion: completion.clone(),
        deadline,
        done: false,
    };
    if response
        .send(Ok(FileReadResponse {
            head: FileReadHead::File(metadata),
            body: Box::pin(body),
        }))
        .is_err()
    {
        return;
    }
    let file = &mut file;
    let producer_completion = completion.clone();
    let produce = async move {
        let sender_ref = &sender;
        let outcome = async move {
            while remaining != 0 {
                let slot = sender_ref
                    .reserve()
                    .await
                    .map_err(|_| FileReadError::Lifecycle)?;
                if Instant::now() >= deadline {
                    return Err(FileReadError::DeadlineExceeded);
                }
                let length = remaining.min(FILE_READ_CHUNK_SIZE as u64) as usize;
                let bytes = read_file(handle, file, ReadRange { offset, length })
                    .map_err(|_| FileReadError::Lifecycle)?
                    .await
                    .map_err(|error| match error {
                        super::Error::Sandbox(_) => FileReadError::Storage,
                        _ => FileReadError::Lifecycle,
                    })?;
                if bytes.is_empty() || bytes.len() > length {
                    // A short read is allowed, but premature EOF cannot silently truncate a head.
                    return Err(FileReadError::Storage);
                }
                offset += bytes.len() as u64;
                remaining -= bytes.len() as u64;
                slot.send(bytes);
            }
            Ok(())
        }
        .await;
        // Publish the result before closing the channel, including on an I/O error.
        {
            let mut state = producer_completion.state.lock().unwrap();
            if *state == State::Streaming {
                *state = match outcome {
                    Ok(()) => State::AllQueued,
                    Err(error) => State::Aborted(error),
                };
            }
        }
        drop(sender);
        producer_completion.wait_for_consumer().await;
    };
    tokio::select! {
        biased;
        _ = sleep_until(deadline) => completion.finish(State::Aborted(FileReadError::DeadlineExceeded)),
        _ = completion.wait_for_consumer() => {},
        _ = produce => {},
    }
}
