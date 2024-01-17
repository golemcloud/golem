use wasmtime::component::Resource;

use crate::durable_host::DurableWorkerCtx;
use crate::metrics::wasm::record_host_function_call;
use crate::workerctx::WorkerCtx;
use wasmtime_wasi::preview2::bindings::wasi::cli::stdout::{Host, OutputStream};

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    fn get_stdout(&mut self) -> anyhow::Result<Resource<OutputStream>> {
        record_host_function_call("cli::stdout", "get_stdout");
        self.as_wasi_view().get_stdout()
    }
}
