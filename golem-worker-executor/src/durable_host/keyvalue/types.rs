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
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::host_functions::{
    P3KeyvalueCacheVacancyFill, P3KeyvalueTypesIncomingValueConsumeAsync,
};
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestNoInput, HostResponseKVUnit,
    HostResponseP3KeyvalueIncomingValueStream,
};

use crate::durable_host::concurrent::{CallHandle, CallReplayOutcome, Cancellable};
use crate::durable_host::keyvalue::error::ErrorEntry;
use crate::durable_host::{DurabilityHost, DurableWorkerCtx};
use crate::preview2::wasi::keyvalue::types::{
    Error, Host, HostBucket, HostIncomingValue, HostIncomingValueWithStore, HostOutgoingValue,
    HostOutgoingValueWithStore, IncomingValue,
};
use crate::workerctx::WorkerCtx;
use wasmtime::AsContextMut;
use wasmtime::StoreContextMut;
use wasmtime::component::{
    Access, Accessor, AccessorTask, Destination, HasSelf, Resource, StreamProducer, StreamReader,
    StreamResult,
};
use wasmtime_wasi::IoView;

const KEYVALUE_STREAM_BUFFER_CAPACITY: usize = 8192;

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
                            "keyvalue incoming value replay task dropped",
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

                    let mut dst = dst.as_direct(store, KEYVALUE_STREAM_BUFFER_CAPACITY);
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
                CallHandle::<P3KeyvalueTypesIncomingValueConsumeAsync, Cancellable>::start_access(
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
                    HostResponseP3KeyvalueIncomingValueStream {
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

impl<Ctx: WorkerCtx> HostBucket for DurableWorkerCtx<Ctx> {
    async fn open_bucket(
        &mut self,
        name: String,
    ) -> anyhow::Result<Result<Resource<BucketEntry>, Resource<Error>>> {
        self.observe_function_call("keyvalue::types::bucket", "open");
        let bucket = self.as_wasi_view().table().push(BucketEntry::new(name))?;
        Ok(Ok(bucket))
    }

    async fn drop(&mut self, rep: Resource<BucketEntry>) -> anyhow::Result<()> {
        self.observe_function_call("keyvalue::types::bucket", "drop");
        self.as_wasi_view().table().delete::<BucketEntry>(rep)?;
        Ok(())
    }
}

impl<Ctx: WorkerCtx> HostOutgoingValue for DurableWorkerCtx<Ctx> {
    async fn new_outgoing_value(&mut self) -> anyhow::Result<Resource<OutgoingValueEntry>> {
        self.observe_function_call("keyvalue::types::outgoing_value", "new_outgoing_value");
        let outgoing_value = self
            .as_wasi_view()
            .table()
            .push(OutgoingValueEntry::new())?;
        Ok(outgoing_value)
    }

    async fn outgoing_value_write_body_async(
        &mut self,
        _self_: Resource<OutgoingValueEntry>,
    ) -> anyhow::Result<Result<StreamReader<u8>, Resource<Error>>> {
        self.observe_function_call(
            "keyvalue::types::outgoing_value",
            "outgoing_value_write_body_async",
        );
        let error = self.as_wasi_view().table().push(ErrorEntry::new(
            "keyvalue outgoing async body streams are not supported by this host binding"
                .to_string(),
        ))?;
        Ok(Err(error))
    }

    async fn outgoing_value_write_body_sync(
        &mut self,
        self_: Resource<OutgoingValueEntry>,
        value: Vec<u8>,
    ) -> anyhow::Result<Result<(), Resource<Error>>> {
        self.observe_function_call(
            "keyvalue::types::outgoing_value",
            "outgoing_value_write_body_sync",
        );
        let body = self
            .as_wasi_view()
            .table()
            .get::<OutgoingValueEntry>(&self_)?
            .body
            .clone();
        body.write().unwrap().extend_from_slice(&value);
        Ok(Ok(()))
    }
}

impl<U: Send + 'static, Ctx: WorkerCtx> HostOutgoingValueWithStore<U>
    for HasSelf<DurableWorkerCtx<Ctx>>
{
    async fn drop(
        accessor: &Accessor<U, Self>,
        rep: Resource<OutgoingValueEntry>,
    ) -> anyhow::Result<()> {
        let (mut entry, key_value_service) = accessor.with(|mut access| {
            let ctx = access.get();
            ctx.observe_function_call("keyvalue::types::outgoing_value", "drop");
            Ok::<_, anyhow::Error>((
                ctx.as_wasi_view()
                    .table()
                    .delete::<OutgoingValueEntry>(rep)?,
                ctx.state.key_value_service.clone(),
            ))
        })?;

        if let Some(mut fill) = entry.cache_fill.take()
            && let Some(mut handle) = fill.handle.take()
        {
            let value = entry.body.read().unwrap().clone();
            let response = 'resp: {
                if !handle.is_live() {
                    match handle.replay_access(accessor, accessor.getter()).await? {
                        CallReplayOutcome::Replayed(response) => break 'resp response,
                        CallReplayOutcome::Incomplete(live) => handle = live,
                    }
                }

                let result = key_value_service
                    .set(
                        fill.environment_id,
                        CACHE_BUCKET.to_string(),
                        fill.key,
                        value,
                    )
                    .await
                    .map_err(|err| err.to_string());
                handle
                    .complete_access(accessor, accessor.getter(), HostResponseKVUnit { result })
                    .await?
            };

            if let Err(error) = response.result {
                tracing::debug!(error, "keyvalue::cache vacancy fill failed");
            }
        }
        Ok(())
    }
}

impl<Ctx: WorkerCtx> HostIncomingValue for DurableWorkerCtx<Ctx> {
    async fn incoming_value_consume_sync(
        &mut self,
        self_: Resource<IncomingValue>,
    ) -> anyhow::Result<Result<Vec<u8>, Resource<Error>>> {
        self.observe_function_call(
            "keyvalue::types::incoming_value",
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

    async fn incoming_value_size(
        &mut self,
        self_: Resource<IncomingValue>,
    ) -> anyhow::Result<Result<u64, Resource<Error>>> {
        self.observe_function_call("keyvalue::types::incoming_value", "size");
        let body = self
            .as_wasi_view()
            .table()
            .get::<IncomingValueEntry>(&self_)?
            .body
            .clone();
        let size = body.read().unwrap().len() as u64;
        Ok(Ok(size))
    }

    async fn drop(&mut self, rep: Resource<IncomingValue>) -> anyhow::Result<()> {
        self.observe_function_call("keyvalue::types::incoming_value", "drop");
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
    ) -> anyhow::Result<Result<StreamReader<u8>, Resource<Error>>> {
        let contents = {
            let ctx = host.get();
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

pub(crate) const CACHE_BUCKET: &str = "__golem_wasi_keyvalue_cache";

pub struct BucketEntry {
    pub name: String,
}

impl BucketEntry {
    pub fn new(name: String) -> Self {
        Self { name }
    }
}

pub struct OutgoingValueEntry {
    pub body: Arc<RwLock<Vec<u8>>>,
    pub(crate) cache_fill: Option<CacheFillState>,
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
            cache_fill: None,
        }
    }

    pub(crate) fn new_cache_fill(cache_fill: CacheFillState) -> Self {
        Self {
            body: Arc::new(RwLock::new(Vec::new())),
            cache_fill: Some(cache_fill),
        }
    }
}

pub(crate) struct CacheFillState {
    pub handle: Option<CallHandle<P3KeyvalueCacheVacancyFill, Cancellable>>,
    pub environment_id: EnvironmentId,
    pub key: String,
}

pub struct IncomingValueEntry {
    body: Arc<RwLock<Vec<u8>>>,
}

impl IncomingValueEntry {
    pub fn new(body: Vec<u8>) -> IncomingValueEntry {
        IncomingValueEntry {
            body: Arc::new(RwLock::new(body)),
        }
    }
}
