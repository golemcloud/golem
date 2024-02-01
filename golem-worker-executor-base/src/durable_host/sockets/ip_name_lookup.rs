use async_trait::async_trait;
use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};
use wasmtime::component::Resource;

use crate::durable_host::{Durability, DurableWorkerCtx, SerializableError};
use crate::metrics::wasm::record_host_function_call;
use crate::workerctx::WorkerCtx;
use golem_common::model::WrappedFunctionType;
use wasmtime_wasi::preview2::bindings::sockets::network::ErrorCode;
use wasmtime_wasi::preview2::bindings::wasi::sockets::ip_name_lookup::{
    Host, HostResolveAddressStream, IpAddress, Network, Pollable, ResolveAddressStream,
};
use wasmtime_wasi::preview2::{SocketError, Subscribe};

#[async_trait]
impl<Ctx: WorkerCtx> HostResolveAddressStream for DurableWorkerCtx<Ctx> {
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
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn resolve_addresses(
        &mut self,
        network: Resource<Network>,
        name: String,
    ) -> Result<Resource<ResolveAddressStream>, SocketError> {
        record_host_function_call("sockets::ip_name_lookup", "resolve_addresses");

        let addresses: Result<Vec<IpAddress>, SocketError> =
            Durability::<Ctx, SerializableIpAddresses, SerializableError>::wrap(
                self,
                WrappedFunctionType::ReadRemote,
                "sockets::ip_name_lookup::resolve_addresses",
                |ctx| {
                    Box::pin(async move { resolve_and_drain_addresses(ctx, network, name).await })
                },
            )
            .await;

        let stream = ResolveAddressStream::Done(Ok(addresses?.into_iter()));
        Ok(self.table.push(stream)?)
    }
}

async fn resolve_and_drain_addresses<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    network: Resource<Network>,
    name: String,
) -> Result<Vec<IpAddress>, SocketError> {
    let stream = Host::resolve_addresses(&mut ctx.as_wasi_view(), network, name).await?;
    let stream = ctx.table.delete(stream)?;
    let addresses = drain_resolve_address_stream(stream).await?;
    Ok(addresses)
}

async fn drain_resolve_address_stream(
    mut stream: ResolveAddressStream,
) -> Result<Vec<IpAddress>, SocketError> {
    let mut addresses = Vec::new();

    stream.ready().await;
    match stream {
        ResolveAddressStream::Waiting(_) => return Err(ErrorCode::WouldBlock.into()), // should never happen because of ready() above
        ResolveAddressStream::Done(iter) => {
            let iter = iter?;
            for address in iter {
                addresses.push(address);
            }
        }
    }
    Ok(addresses)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode)]
enum SerializableIpAddress {
    IPv4 { address: [u8; 4] },
    IPv6 { address: [u16; 8] },
}

impl From<IpAddress> for SerializableIpAddress {
    fn from(value: IpAddress) -> Self {
        match value {
            IpAddress::Ipv4(address) => SerializableIpAddress::IPv4 {
                address: [address.0, address.1, address.2, address.3],
            },
            IpAddress::Ipv6(address) => SerializableIpAddress::IPv6 {
                address: [
                    address.0, address.1, address.2, address.3, address.4, address.5, address.6,
                    address.7,
                ],
            },
        }
    }
}

impl From<SerializableIpAddress> for IpAddress {
    fn from(value: SerializableIpAddress) -> Self {
        match value {
            SerializableIpAddress::IPv4 { address } => {
                IpAddress::Ipv4((address[0], address[1], address[2], address[3]))
            }
            SerializableIpAddress::IPv6 { address } => IpAddress::Ipv6((
                address[0], address[1], address[2], address[3], address[4], address[5], address[6],
                address[7],
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode)]
struct SerializableIpAddresses(Vec<SerializableIpAddress>);

impl From<Vec<IpAddress>> for SerializableIpAddresses {
    fn from(value: Vec<IpAddress>) -> Self {
        SerializableIpAddresses(value.into_iter().map(|v| v.into()).collect())
    }
}

impl From<SerializableIpAddresses> for Vec<IpAddress> {
    fn from(value: SerializableIpAddresses) -> Self {
        value.0.into_iter().map(|v| v.into()).collect()
    }
}
