// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use golem_common::model::account::AccountId;
use golem_common::model::application::ApplicationId;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::plugin_registration::PluginRegistrationId;
use golem_common::model::{IdempotencyKey, PromiseId, WorkerId};
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

pub fn proto_account_id_string(
    account_id: &Option<golem_api_grpc::proto::golem::common::AccountId>,
) -> Option<String> {
    (*account_id)
        .and_then(|v| TryInto::<AccountId>::try_into(v).ok())
        .map(|v| v.to_string())
}

pub fn proto_application_id_string(
    id: &Option<golem_api_grpc::proto::golem::common::ApplicationId>,
) -> Option<String> {
    (*id)
        .and_then(|v| TryInto::<ApplicationId>::try_into(v).ok())
        .map(|v| v.to_string())
}

pub fn proto_environment_id_string(
    id: &Option<golem_api_grpc::proto::golem::common::EnvironmentId>,
) -> Option<String> {
    (*id)
        .and_then(|v| TryInto::<EnvironmentId>::try_into(v).ok())
        .map(|v| v.to_string())
}

pub fn proto_plugin_registration_id_string(
    plugin_registration_id: &Option<golem_api_grpc::proto::golem::component::PluginRegistrationId>,
) -> Option<String> {
    (*plugin_registration_id)
        .and_then(|v| TryInto::<PluginRegistrationId>::try_into(v).ok())
        .map(|v| v.to_string())
}

pub fn proto_component_id_string(
    component_id: &Option<golem_api_grpc::proto::golem::component::ComponentId>,
) -> Option<String> {
    (*component_id)
        .and_then(|v| TryInto::<ComponentId>::try_into(v).ok())
        .map(|v| v.to_string())
}

pub fn proto_worker_id_string(
    worker_id: &Option<golem_api_grpc::proto::golem::worker::WorkerId>,
) -> Option<String> {
    worker_id
        .clone()
        .and_then(|v| TryInto::<WorkerId>::try_into(v).ok())
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

pub fn proto_invocation_context_parent_worker_id_string(
    invocation_context: &Option<golem_api_grpc::proto::golem::worker::InvocationContext>,
) -> Option<String> {
    proto_worker_id_string(&invocation_context.as_ref().and_then(|c| c.parent.clone()))
}
