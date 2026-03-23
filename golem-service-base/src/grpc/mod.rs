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

pub mod client;
pub mod server;

use golem_common::model::{AgentId, IdempotencyKey, PromiseId};
use std::fmt::{Debug, Display, Formatter};

pub enum GrpcError<E> {
    Transport(tonic::transport::Error),
    Status(tonic::Status),
    Domain(E),
    Unexpected(String),
}

impl<E> GrpcError<E> {
    pub fn empty_response() -> Self {
        Self::Unexpected("empty response".to_string())
    }

    pub fn is_retriable(&self) -> bool {
        match self {
            GrpcError::Transport(_) => true,
            GrpcError::Status(status) => status.code() == tonic::Code::Unavailable,
            GrpcError::Domain(_) => false,
            GrpcError::Unexpected(_) => false,
        }
    }
}

impl<E: Debug> Debug for GrpcError<E> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            GrpcError::Transport(err) => write!(f, "Transport({err:?})"),
            GrpcError::Status(err) => write!(f, "Status({err:?})"),
            GrpcError::Domain(err) => write!(f, "Domain({err:?})"),
            GrpcError::Unexpected(err) => write!(f, "Unexpected({err:?})"),
        }
    }
}

impl<E: Debug> Display for GrpcError<E> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            GrpcError::Transport(err) => write!(f, "gRPC transport error: {err})"),
            GrpcError::Status(err) => write!(f, "Failed gRPC request: {err})"),
            GrpcError::Domain(err) => write!(f, "gRPC request failed with {err:?}"),
            GrpcError::Unexpected(err) => write!(f, "Unexpected error {err}"),
        }
    }
}

impl<E: Debug> std::error::Error for GrpcError<E> {}

impl<E> From<tonic::transport::Error> for GrpcError<E> {
    fn from(value: tonic::transport::Error) -> Self {
        Self::Transport(value)
    }
}

impl<E> From<tonic::Status> for GrpcError<E> {
    fn from(value: tonic::Status) -> Self {
        Self::Status(value)
    }
}

impl<E> From<String> for GrpcError<E> {
    fn from(value: String) -> Self {
        Self::Unexpected(value)
    }
}

impl<E> From<&'static str> for GrpcError<E> {
    fn from(value: &'static str) -> Self {
        Self::from(value.to_string())
    }
}

pub fn proto_agent_id_string(
    agent_id: &Option<golem_api_grpc::proto::golem::worker::AgentId>,
) -> Option<String> {
    agent_id
        .clone()
        .and_then(|v| TryInto::<AgentId>::try_into(v).ok())
        .map(|v| v.to_string())
}

pub fn proto_idempotency_key_string(
    idempotency_key: &Option<golem_api_grpc::proto::golem::worker::IdempotencyKey>,
) -> Option<String> {
    idempotency_key
        .clone()
        .map(|v| Into::<IdempotencyKey>::into(v).to_string())
}

pub fn proto_promise_id_string(
    promise_id: &Option<golem_api_grpc::proto::golem::worker::PromiseId>,
) -> Option<String> {
    promise_id
        .clone()
        .and_then(|v| TryInto::<PromiseId>::try_into(v).ok())
        .map(|v| v.to_string())
}

pub fn proto_invocation_context_parent_agent_id_string(
    invocation_context: &Option<golem_api_grpc::proto::golem::worker::InvocationContext>,
) -> Option<String> {
    proto_agent_id_string(&invocation_context.as_ref().and_then(|c| c.parent.clone()))
}
