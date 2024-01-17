use crate::golem_host::GolemCtx;
use crate::metrics::wasm::record_host_function_call;
use crate::workerctx::WorkerCtx;
use async_trait::async_trait;
use wasmtime::component::Resource;
use wasmtime_wasi::preview2::bindings::wasi::io::error::{Error, Host, HostError};

#[async_trait]
impl<Ctx: WorkerCtx> HostError for GolemCtx<Ctx> {
    fn to_debug_string(&mut self, self_: Resource<Error>) -> anyhow::Result<String> {
        record_host_function_call("io::error", "to_debug_string");
        HostError::to_debug_string(&mut self.as_wasi_view(), self_)
    }

    fn drop(&mut self, rep: Resource<Error>) -> anyhow::Result<()> {
        record_host_function_call("io::error", "drop");
        HostError::drop(&mut self.as_wasi_view(), rep)
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> Host for GolemCtx<Ctx> {}
