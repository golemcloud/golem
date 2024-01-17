use async_trait::async_trait;
use wasmtime::component::Resource;

use crate::durable_host::DurableWorkerCtx;
use crate::metrics::wasm::record_host_function_call;
use crate::workerctx::WorkerCtx;
use wasmtime_wasi::preview2::bindings::wasi::cli::terminal_output::{
    Host, HostTerminalOutput, TerminalOutput,
};

#[async_trait]
impl<Ctx: WorkerCtx> HostTerminalOutput for DurableWorkerCtx<Ctx> {
    fn drop(&mut self, rep: Resource<TerminalOutput>) -> anyhow::Result<()> {
        record_host_function_call("cli::terminal_output::terminal_output", "drop");
        HostTerminalOutput::drop(&mut self.as_wasi_view(), rep)
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {}
