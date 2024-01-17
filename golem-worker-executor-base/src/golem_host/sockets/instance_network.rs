use async_trait::async_trait;
use wasmtime::component::Resource;

use crate::golem_host::GolemCtx;
use crate::metrics::wasm::record_host_function_call;
use wasmtime_wasi::preview2::bindings::wasi::sockets::instance_network::{Host, Network};
use crate::workerctx::WorkerCtx;

#[async_trait]
impl<Ctx: WorkerCtx> Host for GolemCtx<Ctx> {
    fn instance_network(&mut self) -> anyhow::Result<Resource<Network>> {
        record_host_function_call("sockets::instance_network", "instance_network");
        Host::instance_network(&mut self.as_wasi_view())
    }
}
