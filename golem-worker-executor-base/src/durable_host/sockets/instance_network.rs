use async_trait::async_trait;
use wasmtime::component::Resource;

use crate::durable_host::DurableWorkerCtx;
use crate::metrics::wasm::record_host_function_call;
use crate::workerctx::WorkerCtx;
use wasmtime_wasi::preview2::bindings::wasi::sockets::instance_network::{Host, Network};

#[async_trait]
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    fn instance_network(&mut self) -> anyhow::Result<Resource<Network>> {
        record_host_function_call("sockets::instance_network", "instance_network");
        Host::instance_network(&mut self.as_wasi_view())
    }
}
