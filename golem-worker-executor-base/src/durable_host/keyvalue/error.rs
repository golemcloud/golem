use async_trait::async_trait;
use wasmtime::component::Resource;
use wasmtime_wasi::preview2::WasiView;

use crate::durable_host::DurableWorkerCtx;
use crate::metrics::wasm::record_host_function_call;
use crate::preview2::wasi::keyvalue::wasi_cloud_error::{Error, Host, HostError};
use crate::workerctx::WorkerCtx;

#[async_trait]
impl<Ctx: WorkerCtx> HostError for DurableWorkerCtx<Ctx> {
    async fn trace(&mut self, self_: Resource<Error>) -> anyhow::Result<String> {
        record_host_function_call("keyvalue::wasi_cloud_error", "trace");
        let trace = self
            .as_wasi_view()
            .table()
            .get::<ErrorEntry>(&self_)?
            .trace
            .clone();
        Ok(trace)
    }

    fn drop(&mut self, rep: Resource<Error>) -> anyhow::Result<()> {
        record_host_function_call("keyvalue::wasi_cloud_error", "drop_error");
        self.as_wasi_view().table_mut().delete::<ErrorEntry>(rep)?;
        Ok(())
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {}

pub struct ErrorEntry {
    trace: String,
}

impl ErrorEntry {
    pub fn new(trace: String) -> Self {
        Self { trace }
    }
}
