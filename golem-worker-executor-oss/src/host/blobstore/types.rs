use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use tonic::codegen::Bytes;
use wasmtime_wasi::preview2::{HostInputStream, HostOutputStream, StreamState, TableStreamExt};

use crate::context::Context;
use crate::preview2::wasi::blobstore::types::{
    Error, Host, IncomingValue, IncomingValueAsyncBody, IncomingValueSyncBody, OutgoingValue,
    OutputStream as OutgoingValueBodyAsync,
};

#[async_trait]
impl Host for Context {
    async fn drop_outgoing_value(&mut self, outgoing_value: OutgoingValue) -> anyhow::Result<()> {
        self.table_mut()
            .delete::<OutgoingValueEntry>(outgoing_value)?;
        Ok(())
    }

    async fn new_outgoing_value(&mut self) -> anyhow::Result<OutgoingValue> {
        let outgoing_value = self.table_mut().push(Box::new(OutgoingValueEntry::new()))?;
        Ok(outgoing_value)
    }

    async fn outgoing_value_write_body(
        &mut self,
        outgoing_value: OutgoingValue,
    ) -> anyhow::Result<Result<OutgoingValueBodyAsync, ()>> {
        let body = self
            .table()
            .get::<OutgoingValueEntry>(outgoing_value)?
            .body
            .clone();
        let outgoing_value_async_body = self
            .table_mut()
            .push_output_stream(Box::new(OutgoingValueBodyAsyncEntry::new(body)))?;
        Ok(Ok(outgoing_value_async_body))
    }

    async fn drop_incoming_value(&mut self, incoming_value: IncomingValue) -> anyhow::Result<()> {
        self.table_mut()
            .delete::<IncomingValueEntry>(incoming_value)?;
        Ok(())
    }

    async fn incoming_value_consume_sync(
        &mut self,
        incoming_value: IncomingValue,
    ) -> anyhow::Result<Result<IncomingValueSyncBody, Error>> {
        let body = self
            .table()
            .get::<IncomingValueEntry>(incoming_value)?
            .body
            .clone();
        let value = body.write().unwrap().drain(..).collect();
        Ok(Ok(value))
    }

    async fn incoming_value_consume_async(
        &mut self,
        incoming_value: IncomingValue,
    ) -> anyhow::Result<Result<IncomingValueAsyncBody, Error>> {
        let body = self
            .table()
            .get::<IncomingValueEntry>(incoming_value)?
            .body
            .clone();
        let incoming_value_async_body = self
            .table_mut()
            .push_input_stream(Box::new(IncomingValueAsyncBodyEntry::new(body)))?;
        Ok(Ok(incoming_value_async_body))
    }

    async fn size(&mut self, incoming_value: IncomingValue) -> anyhow::Result<u64> {
        let body = self
            .table()
            .get::<OutgoingValueEntry>(incoming_value)?
            .body
            .clone();
        let size = body.read().unwrap().len() as u64;
        Ok(size)
    }
}

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
impl HostOutputStream for OutgoingValueBodyAsyncEntry {
    fn write(&mut self, bytes: Bytes) -> Result<(usize, StreamState), anyhow::Error> {
        self.body.write().unwrap().extend_from_slice(&bytes);
        Ok((bytes.len(), StreamState::Open))
    }

    async fn ready(&mut self) -> Result<(), anyhow::Error> {
        Ok(())
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

struct IncomingValueAsyncBodyEntry {
    body: Arc<RwLock<Vec<u8>>>,
}

impl IncomingValueAsyncBodyEntry {
    pub fn new(body: Arc<RwLock<Vec<u8>>>) -> IncomingValueAsyncBodyEntry {
        IncomingValueAsyncBodyEntry { body }
    }
}

#[async_trait]
impl HostInputStream for IncomingValueAsyncBodyEntry {
    fn read(&mut self, size: usize) -> Result<(Bytes, StreamState), anyhow::Error> {
        let mut buf = vec![0u8; size];
        let mut body = self.body.write().unwrap();
        let size = std::cmp::min(buf.len(), body.len());
        buf[..size].copy_from_slice(&body[..size]);
        body.drain(..size);
        Ok((buf.into(), StreamState::Open))
    }

    async fn ready(&mut self) -> Result<(), anyhow::Error> {
        Ok(())
    }
}
