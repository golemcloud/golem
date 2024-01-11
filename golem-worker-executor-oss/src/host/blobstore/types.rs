use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use tonic::codegen::Bytes;
use wasmtime::component::Resource;
use wasmtime_wasi::preview2::{
    HostInputStream, HostOutputStream, InputStream, StreamResult, Subscribe,
};

use crate::context::Context;
use crate::preview2::wasi::blobstore::types::{
    Error, Host, HostIncomingValue, HostOutgoingValue, IncomingValue, IncomingValueAsyncBody,
    IncomingValueSyncBody, OutputStream as OutgoingValueBodyAsync,
};

#[async_trait]
impl HostIncomingValue for Context {
    async fn incoming_value_consume_sync(
        &mut self,
        self_: Resource<IncomingValue>,
    ) -> anyhow::Result<Result<IncomingValueSyncBody, Error>> {
        let body = self.table().get::<IncomingValue>(&self_)?.body.clone();
        let bytes = body.write().unwrap().drain(..).collect();
        Ok(Ok(bytes))
    }

    async fn incoming_value_consume_async(
        &mut self,
        self_: Resource<IncomingValue>,
    ) -> anyhow::Result<Result<Resource<IncomingValueAsyncBody>, Error>> {
        let body = self.table().get::<IncomingValue>(&self_)?.body.clone();
        let input_stream: InputStream =
            InputStream::Host(Box::new(IncomingValueAsyncBodyEntry::new(body)));
        let incoming_value_async_body = self.table_mut().push(input_stream)?;
        Ok(Ok(incoming_value_async_body))
    }

    async fn size(&mut self, self_: Resource<IncomingValue>) -> anyhow::Result<u64> {
        let body = self.table().get::<IncomingValue>(&self_)?.body.clone();
        let size = body.read().unwrap().len();
        Ok(size as u64)
    }

    fn drop(&mut self, rep: Resource<IncomingValue>) -> anyhow::Result<()> {
        self.table_mut().delete::<IncomingValue>(rep)?;
        Ok(())
    }
}

#[async_trait]
impl HostOutgoingValue for Context {
    async fn new_outgoing_value(&mut self) -> anyhow::Result<Resource<OutgoingValueEntry>> {
        let outgoing_value = self.table_mut().push(OutgoingValueEntry::new())?;
        Ok(outgoing_value)
    }

    async fn outgoing_value_write_body(
        &mut self,
        self_: Resource<OutgoingValueEntry>,
    ) -> anyhow::Result<Result<Resource<OutgoingValueBodyAsync>, ()>> {
        let body = self.table().get::<OutgoingValueEntry>(&self_)?.body.clone();
        let output_stream: Box<dyn HostOutputStream> =
            Box::new(OutgoingValueBodyAsyncEntry::new(body));
        let outgoing_value_async_body = self.table_mut().push(output_stream)?;
        Ok(Ok(outgoing_value_async_body))
    }

    fn drop(&mut self, rep: Resource<OutgoingValueEntry>) -> anyhow::Result<()> {
        self.table_mut().delete::<OutgoingValueEntry>(rep)?;
        Ok(())
    }
}

#[async_trait]
impl Host for Context {}

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

struct OutgoingValueBodyAsyncEntry {
    body: Arc<RwLock<Vec<u8>>>,
}

impl OutgoingValueBodyAsyncEntry {
    pub fn new(body: Arc<RwLock<Vec<u8>>>) -> OutgoingValueBodyAsyncEntry {
        OutgoingValueBodyAsyncEntry { body }
    }
}

#[async_trait]
impl Subscribe for OutgoingValueBodyAsyncEntry {
    async fn ready(&mut self) {}
}

#[async_trait]
impl HostOutputStream for OutgoingValueBodyAsyncEntry {
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
    #[allow(unused)]
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

struct IncomingValueAsyncBodyEntry {
    body: Arc<RwLock<Vec<u8>>>,
}

impl IncomingValueAsyncBodyEntry {
    pub fn new(body: Arc<RwLock<Vec<u8>>>) -> IncomingValueAsyncBodyEntry {
        IncomingValueAsyncBodyEntry { body }
    }
}

#[async_trait]
impl Subscribe for IncomingValueAsyncBodyEntry {
    async fn ready(&mut self) {}
}

#[async_trait]
impl HostInputStream for IncomingValueAsyncBodyEntry {
    fn read(&mut self, size: usize) -> StreamResult<Bytes> {
        let mut buf = vec![0u8; size];
        let mut body = self.body.write().unwrap();
        let size = std::cmp::min(buf.len(), body.len());
        buf[..size].copy_from_slice(&body[..size]);
        body.drain(..size);
        Ok(buf.into())
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
