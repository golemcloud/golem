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

mod generated {
    use ::wasmtime::component::bindgen;
    bindgen!({
        path: "wit",
        world: "grpc",
        tracing: false,
        async: true,
        trappable_imports: true,
        with: {
            "golem:grpc/types/grpc": super::GrpcEntry
        },
        wasmtime_crate: ::wasmtime,
    });
}

pub use generated::golem::grpc0_1_0 as golem_grpc_0_1_x;

use golem_common::model::invocation_context::SpanId;
pub use golem_grpc_0_1_x::types::{Host, HostGrpc};
use prost_reflect::DynamicMessage;
use serde::Deserialize;
use tokio::sync::{mpsc::UnboundedSender, oneshot};
use tonic::{Response, Status, Streaming};

#[allow(dead_code)]
fn main() -> std::io::Result<()> {
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct GrpcConfiguration {
    pub url: String,
    pub secret_token: String,
}

pub struct GrpcEntry {
    pub payload: Box<GrpcEntryPayload>,
}

pub struct GrpcEntryPayload {
    pub span_id: SpanId,
    pub constructor_params: String,
    pub rx_stream: Option<tokio::sync::Mutex<Streaming<DynamicMessage>>>,
    pub resp_rx: Option<oneshot::Receiver<Result<Response<DynamicMessage>, Status>>>,
    pub sender: Option<UnboundedSender<DynamicMessage>>,
}

impl GrpcEntryPayload {
    pub async fn send(&self, message: DynamicMessage) -> anyhow::Result<Option<bool>, Status> {
        if let Some(sender) = self.sender.as_ref() {
            match sender.send(message) {
                Ok(_) => Ok(Some(true)),
                Err(_) => Ok(None),
            }
        } else {
            Err(tonic::Status::internal("sender not found"))
        }
    }

    pub async fn receive(&mut self) -> anyhow::Result<Option<DynamicMessage>, Status> {
        if let Some(ref mut rx_stream) = self.rx_stream.as_mut() {
            match rx_stream.get_mut().message().await {
                Ok(message_option) => Ok(message_option),
                Err(status) => Err(status),
            }
        } else {
            Err(tonic::Status::internal("receiver not found"))
        }
    }
}

impl GrpcEntryPayload {
    pub fn span_id(&self) -> &SpanId {
        &self.span_id
    }
}
