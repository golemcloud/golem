use async_trait::async_trait;
use wasmtime::component::Resource;

use crate::golem_host::GolemCtx;
use crate::metrics::wasm::record_host_function_call;
use crate::workerctx::WorkerCtx;
use wasmtime_wasi::preview2::bindings::wasi::filesystem::preopens::{Descriptor, Host};

#[async_trait]
impl<Ctx: WorkerCtx> Host for GolemCtx<Ctx> {
    fn get_directories(&mut self) -> anyhow::Result<Vec<(Resource<Descriptor>, String)>> {
        record_host_function_call("cli_base::preopens", "get_directories");
        Host::get_directories(&mut self.as_wasi_view())
    }
}
