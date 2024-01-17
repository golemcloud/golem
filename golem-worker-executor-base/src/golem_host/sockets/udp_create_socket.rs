use async_trait::async_trait;
use wasmtime::component::Resource;

use crate::golem_host::GolemCtx;
use crate::metrics::wasm::record_host_function_call;
use wasmtime_wasi::preview2::bindings::wasi::sockets::udp_create_socket::{
    Host, IpAddressFamily, UdpSocket,
};
use wasmtime_wasi::preview2::SocketError;
use crate::workerctx::WorkerCtx;

#[async_trait]
impl<Ctx: WorkerCtx> Host for GolemCtx<Ctx> {
    fn create_udp_socket(
        &mut self,
        address_family: IpAddressFamily,
    ) -> Result<Resource<UdpSocket>, SocketError> {
        record_host_function_call("sockets::udp_create_socket", "create_udp_socket");
        Host::create_udp_socket(&mut self.as_wasi_view(), address_family)
    }
}
