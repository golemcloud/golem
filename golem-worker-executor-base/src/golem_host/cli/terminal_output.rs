use async_trait::async_trait;
use wasmtime::component::Resource;

use crate::golem_host::GolemCtx;
use crate::metrics::wasm::record_host_function_call;
use crate::workerctx::WorkerCtx;
use wasmtime_wasi::preview2::bindings::wasi::cli::terminal_output::{
    Host, HostTerminalOutput, TerminalOutput,
};

#[async_trait]
impl<Ctx: WorkerCtx> HostTerminalOutput for GolemCtx<Ctx> {
    fn drop(&mut self, rep: Resource<TerminalOutput>) -> anyhow::Result<()> {
        record_host_function_call("cli::terminal_output::terminal_output", "drop");
        HostTerminalOutput::drop(&mut self.as_wasi_view(), rep)
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> Host for GolemCtx<Ctx> {}
