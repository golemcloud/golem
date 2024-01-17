use async_trait::async_trait;
use wasmtime::component::Resource;

use crate::golem_host::GolemCtx;
use crate::metrics::wasm::record_host_function_call;
use crate::workerctx::WorkerCtx;
use wasmtime_wasi::preview2::bindings::wasi::sockets::ip_name_lookup::{
    Host, HostResolveAddressStream, IpAddress, Network, Pollable, ResolveAddressStream,
};
use wasmtime_wasi::preview2::SocketError;

#[async_trait]
impl<Ctx: WorkerCtx> HostResolveAddressStream for GolemCtx<Ctx> {
    fn resolve_next_address(
        &mut self,
        self_: Resource<ResolveAddressStream>,
    ) -> Result<Option<IpAddress>, SocketError> {
        record_host_function_call(
            "sockets::ip_name_lookup::resolve_address_stream",
            "resolve_next_address",
        );
        HostResolveAddressStream::resolve_next_address(&mut self.as_wasi_view(), self_)
    }

    fn subscribe(
        &mut self,
        self_: Resource<ResolveAddressStream>,
    ) -> anyhow::Result<Resource<Pollable>> {
        record_host_function_call(
            "sockets::ip_name_lookup::resolve_address_stream",
            "subscribe",
        );
        HostResolveAddressStream::subscribe(&mut self.as_wasi_view(), self_)
    }

    fn drop(&mut self, rep: Resource<ResolveAddressStream>) -> anyhow::Result<()> {
        record_host_function_call("sockets::ip_name_lookup::resolve_address_stream", "drop");
        HostResolveAddressStream::drop(&mut self.as_wasi_view(), rep)
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> Host for GolemCtx<Ctx> {
    fn resolve_addresses(
        &mut self,
        network: Resource<Network>,
        name: String,
    ) -> Result<Resource<ResolveAddressStream>, SocketError> {
        record_host_function_call("sockets::ip_name_lookup", "resolve_addresses");
        Host::resolve_addresses(&mut self.as_wasi_view(), network, name)
    }
}
