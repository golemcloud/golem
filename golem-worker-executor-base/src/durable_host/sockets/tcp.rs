// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use async_trait::async_trait;
use wasmtime::component::Resource;

use crate::durable_host::DurableWorkerCtx;
use crate::metrics::wasm::record_host_function_call;
use crate::workerctx::WorkerCtx;
use wasmtime_wasi::bindings::sockets::tcp::{
    Duration, Host, HostTcpSocket, InputStream, IpAddressFamily, IpSocketAddress, Network,
    OutputStream, Pollable, ShutdownType, TcpSocket,
};
use wasmtime_wasi::SocketError;

#[async_trait]
impl<Ctx: WorkerCtx> HostTcpSocket for DurableWorkerCtx<Ctx> {
    async fn start_bind(
        &mut self,
        self_: Resource<TcpSocket>,
        network: Resource<Network>,
        local_address: IpSocketAddress,
    ) -> Result<(), SocketError> {
        record_host_function_call("sockets::tcp", "start_bind");
        HostTcpSocket::start_bind(&mut self.as_wasi_view(), self_, network, local_address).await
    }

    fn finish_bind(&mut self, self_: Resource<TcpSocket>) -> Result<(), SocketError> {
        record_host_function_call("sockets::tcp", "finish_bind");
        HostTcpSocket::finish_bind(&mut self.as_wasi_view(), self_)
    }

    async fn start_connect(
        &mut self,
        self_: Resource<TcpSocket>,
        network: Resource<Network>,
        remote_address: IpSocketAddress,
    ) -> Result<(), SocketError> {
        record_host_function_call("sockets::tcp", "start_connect");
        HostTcpSocket::start_connect(&mut self.as_wasi_view(), self_, network, remote_address).await
    }

    fn finish_connect(
        &mut self,
        self_: Resource<TcpSocket>,
    ) -> Result<(Resource<InputStream>, Resource<OutputStream>), SocketError> {
        record_host_function_call("sockets::tcp", "finish_connect");
        HostTcpSocket::finish_connect(&mut self.as_wasi_view(), self_)
    }

    fn start_listen(&mut self, self_: Resource<TcpSocket>) -> Result<(), SocketError> {
        record_host_function_call("sockets::tcp", "start_listen");
        HostTcpSocket::start_listen(&mut self.as_wasi_view(), self_)
    }

    fn finish_listen(&mut self, self_: Resource<TcpSocket>) -> Result<(), SocketError> {
        record_host_function_call("sockets::tcp", "finish_listen");
        HostTcpSocket::finish_listen(&mut self.as_wasi_view(), self_)
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
        record_host_function_call("sockets::tcp", "accept");
        HostTcpSocket::accept(&mut self.as_wasi_view(), self_)
    }

    fn local_address(
        &mut self,
        self_: Resource<TcpSocket>,
    ) -> Result<IpSocketAddress, SocketError> {
        record_host_function_call("sockets::tcp", "local_address");
        HostTcpSocket::local_address(&mut self.as_wasi_view(), self_)
    }

    fn remote_address(
        &mut self,
        self_: Resource<TcpSocket>,
    ) -> Result<IpSocketAddress, SocketError> {
        record_host_function_call("sockets::tcp", "remote_address");
        HostTcpSocket::remote_address(&mut self.as_wasi_view(), self_)
    }

    fn is_listening(&mut self, self_: Resource<TcpSocket>) -> anyhow::Result<bool> {
        record_host_function_call("sockets::tcp", "is_listening");
        HostTcpSocket::is_listening(&mut self.as_wasi_view(), self_)
    }

    fn address_family(&mut self, self_: Resource<TcpSocket>) -> anyhow::Result<IpAddressFamily> {
        record_host_function_call("sockets::tcp", "address_family");
        HostTcpSocket::address_family(&mut self.as_wasi_view(), self_)
    }

    fn set_listen_backlog_size(
        &mut self,
        self_: Resource<TcpSocket>,
        value: u64,
    ) -> Result<(), SocketError> {
        record_host_function_call("sockets::tcp", "set_listen_backlog_size");
        HostTcpSocket::set_listen_backlog_size(&mut self.as_wasi_view(), self_, value)
    }

    fn keep_alive_enabled(&mut self, self_: Resource<TcpSocket>) -> Result<bool, SocketError> {
        record_host_function_call("sockets::tcp", "keep_alive_enabled");
        HostTcpSocket::keep_alive_enabled(&mut self.as_wasi_view(), self_)
    }

    fn set_keep_alive_enabled(
        &mut self,
        self_: Resource<TcpSocket>,
        value: bool,
    ) -> Result<(), SocketError> {
        record_host_function_call("sockets::tcp", "set_keep_alive_enabled");
        HostTcpSocket::set_keep_alive_enabled(&mut self.as_wasi_view(), self_, value)
    }

    fn keep_alive_idle_time(
        &mut self,
        self_: Resource<TcpSocket>,
    ) -> Result<Duration, SocketError> {
        record_host_function_call("sockets::tcp", "keep_alive_idle_time");
        HostTcpSocket::keep_alive_idle_time(&mut self.as_wasi_view(), self_)
    }

    fn set_keep_alive_idle_time(
        &mut self,
        self_: Resource<TcpSocket>,
        value: Duration,
    ) -> Result<(), SocketError> {
        record_host_function_call("sockets::tcp", "set_keep_alive_idle_time");
        HostTcpSocket::set_keep_alive_idle_time(&mut self.as_wasi_view(), self_, value)
    }

    fn keep_alive_interval(&mut self, self_: Resource<TcpSocket>) -> Result<Duration, SocketError> {
        record_host_function_call("sockets::tcp", "keep_alive_interval");
        HostTcpSocket::keep_alive_interval(&mut self.as_wasi_view(), self_)
    }

    fn set_keep_alive_interval(
        &mut self,
        self_: Resource<TcpSocket>,
        value: Duration,
    ) -> Result<(), SocketError> {
        record_host_function_call("sockets::tcp", "set_keep_alive_interval");
        HostTcpSocket::set_keep_alive_interval(&mut self.as_wasi_view(), self_, value)
    }

    fn keep_alive_count(&mut self, self_: Resource<TcpSocket>) -> Result<u32, SocketError> {
        record_host_function_call("sockets::tcp", "keep_alive_count");
        HostTcpSocket::keep_alive_count(&mut self.as_wasi_view(), self_)
    }

    fn set_keep_alive_count(
        &mut self,
        self_: Resource<TcpSocket>,
        value: u32,
    ) -> Result<(), SocketError> {
        record_host_function_call("sockets::tcp", "set_keep_alive_count");
        HostTcpSocket::set_keep_alive_count(&mut self.as_wasi_view(), self_, value)
    }

    fn hop_limit(&mut self, self_: Resource<TcpSocket>) -> Result<u8, SocketError> {
        record_host_function_call("sockets::tcp", "hop_limit");
        HostTcpSocket::hop_limit(&mut self.as_wasi_view(), self_)
    }

    fn set_hop_limit(&mut self, self_: Resource<TcpSocket>, value: u8) -> Result<(), SocketError> {
        record_host_function_call("sockets::tcp", "set_hop_limit");
        HostTcpSocket::set_hop_limit(&mut self.as_wasi_view(), self_, value)
    }

    fn receive_buffer_size(&mut self, self_: Resource<TcpSocket>) -> Result<u64, SocketError> {
        record_host_function_call("sockets::tcp", "receive_buffer_size");
        HostTcpSocket::receive_buffer_size(&mut self.as_wasi_view(), self_)
    }

    fn set_receive_buffer_size(
        &mut self,
        self_: Resource<TcpSocket>,
        value: u64,
    ) -> Result<(), SocketError> {
        record_host_function_call("sockets::tcp", "set_receive_buffer_size");
        HostTcpSocket::set_receive_buffer_size(&mut self.as_wasi_view(), self_, value)
    }

    fn send_buffer_size(&mut self, self_: Resource<TcpSocket>) -> Result<u64, SocketError> {
        record_host_function_call("sockets::tcp", "send_buffer_size");
        HostTcpSocket::send_buffer_size(&mut self.as_wasi_view(), self_)
    }

    fn set_send_buffer_size(
        &mut self,
        self_: Resource<TcpSocket>,
        value: u64,
    ) -> Result<(), SocketError> {
        record_host_function_call("sockets::tcp", "set_send_buffer_size");
        HostTcpSocket::set_send_buffer_size(&mut self.as_wasi_view(), self_, value)
    }

    fn subscribe(&mut self, self_: Resource<TcpSocket>) -> anyhow::Result<Resource<Pollable>> {
        record_host_function_call("sockets::tcp", "subscribe");
        HostTcpSocket::subscribe(&mut self.as_wasi_view(), self_)
    }

    fn shutdown(
        &mut self,
        self_: Resource<TcpSocket>,
        shutdown_type: ShutdownType,
    ) -> Result<(), SocketError> {
        record_host_function_call("sockets::tcp", "shutdown");
        HostTcpSocket::shutdown(&mut self.as_wasi_view(), self_, shutdown_type)
    }

    fn drop(&mut self, rep: Resource<TcpSocket>) -> anyhow::Result<()> {
        record_host_function_call("sockets::tcp", "drop");
        HostTcpSocket::drop(&mut self.as_wasi_view(), rep)
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {}
