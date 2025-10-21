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

use golem_common::model::environment::EnvironmentId;
use std::fmt::{Debug, Display, Formatter};
use uuid::Uuid;

pub fn proto_environment_id_string(
    id: &Option<golem_api_grpc::proto::golem::common::EnvironmentId>,
) -> Option<String> {
    (*id)
        .and_then(|v| TryInto::<EnvironmentId>::try_into(v).ok())
        .map(|v| v.to_string())
}

pub fn authorised_grpc_request<T>(request: T, access_token: &Uuid) -> tonic::Request<T> {
    let mut req = tonic::Request::new(request);
    req.metadata_mut().insert(
        "authorization",
        format!("Bearer {access_token}").parse().unwrap(),
    );
    req
}

pub enum GrpcError<E> {
    Transport(tonic::transport::Error),
    Status(tonic::Status),
    Domain(E),
    Unexpected(String),
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

pub fn is_grpc_retriable<E>(error: &GrpcError<E>) -> bool {
    match error {
        GrpcError::Transport(_) => true,
        GrpcError::Status(status) => status.code() == tonic::Code::Unavailable,
        GrpcError::Domain(_) => false,
        GrpcError::Unexpected(_) => false,
    }
}
