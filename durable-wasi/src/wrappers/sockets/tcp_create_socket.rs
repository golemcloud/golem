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

use crate::bindings::exports::wasi::sockets::tcp_create_socket::{
    ErrorCode, IpAddressFamily, TcpSocket,
};
use crate::bindings::golem::durability::durability::observe_function_call;
use crate::bindings::wasi::sockets::tcp_create_socket::create_tcp_socket;
use crate::wrappers::sockets::tcp::WrappedTcpSocket;
use std::mem::transmute;

impl crate::bindings::exports::wasi::sockets::tcp_create_socket::Guest for crate::Component {
    fn create_tcp_socket(address_family: IpAddressFamily) -> Result<TcpSocket, ErrorCode> {
        observe_function_call("sockets::tcp_create_socket", "create_tcp_socket");
        let address_family = unsafe { transmute(address_family) };
        let socket = create_tcp_socket(address_family)?;
        Ok(TcpSocket::new(WrappedTcpSocket { tcp_socket: socket }))
    }
}
