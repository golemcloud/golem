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

use crate::bindings::exports::wasi::sockets::tcp::{
    Duration, ErrorCode, InputStream, IpAddressFamily, IpSocketAddress, NetworkBorrow,
    OutputStream, Pollable, ShutdownType, TcpSocket,
};
use crate::bindings::golem::durability::durability::observe_function_call;
use crate::wrappers::io::poll::WrappedPollable;
use crate::wrappers::io::streams::{WrappedInputStream, WrappedOutputStream};
use crate::wrappers::sockets::network::WrappedNetwork;
use std::mem::transmute;

pub struct WrappedTcpSocket {
    pub tcp_socket: crate::bindings::wasi::sockets::tcp::TcpSocket,
}

impl crate::bindings::exports::wasi::sockets::tcp::GuestTcpSocket for WrappedTcpSocket {
    fn start_bind(
        &self,
        network: NetworkBorrow<'_>,
        local_address: IpSocketAddress,
    ) -> Result<(), ErrorCode> {
        observe_function_call("sockets::tcp", "start_bind");
        let network = &network.get::<WrappedNetwork>().network;
        let local_address = unsafe { transmute(local_address) };
        self.tcp_socket.start_bind(network, local_address)?;
        Ok(())
    }

    fn finish_bind(&self) -> Result<(), ErrorCode> {
        observe_function_call("sockets::tcp", "finish_bind");
        self.tcp_socket.finish_bind()?;
        Ok(())
    }

    fn start_connect(
        &self,
        network: NetworkBorrow<'_>,
        remote_address: IpSocketAddress,
    ) -> Result<(), ErrorCode> {
        observe_function_call("sockets::tcp", "start_connect");
        let network = &network.get::<WrappedNetwork>().network;
        let remote_address = unsafe { transmute(remote_address) };
        self.tcp_socket.start_connect(network, remote_address)?;
        Ok(())
    }

    fn finish_connect(&self) -> Result<(InputStream, OutputStream), ErrorCode> {
        observe_function_call("sockets::tcp", "finish_connect");
        let (input, output) = self.tcp_socket.finish_connect()?;
        let input = InputStream::new(WrappedInputStream {
            input_stream: input,
            is_incoming_http_body_stream: false,
        });
        let output = OutputStream::new(WrappedOutputStream {
            output_stream: output,
        });
        Ok((input, output))
    }

    fn start_listen(&self) -> Result<(), ErrorCode> {
        observe_function_call("sockets::tcp", "start_listen");
        self.tcp_socket.start_listen()?;
        Ok(())
    }

    fn finish_listen(&self) -> Result<(), ErrorCode> {
        observe_function_call("sockets::tcp", "finish_listen");
        self.tcp_socket.finish_listen()?;
        Ok(())
    }

    fn accept(&self) -> Result<(TcpSocket, InputStream, OutputStream), ErrorCode> {
        observe_function_call("sockets::tcp", "accept");
        let (socket, input, output) = self.tcp_socket.accept()?;
        let socket = TcpSocket::new(WrappedTcpSocket { tcp_socket: socket });
        let input = InputStream::new(WrappedInputStream {
            input_stream: input,
            is_incoming_http_body_stream: false,
        });
        let output = OutputStream::new(WrappedOutputStream {
            output_stream: output,
        });
        Ok((socket, input, output))
    }

    fn local_address(&self) -> Result<IpSocketAddress, ErrorCode> {
        observe_function_call("sockets::tcp", "local_address");
        let address = self.tcp_socket.local_address()?;
        let address = unsafe { transmute(address) };
        Ok(address)
    }

    fn remote_address(&self) -> Result<IpSocketAddress, ErrorCode> {
        observe_function_call("sockets::tcp", "remote_address");
        let address = self.tcp_socket.local_address()?;
        let address = unsafe { transmute(address) };
        Ok(address)
    }

    fn is_listening(&self) -> bool {
        observe_function_call("sockets::tcp", "is_listening");
        self.tcp_socket.is_listening()
    }

    fn address_family(&self) -> IpAddressFamily {
        observe_function_call("sockets::tcp", "address_family");
        let family = self.tcp_socket.address_family();
        let family = unsafe { transmute(family) };
        family
    }

    fn set_listen_backlog_size(&self, value: u64) -> Result<(), ErrorCode> {
        observe_function_call("sockets::tcp", "set_listen_backlog_size");
        self.tcp_socket.set_listen_backlog_size(value)?;
        Ok(())
    }

    fn keep_alive_enabled(&self) -> Result<bool, ErrorCode> {
        observe_function_call("sockets::tcp", "keep_alive_enabled");
        Ok(self.tcp_socket.keep_alive_enabled()?)
    }

    fn set_keep_alive_enabled(&self, value: bool) -> Result<(), ErrorCode> {
        observe_function_call("sockets::tcp", "set_keep_alive_enabled");
        self.tcp_socket.set_keep_alive_enabled(value)?;
        Ok(())
    }

    fn keep_alive_idle_time(&self) -> Result<Duration, ErrorCode> {
        observe_function_call("sockets::tcp", "keep_alive_idle_time");
        let duration = self.tcp_socket.keep_alive_idle_time()?;
        Ok(duration)
    }

    fn set_keep_alive_idle_time(&self, value: Duration) -> Result<(), ErrorCode> {
        observe_function_call("sockets::tcp", "set_keep_alive_idle_time");
        self.tcp_socket.set_keep_alive_idle_time(value)?;
        Ok(())
    }

    fn keep_alive_interval(&self) -> Result<Duration, ErrorCode> {
        observe_function_call("sockets::tcp", "keep_alive_interval");
        let duration = self.tcp_socket.keep_alive_interval()?;
        Ok(duration)
    }

    fn set_keep_alive_interval(&self, value: Duration) -> Result<(), ErrorCode> {
        observe_function_call("sockets::tcp", "set_keep_alive_interval");
        self.tcp_socket.set_keep_alive_interval(value)?;
        Ok(())
    }

    fn keep_alive_count(&self) -> Result<u32, ErrorCode> {
        observe_function_call("sockets::tcp", "keep_alive_count");
        Ok(self.tcp_socket.keep_alive_count()?)
    }

    fn set_keep_alive_count(&self, value: u32) -> Result<(), ErrorCode> {
        observe_function_call("sockets::tcp", "set_keep_alive_count");
        self.tcp_socket.set_keep_alive_count(value)?;
        Ok(())
    }

    fn hop_limit(&self) -> Result<u8, ErrorCode> {
        observe_function_call("sockets::tcp", "hop_limit");
        Ok(self.tcp_socket.hop_limit()?)
    }

    fn set_hop_limit(&self, value: u8) -> Result<(), ErrorCode> {
        observe_function_call("sockets::tcp", "set_hop_limit");
        self.tcp_socket.set_hop_limit(value)?;
        Ok(())
    }

    fn receive_buffer_size(&self) -> Result<u64, ErrorCode> {
        observe_function_call("sockets::tcp", "receive_buffer_size");
        Ok(self.tcp_socket.receive_buffer_size()?)
    }

    fn set_receive_buffer_size(&self, value: u64) -> Result<(), ErrorCode> {
        observe_function_call("sockets::tcp", "set_receive_buffer_size");
        self.tcp_socket.set_receive_buffer_size(value)?;
        Ok(())
    }

    fn send_buffer_size(&self) -> Result<u64, ErrorCode> {
        observe_function_call("sockets::tcp", "send_buffer_size");
        Ok(self.tcp_socket.send_buffer_size()?)
    }

    fn set_send_buffer_size(&self, value: u64) -> Result<(), ErrorCode> {
        observe_function_call("sockets::tcp", "set_send_buffer_size");
        self.tcp_socket.set_send_buffer_size(value)?;
        Ok(())
    }

    fn subscribe(&self) -> Pollable {
        observe_function_call("sockets::tcp", "subscribe");
        let pollable = self.tcp_socket.subscribe();
        Pollable::new(WrappedPollable::Proxy(pollable))
    }

    fn shutdown(&self, shutdown_type: ShutdownType) -> Result<(), ErrorCode> {
        observe_function_call("sockets::tcp", "shutdown");
        let shutdown_type = unsafe { transmute(shutdown_type) };
        self.tcp_socket.shutdown(shutdown_type)?;
        Ok(())
    }
}

impl Drop for WrappedTcpSocket {
    fn drop(&mut self) {
        observe_function_call("sockets::tcp", "drop");
    }
}

impl crate::bindings::exports::wasi::sockets::tcp::Guest for crate::Component {
    type TcpSocket = WrappedTcpSocket;
}
