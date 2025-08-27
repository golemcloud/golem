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

mod agent_types;
mod component;
mod plugin;

use crate::bootstrap::Services;
use crate::grpcapi::agent_types::AgentTypesGrpcApi;
use crate::grpcapi::component::ComponentGrpcApi;
use crate::grpcapi::plugin::PluginGrpcApi;
use futures::TryFutureExt;
use golem_api_grpc::proto;
use golem_api_grpc::proto::golem::common::{ErrorBody, ErrorsBody};
use golem_api_grpc::proto::golem::component::v1::agent_types_service_server::AgentTypesServiceServer;
use golem_api_grpc::proto::golem::component::v1::component_service_server::ComponentServiceServer;
use golem_api_grpc::proto::golem::component::v1::plugin_service_server::PluginServiceServer;
use golem_api_grpc::proto::golem::component::v1::{component_error, ComponentError};
use golem_common::model::auth::AuthCtx;
use golem_common::model::{ComponentId, ProjectId};
use golem_service_base::clients::get_authorisation_token;
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tokio::task::JoinSet;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::codec::CompressionEncoding;
use tonic::metadata::MetadataMap;
use tonic::transport::Server;
use tracing::Instrument;

pub async fn start_grpc_server(
    addr: SocketAddr,
    services: &Services,
    join_set: &mut JoinSet<Result<(), anyhow::Error>>,
) -> anyhow::Result<u16> {
    let (health_reporter, health_service) = tonic_health::server::health_reporter();

    let listener = TcpListener::bind(addr).await?;
    let port = listener.local_addr()?.port();

    health_reporter
        .set_serving::<ComponentServiceServer<ComponentGrpcApi>>()
        .await;

    let reflection_service = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(proto::FILE_DESCRIPTOR_SET)
        .build_v1()
        .unwrap();

    join_set.spawn(
        Server::builder()
            .add_service(reflection_service)
            .add_service(health_service)
            .add_service(
                ComponentServiceServer::new(ComponentGrpcApi::new(
                    services.component_service.clone(),
                ))
                .send_compressed(CompressionEncoding::Gzip)
                .accept_compressed(CompressionEncoding::Gzip),
            )
            .add_service(
                PluginServiceServer::new(PluginGrpcApi::new(services.plugin_service.clone()))
                    .send_compressed(CompressionEncoding::Gzip)
                    .accept_compressed(CompressionEncoding::Gzip),
            )
            .add_service(
                AgentTypesServiceServer::new(AgentTypesGrpcApi::new(
                    services.agent_types_service.clone(),
                ))
                .send_compressed(CompressionEncoding::Gzip)
                .accept_compressed(CompressionEncoding::Gzip),
            )
            .serve_with_incoming(TcpListenerStream::new(listener))
            .map_err(anyhow::Error::from)
            .in_current_span(),
    );

    Ok(port)
}

fn bad_request_error(error: &str) -> ComponentError {
    ComponentError {
        error: Some(component_error::Error::BadRequest(ErrorsBody {
            errors: vec![error.to_string()],
        })),
    }
}

fn internal_error(error: &str) -> ComponentError {
    ComponentError {
        error: Some(component_error::Error::InternalError(ErrorBody {
            error: error.to_string(),
        })),
    }
}

fn auth(metadata: MetadataMap) -> Result<AuthCtx, ComponentError> {
    match get_authorisation_token(metadata) {
        Some(t) => Ok(AuthCtx::new(t)),
        None => Err(ComponentError {
            error: Some(component_error::Error::Unauthorized(ErrorBody {
                error: "Missing token".into(),
            })),
        }),
    }
}

fn require_component_id(
    source: &Option<proto::golem::component::ComponentId>,
) -> Result<ComponentId, ComponentError> {
    match source {
        Some(id) => (*id)
            .try_into()
            .map_err(|err| bad_request_error(&format!("Invalid component id: {err}"))),
        None => Err(bad_request_error("Missing component id")),
    }
}

pub fn proto_project_id_string(
    id: &Option<golem_api_grpc::proto::golem::common::ProjectId>,
) -> Option<String> {
    (*id)
        .and_then(|v| TryInto::<ProjectId>::try_into(v).ok())
        .map(|v| v.to_string())
}
