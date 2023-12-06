use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use tonic::codegen::Bytes;
use wasmtime::component::Resource;
use wasmtime_wasi::preview2::{HostInputStream, HostOutputStream, StreamResult, Subscribe};

use crate::context::Context;
use crate::preview2::wasi::keyvalue::types::{
    Error, Host, HostBucket, HostIncomingValue, HostOutgoingValue, IncomingValue,
    IncomingValueAsyncBody, IncomingValueSyncBody, OutgoingValueBodyAsync, OutgoingValueBodySync,
};
use crate::preview2::{InputStream, OutputStream};

#[async_trait]
impl HostBucket for Context {
    async fn open_bucket(
        &mut self,
        name: String,
    ) -> anyhow::Result<Result<Resource<BucketEntry>, Resource<Error>>> {
        let bucket = self.table_mut().push(BucketEntry::new(name))?;
        Ok(Ok(bucket))
    }

    fn drop(&mut self, rep: Resource<BucketEntry>) -> anyhow::Result<()> {
        self.table_mut().delete::<BucketEntry>(rep)?;
        Ok(())
    }
}

#[async_trait]
impl HostIncomingValue for Context {
    async fn incoming_value_consume_sync(
        &mut self,
        self_: Resource<IncomingValue>,
    ) -> anyhow::Result<Result<IncomingValueSyncBody, Resource<Error>>> {
        let body = self.table().get::<IncomingValueEntry>(&self_)?.body.clone();
        let value = body.write().unwrap().drain(..).collect();
        Ok(Ok(value))
    }

    async fn incoming_value_consume_async(
        &mut self,
        self_: Resource<IncomingValue>,
    ) -> anyhow::Result<Result<Resource<IncomingValueAsyncBody>, Resource<Error>>> {
        let body = self.table().get::<IncomingValueEntry>(&self_)?.body.clone();
        let input_stream: InputStream =
            InputStream::Host(Box::new(IncomingValueAsyncBodyEntry::new(body)));
        let incoming_value_async_body = self.table_mut().push(input_stream)?;
        Ok(Ok(incoming_value_async_body))
    }

    async fn size(&mut self, self_: Resource<IncomingValue>) -> anyhow::Result<u64> {
        let body = self.table().get::<IncomingValue>(&self_)?.body.clone();
        let size = body.read().unwrap().len() as u64;
        Ok(size)
    }

    fn drop(&mut self, rep: Resource<IncomingValue>) -> anyhow::Result<()> {
        self.table_mut().delete::<IncomingValueEntry>(rep)?;
        Ok(())
    }
}

#[async_trait]
impl HostOutgoingValue for Context {
    async fn new_outgoing_value(
        &mut self,
        _self_: Resource<OutgoingValueEntry>,
    ) -> anyhow::Result<Resource<OutgoingValueEntry>> {
        let outgoing_value = self.table_mut().push(OutgoingValueEntry::new())?;
        Ok(outgoing_value)
    }

    async fn outgoing_value_write_body_async(
        &mut self,
        self_: Resource<OutgoingValueEntry>,
    ) -> anyhow::Result<Result<Resource<OutgoingValueBodyAsync>, Resource<Error>>> {
        let body = self.table().get::<OutgoingValueEntry>(&self_)?.body.clone();
        let output_stream: OutputStream = Box::new(OutgoingValueBodyAsyncEntry::new(body));
        let outgoing_value_async_body = self.table_mut().push(output_stream)?;
        Ok(Ok(outgoing_value_async_body))
    }

    async fn outgoing_value_write_body_sync(
        &mut self,
        self_: Resource<OutgoingValueEntry>,
        value: OutgoingValueBodySync,
    ) -> anyhow::Result<Result<(), Resource<Error>>> {
        let body = self.table().get::<OutgoingValueEntry>(&self_)?.body.clone();
        body.write().unwrap().extend_from_slice(&value);
        Ok(Ok(()))
    }

    fn drop(&mut self, rep: Resource<OutgoingValueEntry>) -> anyhow::Result<()> {
        self.table_mut().delete::<OutgoingValueEntry>(rep)?;
        Ok(())
    }
}

#[async_trait]
impl Host for Context {}

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
    body: Arc<RwLock<Vec<u8>>>,
}

impl IncomingValueEntry {
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
        let size = std::cmp::min(size, body.len());
        buf[..size].copy_from_slice(&body[..size]);
        body.drain(..size);
        Ok(buf.into())
    }
}
