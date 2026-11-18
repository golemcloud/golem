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

pub mod instance_network;
pub mod ip_name_lookup;
pub mod network;
pub mod policy;
pub mod resolved_names;
pub mod tcp;
pub mod tcp_create_socket;
pub mod udp;
pub mod udp_create_socket;

use golem_common::model::oplog::types::{
    SerializableP3IpSocketAddress, SerializableP3SocketErrorCode,
};
use std::collections::HashMap;
use std::sync::OnceLock;
use wasmtime_wasi::p3::bindings::sockets::types::{
    IpAddressFamily, IpSocketAddress, Ipv4SocketAddress, Ipv6SocketAddress,
};

/// Store-local socket state, reconstructed from recorded bind, connect and acquire results.
pub(crate) type SocketEntries = HashMap<u32, SocketEntry>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SocketPhase {
    Created,
    Bound,
    Connecting,
    Connected,
    Lost,
    Closed(SerializableP3SocketErrorCode),
}

#[derive(Debug)]
pub(crate) struct SocketEntry {
    pub family: IpAddressFamily,
    pub phase: SocketPhase,
    pub local_address: Option<SerializableP3IpSocketAddress>,
    pub remote_address: Option<SerializableP3IpSocketAddress>,
    /// The address an explicit UDP bind named, or the address of a recorded endpoint rebuild. A
    /// disconnect resets the local address to it, because Linux keeps only the address and port
    /// that a native bind named.
    pub bind_request: Option<SerializableP3IpSocketAddress>,
    pub send_taken: bool,
    pub receive_taken: bool,
    pub native_bound: bool,
    /// P2: `start-bind` succeeded and `finish-bind` has not completed it yet.
    pub bind_pending: bool,
    /// The endpoint rebuild bound the native UDP socket on a fixed port, and no recorded
    /// endpoint change reported it yet.
    pub native_rebound: bool,
    pub options: TcpSocketOptions,
}

impl SocketEntry {
    pub fn new(family: IpAddressFamily) -> Self {
        Self {
            family,
            phase: SocketPhase::Created,
            local_address: None,
            remote_address: None,
            bind_request: None,
            send_taken: false,
            receive_taken: false,
            native_bound: false,
            bind_pending: false,
            native_rebound: false,
            options: TcpSocketOptions::default(),
        }
    }

    /// The unspecified address of the socket's family, with port 0.
    pub fn unspecified_address(&self) -> SerializableP3IpSocketAddress {
        match self.family {
            IpAddressFamily::Ipv4 => IpSocketAddress::Ipv4(Ipv4SocketAddress {
                address: (0, 0, 0, 0),
                port: 0,
            }),
            IpAddressFamily::Ipv6 => IpSocketAddress::Ipv6(Ipv6SocketAddress {
                address: (0, 0, 0, 0, 0, 0, 0, 0),
                port: 0,
                flow_info: 0,
                scope_id: 0,
            }),
        }
        .into()
    }

    /// The UDP local address after a disconnect. Linux keeps only the address and port that an
    /// explicit bind named, so this is the bind request, or the unspecified address and port 0
    /// after an implicit bind.
    pub fn udp_local_address_after_disconnect(&self) -> SerializableP3IpSocketAddress {
        self.bind_request
            .clone()
            .unwrap_or_else(|| self.unspecified_address())
    }

    /// Disconnects the UDP entry: no remote address, and the local address after a disconnect.
    pub fn udp_disconnect(&mut self) {
        self.remote_address = None;
        self.local_address = Some(self.udp_local_address_after_disconnect());
        self.phase = SocketPhase::Bound;
    }

    /// The address a UDP endpoint rebuild binds the native socket on. A local address that names
    /// an interface, such as the one a connect selected, is bound as it is: the socket address
    /// policy allows a fixed port only on a loopback address, so the unspecified address of the
    /// bind request cannot carry it. An unspecified local address is bound on the address of the
    /// bind request, or the unspecified address after an implicit bind, with the recorded port;
    /// with a fixed port the policy refuses that, and the socket becomes `Lost`. `None` when the
    /// socket has no local address.
    pub fn udp_rebuild_bind_address(&self) -> Option<SerializableP3IpSocketAddress> {
        let unspecified = self.unspecified_address();
        self.local_address.as_ref().map(|local_address| {
            if local_address.address == unspecified.address {
                SerializableP3IpSocketAddress {
                    port: local_address.port,
                    ..self.bind_request.clone().unwrap_or(unspecified)
                }
            } else {
                local_address.clone()
            }
        })
    }

    /// The native UDP local address after a live connect or send when it differs from
    /// `expected`: an implicit bind, or the interface address that a connect selects for a
    /// wildcard bind. Every such change is recorded.
    pub fn udp_local_address_change(
        expected: Option<&SerializableP3IpSocketAddress>,
        native: Option<SerializableP3IpSocketAddress>,
    ) -> Option<SerializableP3IpSocketAddress> {
        native.filter(|address| expected != Some(address))
    }
}

#[derive(Debug, Default)]
pub(crate) struct TcpSocketOptions {
    // A successful setter invalidates its cache; the next async getter reads the native effective value.
    pub keep_alive_enabled: Option<OnceLock<bool>>,
    pub keep_alive_idle_time: Option<OnceLock<u64>>,
    pub keep_alive_interval: Option<OnceLock<u64>>,
    pub keep_alive_count: Option<OnceLock<u32>>,
    pub hop_limit: Option<OnceLock<u8>>,
    pub receive_buffer_size: Option<OnceLock<u64>>,
    pub send_buffer_size: Option<OnceLock<u64>>,
}
