use async_trait::async_trait;

use crate::golem_host::GolemCtx;
use crate::metrics::wasm::record_host_function_call;
use wasmtime_wasi::preview2::bindings::wasi::cli::exit::Host;
use crate::workerctx::WorkerCtx;

#[async_trait]
impl<Ctx: WorkerCtx> Host for GolemCtx<Ctx> {
    fn exit(&mut self, status: Result<(), ()>) -> anyhow::Result<()> {
        record_host_function_call("cli::exit", "exit");
        Host::exit(&mut self.as_wasi_view(), status)
    }
}
