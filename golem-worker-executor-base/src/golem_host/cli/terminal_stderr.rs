use async_trait::async_trait;
use wasmtime::component::Resource;

use crate::golem_host::GolemCtx;
use crate::metrics::wasm::record_host_function_call;
use wasmtime_wasi::preview2::bindings::wasi::cli::terminal_stderr::{Host, TerminalOutput};
use crate::workerctx::WorkerCtx;

#[async_trait]
impl<Ctx: WorkerCtx> Host for GolemCtx<Ctx> {
    fn get_terminal_stderr(&mut self) -> anyhow::Result<Option<Resource<TerminalOutput>>> {
        record_host_function_call("cli::terminal_stderr", "get_terminal_stderr");
        self.as_wasi_view().get_terminal_stderr()
    }
}
