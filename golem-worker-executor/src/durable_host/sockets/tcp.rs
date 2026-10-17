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

use wasmtime::component::Resource;

use crate::durable_host::authorization::targets::tcp_target;
use crate::durable_host::{DurabilityHost, DurableCallSession, DurableWorkerCtx, NotCancellable};
use crate::workerctx::WorkerCtx;
use golem_common::SafeDisplay;
use golem_common::model::oplog::host_functions::SocketsTcpConnectPermissionCheck;
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestSocketsTcpConnectPermissionCheck, HostResponsePermissionCheck,
};
use wasmtime_wasi::p2::SocketError;
use wasmtime_wasi::p2::bindings::sockets::network::ErrorCode;
use wasmtime_wasi::p2::bindings::sockets::tcp::{
    Duration, Host, HostTcpSocket, InputStream, IpAddressFamily, IpSocketAddress, Network,
    OutputStream, Pollable, ShutdownType, TcpSocket,
};
use wasmtime_wasi::sockets::WasiSocketsView as _;

impl<Ctx: WorkerCtx> HostTcpSocket for DurableWorkerCtx<Ctx> {
    async fn start_bind(
        &mut self,
        self_: Resource<TcpSocket>,
        network: Resource<Network>,
        local_address: IpSocketAddress,
    ) -> Result<(), SocketError> {
        self.observe_function_call("sockets::tcp", "start_bind");
        let mut view = self.as_wasi_view();
        HostTcpSocket::start_bind(&mut view.sockets(), self_, network, local_address).await
    }

    fn finish_bind(&mut self, self_: Resource<TcpSocket>) -> Result<(), SocketError> {
        self.observe_function_call("sockets::tcp", "finish_bind");
        HostTcpSocket::finish_bind(&mut self.as_wasi_view().sockets(), self_)
    }

    async fn start_connect(
        &mut self,
        self_: Resource<TcpSocket>,
        network: Resource<Network>,
        remote_address: IpSocketAddress,
    ) -> Result<(), SocketError> {
        authorize_connect(self, &remote_address).await?;
        self.observe_function_call("sockets::tcp", "start_connect");
        let mut view = self.as_wasi_view();
        HostTcpSocket::start_connect(&mut view.sockets(), self_, network, remote_address).await
    }

    fn finish_connect(
        &mut self,
        self_: Resource<TcpSocket>,
    ) -> Result<(Resource<InputStream>, Resource<OutputStream>), SocketError> {
        self.observe_function_call("sockets::tcp", "finish_connect");
        HostTcpSocket::finish_connect(&mut self.as_wasi_view().sockets(), self_)
    }

    fn start_listen(&mut self, self_: Resource<TcpSocket>) -> Result<(), SocketError> {
        self.observe_function_call("sockets::tcp", "start_listen");
        HostTcpSocket::start_listen(&mut self.as_wasi_view().sockets(), self_)
    }

    fn finish_listen(&mut self, self_: Resource<TcpSocket>) -> Result<(), SocketError> {
        self.observe_function_call("sockets::tcp", "finish_listen");
        HostTcpSocket::finish_listen(&mut self.as_wasi_view().sockets(), self_)
    }

    fn accept(
        &mut self,
        self_: Resource<TcpSocket>,
    ) -> Result<
        (
            Resource<TcpSocket>,
            Resource<InputStream>,
            Resource<OutputStream>,
        ),
        SocketError,
    > {
        self.observe_function_call("sockets::tcp", "accept");
        HostTcpSocket::accept(&mut self.as_wasi_view().sockets(), self_)
    }

    fn local_address(
        &mut self,
        self_: Resource<TcpSocket>,
    ) -> Result<IpSocketAddress, SocketError> {
        self.observe_function_call("sockets::tcp", "local_address");
        HostTcpSocket::local_address(&mut self.as_wasi_view().sockets(), self_)
    }

    fn remote_address(
        &mut self,
        self_: Resource<TcpSocket>,
    ) -> Result<IpSocketAddress, SocketError> {
        self.observe_function_call("sockets::tcp", "remote_address");
        HostTcpSocket::remote_address(&mut self.as_wasi_view().sockets(), self_)
    }

    fn is_listening(&mut self, self_: Resource<TcpSocket>) -> wasmtime::Result<bool> {
        self.observe_function_call("sockets::tcp", "is_listening");
        HostTcpSocket::is_listening(&mut self.as_wasi_view().sockets(), self_)
    }

    fn address_family(&mut self, self_: Resource<TcpSocket>) -> wasmtime::Result<IpAddressFamily> {
        self.observe_function_call("sockets::tcp", "address_family");
        HostTcpSocket::address_family(&mut self.as_wasi_view().sockets(), self_)
    }

    fn set_listen_backlog_size(
        &mut self,
        self_: Resource<TcpSocket>,
        value: u64,
    ) -> Result<(), SocketError> {
        self.observe_function_call("sockets::tcp", "set_listen_backlog_size");
        HostTcpSocket::set_listen_backlog_size(&mut self.as_wasi_view().sockets(), self_, value)
    }

    fn keep_alive_enabled(&mut self, self_: Resource<TcpSocket>) -> Result<bool, SocketError> {
        self.observe_function_call("sockets::tcp", "keep_alive_enabled");
        HostTcpSocket::keep_alive_enabled(&mut self.as_wasi_view().sockets(), self_)
    }

    fn set_keep_alive_enabled(
        &mut self,
        self_: Resource<TcpSocket>,
        value: bool,
    ) -> Result<(), SocketError> {
        self.observe_function_call("sockets::tcp", "set_keep_alive_enabled");
        HostTcpSocket::set_keep_alive_enabled(&mut self.as_wasi_view().sockets(), self_, value)
    }

    fn keep_alive_idle_time(
        &mut self,
        self_: Resource<TcpSocket>,
    ) -> Result<Duration, SocketError> {
        self.observe_function_call("sockets::tcp", "keep_alive_idle_time");
        HostTcpSocket::keep_alive_idle_time(&mut self.as_wasi_view().sockets(), self_)
    }

    fn set_keep_alive_idle_time(
        &mut self,
        self_: Resource<TcpSocket>,
        value: Duration,
    ) -> Result<(), SocketError> {
        self.observe_function_call("sockets::tcp", "set_keep_alive_idle_time");
        HostTcpSocket::set_keep_alive_idle_time(&mut self.as_wasi_view().sockets(), self_, value)
    }

    fn keep_alive_interval(&mut self, self_: Resource<TcpSocket>) -> Result<Duration, SocketError> {
        self.observe_function_call("sockets::tcp", "keep_alive_interval");
        HostTcpSocket::keep_alive_interval(&mut self.as_wasi_view().sockets(), self_)
    }

    fn set_keep_alive_interval(
        &mut self,
        self_: Resource<TcpSocket>,
        value: Duration,
    ) -> Result<(), SocketError> {
        self.observe_function_call("sockets::tcp", "set_keep_alive_interval");
        HostTcpSocket::set_keep_alive_interval(&mut self.as_wasi_view().sockets(), self_, value)
    }

    fn keep_alive_count(&mut self, self_: Resource<TcpSocket>) -> Result<u32, SocketError> {
        self.observe_function_call("sockets::tcp", "keep_alive_count");
        HostTcpSocket::keep_alive_count(&mut self.as_wasi_view().sockets(), self_)
    }

    fn set_keep_alive_count(
        &mut self,
        self_: Resource<TcpSocket>,
        value: u32,
    ) -> Result<(), SocketError> {
        self.observe_function_call("sockets::tcp", "set_keep_alive_count");
        HostTcpSocket::set_keep_alive_count(&mut self.as_wasi_view().sockets(), self_, value)
    }

    fn hop_limit(&mut self, self_: Resource<TcpSocket>) -> Result<u8, SocketError> {
        self.observe_function_call("sockets::tcp", "hop_limit");
        HostTcpSocket::hop_limit(&mut self.as_wasi_view().sockets(), self_)
    }

    fn set_hop_limit(&mut self, self_: Resource<TcpSocket>, value: u8) -> Result<(), SocketError> {
        self.observe_function_call("sockets::tcp", "set_hop_limit");
        HostTcpSocket::set_hop_limit(&mut self.as_wasi_view().sockets(), self_, value)
    }

    fn receive_buffer_size(&mut self, self_: Resource<TcpSocket>) -> Result<u64, SocketError> {
        self.observe_function_call("sockets::tcp", "receive_buffer_size");
        HostTcpSocket::receive_buffer_size(&mut self.as_wasi_view().sockets(), self_)
    }

    fn set_receive_buffer_size(
        &mut self,
        self_: Resource<TcpSocket>,
        value: u64,
    ) -> Result<(), SocketError> {
        self.observe_function_call("sockets::tcp", "set_receive_buffer_size");
        HostTcpSocket::set_receive_buffer_size(&mut self.as_wasi_view().sockets(), self_, value)
    }

    fn send_buffer_size(&mut self, self_: Resource<TcpSocket>) -> Result<u64, SocketError> {
        self.observe_function_call("sockets::tcp", "send_buffer_size");
        HostTcpSocket::send_buffer_size(&mut self.as_wasi_view().sockets(), self_)
    }

    fn set_send_buffer_size(
        &mut self,
        self_: Resource<TcpSocket>,
        value: u64,
    ) -> Result<(), SocketError> {
        self.observe_function_call("sockets::tcp", "set_send_buffer_size");
        HostTcpSocket::set_send_buffer_size(&mut self.as_wasi_view().sockets(), self_, value)
    }

    fn subscribe(&mut self, self_: Resource<TcpSocket>) -> wasmtime::Result<Resource<Pollable>> {
        self.observe_function_call("sockets::tcp", "subscribe");
        HostTcpSocket::subscribe(&mut self.as_wasi_view().sockets(), self_)
    }

    fn shutdown(
        &mut self,
        self_: Resource<TcpSocket>,
        shutdown_type: ShutdownType,
    ) -> Result<(), SocketError> {
        self.observe_function_call("sockets::tcp", "shutdown");
        HostTcpSocket::shutdown(&mut self.as_wasi_view().sockets(), self_, shutdown_type)
    }

    fn drop(&mut self, rep: Resource<TcpSocket>) -> wasmtime::Result<()> {
        self.observe_function_call("sockets::tcp", "drop");
        HostTcpSocket::drop(&mut self.as_wasi_view().sockets(), rep)
    }
}

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {}

/// Checks the permission of a connect in a durable call, so that replay returns the recorded
/// decision instead of checking again. An address that has no permission target is denied. The
/// repair of an incomplete check during replay cannot authorize, so it completes as allowed.
async fn authorize_connect<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    remote_address: &IpSocketAddress,
) -> Result<(), SocketError> {
    let response = DurableCallSession::<SocketsTcpConnectPermissionCheck, NotCancellable>::invoke(
        ctx,
        HostRequestSocketsTcpConnectPermissionCheck {
            address: std::net::SocketAddr::from(*remote_address).to_string(),
        },
        DurableFunctionType::ReadLocal,
        async |ctx| -> wasmtime::Result<_> {
            let result = if !ctx.is_live() {
                Ok(())
            } else {
                match ip_socket_target(remote_address) {
                    Some(target) => ctx
                        .authorize_live_permission(&target)
                        .await?
                        .map(|_| ())
                        .map_err(|error| error.to_safe_string()),
                    None => Err("the address has no permission target".to_string()),
                }
            };
            Ok(HostResponsePermissionCheck { result })
        },
    )
    .await
    .map_err(SocketError::trap)?;
    response.result.map_err(|_| ErrorCode::AccessDenied.into())
}

fn ip_socket_target(
    address: &IpSocketAddress,
) -> Option<golem_common::model::card::PermissionTarget> {
    match address {
        IpSocketAddress::Ipv4(address) => {
            let (a, b, c, d) = address.address;
            tcp_target(
                &std::net::Ipv4Addr::new(a, b, c, d).to_string(),
                address.port,
            )
            .ok()
        }
        IpSocketAddress::Ipv6(_) => None,
    }
}
