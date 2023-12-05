use async_trait::async_trait;
use wasmtime::component::Resource;

use crate::context::Context;
use crate::preview2::wasi::keyvalue::wasi_cloud_error::{Error, Host};

#[async_trait]
impl Host for Context {
    async fn drop_error(&mut self, error: Error) -> anyhow::Result<()> {
        let error = Resource::new_own(error);
        self.table_mut().delete::<ErrorEntry>(error)?;
        Ok(())
    }

    async fn trace(&mut self, error: Error) -> anyhow::Result<String> {
        let trace = self.table().get::<ErrorEntry>(error)?.trace.clone();
        Ok(trace)
    }
}

pub struct ErrorEntry {
    trace: String,
}

impl ErrorEntry {
    pub fn new(trace: String) -> Self {
        Self { trace }
    }
}
