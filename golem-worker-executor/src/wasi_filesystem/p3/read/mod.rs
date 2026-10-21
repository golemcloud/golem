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

const FILESYSTEM_READ_CHUNK_SIZE: usize = 64 * 1024;

type FilesystemReadResult = wasmtime::Result<Result<(), types::ErrorCode>>;

pub(super) struct FilesystemReadProducer {
    interrupt: FilesystemInterruptSignal,
    generation_handle: FilesystemGenerationHandle,
    descriptor: AgentDescriptor,
    offset: u64,
    buffered: Bytes,
    pending: Option<Pin<Box<agent_filesystem::FilesystemCall<Bytes>>>>,
    result_tx: Option<tokio::sync::oneshot::Sender<FilesystemReadResult>>,
    #[cfg(test)]
    read_calls: Arc<std::sync::atomic::AtomicUsize>,
}

impl FilesystemReadProducer {
    pub(super) async fn new(
        generation_handle: FilesystemGenerationHandle,
        descriptor: AgentDescriptor,
        offset: u64,
        result_tx: tokio::sync::oneshot::Sender<FilesystemReadResult>,
        interrupt: FilesystemInterruptSignal,
    ) -> wasmtime::Result<Self> {
        #[cfg(test)]
        let read_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut producer = Self {
            interrupt,
            generation_handle,
            descriptor,
            offset,
            buffered: Bytes::new(),
            pending: None,
            result_tx: Some(result_tx),
            #[cfg(test)]
            read_calls,
        };
        let prefetched = producer.read_chunk(FILESYSTEM_READ_CHUNK_SIZE).await;
        match prefetched {
            Ok(Some(bytes)) => producer.buffered = bytes,
            Ok(None) => producer.close(Ok(Ok(()))),
            Err(error) => match error.downcast() {
                Ok(error) => producer.close(Ok(Err(error))),
                Err(error) => {
                    if error.downcast_ref::<InterruptKind>().is_some() {
                        producer.result_tx.take();
                        return Err(error);
                    }
                    producer.close(Err(error));
                }
            },
        }
        Ok(producer)
    }

    fn close(&mut self, result: FilesystemReadResult) {
        if let Some(result_tx) = self.result_tx.take() {
            let _ = result_tx.send(result);
        }
    }

    fn start_read_chunk(
        &self,
        length: usize,
    ) -> FilesystemResult<agent_filesystem::FilesystemCall<Bytes>> {
        #[cfg(test)]
        self.read_calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.descriptor.with_node(|node| match node {
            OpenNode::File(file) => agent_filesystem::read_file(
                &self.generation_handle,
                file,
                agent_filesystem::ReadRange {
                    offset: self.offset,
                    length,
                },
            )
            .map_err(|error| p3_agent_error(AgentFilesystemError::Access(error))),
            OpenNode::Directory(_) => Err(types::ErrorCode::BadDescriptor.into()),
        })
    }

    async fn read_chunk(&mut self, length: usize) -> FilesystemResult<Option<Bytes>> {
        let call = self.start_read_chunk(length)?;
        let bytes = observe_filesystem_operation(&mut self.interrupt, call)
            .await
            .map_err(FilesystemError::trap)?
            .map_err(p3_agent_error)?;
        if bytes.is_empty() {
            return Ok(None);
        }
        self.offset = self
            .offset
            .checked_add(bytes.len() as u64)
            .ok_or(types::ErrorCode::Overflow)?;
        Ok(Some(bytes))
    }

    fn take_buffered(&mut self, length: usize) -> Bytes {
        let length = self.buffered.len().min(length);
        self.buffered.split_to(length)
    }

    #[cfg(test)]
    fn read_call_count(&self) -> usize {
        self.read_calls.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn poll_read_chunk(
        &mut self,
        cx: &mut Context<'_>,
        length: usize,
    ) -> Poll<FilesystemResult<Option<Bytes>>> {
        if self.pending.is_none() {
            let call = match self.start_read_chunk(length) {
                Ok(call) => call,
                Err(error) => return Poll::Ready(Err(error)),
            };
            self.pending = Some(Box::pin(call));
        }

        let result = match self
            .pending
            .as_mut()
            .expect("read call missing")
            .as_mut()
            .poll(cx)
        {
            Poll::Pending => {
                if let Poll::Ready(kind) = self.interrupt.as_mut().poll(cx) {
                    self.pending = None;
                    return Poll::Ready(Err(FilesystemError::trap(kind)));
                }
                return Poll::Pending;
            }
            Poll::Ready(result) => result,
        };
        self.pending = None;
        let bytes = result.map_err(p3_agent_error)?;
        if bytes.is_empty() {
            return Poll::Ready(Ok(None));
        }
        self.offset = self
            .offset
            .checked_add(bytes.len() as u64)
            .ok_or(types::ErrorCode::Overflow)?;
        Poll::Ready(Ok(Some(bytes)))
    }
}

impl<D> StreamProducer<D> for FilesystemReadProducer {
    type Item = u8;
    type Buffer = BytesMut;

    fn poll_produce<'a>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: StoreContextMut<'a, D>,
        dst: Destination<'a, Self::Item, Self::Buffer>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        if finish {
            self.pending.take();
            return Poll::Ready(Ok(StreamResult::Cancelled));
        }
        if dst.remaining(store.as_context_mut()) == Some(0) {
            return Poll::Ready(Ok(StreamResult::Completed));
        }

        let mut dst = dst.as_direct(store, FILESYSTEM_READ_CHUNK_SIZE);
        let length = dst.remaining().len().min(FILESYSTEM_READ_CHUNK_SIZE);
        if !self.buffered.is_empty() {
            let bytes = self.take_buffered(length);
            let length = bytes.len();
            dst.remaining()[..length].copy_from_slice(&bytes);
            dst.mark_written(length);
            return Poll::Ready(Ok(StreamResult::Completed));
        }
        if self.result_tx.is_none() {
            return Poll::Ready(Ok(StreamResult::Dropped));
        }
        match self.poll_read_chunk(cx, length) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(Some(bytes))) => {
                let length = bytes.len();
                dst.remaining()[..length].copy_from_slice(&bytes);
                dst.mark_written(length);
                Poll::Ready(Ok(StreamResult::Completed))
            }
            Poll::Ready(Ok(None)) => {
                self.close(Ok(Ok(())));
                Poll::Ready(Ok(StreamResult::Dropped))
            }
            Poll::Ready(Err(error)) => {
                match error.downcast() {
                    Ok(error) => self.close(Ok(Err(error))),
                    Err(error) => {
                        if let Some(kind) = error.downcast_ref::<InterruptKind>().copied() {
                            self.close(Err(wasmtime::Error::from(kind)));
                            return Poll::Ready(Err(error));
                        }
                        self.close(Err(error));
                    }
                }
                Poll::Ready(Ok(StreamResult::Dropped))
            }
        }
    }
}

impl Drop for FilesystemReadProducer {
    fn drop(&mut self) {
        self.close(Ok(Ok(())));
    }
}

#[cfg(test)]
mod tests;
