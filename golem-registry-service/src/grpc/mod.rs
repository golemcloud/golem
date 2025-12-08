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

mod api_impl;
mod error;

use self::api_impl::RegistryServiceGrpcApi;
use crate::bootstrap::Services;
use futures::TryFutureExt;
use golem_api_grpc::proto;
use golem_api_grpc::proto::golem::registry::v1::registry_service_server::RegistryServiceServer;
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tokio::task::JoinSet;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::codec::CompressionEncoding;
use tonic::transport::Server;
use tonic_tracing_opentelemetry::middleware::server::OtelGrpcLayer;
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
        .set_serving::<RegistryServiceServer<RegistryServiceGrpcApi>>()
        .await;

    let reflection_service = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(proto::FILE_DESCRIPTOR_SET)
        .build_v1()
        .unwrap();

    join_set.spawn(
        Server::builder()
            .layer(
                OtelGrpcLayer::default()
                    .filter(tonic_tracing_opentelemetry::middleware::filters::reject_healthcheck),
            )
            .add_service(reflection_service)
            .add_service(health_service)
            .add_service(
                RegistryServiceServer::new(RegistryServiceGrpcApi::new(
                    services.auth_service.clone(),
                    services.environment_service.clone(),
                    services.account_usage_service.clone(),
                    services.component_service.clone(),
                    services.component_resolver_service.clone(),
                    services.deployment_service.clone(),
                    services.deployed_routes_service.clone(),
                ))
                .send_compressed(CompressionEncoding::Gzip)
                .accept_compressed(CompressionEncoding::Gzip),
            )
            .serve_with_incoming(TcpListenerStream::new(listener))
            .map_err(anyhow::Error::from)
            .in_current_span(),
    );

    services
        .component_compilation_service
        .set_own_grpc_port(port);

    Ok(port)
}
