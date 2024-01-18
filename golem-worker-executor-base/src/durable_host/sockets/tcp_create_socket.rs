use async_trait::async_trait;
use wasmtime::component::Resource;

use crate::durable_host::DurableWorkerCtx;
use crate::metrics::wasm::record_host_function_call;
use crate::workerctx::WorkerCtx;
use wasmtime_wasi::preview2::bindings::wasi::sockets::tcp_create_socket::{
    Host, IpAddressFamily, TcpSocket,
};
use wasmtime_wasi::preview2::SocketError;

#[async_trait]
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    fn create_tcp_socket(
        &mut self,
        address_family: IpAddressFamily,
    ) -> Result<Resource<TcpSocket>, SocketError> {
        record_host_function_call("sockets::tcp_create_socket", "create_tcp_socket");
        Host::create_tcp_socket(&mut self.as_wasi_view(), address_family)
    }
}
