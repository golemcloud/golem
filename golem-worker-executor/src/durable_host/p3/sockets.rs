// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::durable_host::p3::{DurableP3, DurableP3View, run_read_access, wasi_sockets_view};
use crate::workerctx::WorkerCtx;
use golem_common::model::oplog::host_functions::P3SocketsIpNameLookupResolveAddresses;
use golem_common::model::oplog::types::SerializableP3IpAddresses;
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestP3SocketsResolveName, HostResponseP3SocketsResolveName,
};
use wasmtime::AsContextMut;
use wasmtime::component::{Access, Accessor, FutureReader, Resource, StreamReader};
use wasmtime_wasi::p3::bindings::sockets::{ip_name_lookup, types};
use wasmtime_wasi::p3::sockets::{SocketError, SocketResult};
use wasmtime_wasi::sockets::{TcpSocket, UdpSocket, WasiSockets, WasiSocketsView};

impl<Ctx: WorkerCtx> types::Host for DurableP3View<'_, Ctx> {
    fn convert_error_code(&mut self, error: SocketError) -> wasmtime::Result<types::ErrorCode> {
        types::Host::convert_error_code(&mut WasiSocketsView::sockets(self.0), error)
    }
}

impl<Ctx: WorkerCtx> types::HostTcpSocket for DurableP3View<'_, Ctx> {
    async fn bind(
        &mut self,
        socket: Resource<TcpSocket>,
        local_address: types::IpSocketAddress,
    ) -> SocketResult<()> {
        let mut view = WasiSocketsView::sockets(self.0);
        types::HostTcpSocket::bind(&mut view, socket, local_address).await
    }

    fn create(
        &mut self,
        address_family: types::IpAddressFamily,
    ) -> SocketResult<Resource<TcpSocket>> {
        types::HostTcpSocket::create(&mut WasiSocketsView::sockets(self.0), address_family)
    }

    fn get_local_address(
        &mut self,
        socket: Resource<TcpSocket>,
    ) -> SocketResult<types::IpSocketAddress> {
        types::HostTcpSocket::get_local_address(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn get_remote_address(
        &mut self,
        socket: Resource<TcpSocket>,
    ) -> SocketResult<types::IpSocketAddress> {
        types::HostTcpSocket::get_remote_address(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn get_is_listening(&mut self, socket: Resource<TcpSocket>) -> wasmtime::Result<bool> {
        types::HostTcpSocket::get_is_listening(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn get_address_family(
        &mut self,
        socket: Resource<TcpSocket>,
    ) -> wasmtime::Result<types::IpAddressFamily> {
        types::HostTcpSocket::get_address_family(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn set_listen_backlog_size(
        &mut self,
        socket: Resource<TcpSocket>,
        value: u64,
    ) -> SocketResult<()> {
        types::HostTcpSocket::set_listen_backlog_size(
            &mut WasiSocketsView::sockets(self.0),
            socket,
            value,
        )
    }

    fn get_keep_alive_enabled(&mut self, socket: Resource<TcpSocket>) -> SocketResult<bool> {
        types::HostTcpSocket::get_keep_alive_enabled(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn set_keep_alive_enabled(
        &mut self,
        socket: Resource<TcpSocket>,
        value: bool,
    ) -> SocketResult<()> {
        types::HostTcpSocket::set_keep_alive_enabled(
            &mut WasiSocketsView::sockets(self.0),
            socket,
            value,
        )
    }

    fn get_keep_alive_idle_time(
        &mut self,
        socket: Resource<TcpSocket>,
    ) -> SocketResult<types::Duration> {
        types::HostTcpSocket::get_keep_alive_idle_time(
            &mut WasiSocketsView::sockets(self.0),
            socket,
        )
    }

    fn set_keep_alive_idle_time(
        &mut self,
        socket: Resource<TcpSocket>,
        value: types::Duration,
    ) -> SocketResult<()> {
        types::HostTcpSocket::set_keep_alive_idle_time(
            &mut WasiSocketsView::sockets(self.0),
            socket,
            value,
        )
    }

    fn get_keep_alive_interval(
        &mut self,
        socket: Resource<TcpSocket>,
    ) -> SocketResult<types::Duration> {
        types::HostTcpSocket::get_keep_alive_interval(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn set_keep_alive_interval(
        &mut self,
        socket: Resource<TcpSocket>,
        value: types::Duration,
    ) -> SocketResult<()> {
        types::HostTcpSocket::set_keep_alive_interval(
            &mut WasiSocketsView::sockets(self.0),
            socket,
            value,
        )
    }

    fn get_keep_alive_count(&mut self, socket: Resource<TcpSocket>) -> SocketResult<u32> {
        types::HostTcpSocket::get_keep_alive_count(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn set_keep_alive_count(
        &mut self,
        socket: Resource<TcpSocket>,
        value: u32,
    ) -> SocketResult<()> {
        types::HostTcpSocket::set_keep_alive_count(
            &mut WasiSocketsView::sockets(self.0),
            socket,
            value,
        )
    }

    fn get_hop_limit(&mut self, socket: Resource<TcpSocket>) -> SocketResult<u8> {
        types::HostTcpSocket::get_hop_limit(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn set_hop_limit(&mut self, socket: Resource<TcpSocket>, value: u8) -> SocketResult<()> {
        types::HostTcpSocket::set_hop_limit(&mut WasiSocketsView::sockets(self.0), socket, value)
    }

    fn get_receive_buffer_size(&mut self, socket: Resource<TcpSocket>) -> SocketResult<u64> {
        types::HostTcpSocket::get_receive_buffer_size(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn set_receive_buffer_size(
        &mut self,
        socket: Resource<TcpSocket>,
        value: u64,
    ) -> SocketResult<()> {
        types::HostTcpSocket::set_receive_buffer_size(
            &mut WasiSocketsView::sockets(self.0),
            socket,
            value,
        )
    }

    fn get_send_buffer_size(&mut self, socket: Resource<TcpSocket>) -> SocketResult<u64> {
        types::HostTcpSocket::get_send_buffer_size(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn set_send_buffer_size(
        &mut self,
        socket: Resource<TcpSocket>,
        value: u64,
    ) -> SocketResult<()> {
        types::HostTcpSocket::set_send_buffer_size(
            &mut WasiSocketsView::sockets(self.0),
            socket,
            value,
        )
    }

    fn drop(&mut self, sock: Resource<TcpSocket>) -> wasmtime::Result<()> {
        types::HostTcpSocket::drop(&mut WasiSocketsView::sockets(self.0), sock)
    }
}

impl<Ctx: WorkerCtx> types::HostTcpSocketWithStore for DurableP3<Ctx> {
    async fn connect<U: Send>(
        store: &Accessor<U, Self>,
        socket: Resource<TcpSocket>,
        remote_address: types::IpSocketAddress,
    ) -> SocketResult<()> {
        let store = store.with_getter::<WasiSockets>(wasi_sockets_view::<Ctx, U>);
        <WasiSockets as types::HostTcpSocketWithStore>::connect(&store, socket, remote_address)
            .await
    }

    fn listen<U: 'static>(
        mut store: Access<U, Self>,
        socket: Resource<TcpSocket>,
    ) -> SocketResult<StreamReader<Resource<TcpSocket>>> {
        let store =
            Access::<U, WasiSockets>::new(store.as_context_mut(), wasi_sockets_view::<Ctx, U>);
        <WasiSockets as types::HostTcpSocketWithStore>::listen(store, socket)
    }

    fn send<U: 'static>(
        mut store: Access<U, Self>,
        socket: Resource<TcpSocket>,
        data: StreamReader<u8>,
    ) -> wasmtime::Result<FutureReader<Result<(), types::ErrorCode>>> {
        let store =
            Access::<U, WasiSockets>::new(store.as_context_mut(), wasi_sockets_view::<Ctx, U>);
        <WasiSockets as types::HostTcpSocketWithStore>::send(store, socket, data)
    }

    fn receive<U: 'static>(
        mut store: Access<U, Self>,
        socket: Resource<TcpSocket>,
    ) -> wasmtime::Result<(StreamReader<u8>, FutureReader<Result<(), types::ErrorCode>>)> {
        let store =
            Access::<U, WasiSockets>::new(store.as_context_mut(), wasi_sockets_view::<Ctx, U>);
        <WasiSockets as types::HostTcpSocketWithStore>::receive(store, socket)
    }
}

impl<Ctx: WorkerCtx> types::HostUdpSocket for DurableP3View<'_, Ctx> {
    async fn bind(
        &mut self,
        socket: Resource<UdpSocket>,
        local_address: types::IpSocketAddress,
    ) -> SocketResult<()> {
        let mut view = WasiSocketsView::sockets(self.0);
        types::HostUdpSocket::bind(&mut view, socket, local_address).await
    }

    async fn connect(
        &mut self,
        socket: Resource<UdpSocket>,
        remote_address: types::IpSocketAddress,
    ) -> SocketResult<()> {
        let mut view = WasiSocketsView::sockets(self.0);
        types::HostUdpSocket::connect(&mut view, socket, remote_address).await
    }

    fn create(
        &mut self,
        address_family: types::IpAddressFamily,
    ) -> SocketResult<Resource<UdpSocket>> {
        types::HostUdpSocket::create(&mut WasiSocketsView::sockets(self.0), address_family)
    }

    fn disconnect(&mut self, socket: Resource<UdpSocket>) -> SocketResult<()> {
        types::HostUdpSocket::disconnect(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn get_local_address(
        &mut self,
        socket: Resource<UdpSocket>,
    ) -> SocketResult<types::IpSocketAddress> {
        types::HostUdpSocket::get_local_address(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn get_remote_address(
        &mut self,
        socket: Resource<UdpSocket>,
    ) -> SocketResult<types::IpSocketAddress> {
        types::HostUdpSocket::get_remote_address(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn get_address_family(
        &mut self,
        socket: Resource<UdpSocket>,
    ) -> wasmtime::Result<types::IpAddressFamily> {
        types::HostUdpSocket::get_address_family(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn get_unicast_hop_limit(&mut self, socket: Resource<UdpSocket>) -> SocketResult<u8> {
        types::HostUdpSocket::get_unicast_hop_limit(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn set_unicast_hop_limit(
        &mut self,
        socket: Resource<UdpSocket>,
        value: u8,
    ) -> SocketResult<()> {
        types::HostUdpSocket::set_unicast_hop_limit(
            &mut WasiSocketsView::sockets(self.0),
            socket,
            value,
        )
    }

    fn get_receive_buffer_size(&mut self, socket: Resource<UdpSocket>) -> SocketResult<u64> {
        types::HostUdpSocket::get_receive_buffer_size(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn set_receive_buffer_size(
        &mut self,
        socket: Resource<UdpSocket>,
        value: u64,
    ) -> SocketResult<()> {
        types::HostUdpSocket::set_receive_buffer_size(
            &mut WasiSocketsView::sockets(self.0),
            socket,
            value,
        )
    }

    fn get_send_buffer_size(&mut self, socket: Resource<UdpSocket>) -> SocketResult<u64> {
        types::HostUdpSocket::get_send_buffer_size(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn set_send_buffer_size(
        &mut self,
        socket: Resource<UdpSocket>,
        value: u64,
    ) -> SocketResult<()> {
        types::HostUdpSocket::set_send_buffer_size(
            &mut WasiSocketsView::sockets(self.0),
            socket,
            value,
        )
    }

    fn drop(&mut self, sock: Resource<UdpSocket>) -> wasmtime::Result<()> {
        types::HostUdpSocket::drop(&mut WasiSocketsView::sockets(self.0), sock)
    }
}

impl<Ctx: WorkerCtx> types::HostUdpSocketWithStore for DurableP3<Ctx> {
    async fn send<U: Send>(
        store: &Accessor<U, Self>,
        socket: Resource<UdpSocket>,
        data: Vec<u8>,
        remote_address: Option<types::IpSocketAddress>,
    ) -> SocketResult<()> {
        let store = store.with_getter::<WasiSockets>(wasi_sockets_view::<Ctx, U>);
        <WasiSockets as types::HostUdpSocketWithStore>::send(&store, socket, data, remote_address)
            .await
    }

    async fn receive<U: Send>(
        store: &Accessor<U, Self>,
        socket: Resource<UdpSocket>,
    ) -> SocketResult<(Vec<u8>, types::IpSocketAddress)> {
        let store = store.with_getter::<WasiSockets>(wasi_sockets_view::<Ctx, U>);
        <WasiSockets as types::HostUdpSocketWithStore>::receive(&store, socket).await
    }
}

impl<Ctx: WorkerCtx> ip_name_lookup::Host for DurableP3View<'_, Ctx> {}

impl<Ctx: WorkerCtx> ip_name_lookup::HostWithStore for DurableP3<Ctx> {
    async fn resolve_addresses<U: Send + 'static>(
        store: &Accessor<U, Self>,
        name: String,
    ) -> wasmtime::Result<Result<Vec<types::IpAddress>, ip_name_lookup::ErrorCode>> {
        let response = run_read_access::<_, _, Ctx, P3SocketsIpNameLookupResolveAddresses, _, _>(
            store,
            HostRequestP3SocketsResolveName { name: name.clone() },
            DurableFunctionType::ReadRemote,
            || async {
                let sockets = store.with_getter::<WasiSockets>(wasi_sockets_view::<Ctx, U>);
                let result = <WasiSockets as ip_name_lookup::HostWithStore>::resolve_addresses(
                    &sockets,
                    name.clone(),
                )
                .await?;

                Ok(HostResponseP3SocketsResolveName {
                    result: result
                        .map(SerializableP3IpAddresses::from)
                        .map_err(Into::into),
                })
            },
        )
        .await?;

        Ok(response
            .result
            .map(Vec::<types::IpAddress>::from)
            .map_err(Into::into))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::oplog::types::SerializableP3IpNameLookupError;
    use test_r::test;

    #[test]
    fn p3_ip_name_lookup_address_payload_mapping_roundtrips() {
        let ipv4 = types::IpAddress::Ipv4((127, 0, 0, 1));
        let ipv6 = types::IpAddress::Ipv6((0, 0, 0, 0, 0, 0, 0, 1));

        let serialized = SerializableP3IpAddresses::from(vec![ipv4, ipv6]);
        let replayed = Vec::<types::IpAddress>::from(serialized);

        assert_p3_ip_address_eq(replayed[0], ipv4);
        assert_p3_ip_address_eq(replayed[1], ipv6);
    }

    #[test]
    fn p3_ip_name_lookup_error_payload_mapping_roundtrips_named_codes() {
        assert_p3_ip_name_lookup_error_roundtrip(ip_name_lookup::ErrorCode::AccessDenied);
        assert_p3_ip_name_lookup_error_roundtrip(ip_name_lookup::ErrorCode::InvalidArgument);
        assert_p3_ip_name_lookup_error_roundtrip(ip_name_lookup::ErrorCode::NameUnresolvable);
        assert_p3_ip_name_lookup_error_roundtrip(
            ip_name_lookup::ErrorCode::TemporaryResolverFailure,
        );
        assert_p3_ip_name_lookup_error_roundtrip(
            ip_name_lookup::ErrorCode::PermanentResolverFailure,
        );
    }

    #[test]
    fn p3_ip_name_lookup_other_error_payload_mapping_preserves_message() {
        let error = ip_name_lookup::ErrorCode::Other(Some("resolver said no".to_string()));
        let serialized = SerializableP3IpNameLookupError::from(error);
        let replayed = ip_name_lookup::ErrorCode::from(serialized);

        match replayed {
            ip_name_lookup::ErrorCode::Other(Some(message)) => {
                assert_eq!(message, "resolver said no")
            }
            other => panic!("unexpected replayed error: {other:?}"),
        }
    }

    fn assert_p3_ip_name_lookup_error_roundtrip(error: ip_name_lookup::ErrorCode) {
        let expected = format!("{error:?}");
        let serialized = SerializableP3IpNameLookupError::from(error);
        let replayed = ip_name_lookup::ErrorCode::from(serialized);
        assert_eq!(format!("{replayed:?}"), expected);
    }

    fn assert_p3_ip_address_eq(actual: types::IpAddress, expected: types::IpAddress) {
        match (actual, expected) {
            (types::IpAddress::Ipv4(actual), types::IpAddress::Ipv4(expected)) => {
                assert_eq!(actual, expected)
            }
            (types::IpAddress::Ipv6(actual), types::IpAddress::Ipv6(expected)) => {
                assert_eq!(actual, expected)
            }
            (actual, expected) => panic!("IP address mismatch: {actual:?} != {expected:?}"),
        }
    }
}
