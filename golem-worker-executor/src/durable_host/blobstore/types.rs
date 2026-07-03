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

use std::io::Cursor;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use std::task::{Context, Poll};

use bytes::BytesMut;
use golem_common::model::oplog::host_functions::P3BlobstoreTypesIncomingValueConsumeAsync;
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestNoInput, HostResponseP3BlobstoreIncomingValueStream,
};
use wasmtime::AsContextMut;
use wasmtime::StoreContextMut;
use wasmtime::component::{
    Access, Accessor, AccessorTask, Destination, HasSelf, Resource, Source, StreamConsumer,
    StreamProducer, StreamReader, StreamResult,
};
use wasmtime_wasi::IoView;

use crate::durable_host::concurrent::{CallHandle, CallReplayOutcome, Cancellable};
use crate::durable_host::{DurabilityHost, DurableWorkerCtx};

use crate::preview2::wasi::blobstore::types::{
    Error, Host, HostIncomingValue, HostIncomingValueWithStore, HostOutgoingValue,
    HostOutgoingValueWithStore, IncomingValue,
};
use crate::workerctx::WorkerCtx;

const BLOBSTORE_STREAM_BUFFER_CAPACITY: usize = 8192;

enum DeferredIncomingValueStreamState {
    Awaiting(tokio::sync::oneshot::Receiver<wasmtime::Result<Vec<u8>>>),
    Streaming(Cursor<BytesMut>),
    Done,
}

struct DeferredIncomingValueStreamProducer {
    state: DeferredIncomingValueStreamState,
}

impl DeferredIncomingValueStreamProducer {
    fn new(rx: tokio::sync::oneshot::Receiver<wasmtime::Result<Vec<u8>>>) -> Self {
        Self {
            state: DeferredIncomingValueStreamState::Awaiting(rx),
        }
    }
}

impl<D> StreamProducer<D> for DeferredIncomingValueStreamProducer {
    type Item = u8;
    type Buffer = BytesMut;

    fn poll_produce<'a>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: StoreContextMut<'a, D>,
        dst: Destination<'a, Self::Item, Self::Buffer>,
        _finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        loop {
            match &mut self.state {
                DeferredIncomingValueStreamState::Awaiting(rx) => match Pin::new(rx).poll(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(Ok(Ok(contents))) => {
                        self.state = DeferredIncomingValueStreamState::Streaming(Cursor::new(
                            BytesMut::from(contents.as_slice()),
                        ));
                    }
                    Poll::Ready(Ok(Err(error))) => {
                        self.state = DeferredIncomingValueStreamState::Done;
                        return Poll::Ready(Err(error));
                    }
                    Poll::Ready(Err(_)) => {
                        self.state = DeferredIncomingValueStreamState::Done;
                        return Poll::Ready(Err(wasmtime::Error::msg(
                            "blobstore incoming value replay task dropped",
                        )));
                    }
                },
                DeferredIncomingValueStreamState::Streaming(contents) => {
                    if dst.remaining(store.as_context_mut()) == Some(0) {
                        return Poll::Ready(Ok(StreamResult::Completed));
                    }

                    let bytes = contents.get_ref();
                    let position = contents.position() as usize;
                    if position >= bytes.len() {
                        self.state = DeferredIncomingValueStreamState::Done;
                        return Poll::Ready(Ok(StreamResult::Dropped));
                    }

                    let mut dst = dst.as_direct(store, BLOBSTORE_STREAM_BUFFER_CAPACITY);
                    let remaining = &bytes[position..];
                    let n = remaining.len().min(dst.remaining().len());
                    dst.remaining()[..n].copy_from_slice(&remaining[..n]);
                    dst.mark_written(n);
                    contents.set_position((position + n) as u64);
                    return Poll::Ready(Ok(StreamResult::Completed));
                }
                DeferredIncomingValueStreamState::Done => {
                    return Poll::Ready(Ok(StreamResult::Dropped));
                }
            }
        }
    }
}

struct IncomingValueConsumeTask<Ctx> {
    contents: Vec<u8>,
    result_tx: tokio::sync::oneshot::Sender<wasmtime::Result<Vec<u8>>>,
    _phantom: PhantomData<fn() -> Ctx>,
}

impl<Ctx> IncomingValueConsumeTask<Ctx> {
    fn new(
        contents: Vec<u8>,
        result_tx: tokio::sync::oneshot::Sender<wasmtime::Result<Vec<u8>>>,
    ) -> Self {
        Self {
            contents,
            result_tx,
            _phantom: PhantomData,
        }
    }
}

impl<Ctx, T> AccessorTask<T, HasSelf<DurableWorkerCtx<Ctx>>> for IncomingValueConsumeTask<Ctx>
where
    Ctx: WorkerCtx,
    T: 'static,
{
    async fn run(
        self,
        accessor: &Accessor<T, HasSelf<DurableWorkerCtx<Ctx>>>,
    ) -> wasmtime::Result<()> {
        let result = async {
            let mut handle =
                CallHandle::<P3BlobstoreTypesIncomingValueConsumeAsync, Cancellable>::start_access(
                    accessor,
                    accessor.getter(),
                    HostRequestNoInput {},
                    DurableFunctionType::ReadRemote,
                )
                .await
                .map_err(wasmtime::Error::from)?;

            if !handle.is_live() {
                match handle
                    .replay_access(accessor, accessor.getter())
                    .await
                    .map_err(wasmtime::Error::from)?
                {
                    CallReplayOutcome::Replayed(response) => return Ok(response.contents),
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

            let response = handle
                .complete_access(
                    accessor,
                    accessor.getter(),
                    HostResponseP3BlobstoreIncomingValueStream {
                        contents: self.contents,
                    },
                )
                .await
                .map_err(wasmtime::Error::from)?;
            Ok(response.contents)
        }
        .await;

        let _ = self.result_tx.send(result);
        Ok(())
    }
}

impl<Ctx: WorkerCtx> HostOutgoingValue for DurableWorkerCtx<Ctx> {
    async fn new_outgoing_value(&mut self) -> anyhow::Result<Resource<OutgoingValueEntry>> {
        self.observe_function_call("blobstore::types::outgoing_value", "new_outgoing_value");
        let outgoing_value = self
            .as_wasi_view()
            .table()
            .push(OutgoingValueEntry::new())?;
        Ok(outgoing_value)
    }

    async fn drop(&mut self, rep: Resource<OutgoingValueEntry>) -> anyhow::Result<()> {
        self.observe_function_call("blobstore::types::outgoing_value", "drop");
        self.as_wasi_view()
            .table()
            .delete::<OutgoingValueEntry>(rep)?;
        Ok(())
    }
}

/// Consumes the guest-provided `stream<u8>` written into an outgoing value and
/// appends the bytes to the outgoing value's in-memory body buffer. The buffer
/// is later captured durably by the consuming `container::write-data` call, so
/// this consumer itself performs no oplog recording.
struct OutgoingValueWriteConsumer {
    body: Arc<RwLock<Vec<u8>>>,
}

impl<D> StreamConsumer<D> for OutgoingValueWriteConsumer {
    type Item = u8;

    fn poll_consume(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        store: StoreContextMut<D>,
        source: Source<'_, Self::Item>,
        _finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        let mut source = source.as_direct(store);
        let bytes = source.remaining();
        if bytes.is_empty() {
            return Poll::Ready(Ok(StreamResult::Completed));
        }
        let len = bytes.len();
        self.body.write().unwrap().extend_from_slice(bytes);
        source.mark_read(len);
        Poll::Ready(Ok(StreamResult::Completed))
    }
}

impl<U: Send + 'static, Ctx: WorkerCtx> HostOutgoingValueWithStore<U>
    for HasSelf<DurableWorkerCtx<Ctx>>
{
    fn outgoing_value_write_body(
        mut host: Access<U, Self>,
        self_: Resource<OutgoingValueEntry>,
        data: StreamReader<u8>,
    ) -> anyhow::Result<Result<(), ()>> {
        let body = {
            let ctx = host.get();
            ctx.observe_function_call(
                "blobstore::types::outgoing_value",
                "outgoing_value_write_body",
            );
            ctx.as_wasi_view()
                .table()
                .get::<OutgoingValueEntry>(&self_)?
                .body
                .clone()
        };
        data.pipe(&mut host, OutgoingValueWriteConsumer { body })?;
        Ok(Ok(()))
    }
}

impl<Ctx: WorkerCtx> HostIncomingValue for DurableWorkerCtx<Ctx> {
    async fn incoming_value_consume_sync(
        &mut self,
        self_: Resource<IncomingValue>,
    ) -> anyhow::Result<Result<Vec<u8>, Error>> {
        self.observe_function_call(
            "blobstore::types::incoming_value",
            "incoming_value_consume_sync",
        );
        let body = self
            .as_wasi_view()
            .table()
            .get::<IncomingValueEntry>(&self_)?
            .body
            .clone();
        let value = body.write().unwrap().drain(..).collect();
        Ok(Ok(value))
    }

    async fn size(&mut self, self_: Resource<IncomingValue>) -> anyhow::Result<u64> {
        self.observe_function_call("blobstore::types::incoming_value", "size");
        let body = self
            .as_wasi_view()
            .table()
            .get::<IncomingValueEntry>(&self_)?
            .body
            .clone();
        let size = body.read().unwrap().len() as u64;
        Ok(size)
    }

    async fn drop(&mut self, rep: Resource<IncomingValue>) -> anyhow::Result<()> {
        self.observe_function_call("blobstore::types::incoming_value", "drop");
        self.as_wasi_view()
            .table()
            .delete::<IncomingValueEntry>(rep)?;
        Ok(())
    }
}

impl<U: Send + 'static, Ctx: WorkerCtx> HostIncomingValueWithStore<U>
    for HasSelf<DurableWorkerCtx<Ctx>>
{
    fn incoming_value_consume_async(
        mut host: Access<U, Self>,
        self_: Resource<IncomingValue>,
    ) -> anyhow::Result<Result<StreamReader<u8>, Error>> {
        let contents = {
            let ctx = host.get();
            ctx.observe_function_call(
                "blobstore::types::incoming_value",
                "incoming_value_consume_async",
            );
            let body = ctx
                .as_wasi_view()
                .table()
                .get::<IncomingValueEntry>(&self_)?
                .body
                .clone();
            body.write().unwrap().drain(..).collect::<Vec<u8>>()
        };

        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        host.spawn(IncomingValueConsumeTask::<Ctx>::new(contents, result_tx));
        Ok(Ok(StreamReader::new(
            &mut host,
            DeferredIncomingValueStreamProducer::new(result_rx),
        )?))
    }
}

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {}

pub struct ContainerEntry {
    pub name: String,
    pub created_at: u64,
}

impl ContainerEntry {
    pub fn new(name: String, created_at: u64) -> Self {
        Self { name, created_at }
    }
}

pub struct OutgoingValueEntry {
    pub body: Arc<RwLock<Vec<u8>>>,
}

impl Default for OutgoingValueEntry {
    fn default() -> Self {
        Self::new()
    }
}

impl OutgoingValueEntry {
    pub fn new() -> Self {
        Self {
            body: Arc::new(RwLock::new(Vec::new())),
        }
    }
}

pub struct IncomingValueEntry {
    body: Arc<RwLock<Vec<u8>>>,
}

impl IncomingValueEntry {
    #[allow(unused)]
    pub fn new(body: Vec<u8>) -> IncomingValueEntry {
        IncomingValueEntry {
            body: Arc::new(RwLock::new(body)),
        }
    }
}

pub struct StreamObjectNamesEntry {
    pub names: Arc<RwLock<Vec<String>>>,
}

impl StreamObjectNamesEntry {
    pub fn new(names: Vec<String>) -> Self {
        Self {
            names: Arc::new(RwLock::new(names)),
        }
    }
}
