use async_trait::async_trait;
use wasmtime::component::Resource;

use crate::context::Context;
use crate::preview2::wasi::keyvalue::wasi_cloud_error::{Error, Host, HostError};

#[async_trait]
impl HostError for Context {
    async fn trace(&mut self, self_: Resource<Error>) -> anyhow::Result<String> {
        let trace = self.table().get::<ErrorEntry>(&self_)?.trace.clone();
        Ok(trace)
    }

    fn drop(&mut self, rep: Resource<Error>) -> anyhow::Result<()> {
        self.table_mut().delete::<ErrorEntry>(rep)?;
        Ok(())
    }
}

#[async_trait]
impl Host for Context {}

pub struct ErrorEntry {
    trace: String,
}

impl ErrorEntry {
    pub fn new(trace: String) -> Self {
        Self { trace }
    }
}
