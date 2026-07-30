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

use crate::durable_host::p3::{DurableP3View, observe_function_call};
use crate::workerctx::WorkerCtx;
use golem_common::model::oplog::types::SerializableP3SocketErrorCode;
use wasmtime_wasi::p3::bindings::sockets::types;
use wasmtime_wasi::p3::sockets::SocketError;
use wasmtime_wasi::sockets::WasiSocketsView;

mod dns;
mod tcp;
mod udp;

#[cfg(test)]
mod tests;

#[cfg(test)]
use dns::classify_p3_ip_name_lookup_error;
#[cfg(test)]
use tcp::TcpReceiveForwardConsumer;

fn serialize_socket_error(error: SocketError) -> wasmtime::Result<SerializableP3SocketErrorCode> {
    Ok(SerializableP3SocketErrorCode::from(error.downcast()?))
}

impl<Ctx: WorkerCtx> types::Host for DurableP3View<'_, Ctx> {
    fn convert_error_code(&mut self, error: SocketError) -> wasmtime::Result<types::ErrorCode> {
        observe_function_call(&*self.0, "sockets::types", "convert-error-code");
        types::Host::convert_error_code(&mut WasiSocketsView::sockets(self.0), error)
    }
}
