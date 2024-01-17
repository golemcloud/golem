use wasmtime::component::Resource;

use crate::golem_host::GolemCtx;
use crate::metrics::wasm::record_host_function_call;
use wasmtime_wasi::preview2::bindings::wasi::cli::stdout::{Host, OutputStream};
use crate::workerctx::WorkerCtx;

impl<Ctx: WorkerCtx> Host for GolemCtx<Ctx> {
    fn get_stdout(&mut self) -> anyhow::Result<Resource<OutputStream>> {
        record_host_function_call("cli::stdout", "get_stdout");
        self.as_wasi_view().get_stdout()
    }
}
