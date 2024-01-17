use async_trait::async_trait;
use wasmtime::component::Resource;

use crate::golem_host::GolemCtx;
use crate::metrics::wasm::record_host_function_call;
use wasmtime_wasi::preview2::bindings::wasi::sockets::network::{
    ErrorCode, Host, HostNetwork, Network,
};
use wasmtime_wasi::preview2::SocketError;
use crate::workerctx::WorkerCtx;

impl<Ctx: WorkerCtx> HostNetwork for GolemCtx<Ctx> {
    fn drop(&mut self, rep: Resource<Network>) -> anyhow::Result<()> {
        record_host_function_call("sockets::network", "drop_network");
        HostNetwork::drop(&mut self.as_wasi_view(), rep)
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> Host for GolemCtx<Ctx> {
    fn convert_error_code(&mut self, err: SocketError) -> anyhow::Result<ErrorCode> {
        record_host_function_call("sockets::network", "convert_error_code");
        Host::convert_error_code(&mut self.as_wasi_view(), err)
    }
}
