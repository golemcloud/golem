use async_trait::async_trait;
use wasmtime::component::Resource;

use crate::durable_host::DurableWorkerCtx;
use crate::metrics::wasm::record_host_function_call;
use crate::workerctx::WorkerCtx;
use wasmtime_wasi::preview2::bindings::wasi::cli::terminal_stdin::{Host, TerminalInput};

#[async_trait]
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    fn get_terminal_stdin(&mut self) -> anyhow::Result<Option<Resource<TerminalInput>>> {
        record_host_function_call("cli::terminal_stdin", "get_terminal_stdin");
        self.as_wasi_view().get_terminal_stdin()
    }
}
