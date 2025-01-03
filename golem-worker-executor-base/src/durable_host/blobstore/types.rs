// Copyright 2024-2025 Golem Cloud
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

use std::any::Any;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use bytes::Bytes;
use wasmtime::component::Resource;
use wasmtime_wasi::{
    HostInputStream, HostOutputStream, InputStream, StreamResult, Subscribe, WasiView,
};

use crate::durable_host::DurableWorkerCtx;
use crate::metrics::wasm::record_host_function_call;

use crate::preview2::wasi::blobstore::types::{
    Error, Host, HostIncomingValue, HostOutgoingValue, IncomingValue, IncomingValueAsyncBody,
    IncomingValueSyncBody, OutputStream as OutgoingValueBodyAsync,
};
use crate::workerctx::WorkerCtx;

#[async_trait]
impl<Ctx: WorkerCtx> HostOutgoingValue for DurableWorkerCtx<Ctx> {
    async fn new_outgoing_value(&mut self) -> anyhow::Result<Resource<OutgoingValueEntry>> {
        record_host_function_call("blobstore::types::outgoing_value", "new_outgoing_value");
        let outgoing_value = self
            .as_wasi_view()
            .table()
            .push(OutgoingValueEntry::new())?;
        Ok(outgoing_value)
    }

    async fn outgoing_value_write_body(
        &mut self,
        self_: Resource<OutgoingValueEntry>,
    ) -> anyhow::Result<Result<Resource<OutgoingValueBodyAsync>, ()>> {
        record_host_function_call(
            "blobstore::types::outgoing_value",
            "outgoing_value_write_body",
        );
        let body = self
            .as_wasi_view()
            .table()
            .get::<OutgoingValueEntry>(&self_)?
            .body
            .clone();
        let body: Box<dyn HostOutputStream> = Box::new(OutgoingValueEntryStream::new(body));
        let outgoing_value_async_body = self.as_wasi_view().table().push(body)?;
        Ok(Ok(outgoing_value_async_body))
    }

    async fn drop(&mut self, rep: Resource<OutgoingValueEntry>) -> anyhow::Result<()> {
        record_host_function_call("blobstore::types::outgoing_value", "drop");
        self.as_wasi_view()
            .table()
            .delete::<OutgoingValueEntry>(rep)?;
        Ok(())
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostIncomingValue for DurableWorkerCtx<Ctx> {
    async fn incoming_value_consume_sync(
        &mut self,
        self_: Resource<IncomingValue>,
    ) -> anyhow::Result<Result<IncomingValueSyncBody, Error>> {
        record_host_function_call(
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

    async fn incoming_value_consume_async(
        &mut self,
        self_: Resource<IncomingValue>,
    ) -> anyhow::Result<Result<Resource<IncomingValueAsyncBody>, Error>> {
        record_host_function_call(
            "blobstore::types::incoming_value",
            "incoming_value_consume_async",
        );
        let body = self
            .as_wasi_view()
            .table()
            .get::<IncomingValueEntry>(&self_)?
            .body
            .clone();
        let body: InputStream = Box::new(IncomingValueEntryStream::new(body));
        let incoming_value_async_body = self.as_wasi_view().table().push(body)?;
        Ok(Ok(incoming_value_async_body))
    }

    async fn size(&mut self, self_: Resource<IncomingValue>) -> anyhow::Result<u64> {
        record_host_function_call("blobstore::types::incoming_value", "size");
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
        record_host_function_call("blobstore::types::incoming_value", "drop");
        self.as_wasi_view()
            .table()
            .delete::<IncomingValueEntry>(rep)?;
        Ok(())
    }
}

#[async_trait]
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

pub struct OutgoingValueEntryStream {
    pub body: Arc<RwLock<Vec<u8>>>,
}

impl OutgoingValueEntryStream {
    pub fn new(body: Arc<RwLock<Vec<u8>>>) -> Self {
        Self { body }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[async_trait]
impl Subscribe for OutgoingValueEntryStream {
    async fn ready(&mut self) {}
}

#[async_trait]
impl HostOutputStream for OutgoingValueEntryStream {
    fn as_any(&self) -> &dyn Any {
        OutgoingValueEntryStream::as_any(self)
    }

    fn write(&mut self, bytes: Bytes) -> StreamResult<()> {
        self.body.write().unwrap().extend_from_slice(&bytes);
        Ok(())
    }

    fn flush(&mut self) -> StreamResult<()> {
        Ok(())
    }

    fn check_write(&mut self) -> StreamResult<usize> {
        Ok(usize::MAX)
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

pub struct IncomingValueEntryStream {
    body: Arc<RwLock<Vec<u8>>>,
}

impl IncomingValueEntryStream {
    pub fn new(body: Arc<RwLock<Vec<u8>>>) -> IncomingValueEntryStream {
        IncomingValueEntryStream { body }
    }
}

#[async_trait]
impl Subscribe for IncomingValueEntryStream {
    async fn ready(&mut self) {}
}

#[async_trait]
impl HostInputStream for IncomingValueEntryStream {
    fn read(&mut self, size: usize) -> StreamResult<Bytes> {
        let mut buf = vec![0u8; size];
        let mut body = self.body.write().unwrap();
        let size = std::cmp::min(buf.len(), body.len());
        buf[..size].copy_from_slice(&body[..size]);
        body.drain(..size);
        Ok(buf.into())
    }

    fn as_any(&self) -> &dyn Any {
        self
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
