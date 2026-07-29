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

use crate::durable_host::p3::{
    DurableP3, DurableP3View, observe_function_call, run_read_access, wasi_sockets_view,
};
use crate::workerctx::WorkerCtx;
use golem_common::model::oplog::host_functions::{
    P3SocketsTypesUdpSocketReceive, P3SocketsTypesUdpSocketSend,
};
use golem_common::model::oplog::types::SerializableP3UdpDatagram;
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestNoInput, HostRequestP3SocketsUdpSend,
    HostResponseP3SocketsUdpReceive, HostResponseP3SocketsUdpSend,
};
use wasmtime::component::{Accessor, Resource};
use wasmtime_wasi::p3::bindings::sockets::types;
use wasmtime_wasi::p3::sockets::{SocketError, SocketResult};
use wasmtime_wasi::sockets::{UdpSocket, WasiSockets, WasiSocketsView};

use super::serialize_socket_error;

impl<Ctx: WorkerCtx> types::HostUdpSocket for DurableP3View<'_, Ctx> {
    async fn bind(
        &mut self,
        socket: Resource<UdpSocket>,
        local_address: types::IpSocketAddress,
    ) -> SocketResult<()> {
        observe_function_call(&*self.0, "sockets::types::udp-socket", "bind");
        let mut view = WasiSocketsView::sockets(self.0);
        types::HostUdpSocket::bind(&mut view, socket, local_address).await
    }

    async fn connect(
        &mut self,
        socket: Resource<UdpSocket>,
        remote_address: types::IpSocketAddress,
    ) -> SocketResult<()> {
        observe_function_call(&*self.0, "sockets::types::udp-socket", "connect");
        let mut view = WasiSocketsView::sockets(self.0);
        types::HostUdpSocket::connect(&mut view, socket, remote_address).await
    }

    fn create(
        &mut self,
        address_family: types::IpAddressFamily,
    ) -> SocketResult<Resource<UdpSocket>> {
        observe_function_call(&*self.0, "sockets::types::udp-socket", "create");
        types::HostUdpSocket::create(&mut WasiSocketsView::sockets(self.0), address_family)
    }

    fn disconnect(&mut self, socket: Resource<UdpSocket>) -> SocketResult<()> {
        observe_function_call(&*self.0, "sockets::types::udp-socket", "disconnect");
        types::HostUdpSocket::disconnect(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn get_local_address(
        &mut self,
        socket: Resource<UdpSocket>,
    ) -> SocketResult<types::IpSocketAddress> {
        observe_function_call(&*self.0, "sockets::types::udp-socket", "get-local-address");
        types::HostUdpSocket::get_local_address(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn get_remote_address(
        &mut self,
        socket: Resource<UdpSocket>,
    ) -> SocketResult<types::IpSocketAddress> {
        observe_function_call(&*self.0, "sockets::types::udp-socket", "get-remote-address");
        types::HostUdpSocket::get_remote_address(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn get_address_family(
        &mut self,
        socket: Resource<UdpSocket>,
    ) -> wasmtime::Result<types::IpAddressFamily> {
        observe_function_call(&*self.0, "sockets::types::udp-socket", "get-address-family");
        types::HostUdpSocket::get_address_family(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn get_unicast_hop_limit(&mut self, socket: Resource<UdpSocket>) -> SocketResult<u8> {
        observe_function_call(
            &*self.0,
            "sockets::types::udp-socket",
            "get-unicast-hop-limit",
        );
        types::HostUdpSocket::get_unicast_hop_limit(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn set_unicast_hop_limit(
        &mut self,
        socket: Resource<UdpSocket>,
        value: u8,
    ) -> SocketResult<()> {
        observe_function_call(
            &*self.0,
            "sockets::types::udp-socket",
            "set-unicast-hop-limit",
        );
        types::HostUdpSocket::set_unicast_hop_limit(
            &mut WasiSocketsView::sockets(self.0),
            socket,
            value,
        )
    }

    fn get_receive_buffer_size(&mut self, socket: Resource<UdpSocket>) -> SocketResult<u64> {
        observe_function_call(
            &*self.0,
            "sockets::types::udp-socket",
            "get-receive-buffer-size",
        );
        types::HostUdpSocket::get_receive_buffer_size(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn set_receive_buffer_size(
        &mut self,
        socket: Resource<UdpSocket>,
        value: u64,
    ) -> SocketResult<()> {
        observe_function_call(
            &*self.0,
            "sockets::types::udp-socket",
            "set-receive-buffer-size",
        );
        types::HostUdpSocket::set_receive_buffer_size(
            &mut WasiSocketsView::sockets(self.0),
            socket,
            value,
        )
    }

    fn get_send_buffer_size(&mut self, socket: Resource<UdpSocket>) -> SocketResult<u64> {
        observe_function_call(
            &*self.0,
            "sockets::types::udp-socket",
            "get-send-buffer-size",
        );
        types::HostUdpSocket::get_send_buffer_size(&mut WasiSocketsView::sockets(self.0), socket)
    }

    fn set_send_buffer_size(
        &mut self,
        socket: Resource<UdpSocket>,
        value: u64,
    ) -> SocketResult<()> {
        observe_function_call(
            &*self.0,
            "sockets::types::udp-socket",
            "set-send-buffer-size",
        );
        types::HostUdpSocket::set_send_buffer_size(
            &mut WasiSocketsView::sockets(self.0),
            socket,
            value,
        )
    }

    fn drop(&mut self, sock: Resource<UdpSocket>) -> wasmtime::Result<()> {
        observe_function_call(&*self.0, "sockets::types::udp-socket", "drop");
        types::HostUdpSocket::drop(&mut WasiSocketsView::sockets(self.0), sock)
    }
}

impl<U: Send + 'static, Ctx: WorkerCtx> types::HostUdpSocketWithStore<U> for DurableP3<Ctx> {
    async fn send(
        store: &Accessor<U, Self>,
        socket: Resource<UdpSocket>,
        data: Vec<u8>,
        remote_address: Option<types::IpSocketAddress>,
    ) -> SocketResult<()> {
        let response = run_read_access::<_, _, Ctx, P3SocketsTypesUdpSocketSend, _, _>(
            store,
            HostRequestP3SocketsUdpSend {
                data: data.clone(),
                remote_address: remote_address.map(Into::into),
            },
            DurableFunctionType::WriteRemoteBatched(None),
            || async {
                let sockets = store.with_getter::<WasiSockets>(wasi_sockets_view::<Ctx, U>);
                let result = <WasiSockets as types::HostUdpSocketWithStore<U>>::send(
                    &sockets,
                    socket,
                    data,
                    remote_address,
                )
                .await;

                Ok(HostResponseP3SocketsUdpSend {
                    result: match result {
                        Ok(()) => Ok(()),
                        Err(error) => Err(serialize_socket_error(error)?),
                    },
                })
            },
        )
        .await
        .map_err(SocketError::trap)?;

        match response.result {
            Ok(()) => Ok(()),
            Err(error) => Err(types::ErrorCode::from(error).into()),
        }
    }

    async fn receive(
        store: &Accessor<U, Self>,
        socket: Resource<UdpSocket>,
    ) -> SocketResult<(Vec<u8>, types::IpSocketAddress)> {
        let response = run_read_access::<_, _, Ctx, P3SocketsTypesUdpSocketReceive, _, _>(
            store,
            HostRequestNoInput {},
            DurableFunctionType::ReadRemote,
            || async {
                let sockets = store.with_getter::<WasiSockets>(wasi_sockets_view::<Ctx, U>);
                let result =
                    <WasiSockets as types::HostUdpSocketWithStore<U>>::receive(&sockets, socket)
                        .await;

                Ok(HostResponseP3SocketsUdpReceive {
                    result: match result {
                        Ok((data, remote_address)) => Ok(SerializableP3UdpDatagram {
                            data,
                            remote_address: remote_address.into(),
                        }),
                        Err(error) => Err(serialize_socket_error(error)?),
                    },
                })
            },
        )
        .await
        .map_err(SocketError::trap)?;

        match response.result {
            Ok(SerializableP3UdpDatagram {
                data,
                remote_address,
            }) => Ok((data, remote_address.into())),
            Err(error) => Err(types::ErrorCode::from(error).into()),
        }
    }
}
