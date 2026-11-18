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

pub mod dns;
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
use wasmtime_wasi::p3::bindings::sockets::types::IpAddressFamily;

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
    pub send_taken: bool,
    pub receive_taken: bool,
    pub native_bound: bool,
    pub options: TcpSocketOptions,
}

impl SocketEntry {
    pub fn new(family: IpAddressFamily) -> Self {
        Self {
            family,
            phase: SocketPhase::Created,
            local_address: None,
            remote_address: None,
            send_taken: false,
            receive_taken: false,
            native_bound: false,
            options: TcpSocketOptions::default(),
        }
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
