use async_trait::async_trait;
use wasmtime::component::Resource;

use crate::durable_host::DurableWorkerCtx;
use crate::metrics::wasm::record_host_function_call;
use crate::workerctx::WorkerCtx;
use wasmtime_wasi::preview2::bindings::wasi::cli::terminal_input::{
    Host, HostTerminalInput, TerminalInput,
};

#[async_trait]
impl<Ctx: WorkerCtx> HostTerminalInput for DurableWorkerCtx<Ctx> {
    fn drop(&mut self, rep: Resource<TerminalInput>) -> anyhow::Result<()> {
        record_host_function_call("cli::terminal_input::terminal_input", "drop");
        self.as_wasi_view().drop(rep)
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {}
