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

use std::collections::VecDeque;
use std::io::{Cursor, Seek, SeekFrom, Write};
use std::marker::PhantomData;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::task::{Context, Poll, ready};

use crate::durable_host::concurrent::{CallHandle, CallReplayOutcome, Cancellable};
use crate::durable_host::p3::{
    DurableP3, DurableP3View, durable_worker_ctx, run_read_access, wasi_filesystem_view,
};
use crate::workerctx::WorkerCtx;
use bytes::BytesMut;
use cap_std::fs::FileExt;
use golem_common::model::oplog::host_functions::{
    P3FilesystemTypesDescriptorAppendViaStream, P3FilesystemTypesDescriptorReadDirectory,
    P3FilesystemTypesDescriptorReadViaStream, P3FilesystemTypesDescriptorStat,
    P3FilesystemTypesDescriptorStatAt, P3FilesystemTypesDescriptorWriteViaStream,
};
use golem_common::model::oplog::types::{
    SerializableFileTimes, SerializableP3DirectoryEntry, SerializableP3FileSystemError,
    SerializableP3FsErrorCode,
};
use golem_common::model::oplog::{
    DurableFunctionType, HostPayloadPair, HostRequestFileSystemPath,
    HostRequestFileSystemPathAndOffset, HostResponseP3FileSystemByteStream,
    HostResponseP3FileSystemDirectoryEntryStream, HostResponseP3FileSystemStat, OplogEntry,
};
use tokio::task::JoinHandle;
use wasmtime::AsContextMut;
use wasmtime::StoreContextMut;
use wasmtime::component::{
    Access, Accessor, AccessorTask, Destination, FutureReader, Resource, Source, StreamConsumer,
    StreamProducer, StreamReader, StreamResult,
};
use wasmtime_wasi::filesystem::{Descriptor, Dir, File, WasiFilesystem, WasiFilesystemView};
use wasmtime_wasi::p3::bindings::filesystem::{preopens, types};
use wasmtime_wasi::p3::filesystem::{FilesystemError, FilesystemResult};
use wasmtime_wasi::runtime::spawn_blocking;
use wasmtime_wasi::{DirPerms, FilePerms};

const FILESYSTEM_STREAM_BUFFER_CAPACITY: usize = 8192;
static FILESYSTEM_APPEND_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

#[derive(Clone)]
struct CapturedByteStream {
    contents: Vec<u8>,
    result: Result<(), types::ErrorCode>,
}

enum DeferredByteStreamMode {
    Stream(CapturedByteStream),
    Live,
    Error(String),
}

struct FileReadStreamProducer {
    file: File,
    offset: types::Filesize,
    contents: Vec<u8>,
    result_tx: Option<tokio::sync::oneshot::Sender<CapturedByteStream>>,
}

impl FileReadStreamProducer {
    fn new(
        file: File,
        offset: types::Filesize,
        result_tx: tokio::sync::oneshot::Sender<CapturedByteStream>,
    ) -> Self {
        Self {
            file,
            offset,
            contents: Vec::new(),
            result_tx: Some(result_tx),
        }
    }

    fn close(&mut self, result: Result<(), types::ErrorCode>) {
        if let Some(result_tx) = self.result_tx.take() {
            let _ = result_tx.send(CapturedByteStream {
                contents: std::mem::take(&mut self.contents),
                result,
            });
        }
    }
}

impl<D> StreamProducer<D> for FileReadStreamProducer {
    type Item = u8;
    type Buffer = Cursor<BytesMut>;

    fn poll_produce<'a>(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        mut store: StoreContextMut<'a, D>,
        dst: Destination<'a, Self::Item, Self::Buffer>,
        _finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        if dst.remaining(store.as_context_mut()) == Some(0) {
            return Poll::Ready(Ok(StreamResult::Completed));
        }

        let this = &mut *self;
        if !this.file.perms.contains(FilePerms::READ) {
            this.close(Err(types::ErrorCode::NotPermitted));
            return Poll::Ready(Ok(StreamResult::Dropped));
        }

        let mut dst = dst.as_direct(store, FILESYSTEM_STREAM_BUFFER_CAPACITY);
        let buffer = dst.remaining();
        if buffer.is_empty() {
            return Poll::Ready(Ok(StreamResult::Completed));
        }

        match this.file.file.read_at(buffer, this.offset) {
            Ok(0) => {
                this.close(Ok(()));
                Poll::Ready(Ok(StreamResult::Dropped))
            }
            Ok(read) => {
                let Ok(read_u64) = u64::try_from(read) else {
                    this.close(Err(types::ErrorCode::Overflow));
                    return Poll::Ready(Ok(StreamResult::Dropped));
                };
                let Some(offset) = this.offset.checked_add(read_u64) else {
                    this.close(Err(types::ErrorCode::Overflow));
                    return Poll::Ready(Ok(StreamResult::Dropped));
                };
                this.contents.extend_from_slice(&buffer[..read]);
                dst.mark_written(read);
                this.offset = offset;
                Poll::Ready(Ok(StreamResult::Completed))
            }
            Err(error) => {
                this.close(Err(error.into()));
                Poll::Ready(Ok(StreamResult::Dropped))
            }
        }
    }
}

impl Drop for FileReadStreamProducer {
    fn drop(&mut self) {
        self.close(Ok(()));
    }
}

struct ByteStreamProducer {
    contents: Cursor<BytesMut>,
    result: Result<(), types::ErrorCode>,
    result_tx: Option<tokio::sync::oneshot::Sender<CapturedByteStream>>,
}

impl ByteStreamProducer {
    fn new(
        contents: Vec<u8>,
        result: Result<(), types::ErrorCode>,
        result_tx: tokio::sync::oneshot::Sender<CapturedByteStream>,
    ) -> Self {
        Self {
            contents: Cursor::new(BytesMut::from(contents.as_slice())),
            result,
            result_tx: Some(result_tx),
        }
    }

    fn close(&mut self) {
        if let Some(result_tx) = self.result_tx.take() {
            let bytes = self.contents.get_ref();
            let position = self.contents.position() as usize;
            let result = if position >= bytes.len() {
                self.result.clone()
            } else {
                Ok(())
            };
            let _ = result_tx.send(CapturedByteStream {
                contents: bytes[..position.min(bytes.len())].to_vec(),
                result,
            });
        }
    }
}

impl<D> StreamProducer<D> for ByteStreamProducer {
    type Item = u8;
    type Buffer = Cursor<BytesMut>;

    fn poll_produce<'a>(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        mut store: StoreContextMut<'a, D>,
        dst: Destination<'a, Self::Item, Self::Buffer>,
        _finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        if dst.remaining(store.as_context_mut()) == Some(0) {
            return Poll::Ready(Ok(StreamResult::Completed));
        }

        let bytes = self.contents.get_ref();
        let position = self.contents.position() as usize;
        if position >= bytes.len() {
            self.close();
            return Poll::Ready(Ok(StreamResult::Dropped));
        }

        let mut dst = dst.as_direct(store, FILESYSTEM_STREAM_BUFFER_CAPACITY);
        let remaining = &bytes[position..];
        let n = remaining.len().min(dst.remaining().len());
        dst.remaining()[..n].copy_from_slice(&remaining[..n]);
        dst.mark_written(n);
        self.contents.set_position((position + n) as u64);
        Poll::Ready(Ok(StreamResult::Completed))
    }
}

impl Drop for ByteStreamProducer {
    fn drop(&mut self) {
        self.close();
    }
}

enum DeferredByteStreamProducerState {
    Awaiting {
        rx: tokio::sync::oneshot::Receiver<DeferredByteStreamMode>,
        file: Option<File>,
        offset: types::Filesize,
        result_tx: Option<tokio::sync::oneshot::Sender<CapturedByteStream>>,
    },
    Streaming(ByteStreamProducer),
    Live(FileReadStreamProducer),
    Done,
}

struct DeferredByteStreamProducer {
    state: DeferredByteStreamProducerState,
}

impl DeferredByteStreamProducer {
    fn new(
        file: File,
        offset: types::Filesize,
        rx: tokio::sync::oneshot::Receiver<DeferredByteStreamMode>,
        result_tx: tokio::sync::oneshot::Sender<CapturedByteStream>,
    ) -> Self {
        Self {
            state: DeferredByteStreamProducerState::Awaiting {
                rx,
                file: Some(file),
                offset,
                result_tx: Some(result_tx),
            },
        }
    }
}

impl<D> StreamProducer<D> for DeferredByteStreamProducer {
    type Item = u8;
    type Buffer = Cursor<BytesMut>;

    fn poll_produce<'a>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        store: StoreContextMut<'a, D>,
        dst: Destination<'a, Self::Item, Self::Buffer>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        loop {
            match &mut self.state {
                DeferredByteStreamProducerState::Awaiting {
                    rx,
                    file,
                    offset,
                    result_tx,
                } => match Pin::new(rx).poll(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(Ok(DeferredByteStreamMode::Stream(captured))) => {
                        let result_tx = result_tx
                            .take()
                            .expect("filesystem result sender available for replay");
                        self.state = DeferredByteStreamProducerState::Streaming(
                            ByteStreamProducer::new(captured.contents, captured.result, result_tx),
                        );
                    }
                    Poll::Ready(Ok(DeferredByteStreamMode::Live)) => {
                        let file = file
                            .take()
                            .expect("live filesystem file available for incomplete replay");
                        let result_tx = result_tx
                            .take()
                            .expect("filesystem result sender available for incomplete replay");
                        self.state = DeferredByteStreamProducerState::Live(
                            FileReadStreamProducer::new(file, *offset, result_tx),
                        );
                    }
                    Poll::Ready(Ok(DeferredByteStreamMode::Error(error))) => {
                        self.state = DeferredByteStreamProducerState::Done;
                        return Poll::Ready(Err(wasmtime::Error::msg(error)));
                    }
                    Poll::Ready(Err(_)) => {
                        self.state = DeferredByteStreamProducerState::Done;
                        return Poll::Ready(Err(wasmtime::Error::msg(
                            "filesystem replay task dropped",
                        )));
                    }
                },
                DeferredByteStreamProducerState::Streaming(producer) => {
                    return Pin::new(producer).poll_produce(cx, store, dst, finish);
                }
                DeferredByteStreamProducerState::Live(producer) => {
                    return Pin::new(producer).poll_produce(cx, store, dst, finish);
                }
                DeferredByteStreamProducerState::Done => {
                    return Poll::Ready(Ok(StreamResult::Dropped));
                }
            }
        }
    }
}

impl Drop for DeferredByteStreamProducer {
    fn drop(&mut self) {
        if let DeferredByteStreamProducerState::Awaiting { result_tx, .. } = &mut self.state
            && let Some(result_tx) = result_tx.take()
        {
            let _ = result_tx.send(CapturedByteStream {
                contents: Vec::new(),
                result: Ok(()),
            });
        }
    }
}

struct FilesystemWriteChunk {
    contents: Vec<u8>,
    result_tx: tokio::sync::oneshot::Sender<Result<(), types::ErrorCode>>,
}

struct FilesystemWriteConsumer {
    chunks_tx: Option<tokio::sync::mpsc::UnboundedSender<FilesystemWriteChunk>>,
    pending_chunk: Option<(
        usize,
        tokio::sync::oneshot::Receiver<Result<(), types::ErrorCode>>,
    )>,
}

impl FilesystemWriteConsumer {
    fn new(chunks_tx: tokio::sync::mpsc::UnboundedSender<FilesystemWriteChunk>) -> Self {
        Self {
            chunks_tx: Some(chunks_tx),
            pending_chunk: None,
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
        _finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        let mut src = src.as_direct(store);

        if let Some((len, result_rx)) = &mut self.pending_chunk {
            match Pin::new(result_rx).poll(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Ok(Ok(()))) => {
                    let len = *len;
                    self.pending_chunk = None;
                    src.mark_read(len);
                    return Poll::Ready(Ok(StreamResult::Completed));
                }
                Poll::Ready(Ok(Err(_))) | Poll::Ready(Err(_)) => {
                    let len = *len;
                    self.pending_chunk = None;
                    self.chunks_tx.take();
                    src.mark_read(len);
                    return Poll::Ready(Ok(StreamResult::Dropped));
                }
            }
        }

        let bytes = src.remaining();
        if bytes.is_empty() {
            return Poll::Ready(Ok(StreamResult::Completed));
        }

        let len = bytes.len();
        let Some(chunks_tx) = &self.chunks_tx else {
            src.mark_read(len);
            return Poll::Ready(Ok(StreamResult::Dropped));
        };

        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        chunks_tx
            .send(FilesystemWriteChunk {
                contents: bytes.to_vec(),
                result_tx,
            })
            .map_err(|_| wasmtime::Error::msg("filesystem write task dropped"))?;
        self.pending_chunk = Some((len, result_rx));
        Poll::Pending
    }
}

impl Drop for FilesystemWriteConsumer {
    fn drop(&mut self) {
        self.chunks_tx.take();
    }
}

struct DirectoryEntryStreamProducer {
    entries: VecDeque<types::DirectoryEntry>,
    consumed: Vec<types::DirectoryEntry>,
    result: Result<(), types::ErrorCode>,
    result_tx: Option<tokio::sync::oneshot::Sender<CapturedDirectoryEntryStream>>,
}

struct CapturedDirectoryEntryStream {
    entries: Vec<types::DirectoryEntry>,
    result: Result<(), types::ErrorCode>,
}

enum DeferredDirectoryEntryStreamMode {
    Stream(CapturedDirectoryEntryStream),
    Live,
    Error(String),
}

struct RecordingDirectoryEntryStreamProducer {
    rx: tokio::sync::mpsc::Receiver<types::DirectoryEntry>,
    task: JoinHandle<Result<(), types::ErrorCode>>,
    consumed: Vec<types::DirectoryEntry>,
    result_tx: Option<tokio::sync::oneshot::Sender<CapturedDirectoryEntryStream>>,
}

impl RecordingDirectoryEntryStreamProducer {
    fn new(
        dir: Dir,
        result_tx: tokio::sync::oneshot::Sender<CapturedDirectoryEntryStream>,
    ) -> Self {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let dir = Arc::clone(&dir.dir);
        Self {
            rx,
            task: tokio::task::spawn_blocking(move || {
                let entries = dir.entries()?;
                let mut sorted_entries = Vec::new();
                for entry in entries {
                    let Some(entry) = map_directory_entry(entry)? else {
                        continue;
                    };
                    sorted_entries.push(entry);
                }
                sorted_entries.sort_by_key(|entry| entry.name.clone());
                for entry in sorted_entries {
                    if tx.blocking_send(entry).is_err() {
                        break;
                    }
                }
                Ok(())
            }),
            consumed: Vec::new(),
            result_tx: Some(result_tx),
        }
    }

    fn close(&mut self, result: Result<(), types::ErrorCode>) {
        self.rx.close();
        self.task.abort();
        if let Some(result_tx) = self.result_tx.take() {
            let _ = result_tx.send(CapturedDirectoryEntryStream {
                entries: std::mem::take(&mut self.consumed),
                result,
            });
        }
    }
}

impl<D> StreamProducer<D> for RecordingDirectoryEntryStreamProducer {
    type Item = types::DirectoryEntry;
    type Buffer = Option<types::DirectoryEntry>;

    fn poll_produce<'a>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: StoreContextMut<'a, D>,
        mut dst: Destination<'a, Self::Item, Self::Buffer>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        if dst.remaining(&mut store) == Some(0) {
            return Poll::Ready(Ok(StreamResult::Completed));
        }

        match self.rx.poll_recv(cx) {
            Poll::Ready(Some(entry)) => {
                self.consumed.push(entry.clone());
                dst.set_buffer(Some(entry));
                Poll::Ready(Ok(StreamResult::Completed))
            }
            Poll::Ready(None) => {
                let result = match ready!(Pin::new(&mut self.task).poll(cx)) {
                    Ok(result) => result,
                    Err(error) if error.is_cancelled() => {
                        return Poll::Ready(Ok(StreamResult::Cancelled));
                    }
                    Err(error) => return Poll::Ready(Err(wasmtime::Error::msg(error.to_string()))),
                };
                self.close(result);
                Poll::Ready(Ok(StreamResult::Dropped))
            }
            Poll::Pending if finish => Poll::Ready(Ok(StreamResult::Cancelled)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for RecordingDirectoryEntryStreamProducer {
    fn drop(&mut self) {
        if self.result_tx.is_some() {
            self.close(Ok(()));
        }
    }
}

impl DirectoryEntryStreamProducer {
    fn new(
        entries: Vec<types::DirectoryEntry>,
        result: Result<(), types::ErrorCode>,
        result_tx: tokio::sync::oneshot::Sender<CapturedDirectoryEntryStream>,
    ) -> Self {
        Self {
            entries: entries.into(),
            consumed: Vec::new(),
            result,
            result_tx: Some(result_tx),
        }
    }

    fn close(&mut self) {
        if let Some(result_tx) = self.result_tx.take() {
            let result = if self.entries.is_empty() {
                self.result.clone()
            } else {
                Ok(())
            };
            let _ = result_tx.send(CapturedDirectoryEntryStream {
                entries: std::mem::take(&mut self.consumed),
                result,
            });
        }
    }
}

impl<D> StreamProducer<D> for DirectoryEntryStreamProducer {
    type Item = types::DirectoryEntry;
    type Buffer = Option<types::DirectoryEntry>;

    fn poll_produce<'a>(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        mut store: StoreContextMut<'a, D>,
        mut dst: Destination<'a, Self::Item, Self::Buffer>,
        _finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        if dst.remaining(&mut store) == Some(0) {
            return Poll::Ready(Ok(StreamResult::Completed));
        }

        match self.entries.pop_front() {
            Some(entry) => {
                self.consumed.push(entry.clone());
                dst.set_buffer(Some(entry));
                Poll::Ready(Ok(StreamResult::Completed))
            }
            None => {
                self.close();
                Poll::Ready(Ok(StreamResult::Dropped))
            }
        }
    }
}

impl Drop for DirectoryEntryStreamProducer {
    fn drop(&mut self) {
        self.close();
    }
}

enum DeferredDirectoryEntryStreamProducerState {
    Awaiting {
        rx: tokio::sync::oneshot::Receiver<DeferredDirectoryEntryStreamMode>,
        dir: Option<Dir>,
        result_tx: Option<tokio::sync::oneshot::Sender<CapturedDirectoryEntryStream>>,
    },
    Streaming(DirectoryEntryStreamProducer),
    Live(RecordingDirectoryEntryStreamProducer),
    Done,
}

struct DeferredDirectoryEntryStreamProducer {
    state: DeferredDirectoryEntryStreamProducerState,
}

impl DeferredDirectoryEntryStreamProducer {
    fn new(
        dir: Dir,
        rx: tokio::sync::oneshot::Receiver<DeferredDirectoryEntryStreamMode>,
        result_tx: tokio::sync::oneshot::Sender<CapturedDirectoryEntryStream>,
    ) -> Self {
        Self {
            state: DeferredDirectoryEntryStreamProducerState::Awaiting {
                rx,
                dir: Some(dir),
                result_tx: Some(result_tx),
            },
        }
    }
}

impl<D> StreamProducer<D> for DeferredDirectoryEntryStreamProducer {
    type Item = types::DirectoryEntry;
    type Buffer = Option<types::DirectoryEntry>;

    fn poll_produce<'a>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        store: StoreContextMut<'a, D>,
        dst: Destination<'a, Self::Item, Self::Buffer>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        loop {
            match &mut self.state {
                DeferredDirectoryEntryStreamProducerState::Awaiting { rx, dir, result_tx } => {
                    match Pin::new(rx).poll(cx) {
                        Poll::Pending => return Poll::Pending,
                        Poll::Ready(Ok(DeferredDirectoryEntryStreamMode::Stream(captured))) => {
                            let result_tx = result_tx
                                .take()
                                .expect("filesystem directory result sender available for replay");
                            self.state = DeferredDirectoryEntryStreamProducerState::Streaming(
                                DirectoryEntryStreamProducer::new(
                                    captured.entries,
                                    captured.result,
                                    result_tx,
                                ),
                            );
                        }
                        Poll::Ready(Ok(DeferredDirectoryEntryStreamMode::Live)) => {
                            let dir = dir.take().expect(
                                "live filesystem directory available for incomplete replay",
                            );
                            let result_tx = result_tx
                                .take()
                                .expect("filesystem directory result sender available for incomplete replay");
                            self.state = if dir.perms.contains(DirPerms::READ) {
                                DeferredDirectoryEntryStreamProducerState::Live(
                                    RecordingDirectoryEntryStreamProducer::new(dir, result_tx),
                                )
                            } else {
                                DeferredDirectoryEntryStreamProducerState::Streaming(
                                    DirectoryEntryStreamProducer::new(
                                        Vec::new(),
                                        Err(types::ErrorCode::NotPermitted),
                                        result_tx,
                                    ),
                                )
                            };
                        }
                        Poll::Ready(Ok(DeferredDirectoryEntryStreamMode::Error(error))) => {
                            self.state = DeferredDirectoryEntryStreamProducerState::Done;
                            return Poll::Ready(Err(wasmtime::Error::msg(error)));
                        }
                        Poll::Ready(Err(_)) => {
                            self.state = DeferredDirectoryEntryStreamProducerState::Done;
                            return Poll::Ready(Err(wasmtime::Error::msg(
                                "filesystem directory replay task dropped",
                            )));
                        }
                    }
                }
                DeferredDirectoryEntryStreamProducerState::Streaming(producer) => {
                    return Pin::new(producer).poll_produce(cx, store, dst, finish);
                }
                DeferredDirectoryEntryStreamProducerState::Live(producer) => {
                    return Pin::new(producer).poll_produce(cx, store, dst, finish);
                }
                DeferredDirectoryEntryStreamProducerState::Done => {
                    return Poll::Ready(Ok(StreamResult::Dropped));
                }
            }
        }
    }
}

impl Drop for DeferredDirectoryEntryStreamProducer {
    fn drop(&mut self) {
        if let DeferredDirectoryEntryStreamProducerState::Awaiting { result_tx, .. } =
            &mut self.state
            && let Some(result_tx) = result_tx.take()
        {
            let _ = result_tx.send(CapturedDirectoryEntryStream {
                entries: Vec::new(),
                result: Ok(()),
            });
        }
    }
}

#[derive(Clone, Copy)]
enum FilesystemWriteMode {
    At(types::Filesize),
    Append,
}

struct FilesystemWriteTask<Ctx, Pair>
where
    Pair: HostPayloadPair,
{
    file: File,
    mode: FilesystemWriteMode,
    call: CallHandle<Pair, Cancellable>,
    chunks_rx: tokio::sync::mpsc::UnboundedReceiver<FilesystemWriteChunk>,
    result_tx: tokio::sync::oneshot::Sender<wasmtime::Result<Result<(), types::ErrorCode>>>,
    _phantom: PhantomData<fn() -> (Ctx, Pair)>,
}

struct FilesystemByteReadTask<Ctx> {
    call: CallHandle<P3FilesystemTypesDescriptorReadViaStream, Cancellable>,
    stream_rx: tokio::sync::oneshot::Receiver<CapturedByteStream>,
    result_tx: tokio::sync::oneshot::Sender<wasmtime::Result<Result<(), types::ErrorCode>>>,
    _phantom: PhantomData<fn() -> Ctx>,
}

struct FilesystemDirectoryReadTask<Ctx> {
    call: CallHandle<P3FilesystemTypesDescriptorReadDirectory, Cancellable>,
    stream_rx: tokio::sync::oneshot::Receiver<CapturedDirectoryEntryStream>,
    result_tx: tokio::sync::oneshot::Sender<wasmtime::Result<Result<(), types::ErrorCode>>>,
    _phantom: PhantomData<fn() -> Ctx>,
}

impl<Ctx, Pair> FilesystemWriteTask<Ctx, Pair>
where
    Pair: HostPayloadPair,
{
    fn new(
        file: File,
        mode: FilesystemWriteMode,
        call: CallHandle<Pair, Cancellable>,
        chunks_rx: tokio::sync::mpsc::UnboundedReceiver<FilesystemWriteChunk>,
        result_tx: tokio::sync::oneshot::Sender<wasmtime::Result<Result<(), types::ErrorCode>>>,
    ) -> Self {
        Self {
            file,
            mode,
            call,
            chunks_rx,
            result_tx,
            _phantom: PhantomData,
        }
    }
}

impl<Ctx> FilesystemByteReadTask<Ctx> {
    fn new(
        call: CallHandle<P3FilesystemTypesDescriptorReadViaStream, Cancellable>,
        stream_rx: tokio::sync::oneshot::Receiver<CapturedByteStream>,
        result_tx: tokio::sync::oneshot::Sender<wasmtime::Result<Result<(), types::ErrorCode>>>,
    ) -> Self {
        Self {
            call,
            stream_rx,
            result_tx,
            _phantom: PhantomData,
        }
    }
}

impl<Ctx> FilesystemDirectoryReadTask<Ctx> {
    fn new(
        call: CallHandle<P3FilesystemTypesDescriptorReadDirectory, Cancellable>,
        stream_rx: tokio::sync::oneshot::Receiver<CapturedDirectoryEntryStream>,
        result_tx: tokio::sync::oneshot::Sender<wasmtime::Result<Result<(), types::ErrorCode>>>,
    ) -> Self {
        Self {
            call,
            stream_rx,
            result_tx,
            _phantom: PhantomData,
        }
    }
}

impl<Ctx, U> AccessorTask<U, DurableP3<Ctx>> for FilesystemByteReadTask<Ctx>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    async fn run(self, accessor: &Accessor<U, DurableP3<Ctx>>) -> wasmtime::Result<()> {
        let result = complete_filesystem_read::<Ctx, U>(
            accessor,
            self.call,
            self.stream_rx,
            &self.result_tx,
        )
        .await;
        if !self.result_tx.is_closed() {
            let _ = self.result_tx.send(result);
        }
        Ok(())
    }
}

impl<Ctx, U> AccessorTask<U, DurableP3<Ctx>> for FilesystemDirectoryReadTask<Ctx>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    async fn run(self, accessor: &Accessor<U, DurableP3<Ctx>>) -> wasmtime::Result<()> {
        let result = complete_filesystem_directory_read::<Ctx, U>(
            accessor,
            self.call,
            self.stream_rx,
            &self.result_tx,
        )
        .await;
        if !self.result_tx.is_closed() {
            let _ = self.result_tx.send(result);
        }
        Ok(())
    }
}

impl<Ctx, Pair, U> AccessorTask<U, DurableP3<Ctx>> for FilesystemWriteTask<Ctx, Pair>
where
    Ctx: WorkerCtx,
    Pair: HostPayloadPair<Resp = HostResponseP3FileSystemByteStream> + Send + 'static,
    Pair::Req: Send + 'static,
    U: 'static,
{
    async fn run(self, accessor: &Accessor<U, DurableP3<Ctx>>) -> wasmtime::Result<()> {
        let FilesystemWriteTask {
            file,
            mode,
            call,
            chunks_rx,
            mut result_tx,
            _phantom,
        } = self;

        match complete_filesystem_write::<Ctx, U, Pair>(
            accessor,
            file,
            mode,
            call,
            chunks_rx,
            &mut result_tx,
        )
        .await
        {
            Ok(result) => {
                if !result_tx.is_closed() {
                    let _ = result_tx.send(Ok(result));
                }
                Ok(())
            }
            Err(error) => {
                if !result_tx.is_closed() {
                    let _ = result_tx.send(Err(error));
                }
                Ok(())
            }
        }
    }
}

struct FilesystemByteReadReplayTask<Ctx> {
    call: CallHandle<P3FilesystemTypesDescriptorReadViaStream, Cancellable>,
    stream_mode_tx: tokio::sync::oneshot::Sender<DeferredByteStreamMode>,
    stream_rx: tokio::sync::oneshot::Receiver<CapturedByteStream>,
    result_tx: tokio::sync::oneshot::Sender<wasmtime::Result<Result<(), types::ErrorCode>>>,
    _phantom: PhantomData<fn() -> Ctx>,
}

impl<Ctx> FilesystemByteReadReplayTask<Ctx> {
    fn new(
        call: CallHandle<P3FilesystemTypesDescriptorReadViaStream, Cancellable>,
        stream_mode_tx: tokio::sync::oneshot::Sender<DeferredByteStreamMode>,
        stream_rx: tokio::sync::oneshot::Receiver<CapturedByteStream>,
        result_tx: tokio::sync::oneshot::Sender<wasmtime::Result<Result<(), types::ErrorCode>>>,
    ) -> Self {
        Self {
            call,
            stream_mode_tx,
            stream_rx,
            result_tx,
            _phantom: PhantomData,
        }
    }
}

impl<Ctx, U> AccessorTask<U, DurableP3<Ctx>> for FilesystemByteReadReplayTask<Ctx>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    async fn run(self, accessor: &Accessor<U, DurableP3<Ctx>>) -> wasmtime::Result<()> {
        match self
            .call
            .replay_access(accessor, durable_worker_ctx::<Ctx, U>)
            .await
            .map_err(wasmtime::Error::from)
        {
            Ok(CallReplayOutcome::Replayed(response)) => {
                let captured = CapturedByteStream {
                    contents: response.contents,
                    result: deserialize_stream_result(response.result),
                };
                let _ = self
                    .stream_mode_tx
                    .send(DeferredByteStreamMode::Stream(captured));
                let result = match self.stream_rx.await {
                    Ok(result) => Ok(result.result),
                    Err(_) => Err(wasmtime::Error::msg("filesystem replay stream dropped")),
                };
                let _ = self.result_tx.send(result);
            }
            Ok(CallReplayOutcome::Incomplete(call)) => {
                let _ = self.stream_mode_tx.send(DeferredByteStreamMode::Live);
                let result = complete_filesystem_read::<Ctx, U>(
                    accessor,
                    call,
                    self.stream_rx,
                    &self.result_tx,
                )
                .await;
                if !self.result_tx.is_closed() {
                    let _ = self.result_tx.send(result);
                }
            }
            Err(error) => {
                let error = error.to_string();
                let _ = self
                    .stream_mode_tx
                    .send(DeferredByteStreamMode::Error(error.clone()));
                let _ = self.result_tx.send(Err(wasmtime::Error::msg(error)));
            }
        }
        Ok(())
    }
}

struct FilesystemDirectoryReadReplayTask<Ctx> {
    call: CallHandle<P3FilesystemTypesDescriptorReadDirectory, Cancellable>,
    stream_mode_tx: tokio::sync::oneshot::Sender<DeferredDirectoryEntryStreamMode>,
    stream_rx: tokio::sync::oneshot::Receiver<CapturedDirectoryEntryStream>,
    result_tx: tokio::sync::oneshot::Sender<wasmtime::Result<Result<(), types::ErrorCode>>>,
    _phantom: PhantomData<fn() -> Ctx>,
}

impl<Ctx> FilesystemDirectoryReadReplayTask<Ctx> {
    fn new(
        call: CallHandle<P3FilesystemTypesDescriptorReadDirectory, Cancellable>,
        stream_mode_tx: tokio::sync::oneshot::Sender<DeferredDirectoryEntryStreamMode>,
        stream_rx: tokio::sync::oneshot::Receiver<CapturedDirectoryEntryStream>,
        result_tx: tokio::sync::oneshot::Sender<wasmtime::Result<Result<(), types::ErrorCode>>>,
    ) -> Self {
        Self {
            call,
            stream_mode_tx,
            stream_rx,
            result_tx,
            _phantom: PhantomData,
        }
    }
}

impl<Ctx, U> AccessorTask<U, DurableP3<Ctx>> for FilesystemDirectoryReadReplayTask<Ctx>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    async fn run(self, accessor: &Accessor<U, DurableP3<Ctx>>) -> wasmtime::Result<()> {
        match self
            .call
            .replay_access(accessor, durable_worker_ctx::<Ctx, U>)
            .await
            .map_err(wasmtime::Error::from)
        {
            Ok(CallReplayOutcome::Replayed(response)) => {
                let captured = CapturedDirectoryEntryStream {
                    entries: response.entries.into_iter().map(Into::into).collect(),
                    result: deserialize_stream_result(response.result),
                };
                let _ = self
                    .stream_mode_tx
                    .send(DeferredDirectoryEntryStreamMode::Stream(captured));
                let result = match self.stream_rx.await {
                    Ok(result) => Ok(result.result),
                    Err(_) => Err(wasmtime::Error::msg(
                        "filesystem directory replay stream dropped",
                    )),
                };
                let _ = self.result_tx.send(result);
            }
            Ok(CallReplayOutcome::Incomplete(call)) => {
                let _ = self
                    .stream_mode_tx
                    .send(DeferredDirectoryEntryStreamMode::Live);
                let result = complete_filesystem_directory_read::<Ctx, U>(
                    accessor,
                    call,
                    self.stream_rx,
                    &self.result_tx,
                )
                .await;
                if !self.result_tx.is_closed() {
                    let _ = self.result_tx.send(result);
                }
            }
            Err(error) => {
                let error = error.to_string();
                let _ = self
                    .stream_mode_tx
                    .send(DeferredDirectoryEntryStreamMode::Error(error.clone()));
                let _ = self.result_tx.send(Err(wasmtime::Error::msg(error)));
            }
        }
        Ok(())
    }
}

fn descriptor_path_from_access<Ctx: WorkerCtx, U>(
    store: &mut Access<'_, U, DurableP3<Ctx>>,
    fd: &Resource<Descriptor>,
) -> wasmtime::Result<PathBuf>
where
    U: 'static,
{
    let mut filesystem =
        Access::<U, WasiFilesystem>::new(store.as_context_mut(), wasi_filesystem_view::<Ctx, U>);
    let descriptor = filesystem.get().table.get(fd)?;
    Ok(match descriptor {
        Descriptor::File(file) => file.path.clone(),
        Descriptor::Dir(dir) => dir.path.clone(),
    })
}

fn descriptor_path_from_accessor<Ctx: WorkerCtx, U>(
    store: &Accessor<U, DurableP3<Ctx>>,
    fd: &Resource<Descriptor>,
) -> wasmtime::Result<PathBuf>
where
    U: 'static,
{
    store.with(|mut access| {
        let mut filesystem = Access::<U, WasiFilesystem>::new(
            access.as_context_mut(),
            wasi_filesystem_view::<Ctx, U>,
        );
        let descriptor = filesystem.get().table.get(fd)?;
        Ok(match descriptor {
            Descriptor::File(file) => file.path.clone(),
            Descriptor::Dir(dir) => dir.path.clone(),
        })
    })
}

fn descriptor_path_at_from_accessor<Ctx: WorkerCtx, U>(
    store: &Accessor<U, DurableP3<Ctx>>,
    fd: &Resource<Descriptor>,
    path: &str,
) -> wasmtime::Result<PathBuf>
where
    U: 'static,
{
    store.with(|mut access| {
        let mut filesystem = Access::<U, WasiFilesystem>::new(
            access.as_context_mut(),
            wasi_filesystem_view::<Ctx, U>,
        );
        let descriptor = filesystem.get().table.get(fd)?;
        Ok(match descriptor {
            Descriptor::File(file) => file.path.join(path),
            Descriptor::Dir(dir) => dir.path.join(path),
        })
    })
}

fn file_from_access<Ctx: WorkerCtx, U>(
    store: &mut Access<'_, U, DurableP3<Ctx>>,
    fd: &Resource<Descriptor>,
) -> wasmtime::Result<File>
where
    U: 'static,
{
    let mut filesystem =
        Access::<U, WasiFilesystem>::new(store.as_context_mut(), wasi_filesystem_view::<Ctx, U>);
    match filesystem.get().table.get(fd)? {
        Descriptor::File(file) => Ok(file.clone()),
        Descriptor::Dir(_) => Err(FilesystemError::from(types::ErrorCode::BadDescriptor).into()),
    }
}

fn dir_result_from_access<Ctx: WorkerCtx, U>(
    store: &mut Access<'_, U, DurableP3<Ctx>>,
    fd: &Resource<Descriptor>,
) -> wasmtime::Result<Result<Dir, types::ErrorCode>>
where
    U: 'static,
{
    let mut filesystem =
        Access::<U, WasiFilesystem>::new(store.as_context_mut(), wasi_filesystem_view::<Ctx, U>);
    Ok(match filesystem.get().table.get(fd)? {
        Descriptor::Dir(dir) => Ok(dir.clone()),
        Descriptor::File(_) => Err(types::ErrorCode::NotDirectory),
    })
}

fn write_validation_error_from_access<Ctx: WorkerCtx, U>(
    store: &mut Access<'_, U, DurableP3<Ctx>>,
    fd: &Resource<Descriptor>,
) -> wasmtime::Result<Option<types::ErrorCode>>
where
    U: 'static,
{
    if durable_worker_ctx::<Ctx, U>(store.data_mut()).check_if_file_is_readonly(fd)? {
        return Ok(Some(types::ErrorCode::NotPermitted));
    }

    let mut filesystem =
        Access::<U, WasiFilesystem>::new(store.as_context_mut(), wasi_filesystem_view::<Ctx, U>);
    Ok(match filesystem.get().table.get(fd)? {
        Descriptor::File(file) if !file.perms.contains(FilePerms::WRITE) => {
            Some(types::ErrorCode::NotPermitted)
        }
        Descriptor::File(_) => None,
        Descriptor::Dir(_) => Some(types::ErrorCode::BadDescriptor),
    })
}

fn map_directory_entry(
    entry: std::io::Result<cap_std::fs::DirEntry>,
) -> Result<Option<types::DirectoryEntry>, types::ErrorCode> {
    match entry {
        Ok(entry) => {
            let meta = entry.metadata()?;
            let Ok(name) = entry.file_name().into_string() else {
                return Err(types::ErrorCode::IllegalByteSequence);
            };
            Ok(Some(types::DirectoryEntry {
                type_: meta.file_type().into(),
                name,
            }))
        }
        Err(error) => Err(error.into()),
    }
}

fn serialize_stream_result(
    result: Result<(), types::ErrorCode>,
) -> Result<(), SerializableP3FsErrorCode> {
    result.map_err(Into::into)
}

fn deserialize_stream_result(
    result: Result<(), SerializableP3FsErrorCode>,
) -> Result<(), types::ErrorCode> {
    result.map_err(Into::into)
}

fn serialize_stat_error(error: &FilesystemError) -> SerializableP3FileSystemError {
    SerializableP3FileSystemError::from_result(
        error
            .downcast_ref()
            .cloned()
            .ok_or_else(|| error.to_string()),
    )
}

fn deserialize_stat_error(error: SerializableP3FileSystemError) -> FilesystemError {
    match error {
        SerializableP3FileSystemError::ErrorCode(error_code) => {
            types::ErrorCode::from(error_code).into()
        }
        SerializableP3FileSystemError::Generic(error) => {
            FilesystemError::trap(wasmtime::Error::msg(error))
        }
    }
}

fn serialize_stat_result(
    stat: &Result<types::DescriptorStat, SerializableP3FileSystemError>,
) -> Result<SerializableFileTimes, SerializableP3FileSystemError> {
    stat.clone().map(|stat| SerializableFileTimes {
        data_access_timestamp: stat.data_access_timestamp.map(Into::into),
        data_modification_timestamp: stat.data_modification_timestamp.map(Into::into),
    })
}

async fn run_local_stat<Ctx, U>(
    store: &Accessor<U, DurableP3<Ctx>>,
    fd: Resource<Descriptor>,
) -> Result<types::DescriptorStat, SerializableP3FileSystemError>
where
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    let filesystem = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
    match <WasiFilesystem as types::HostDescriptorWithStore>::stat(&filesystem, fd).await {
        Ok(mut stat) => {
            stat.status_change_timestamp = None;
            Ok(stat)
        }
        Err(error) => Err(serialize_stat_error(&error)),
    }
}

async fn run_local_stat_at<Ctx, U>(
    store: &Accessor<U, DurableP3<Ctx>>,
    fd: Resource<Descriptor>,
    path_flags: types::PathFlags,
    path: String,
) -> Result<types::DescriptorStat, SerializableP3FileSystemError>
where
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    let filesystem = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
    match <WasiFilesystem as types::HostDescriptorWithStore>::stat_at(
        &filesystem,
        fd,
        path_flags,
        path,
    )
    .await
    {
        Ok(mut stat) => {
            stat.status_change_timestamp = None;
            Ok(stat)
        }
        Err(error) => Err(serialize_stat_error(&error)),
    }
}

async fn apply_stat_response(
    stat: Result<types::DescriptorStat, SerializableP3FileSystemError>,
    response: HostResponseP3FileSystemStat,
) -> FilesystemResult<types::DescriptorStat> {
    match response.result {
        Ok(times) => {
            let mut stat = stat.unwrap();
            stat.data_access_timestamp = times.data_access_timestamp.map(Into::into);
            stat.data_modification_timestamp = times.data_modification_timestamp.map(Into::into);
            Ok(stat)
        }
        Err(error) => Err(deserialize_stat_error(error)),
    }
}

async fn complete_directory_response<Ctx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    call: CallHandle<P3FilesystemTypesDescriptorReadDirectory, Cancellable>,
    entries: Vec<types::DirectoryEntry>,
    result: Result<(), types::ErrorCode>,
) -> wasmtime::Result<HostResponseP3FileSystemDirectoryEntryStream>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    let response = HostResponseP3FileSystemDirectoryEntryStream {
        entries: entries
            .into_iter()
            .map(SerializableP3DirectoryEntry::from)
            .collect(),
        result: serialize_stream_result(result),
    };
    call.complete_access(accessor, durable_worker_ctx::<Ctx, U>, response)
        .await
        .map_err(wasmtime::Error::from)
}

async fn complete_filesystem_read<Ctx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    call: CallHandle<P3FilesystemTypesDescriptorReadViaStream, Cancellable>,
    stream_rx: tokio::sync::oneshot::Receiver<CapturedByteStream>,
    result_tx: &tokio::sync::oneshot::Sender<wasmtime::Result<Result<(), types::ErrorCode>>>,
) -> wasmtime::Result<Result<(), types::ErrorCode>>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    let captured = stream_rx.await.unwrap_or_else(|_| CapturedByteStream {
        contents: Vec::new(),
        result: Err(types::ErrorCode::Io),
    });
    let response = HostResponseP3FileSystemByteStream {
        contents: captured.contents,
        result: serialize_stream_result(captured.result),
    };

    if result_tx.is_closed() {
        let result = deserialize_stream_result(response.result.clone());
        call.cancel_access(accessor, durable_worker_ctx::<Ctx, U>, Some(response))
            .await
            .map_err(wasmtime::Error::from)?;
        return Ok(result);
    }

    let response = call
        .complete_access(accessor, durable_worker_ctx::<Ctx, U>, response)
        .await
        .map_err(wasmtime::Error::from)?;

    Ok(deserialize_stream_result(response.result))
}

async fn complete_filesystem_directory_read<Ctx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    call: CallHandle<P3FilesystemTypesDescriptorReadDirectory, Cancellable>,
    stream_rx: tokio::sync::oneshot::Receiver<CapturedDirectoryEntryStream>,
    result_tx: &tokio::sync::oneshot::Sender<wasmtime::Result<Result<(), types::ErrorCode>>>,
) -> wasmtime::Result<Result<(), types::ErrorCode>>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    let captured = stream_rx
        .await
        .unwrap_or_else(|_| CapturedDirectoryEntryStream {
            entries: Vec::new(),
            result: Err(types::ErrorCode::Io),
        });
    let response = HostResponseP3FileSystemDirectoryEntryStream {
        entries: captured
            .entries
            .into_iter()
            .map(SerializableP3DirectoryEntry::from)
            .collect(),
        result: serialize_stream_result(captured.result),
    };

    if result_tx.is_closed() {
        let result = deserialize_stream_result(response.result.clone());
        call.cancel_access(accessor, durable_worker_ctx::<Ctx, U>, Some(response))
            .await
            .map_err(wasmtime::Error::from)?;
        return Ok(result);
    }

    let response = call
        .complete_access(accessor, durable_worker_ctx::<Ctx, U>, response)
        .await
        .map_err(wasmtime::Error::from)?;

    Ok(deserialize_stream_result(response.result))
}

async fn complete_immediate_write_response<Ctx, U, Pair>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    call: CallHandle<Pair, Cancellable>,
    result: Result<(), types::ErrorCode>,
) -> wasmtime::Result<Result<(), types::ErrorCode>>
where
    Ctx: WorkerCtx,
    Pair: HostPayloadPair<Resp = HostResponseP3FileSystemByteStream>,
    U: 'static,
{
    if !call.is_live() {
        return match call
            .replay_access(accessor, durable_worker_ctx::<Ctx, U>)
            .await
            .map_err(wasmtime::Error::from)?
        {
            CallReplayOutcome::Replayed(response) => Ok(deserialize_stream_result(response.result)),
            CallReplayOutcome::Incomplete(call) => {
                let response = call
                    .complete_access(
                        accessor,
                        durable_worker_ctx::<Ctx, U>,
                        HostResponseP3FileSystemByteStream {
                            contents: Vec::new(),
                            result: serialize_stream_result(result),
                        },
                    )
                    .await
                    .map_err(wasmtime::Error::from)?;
                Ok(deserialize_stream_result(response.result))
            }
        };
    }

    let response = call
        .complete_access(
            accessor,
            durable_worker_ctx::<Ctx, U>,
            HostResponseP3FileSystemByteStream {
                contents: Vec::new(),
                result: serialize_stream_result(result),
            },
        )
        .await
        .map_err(wasmtime::Error::from)?;
    Ok(deserialize_stream_result(response.result))
}

async fn complete_immediate_byte_response<Ctx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    call: CallHandle<P3FilesystemTypesDescriptorReadViaStream, Cancellable>,
    contents: Vec<u8>,
    result: Result<(), types::ErrorCode>,
) -> wasmtime::Result<HostResponseP3FileSystemByteStream>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    let response = HostResponseP3FileSystemByteStream {
        contents,
        result: serialize_stream_result(result),
    };
    call.complete_access(accessor, durable_worker_ctx::<Ctx, U>, response)
        .await
        .map_err(wasmtime::Error::from)
}

async fn complete_immediate_directory_response<Ctx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    call: CallHandle<P3FilesystemTypesDescriptorReadDirectory, Cancellable>,
    result: Result<(), types::ErrorCode>,
) -> wasmtime::Result<HostResponseP3FileSystemDirectoryEntryStream>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    if !call.is_live() {
        return match call
            .replay_access(accessor, durable_worker_ctx::<Ctx, U>)
            .await
            .map_err(wasmtime::Error::from)?
        {
            CallReplayOutcome::Replayed(response) => Ok(response),
            CallReplayOutcome::Incomplete(call) => {
                complete_directory_response::<Ctx, U>(accessor, call, Vec::new(), result).await
            }
        };
    }

    complete_directory_response::<Ctx, U>(accessor, call, Vec::new(), result).await
}

async fn start_call<Ctx, U, Pair>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    request: Pair::Req,
) -> wasmtime::Result<CallHandle<Pair, Cancellable>>
where
    Ctx: WorkerCtx,
    Pair: HostPayloadPair,
    U: Send + 'static,
{
    CallHandle::<Pair, Cancellable>::start_access(
        accessor,
        durable_worker_ctx::<Ctx, U>,
        request,
        DurableFunctionType::ReadLocal,
    )
    .await
    .map_err(wasmtime::Error::from)
}

async fn start_cancellable_call<Ctx, U, Pair>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    request: Pair::Req,
) -> wasmtime::Result<CallHandle<Pair, Cancellable>>
where
    Ctx: WorkerCtx,
    Pair: HostPayloadPair,
    U: Send + 'static,
{
    CallHandle::<Pair, Cancellable>::start_access(
        accessor,
        durable_worker_ctx::<Ctx, U>,
        request,
        DurableFunctionType::ReadLocal,
    )
    .await
    .map_err(wasmtime::Error::from)
}

async fn wait_filesystem_task_result(
    result_rx: tokio::sync::oneshot::Receiver<wasmtime::Result<Result<(), types::ErrorCode>>>,
) -> wasmtime::Result<Result<(), types::ErrorCode>> {
    result_rx
        .await
        .unwrap_or_else(|_| Err(wasmtime::Error::msg("filesystem stream task dropped")))
}

#[derive(Clone, Copy)]
struct FilesystemStorageReservation {
    base_size: Option<u64>,
    reserved_growth: u64,
}

async fn filesystem_file_size(file: &File) -> Option<u64> {
    let file = Arc::clone(&file.file);
    spawn_blocking(move || file.metadata().map(|metadata| metadata.len()).ok()).await
}

async fn reserve_filesystem_write_storage<Ctx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    file: &File,
    mode: FilesystemWriteMode,
    write_len: u64,
) -> wasmtime::Result<FilesystemStorageReservation>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    let base_size = filesystem_file_size(file).await;
    let reserved_growth = match (base_size, mode) {
        (Some(current_size), FilesystemWriteMode::At(offset)) => offset
            .saturating_add(write_len)
            .saturating_sub(current_size),
        (Some(current_size), FilesystemWriteMode::Append) => current_size
            .saturating_add(write_len)
            .saturating_sub(current_size),
        (None, _) => write_len,
    };

    if reserved_growth > 0
        && let Some(worker) = accessor
            .with(|mut access| {
                durable_worker_ctx::<Ctx, U>(access.data_mut())
                    .prepare_filesystem_storage_reservation(reserved_growth)
            })
            .map_err(wasmtime::Error::from_anyhow)?
    {
        if let Err(error) = worker
            .acquire_filesystem_storage_space(reserved_growth)
            .await
        {
            accessor.with(|mut access| {
                durable_worker_ctx::<Ctx, U>(access.data_mut())
                    .rollback_filesystem_storage_reservation(reserved_growth);
            });
            return Err(wasmtime::Error::from_anyhow(error));
        }
        worker
            .add_to_oplog(OplogEntry::filesystem_storage_usage_update(
                reserved_growth as i64,
            ))
            .await;
        accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut())
                .finish_filesystem_storage_reservation(reserved_growth);
        });
    }

    Ok(FilesystemStorageReservation {
        base_size,
        reserved_growth,
    })
}

async fn release_filesystem_write_storage<Ctx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    bytes: u64,
) -> wasmtime::Result<()>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    if bytes == 0 {
        return Ok(());
    }

    if let Some((worker, bytes)) = accessor.with(|mut access| {
        durable_worker_ctx::<Ctx, U>(access.data_mut()).prepare_filesystem_storage_release(bytes)
    }) {
        worker
            .add_to_oplog(OplogEntry::filesystem_storage_usage_update(-(bytes as i64)))
            .await;
        worker.release_filesystem_storage_space(bytes).await;
        accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut())
                .finish_filesystem_storage_release(bytes);
        });
    }

    Ok(())
}

async fn reconcile_filesystem_write_storage<Ctx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    file: &File,
    reservation: FilesystemStorageReservation,
    write_result: &Result<(), types::ErrorCode>,
) -> wasmtime::Result<()>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    if reservation.reserved_growth == 0 {
        return Ok(());
    }

    if write_result.is_err() {
        return release_filesystem_write_storage::<Ctx, U>(accessor, reservation.reserved_growth)
            .await;
    }

    let Some(base_size) = reservation.base_size else {
        return Ok(());
    };
    let Some(actual_end) = filesystem_file_size(file).await else {
        return Ok(());
    };

    let actual_growth = actual_end.saturating_sub(base_size);
    let over_reserved = reservation.reserved_growth.saturating_sub(actual_growth);
    release_filesystem_write_storage::<Ctx, U>(accessor, over_reserved).await
}

fn no_side_effect_write_response() -> HostResponseP3FileSystemByteStream {
    HostResponseP3FileSystemByteStream {
        contents: Vec::new(),
        result: serialize_stream_result(Err(types::ErrorCode::Interrupted)),
    }
}

async fn complete_filesystem_write<Ctx, U, Pair>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    file: File,
    mode: FilesystemWriteMode,
    mut call: CallHandle<Pair, Cancellable>,
    mut chunks_rx: tokio::sync::mpsc::UnboundedReceiver<FilesystemWriteChunk>,
    result_tx: &mut tokio::sync::oneshot::Sender<wasmtime::Result<Result<(), types::ErrorCode>>>,
) -> wasmtime::Result<Result<(), types::ErrorCode>>
where
    Ctx: WorkerCtx,
    Pair: HostPayloadPair<Resp = HostResponseP3FileSystemByteStream> + Send + 'static,
    Pair::Req: Send + 'static,
    U: 'static,
{
    if !call.is_live() {
        let captured = capture_replayed_write_input(&mut chunks_rx, result_tx).await?;

        match call
            .replay_access(accessor, durable_worker_ctx::<Ctx, U>)
            .await
            .map_err(wasmtime::Error::from)?
        {
            CallReplayOutcome::Replayed(response) => {
                return apply_replayed_filesystem_write_response(file, mode, response).await;
            }
            CallReplayOutcome::Incomplete(mut live_call) => {
                if matches!(mode, FilesystemWriteMode::Append) {
                    return Err(wasmtime::Error::from_anyhow(live_call.trap(
                        wasmtime::Error::msg(
                            "incomplete append-via-stream cannot be replayed safely",
                        ),
                    )));
                }
                call = live_call;
            }
        }

        let response = HostResponseP3FileSystemByteStream {
            contents: captured.contents,
            result: serialize_stream_result(captured.result),
        };

        return complete_buffered_filesystem_write(accessor, file, mode, call, response, result_tx)
            .await;
    }

    let captured =
        run_streaming_filesystem_write(accessor, &file, mode, &mut chunks_rx, result_tx).await?;
    let response = HostResponseP3FileSystemByteStream {
        contents: captured.contents,
        result: serialize_stream_result(captured.result),
    };

    if result_tx.is_closed() {
        call.cancel_access(
            accessor,
            durable_worker_ctx::<Ctx, U>,
            Some(response.clone()),
        )
        .await
        .map_err(wasmtime::Error::from)?;
        return Ok(deserialize_stream_result(response.result));
    }

    let response = call
        .complete_access(accessor, durable_worker_ctx::<Ctx, U>, response)
        .await
        .map_err(wasmtime::Error::from)?;

    Ok(deserialize_stream_result(response.result))
}

async fn capture_replayed_write_input(
    chunks_rx: &mut tokio::sync::mpsc::UnboundedReceiver<FilesystemWriteChunk>,
    result_tx: &mut tokio::sync::oneshot::Sender<wasmtime::Result<Result<(), types::ErrorCode>>>,
) -> wasmtime::Result<CapturedByteStream> {
    let mut contents = Vec::new();
    loop {
        let chunk = tokio::select! {
            chunk = chunks_rx.recv() => chunk,
            _ = result_tx.closed() => {
                return Ok(CapturedByteStream {
                    contents,
                    result: Err(types::ErrorCode::Interrupted),
                });
            },
        };
        let Some(chunk) = chunk else {
            return Ok(CapturedByteStream {
                contents,
                result: Ok(()),
            });
        };
        contents.extend_from_slice(&chunk.contents);
        let _ = chunk.result_tx.send(Ok(()));
    }
}

async fn complete_buffered_filesystem_write<Ctx, U, Pair>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    file: File,
    mode: FilesystemWriteMode,
    mut call: CallHandle<Pair, Cancellable>,
    response: HostResponseP3FileSystemByteStream,
    result_tx: &mut tokio::sync::oneshot::Sender<wasmtime::Result<Result<(), types::ErrorCode>>>,
) -> wasmtime::Result<Result<(), types::ErrorCode>>
where
    Ctx: WorkerCtx,
    Pair: HostPayloadPair<Resp = HostResponseP3FileSystemByteStream> + Send + 'static,
    Pair::Req: Send + 'static,
    U: 'static,
{
    if result_tx.is_closed() {
        call.cancel_access(
            accessor,
            durable_worker_ctx::<Ctx, U>,
            Some(no_side_effect_write_response()),
        )
        .await
        .map_err(wasmtime::Error::from)?;
        return Ok(Err(types::ErrorCode::Interrupted));
    }

    let reservation = match reserve_filesystem_write_storage::<Ctx, U>(
        accessor,
        &file,
        mode,
        response.contents.len() as u64,
    )
    .await
    {
        Ok(reservation) => reservation,
        Err(error) => {
            return Err(wasmtime::Error::from_anyhow(call.trap(error)));
        }
    };

    if result_tx.is_closed() {
        release_filesystem_write_storage::<Ctx, U>(accessor, reservation.reserved_growth).await?;
        call.cancel_access(
            accessor,
            durable_worker_ctx::<Ctx, U>,
            Some(no_side_effect_write_response()),
        )
        .await
        .map_err(wasmtime::Error::from)?;
        return Ok(Err(types::ErrorCode::Interrupted));
    }

    let captured = run_live_filesystem_write_captured(file.clone(), mode, response.contents).await;
    if let Err(error) =
        reconcile_filesystem_write_storage::<Ctx, U>(accessor, &file, reservation, &captured.result)
            .await
    {
        return Err(wasmtime::Error::from_anyhow(call.trap(error)));
    }
    let response = HostResponseP3FileSystemByteStream {
        contents: captured.contents,
        result: serialize_stream_result(captured.result.clone()),
    };
    if result_tx.is_closed() {
        call.cancel_access(
            accessor,
            durable_worker_ctx::<Ctx, U>,
            Some(response.clone()),
        )
        .await
        .map_err(wasmtime::Error::from)?;
        return Ok(deserialize_stream_result(response.result));
    }
    let response = call
        .complete_access(accessor, durable_worker_ctx::<Ctx, U>, response)
        .await
        .map_err(wasmtime::Error::from)?;

    Ok(deserialize_stream_result(response.result))
}

async fn run_streaming_filesystem_write<Ctx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    file: &File,
    mode: FilesystemWriteMode,
    chunks_rx: &mut tokio::sync::mpsc::UnboundedReceiver<FilesystemWriteChunk>,
    result_tx: &mut tokio::sync::oneshot::Sender<wasmtime::Result<Result<(), types::ErrorCode>>>,
) -> wasmtime::Result<CapturedByteStream>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    let mut contents = Vec::new();
    let mut result = Ok(());
    let mut position = match mode {
        FilesystemWriteMode::At(offset) => Some(offset),
        FilesystemWriteMode::Append => None,
    };

    loop {
        let chunk = tokio::select! {
            chunk = chunks_rx.recv() => chunk,
            _ = result_tx.closed() => {
                result = Err(types::ErrorCode::Interrupted);
                break;
            },
        };
        let Some(chunk) = chunk else {
            break;
        };

        if result.is_ok() {
            let chunk_mode = match position {
                Some(offset) => FilesystemWriteMode::At(offset),
                None => FilesystemWriteMode::Append,
            };
            let write_len = chunk.contents.len() as u64;
            let reservation =
                reserve_filesystem_write_storage::<Ctx, U>(accessor, file, chunk_mode, write_len)
                    .await?;

            let captured =
                run_live_filesystem_write_captured(file.clone(), chunk_mode, chunk.contents).await;
            reconcile_filesystem_write_storage::<Ctx, U>(
                accessor,
                file,
                reservation,
                &captured.result,
            )
            .await?;
            let written_len = captured.contents.len() as u64;
            if let Some(offset) = &mut position {
                *offset = offset.saturating_add(written_len);
            }
            contents.extend_from_slice(&captured.contents);
            result = captured.result;
        }

        let _ = chunk.result_tx.send(result.clone());

        if result.is_err() || result_tx.is_closed() {
            break;
        }
    }

    if result_tx.is_closed() && result.is_ok() {
        result = Err(types::ErrorCode::Interrupted);
    }

    Ok(CapturedByteStream { contents, result })
}

async fn apply_replayed_filesystem_write_response(
    file: File,
    mode: FilesystemWriteMode,
    response: HostResponseP3FileSystemByteStream,
) -> wasmtime::Result<Result<(), types::ErrorCode>> {
    let recorded_result = deserialize_stream_result(response.result);
    if !response.contents.is_empty()
        && let Err(error) = run_live_filesystem_write(file, mode, response.contents).await
    {
        return Err(wasmtime::Error::msg(format!(
            "failed to reconstruct filesystem stream write from oplog: {error:?}"
        )));
    }
    Ok(recorded_result)
}

async fn run_live_filesystem_write_captured(
    file: File,
    mode: FilesystemWriteMode,
    contents: Vec<u8>,
) -> CapturedByteStream {
    let _append_guard = if matches!(mode, FilesystemWriteMode::Append) {
        Some(
            FILESYSTEM_APPEND_LOCK
                .get_or_init(|| tokio::sync::Mutex::new(()))
                .lock()
                .await,
        )
    } else {
        None
    };
    let file = Arc::clone(&file.file);
    let (contents, written, result) = spawn_blocking(move || match mode {
        FilesystemWriteMode::At(mut offset) => {
            let mut written = 0;
            while written < contents.len() {
                match file.write_at(&contents[written..], offset) {
                    Ok(0) => {
                        return (
                            contents,
                            written,
                            Err(std::io::Error::from(std::io::ErrorKind::WriteZero)),
                        );
                    }
                    Ok(n) => {
                        written += n;
                        let n = match u64::try_from(n) {
                            Ok(n) => n,
                            Err(_) => {
                                return (
                                    contents,
                                    written,
                                    Err(std::io::Error::from(std::io::ErrorKind::InvalidData)),
                                );
                            }
                        };
                        offset = match offset.checked_add(n) {
                            Some(offset) => offset,
                            None => {
                                return (
                                    contents,
                                    written,
                                    Err(std::io::Error::from(std::io::ErrorKind::InvalidData)),
                                );
                            }
                        };
                    }
                    Err(error) => return (contents, written, Err(error)),
                }
            }
            (contents, written, Ok(()))
        }
        FilesystemWriteMode::Append => {
            let mut file = file.as_ref();
            if let Err(error) = file.seek(SeekFrom::End(0)) {
                return (contents, 0, Err(error));
            }
            let mut written = 0;
            while written < contents.len() {
                match file.write(&contents[written..]) {
                    Ok(0) => {
                        return (
                            contents,
                            written,
                            Err(std::io::Error::from(std::io::ErrorKind::WriteZero)),
                        );
                    }
                    Ok(n) => {
                        written += n;
                    }
                    Err(error) => return (contents, written, Err(error)),
                }
            }
            (contents, written, Ok(()))
        }
    })
    .await;

    let written = written.min(contents.len());
    CapturedByteStream {
        contents: contents[..written].to_vec(),
        result: result.map_err(Into::into),
    }
}

async fn run_live_filesystem_write(
    file: File,
    mode: FilesystemWriteMode,
    contents: Vec<u8>,
) -> Result<(), types::ErrorCode> {
    let _append_guard = if matches!(mode, FilesystemWriteMode::Append) {
        Some(
            FILESYSTEM_APPEND_LOCK
                .get_or_init(|| tokio::sync::Mutex::new(()))
                .lock()
                .await,
        )
    } else {
        None
    };
    let file = Arc::clone(&file.file);
    spawn_blocking(move || match mode {
        FilesystemWriteMode::At(mut offset) => {
            let mut written = 0;
            while written < contents.len() {
                let n = file.write_at(&contents[written..], offset)?;
                if n == 0 {
                    return Err(std::io::Error::from(std::io::ErrorKind::WriteZero));
                }
                written += n;
                let n = u64::try_from(n)
                    .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidData))?;
                offset = offset
                    .checked_add(n)
                    .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidData))?;
            }
            Ok(())
        }
        FilesystemWriteMode::Append => {
            let mut file = file.as_ref();
            file.seek(SeekFrom::End(0))?;
            file.write_all(&contents)
        }
    })
    .await
    .map_err(Into::into)
}

impl<Ctx: WorkerCtx> types::Host for DurableP3View<'_, Ctx> {
    fn convert_error_code(&mut self, error: FilesystemError) -> wasmtime::Result<types::ErrorCode> {
        types::Host::convert_error_code(&mut WasiFilesystemView::filesystem(self.0), error)
    }
}

impl<Ctx: WorkerCtx> types::HostDescriptor for DurableP3View<'_, Ctx> {
    fn drop(&mut self, fd: Resource<Descriptor>) -> wasmtime::Result<()> {
        types::HostDescriptor::drop(&mut WasiFilesystemView::filesystem(self.0), fd)
    }
}

impl<Ctx: WorkerCtx> preopens::Host for DurableP3View<'_, Ctx> {
    fn get_directories(&mut self) -> wasmtime::Result<Vec<(Resource<Descriptor>, String)>> {
        preopens::Host::get_directories(&mut WasiFilesystemView::filesystem(self.0))
    }
}

impl<Ctx: WorkerCtx> types::HostDescriptorWithStore for DurableP3<Ctx> {
    async fn read_via_stream<U: Send + 'static>(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        offset: types::Filesize,
    ) -> wasmtime::Result<(StreamReader<u8>, FutureReader<Result<(), types::ErrorCode>>)> {
        let (path, file) = accessor.with(|mut store| {
            Ok::<_, wasmtime::Error>((
                descriptor_path_from_access::<Ctx, U>(&mut store, &fd)?,
                file_from_access::<Ctx, U>(&mut store, &fd)?,
            ))
        })?;
        let call = start_call::<Ctx, U, P3FilesystemTypesDescriptorReadViaStream>(
            accessor,
            HostRequestFileSystemPathAndOffset {
                path: path.to_string_lossy().to_string(),
                offset,
            },
        )
        .await?;

        let (stream_tx, stream_rx) = tokio::sync::oneshot::channel();
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        let (stream, future) = if call.is_live() {
            if !file.perms.contains(FilePerms::READ) {
                let response = complete_immediate_byte_response::<Ctx, U>(
                    accessor,
                    call,
                    Vec::new(),
                    Err(types::ErrorCode::NotPermitted),
                )
                .await?;
                let result = deserialize_stream_result(response.result);
                accessor.with(|mut store| {
                    let stream = StreamReader::new(&mut store, std::iter::empty())?;
                    let future = FutureReader::new(&mut store, async move {
                        Ok::<Result<(), types::ErrorCode>, wasmtime::Error>(result)
                    })?;
                    Ok::<_, wasmtime::Error>((stream, future))
                })?
            } else {
                accessor.with(|mut store| {
                    store.spawn(FilesystemByteReadTask::<Ctx>::new(
                        call, stream_rx, result_tx,
                    ));
                    let stream = StreamReader::new(
                        &mut store,
                        FileReadStreamProducer::new(file, offset, stream_tx),
                    )?;
                    let future =
                        FutureReader::new(&mut store, wait_filesystem_task_result(result_rx))?;
                    Ok::<_, wasmtime::Error>((stream, future))
                })?
            }
        } else {
            let (stream_mode_tx, stream_mode_rx) = tokio::sync::oneshot::channel();
            accessor.with(|mut store| {
                store.spawn(FilesystemByteReadReplayTask::<Ctx>::new(
                    call,
                    stream_mode_tx,
                    stream_rx,
                    result_tx,
                ));
                let stream = StreamReader::new(
                    &mut store,
                    DeferredByteStreamProducer::new(file, offset, stream_mode_rx, stream_tx),
                )?;
                let future = FutureReader::new(&mut store, wait_filesystem_task_result(result_rx))?;
                Ok::<_, wasmtime::Error>((stream, future))
            })?
        };
        Ok((stream, future))
    }

    async fn write_via_stream<U: Send + 'static>(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        data: StreamReader<u8>,
        offset: types::Filesize,
    ) -> wasmtime::Result<FutureReader<Result<(), types::ErrorCode>>> {
        let path =
            accessor.with(|mut store| descriptor_path_from_access::<Ctx, U>(&mut store, &fd))?;
        let request = HostRequestFileSystemPathAndOffset {
            path: path.to_string_lossy().to_string(),
            offset,
        };
        let mut call = start_cancellable_call::<Ctx, U, P3FilesystemTypesDescriptorWriteViaStream>(
            accessor, request,
        )
        .await?;

        let write_error = match accessor
            .with(|mut store| write_validation_error_from_access::<Ctx, U>(&mut store, &fd))
        {
            Ok(write_error) => write_error,
            Err(error) => return Err(wasmtime::Error::from_anyhow(call.trap(error))),
        };
        if let Some(error) = write_error {
            let mut data = data;
            if let Err(error) = accessor.with(|mut store| data.close(&mut store)) {
                return Err(wasmtime::Error::from_anyhow(call.trap(error)));
            }
            let result = complete_immediate_write_response::<
                Ctx,
                U,
                P3FilesystemTypesDescriptorWriteViaStream,
            >(accessor, call, Err(error))
            .await?;
            return accessor.with(|mut store| {
                FutureReader::new(&mut store, async move {
                    Ok::<Result<(), types::ErrorCode>, wasmtime::Error>(result)
                })
            });
        }

        let file = match accessor.with(|mut store| file_from_access::<Ctx, U>(&mut store, &fd)) {
            Ok(file) => file,
            Err(error) => return Err(wasmtime::Error::from_anyhow(call.trap(error))),
        };
        let (chunks_tx, chunks_rx) = tokio::sync::mpsc::unbounded_channel();
        if let Err(error) = accessor
            .with(|mut store| data.pipe(&mut store, FilesystemWriteConsumer::new(chunks_tx)))
        {
            return Err(wasmtime::Error::from_anyhow(call.trap(error)));
        }

        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        accessor.with(|mut store| {
            store.spawn(FilesystemWriteTask::<
                Ctx,
                P3FilesystemTypesDescriptorWriteViaStream,
            >::new(
                file,
                FilesystemWriteMode::At(offset),
                call,
                chunks_rx,
                result_tx,
            ));

            FutureReader::new(&mut store, wait_filesystem_task_result(result_rx))
        })
    }

    async fn append_via_stream<U: Send + 'static>(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        data: StreamReader<u8>,
    ) -> wasmtime::Result<FutureReader<Result<(), types::ErrorCode>>> {
        let path =
            accessor.with(|mut store| descriptor_path_from_access::<Ctx, U>(&mut store, &fd))?;
        let request = HostRequestFileSystemPath {
            path: path.to_string_lossy().to_string(),
        };
        let mut call =
            start_cancellable_call::<Ctx, U, P3FilesystemTypesDescriptorAppendViaStream>(
                accessor, request,
            )
            .await?;

        let write_error = match accessor
            .with(|mut store| write_validation_error_from_access::<Ctx, U>(&mut store, &fd))
        {
            Ok(write_error) => write_error,
            Err(error) => return Err(wasmtime::Error::from_anyhow(call.trap(error))),
        };
        if let Some(error) = write_error {
            let mut data = data;
            if let Err(error) = accessor.with(|mut store| data.close(&mut store)) {
                return Err(wasmtime::Error::from_anyhow(call.trap(error)));
            }
            let result = complete_immediate_write_response::<
                Ctx,
                U,
                P3FilesystemTypesDescriptorAppendViaStream,
            >(accessor, call, Err(error))
            .await?;
            return accessor.with(|mut store| {
                FutureReader::new(&mut store, async move {
                    Ok::<Result<(), types::ErrorCode>, wasmtime::Error>(result)
                })
            });
        }

        let file = match accessor.with(|mut store| file_from_access::<Ctx, U>(&mut store, &fd)) {
            Ok(file) => file,
            Err(error) => return Err(wasmtime::Error::from_anyhow(call.trap(error))),
        };
        let (chunks_tx, chunks_rx) = tokio::sync::mpsc::unbounded_channel();
        if let Err(error) = accessor
            .with(|mut store| data.pipe(&mut store, FilesystemWriteConsumer::new(chunks_tx)))
        {
            return Err(wasmtime::Error::from_anyhow(call.trap(error)));
        }

        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        accessor.with(|mut store| {
            store.spawn(FilesystemWriteTask::<
                Ctx,
                P3FilesystemTypesDescriptorAppendViaStream,
            >::new(
                file,
                FilesystemWriteMode::Append,
                call,
                chunks_rx,
                result_tx,
            ));

            FutureReader::new(&mut store, wait_filesystem_task_result(result_rx))
        })
    }

    async fn advise<U: Send>(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        offset: types::Filesize,
        length: types::Filesize,
        advice: types::Advice,
    ) -> FilesystemResult<()> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore>::advise(
            &store, fd, offset, length, advice,
        )
        .await
    }

    async fn sync_data<U: Send>(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
    ) -> FilesystemResult<()> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore>::sync_data(&store, fd).await
    }

    async fn get_flags<U: Send>(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
    ) -> FilesystemResult<types::DescriptorFlags> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore>::get_flags(&store, fd).await
    }

    async fn get_type<U: Send>(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
    ) -> FilesystemResult<types::DescriptorType> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore>::get_type(&store, fd).await
    }

    async fn set_size<U: Send>(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        size: types::Filesize,
    ) -> FilesystemResult<()> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore>::set_size(&store, fd, size).await
    }

    async fn set_times<U: Send>(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        data_access_timestamp: types::NewTimestamp,
        data_modification_timestamp: types::NewTimestamp,
    ) -> FilesystemResult<()> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore>::set_times(
            &store,
            fd,
            data_access_timestamp,
            data_modification_timestamp,
        )
        .await
    }

    async fn read_directory<U: Send + 'static>(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
    ) -> wasmtime::Result<(
        StreamReader<types::DirectoryEntry>,
        FutureReader<Result<(), types::ErrorCode>>,
    )> {
        let path =
            accessor.with(|mut store| descriptor_path_from_access::<Ctx, U>(&mut store, &fd))?;
        let call = start_call::<Ctx, U, P3FilesystemTypesDescriptorReadDirectory>(
            accessor,
            HostRequestFileSystemPath {
                path: path.to_string_lossy().to_string(),
            },
        )
        .await?;

        let (stream_tx, stream_rx) = tokio::sync::oneshot::channel();
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        let dir_result =
            accessor.with(|mut store| dir_result_from_access::<Ctx, U>(&mut store, &fd))?;
        let (stream, future) = match dir_result {
            Ok(dir) => {
                if call.is_live() {
                    if !dir.perms.contains(DirPerms::READ) {
                        let response = complete_directory_response::<Ctx, U>(
                            accessor,
                            call,
                            Vec::new(),
                            Err(types::ErrorCode::NotPermitted),
                        )
                        .await?;
                        let result = deserialize_stream_result(response.result);
                        accessor.with(|mut store| {
                            let stream = StreamReader::new(&mut store, std::iter::empty())?;
                            let future = FutureReader::new(&mut store, async move {
                                Ok::<Result<(), types::ErrorCode>, wasmtime::Error>(result)
                            })?;
                            Ok::<_, wasmtime::Error>((stream, future))
                        })?
                    } else {
                        accessor.with(|mut store| {
                            store.spawn(FilesystemDirectoryReadTask::<Ctx>::new(
                                call, stream_rx, result_tx,
                            ));
                            let stream = StreamReader::new(
                                &mut store,
                                RecordingDirectoryEntryStreamProducer::new(dir, stream_tx),
                            )?;
                            let future = FutureReader::new(
                                &mut store,
                                wait_filesystem_task_result(result_rx),
                            )?;
                            Ok::<_, wasmtime::Error>((stream, future))
                        })?
                    }
                } else {
                    let (stream_mode_tx, stream_mode_rx) = tokio::sync::oneshot::channel();
                    accessor.with(|mut store| {
                        store.spawn(FilesystemDirectoryReadReplayTask::<Ctx>::new(
                            call,
                            stream_mode_tx,
                            stream_rx,
                            result_tx,
                        ));
                        let stream = StreamReader::new(
                            &mut store,
                            DeferredDirectoryEntryStreamProducer::new(
                                dir,
                                stream_mode_rx,
                                stream_tx,
                            ),
                        )?;
                        let future =
                            FutureReader::new(&mut store, wait_filesystem_task_result(result_rx))?;
                        Ok::<_, wasmtime::Error>((stream, future))
                    })?
                }
            }
            Err(error) => {
                let response =
                    complete_immediate_directory_response::<Ctx, U>(accessor, call, Err(error))
                        .await?;
                let result = deserialize_stream_result(response.result);
                accessor.with(|mut store| {
                    let stream = StreamReader::new(&mut store, std::iter::empty())?;
                    let future = FutureReader::new(&mut store, async move {
                        Ok::<Result<(), types::ErrorCode>, wasmtime::Error>(result)
                    })?;
                    Ok::<_, wasmtime::Error>((stream, future))
                })?
            }
        };
        Ok((stream, future))
    }

    async fn sync<U: Send>(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
    ) -> FilesystemResult<()> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore>::sync(&store, fd).await
    }

    async fn create_directory_at<U: Send>(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        path: String,
    ) -> FilesystemResult<()> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore>::create_directory_at(&store, fd, path)
            .await
    }

    async fn stat<U: Send + 'static>(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
    ) -> FilesystemResult<types::DescriptorStat> {
        let path =
            descriptor_path_from_accessor::<Ctx, U>(store, &fd).map_err(FilesystemError::trap)?;
        let fd_rep = fd.rep();
        let live_stat = Arc::new(Mutex::new(None));
        let live_stat_for_call = Arc::clone(&live_stat);

        let response = run_read_access::<_, _, Ctx, P3FilesystemTypesDescriptorStat, _, _>(
            store,
            HostRequestFileSystemPath {
                path: path.to_string_lossy().to_string(),
            },
            DurableFunctionType::ReadLocal,
            || async {
                let stat = run_local_stat::<Ctx, U>(store, Resource::new_borrow(fd_rep)).await;
                *live_stat_for_call.lock().unwrap() = Some(stat.clone());
                Ok(HostResponseP3FileSystemStat {
                    result: serialize_stat_result(&stat),
                })
            },
        )
        .await
        .map_err(FilesystemError::trap)?;
        let live_stat = live_stat.lock().unwrap().take();
        let stat = match live_stat {
            Some(stat) => stat,
            None => run_local_stat::<Ctx, U>(store, Resource::new_borrow(fd_rep)).await,
        };

        apply_stat_response(stat, response).await
    }

    async fn stat_at<U: Send + 'static>(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        path_flags: types::PathFlags,
        path: String,
    ) -> FilesystemResult<types::DescriptorStat> {
        let full_path = descriptor_path_at_from_accessor::<Ctx, U>(store, &fd, &path)
            .map_err(FilesystemError::trap)?;
        let fd_rep = fd.rep();
        let live_stat = Arc::new(Mutex::new(None));
        let live_stat_for_call = Arc::clone(&live_stat);
        let live_path = path.clone();
        let response = run_read_access::<_, _, Ctx, P3FilesystemTypesDescriptorStatAt, _, _>(
            store,
            HostRequestFileSystemPath {
                path: full_path.to_string_lossy().to_string(),
            },
            DurableFunctionType::ReadLocal,
            || async {
                let stat = run_local_stat_at::<Ctx, U>(
                    store,
                    Resource::new_borrow(fd_rep),
                    path_flags,
                    live_path,
                )
                .await;
                *live_stat_for_call.lock().unwrap() = Some(stat.clone());
                Ok(HostResponseP3FileSystemStat {
                    result: serialize_stat_result(&stat),
                })
            },
        )
        .await
        .map_err(FilesystemError::trap)?;
        let live_stat = live_stat.lock().unwrap().take();
        let stat = match live_stat {
            Some(stat) => stat,
            None => {
                run_local_stat_at::<Ctx, U>(store, Resource::new_borrow(fd_rep), path_flags, path)
                    .await
            }
        };

        apply_stat_response(stat, response).await
    }

    async fn set_times_at<U: Send>(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        path_flags: types::PathFlags,
        path: String,
        data_access_timestamp: types::NewTimestamp,
        data_modification_timestamp: types::NewTimestamp,
    ) -> FilesystemResult<()> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore>::set_times_at(
            &store,
            fd,
            path_flags,
            path,
            data_access_timestamp,
            data_modification_timestamp,
        )
        .await
    }

    async fn link_at<U: Send>(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        old_path_flags: types::PathFlags,
        old_path: String,
        new_fd: Resource<Descriptor>,
        new_path: String,
    ) -> FilesystemResult<()> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore>::link_at(
            &store,
            fd,
            old_path_flags,
            old_path,
            new_fd,
            new_path,
        )
        .await
    }

    async fn open_at<U: Send>(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        path_flags: types::PathFlags,
        path: String,
        open_flags: types::OpenFlags,
        flags: types::DescriptorFlags,
    ) -> FilesystemResult<Resource<Descriptor>> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore>::open_at(
            &store, fd, path_flags, path, open_flags, flags,
        )
        .await
    }

    async fn readlink_at<U: Send>(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        path: String,
    ) -> FilesystemResult<String> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore>::readlink_at(&store, fd, path).await
    }

    async fn remove_directory_at<U: Send>(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        path: String,
    ) -> FilesystemResult<()> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore>::remove_directory_at(&store, fd, path)
            .await
    }

    async fn rename_at<U: Send>(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        old_path: String,
        new_fd: Resource<Descriptor>,
        new_path: String,
    ) -> FilesystemResult<()> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore>::rename_at(
            &store, fd, old_path, new_fd, new_path,
        )
        .await
    }

    async fn symlink_at<U: Send>(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        old_path: String,
        new_path: String,
    ) -> FilesystemResult<()> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore>::symlink_at(
            &store, fd, old_path, new_path,
        )
        .await
    }

    async fn unlink_file_at<U: Send>(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        path: String,
    ) -> FilesystemResult<()> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore>::unlink_file_at(&store, fd, path).await
    }

    async fn is_same_object<U: Send>(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        other: Resource<Descriptor>,
    ) -> wasmtime::Result<bool> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore>::is_same_object(&store, fd, other).await
    }

    async fn metadata_hash<U: Send>(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
    ) -> FilesystemResult<types::MetadataHashValue> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore>::metadata_hash(&store, fd).await
    }

    async fn metadata_hash_at<U: Send>(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        path_flags: types::PathFlags,
        path: String,
    ) -> FilesystemResult<types::MetadataHashValue> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore>::metadata_hash_at(
            &store, fd, path_flags, path,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fs_set_times::{SystemTimeSpec, set_symlink_times, set_times};
    use golem_common::model::oplog::types::{SerializableDateTime, SerializableP3DescriptorType};
    use golem_common::model::oplog::{HostRequest, HostResponse, host_functions};
    use std::time::{Duration, SystemTime};
    use test_r::test;

    #[test]
    async fn p3_fs_live_write_future_drop_returns_interrupted() {
        let mut config = wasmtime::Config::default();
        config.wasm_component_model(true);
        config.wasm_component_model_async(true);
        let engine = wasmtime::Engine::new(&config).unwrap();
        let mut store = wasmtime::Store::new(&engine, ());

        let tempdir = tempfile::TempDir::new().unwrap();
        let path = tempdir.path().join("out");
        let file = File::new(
            cap_std::fs::File::from_std(std::fs::File::create(&path).unwrap()),
            FilePerms::WRITE,
            wasmtime_wasi::filesystem::OpenMode::WRITE,
            false,
            path,
        );

        let result = store
            .run_concurrent(async |accessor| {
                let accessor = accessor
                    .with_getter::<DurableP3<crate::workerctx::default::Context>>(
                        unused_durable_p3_view,
                    );
                let (_chunks_tx, mut chunks_rx) = tokio::sync::mpsc::unbounded_channel();
                let (mut result_tx, result_rx) = tokio::sync::oneshot::channel();
                drop(result_rx);

                tokio::time::timeout(
                    Duration::from_millis(100),
                    run_streaming_filesystem_write::<crate::workerctx::default::Context, ()>(
                        &accessor,
                        &file,
                        FilesystemWriteMode::At(0),
                        &mut chunks_rx,
                        &mut result_tx,
                    ),
                )
                .await
            })
            .await
            .unwrap();

        let captured = result
            .expect("write task should observe dropped result future")
            .unwrap();
        assert_eq!(captured.contents, Vec::<u8>::new());
        assert!(matches!(
            captured.result,
            Err(types::ErrorCode::Interrupted)
        ));
    }

    fn unused_durable_p3_view(_: &mut ()) -> DurableP3View<'_, crate::workerctx::default::Context> {
        panic!("test does not access the worker context")
    }

    #[cfg(unix)]
    #[test]
    async fn p3_fs_stat_at_follow_symlink_does_not_mutate_symlink_timestamps() {
        let tempdir = tempfile::TempDir::new().unwrap();
        let target = tempdir.path().join("target");
        let link = tempdir.path().join("link");
        std::fs::write(&target, b"target").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let new = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000);
        set_symlink_times(
            &link,
            Some(SystemTimeSpec::from(old)),
            Some(SystemTimeSpec::from(old)),
        )
        .unwrap();
        let before = std::fs::symlink_metadata(&link)
            .unwrap()
            .modified()
            .unwrap();

        let new_timestamp = SerializableDateTime::from(new);
        apply_stat_response(
            Ok(types::DescriptorStat {
                type_: types::DescriptorType::RegularFile,
                link_count: 1,
                size: 6,
                data_access_timestamp: Some(new_timestamp.clone().into()),
                data_modification_timestamp: Some(new_timestamp.clone().into()),
                status_change_timestamp: None,
            }),
            HostResponseP3FileSystemStat {
                result: Ok(SerializableFileTimes {
                    data_access_timestamp: Some(new_timestamp.clone()),
                    data_modification_timestamp: Some(new_timestamp),
                }),
            },
        )
        .await
        .unwrap();

        let after = std::fs::symlink_metadata(&link)
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(
            after, before,
            "stat-at with symlink-follow is a read-only operation and must not rewrite the symlink itself"
        );
    }

    #[cfg(unix)]
    #[test]
    async fn p3_fs_stat_at_follow_symlink_does_not_mutate_target_timestamps() {
        let tempdir = tempfile::TempDir::new().unwrap();
        let target = tempdir.path().join("target");
        let link = tempdir.path().join("link");
        std::fs::write(&target, b"target").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let new = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000);
        set_times(
            &target,
            Some(SystemTimeSpec::from(old)),
            Some(SystemTimeSpec::from(old)),
        )
        .unwrap();
        let before = std::fs::metadata(&target).unwrap().modified().unwrap();

        let new_timestamp = SerializableDateTime::from(new);
        apply_stat_response(
            Ok(types::DescriptorStat {
                type_: types::DescriptorType::RegularFile,
                link_count: 1,
                size: 6,
                data_access_timestamp: Some(new_timestamp.clone().into()),
                data_modification_timestamp: Some(new_timestamp.clone().into()),
                status_change_timestamp: None,
            }),
            HostResponseP3FileSystemStat {
                result: Ok(SerializableFileTimes {
                    data_access_timestamp: Some(new_timestamp.clone()),
                    data_modification_timestamp: Some(new_timestamp),
                }),
            },
        )
        .await
        .unwrap();

        let after = std::fs::metadata(&target).unwrap().modified().unwrap();
        assert_eq!(
            after, before,
            "stat-at with symlink-follow is a read-only operation and must not rewrite the target"
        );
    }

    #[test]
    fn p3_filesystem_stream_host_payload_pairs_roundtrip() {
        assert_host_payload_pair_roundtrip::<P3FilesystemTypesDescriptorReadViaStream>(
            HostRequestFileSystemPathAndOffset {
                path: "/tmp/file.txt".to_string(),
                offset: 12,
            },
            HostResponseP3FileSystemByteStream {
                contents: b"file bytes".to_vec(),
                result: Ok(()),
            },
        );
        assert_host_payload_pair_roundtrip::<P3FilesystemTypesDescriptorWriteViaStream>(
            HostRequestFileSystemPathAndOffset {
                path: "/tmp/file.txt".to_string(),
                offset: 5,
            },
            HostResponseP3FileSystemByteStream {
                contents: b"written".to_vec(),
                result: Err(SerializableP3FsErrorCode::NoEntry),
            },
        );
        assert_host_payload_pair_roundtrip::<P3FilesystemTypesDescriptorAppendViaStream>(
            HostRequestFileSystemPath {
                path: "/tmp/file.txt".to_string(),
            },
            HostResponseP3FileSystemByteStream {
                contents: b"appended".to_vec(),
                result: Ok(()),
            },
        );
        assert_host_payload_pair_roundtrip::<P3FilesystemTypesDescriptorReadDirectory>(
            HostRequestFileSystemPath {
                path: "/tmp".to_string(),
            },
            HostResponseP3FileSystemDirectoryEntryStream {
                entries: vec![SerializableP3DirectoryEntry {
                    type_: SerializableP3DescriptorType::RegularFile,
                    name: "file.txt".to_string(),
                }],
                result: Ok(()),
            },
        );
    }

    fn assert_host_payload_pair_roundtrip<Pair>(request: Pair::Req, response: Pair::Resp)
    where
        Pair: HostPayloadPair,
        Pair::Req: Clone + std::fmt::Debug + PartialEq + TryFrom<HostRequest, Error = String>,
        Pair::Resp: Clone + std::fmt::Debug + PartialEq,
    {
        let request_payload: HostRequest = request.clone().into();
        let request_bytes = desert_rust::serialize_to_byte_vec(&request_payload).unwrap();
        let request_roundtrip: HostRequest = desert_rust::deserialize(&request_bytes).unwrap();
        assert_eq!(Pair::Req::try_from(request_roundtrip).unwrap(), request);

        let response_payload: HostResponse = response.clone().into();
        let response_bytes = desert_rust::serialize_to_byte_vec(&response_payload).unwrap();
        let response_roundtrip: HostResponse = desert_rust::deserialize(&response_bytes).unwrap();
        assert_eq!(Pair::Resp::try_from(response_roundtrip).unwrap(), response);

        let function_name_bytes =
            desert_rust::serialize_to_byte_vec(&Pair::HOST_FUNCTION_NAME).unwrap();
        let function_name_roundtrip: host_functions::HostFunctionName =
            desert_rust::deserialize(&function_name_bytes).unwrap();
        assert_eq!(function_name_roundtrip, Pair::HOST_FUNCTION_NAME);
    }
}
