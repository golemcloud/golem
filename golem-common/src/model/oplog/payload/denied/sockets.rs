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

use crate::model::oplog::payload::types::SerializableSocketError;
use crate::model::oplog::payload::{
    HostResponseP3SocketsConnect, HostResponseP3SocketsResolveName, HostResponseP3SocketsUdpSend,
    HostResponseSocketsResolveName,
};
use crate::model::oplog::types::{SerializableP3IpNameLookupError, SerializableP3SocketErrorCode};
use wasmtime_wasi::p2::SocketError;
use wasmtime_wasi::p2::bindings::sockets::network::ErrorCode;

pub fn resolve_name<R>(_request: &R, _denial: &str) -> HostResponseSocketsResolveName {
    HostResponseSocketsResolveName {
        result: Err(SerializableSocketError::from(SocketError::from(
            ErrorCode::AccessDenied,
        ))),
    }
}

pub fn p3_resolve_name<R>(_request: &R, _denial: &str) -> HostResponseP3SocketsResolveName {
    HostResponseP3SocketsResolveName {
        result: Err(SerializableP3IpNameLookupError::AccessDenied),
    }
}

pub fn p3_connect<R>(_request: &R, _denial: &str) -> HostResponseP3SocketsConnect {
    HostResponseP3SocketsConnect {
        result: Err(SerializableP3SocketErrorCode::AccessDenied),
    }
}

pub fn p3_udp_send<R>(_request: &R, _denial: &str) -> HostResponseP3SocketsUdpSend {
    HostResponseP3SocketsUdpSend {
        result: Err(SerializableP3SocketErrorCode::AccessDenied),
    }
}
