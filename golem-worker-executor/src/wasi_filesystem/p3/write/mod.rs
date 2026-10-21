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

use super::*;

pub(super) type P3WriteFutureResult =
    wasmtime::Result<Result<(usize, WritePlacement), types::ErrorCode>>;

pub(super) struct FilesystemWriteChunk {
    result_tx: tokio::sync::oneshot::Sender<(usize, FilesystemWriteResult)>,
    bytes: Bytes,
}

#[derive(Clone, Debug)]
pub(super) enum FilesystemWriteFailure {
    Guest(types::ErrorCode),
}

pub(super) type FilesystemWriteResult = Result<(), FilesystemWriteFailure>;

pub(super) struct FilesystemWriteConsumer {
    chunks_tx: Option<tokio::sync::mpsc::UnboundedSender<FilesystemWriteChunk>>,
    pending_chunk: Option<PendingFilesystemWriteChunk>,
}

struct PendingFilesystemWriteChunk {
    result_rx: tokio::sync::oneshot::Receiver<(usize, FilesystemWriteResult)>,
}

impl FilesystemWriteConsumer {
    pub(super) fn new(chunks_tx: tokio::sync::mpsc::UnboundedSender<FilesystemWriteChunk>) -> Self {
        Self {
            chunks_tx: Some(chunks_tx),
            pending_chunk: None,
        }
    }

    fn cancel(&mut self) {
        self.chunks_tx.take();
    }

    fn poll_pending_result(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<wasmtime::Result<Option<(usize, FilesystemWriteResult)>>> {
        let Some(pending) = &mut self.pending_chunk else {
            return Poll::Ready(Ok(None));
        };
        match Pin::new(&mut pending.result_rx).poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(result)) => {
                self.pending_chunk = None;
                Poll::Ready(Ok(Some(result)))
            }
            Poll::Ready(Err(_)) => {
                self.pending_chunk = None;
                self.chunks_tx.take();
                Poll::Ready(Err(wasmtime::Error::msg(
                    "filesystem write task dropped before reporting its effect",
                )))
            }
        }
    }
}

impl<D> StreamConsumer<D> for FilesystemWriteConsumer {
    type Item = u8;

    fn poll_consume(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        store: StoreContextMut<D>,
        src: Source<Self::Item>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        let mut src = src.as_direct(store);

        if finish {
            self.cancel();
            if self.pending_chunk.is_none() {
                return Poll::Ready(Ok(StreamResult::Cancelled));
            }
        }

        loop {
            // Wait for the in-flight chunk to be persisted before reading more.
            // The receiver must be polled here (not just stored) so its waker is
            // registered; otherwise the write task's completion notification
            // could be missed, hanging the stream.
            if self.pending_chunk.is_some() {
                match self.poll_pending_result(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(Ok(Some((written, Ok(()))))) => {
                        src.mark_read(written);
                        return Poll::Ready(Ok(if finish {
                            StreamResult::Cancelled
                        } else {
                            StreamResult::Completed
                        }));
                    }
                    Poll::Ready(Ok(Some((written, Err(FilesystemWriteFailure::Guest(_)))))) => {
                        self.chunks_tx.take();
                        src.mark_read(written);
                        return Poll::Ready(Ok(StreamResult::Dropped));
                    }
                    Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                    Poll::Ready(Ok(None)) => unreachable!("pending result disappeared"),
                }
            }

            let bytes = src.remaining();
            if bytes.is_empty() {
                return Poll::Ready(Ok(StreamResult::Completed));
            }

            let Some(chunks_tx) = &self.chunks_tx else {
                return Poll::Ready(Ok(StreamResult::Dropped));
            };

            let (result_tx, result_rx) = tokio::sync::oneshot::channel();
            chunks_tx
                .send(FilesystemWriteChunk {
                    result_tx,
                    bytes: Bytes::copy_from_slice(bytes),
                })
                .map_err(|_| wasmtime::Error::msg("filesystem write task dropped"))?;
            self.pending_chunk = Some(PendingFilesystemWriteChunk { result_rx });
            // Loop back to poll the freshly created receiver and register its waker.
        }
    }
}

impl Drop for FilesystemWriteConsumer {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub(super) struct FilesystemWriteTask<Ctx> {
    chunks_rx: tokio::sync::mpsc::UnboundedReceiver<FilesystemWriteChunk>,
    result_tx: tokio::sync::oneshot::Sender<wasmtime::Result<Result<(), types::ErrorCode>>>,
    generation_handle: FilesystemGenerationHandle,
    descriptor: AgentDescriptor,
    placement: WritePlacement,
    activity: TailActivity,
    _authorization_permit: Option<LiveAuthorizationPermit>,
    _phantom: PhantomData<fn() -> Ctx>,
}

impl<Ctx> FilesystemWriteTask<Ctx> {
    pub(super) fn new(
        chunks_rx: tokio::sync::mpsc::UnboundedReceiver<FilesystemWriteChunk>,
        result_tx: tokio::sync::oneshot::Sender<wasmtime::Result<Result<(), types::ErrorCode>>>,
        generation_handle: FilesystemGenerationHandle,
        descriptor: AgentDescriptor,
        placement: WritePlacement,
        activity: TailActivity,
        authorization_permit: Option<LiveAuthorizationPermit>,
    ) -> Self {
        Self {
            chunks_rx,
            result_tx,
            generation_handle,
            descriptor,
            placement,
            activity,
            _authorization_permit: authorization_permit,
            _phantom: PhantomData,
        }
    }
}

impl<Ctx, U> AccessorTask<U, DurableP3<Ctx>> for FilesystemWriteTask<Ctx>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    async fn run(self, _accessor: &Accessor<U, DurableP3<Ctx>>) -> wasmtime::Result<()> {
        let FilesystemWriteTask {
            mut chunks_rx,
            result_tx,
            generation_handle,
            descriptor,
            placement,
            activity,
            _authorization_permit,
            _phantom,
        } = self;
        complete_filesystem_write_task(
            result_tx,
            run_streaming_filesystem_write(
                &mut chunks_rx,
                &generation_handle,
                &descriptor,
                placement,
                &activity,
            ),
        )
        .await;
        Ok(())
    }
}

async fn complete_filesystem_write_task(
    result_tx: tokio::sync::oneshot::Sender<wasmtime::Result<Result<(), types::ErrorCode>>>,
    operation: impl Future<Output = wasmtime::Result<Result<(), types::ErrorCode>>>,
) {
    let result = operation.await;
    if !result_tx.is_closed() {
        let _ = result_tx.send(result);
    }
}

pub(super) async fn wait_filesystem_task_result(
    result_rx: tokio::sync::oneshot::Receiver<wasmtime::Result<Result<(), types::ErrorCode>>>,
    interrupt: FilesystemInterruptSignal,
) -> wasmtime::Result<Result<(), types::ErrorCode>> {
    observe_filesystem_operation(interrupt, result_rx)
        .await
        .map_err(wasmtime::Error::from)?
        .unwrap_or_else(|_| Err(wasmtime::Error::msg("filesystem stream task dropped")))
}

// Drains and writes the guest's data stream to the worker filesystem chunk by
// chunk, mirroring the WASI P2 behavior: the file effect is driven entirely by
// the input stream finishing or erroring, never by the liveness of the returned
// result future. The bytes themselves are not recorded in the oplog; on replay
// the guest re-issues the same writes which deterministically rebuild the
// transient worker filesystem.
async fn run_streaming_filesystem_write(
    chunks_rx: &mut tokio::sync::mpsc::UnboundedReceiver<FilesystemWriteChunk>,
    generation_handle: &FilesystemGenerationHandle,
    descriptor: &AgentDescriptor,
    mut placement: WritePlacement,
    activity: &TailActivity,
) -> wasmtime::Result<Result<(), types::ErrorCode>> {
    let mut result: FilesystemWriteResult = Ok(());
    // Safe park: each chunk is guest-produced stream data.
    while let Some(chunk) = activity.park(chunks_rx.recv()).await {
        let written_len = if result.is_ok() {
            let operation = descriptor.with_node(|node| match node {
                OpenNode::File(file) => {
                    route_stream_chunk(generation_handle, file, placement, chunk.bytes)
                }
                OpenNode::Directory(_) => Err(types::ErrorCode::BadDescriptor.into()),
            });
            match operation {
                Ok(operation) => match operation.await? {
                    Ok((written, next)) => {
                        placement = next;
                        written
                    }
                    Err(error) => {
                        result = Err(FilesystemWriteFailure::Guest(error));
                        0
                    }
                },
                Err(error) => match error.downcast() {
                    Ok(error) => {
                        result = Err(FilesystemWriteFailure::Guest(error));
                        0
                    }
                    Err(error) => return Err(error),
                },
            }
        } else {
            0
        };

        if chunk.result_tx.send((written_len, result.clone())).is_err() {
            return Err(wasmtime::Error::msg(
                "filesystem write progress could not be delivered",
            ));
        }
    }

    filesystem_write_result_to_wasi(result)
}

pub(super) fn filesystem_write_result_to_wasi(
    result: FilesystemWriteResult,
) -> wasmtime::Result<Result<(), types::ErrorCode>> {
    match result {
        Ok(()) => Ok(Ok(())),
        Err(FilesystemWriteFailure::Guest(error)) => Ok(Err(error)),
    }
}

#[cfg(test)]
mod tests;
