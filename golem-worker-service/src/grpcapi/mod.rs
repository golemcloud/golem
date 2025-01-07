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

use crate::grpcapi::api_definition::GrpcApiDefinitionService;
use crate::grpcapi::worker::WorkerGrpcApi;
use crate::service::Services;
use futures_util::TryFutureExt;
use golem_api_grpc::proto;
use golem_api_grpc::proto::golem::apidefinition::v1::api_definition_service_server::ApiDefinitionServiceServer;
use golem_api_grpc::proto::golem::worker::v1::worker_service_server::WorkerServiceServer;
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tokio::task::JoinSet;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::codec::CompressionEncoding;
use tonic::transport::Server;
use tracing::Instrument;

mod api_definition;
mod worker;

pub async fn start_grpc_server(
    addr: SocketAddr,
    services: Services,
    join_set: &mut JoinSet<Result<(), anyhow::Error>>,
) -> anyhow::Result<u16> {
    let (mut health_reporter, health_service) = tonic_health::server::health_reporter();

    let listener = TcpListener::bind(addr).await?;
    let port = listener.local_addr()?.port();

    health_reporter
        .set_serving::<WorkerServiceServer<WorkerGrpcApi>>()
        .await;

    health_reporter
        .set_serving::<ApiDefinitionServiceServer<GrpcApiDefinitionService>>()
        .await;

    let reflection_service = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(proto::FILE_DESCRIPTOR_SET)
        .build_v1()
        .unwrap();

    join_set.spawn(
        async move {
            Server::builder()
                .add_service(reflection_service)
                .add_service(health_service)
                .add_service(
                    WorkerServiceServer::new(WorkerGrpcApi::new(
                        services.component_service.clone(),
                        services.worker_service.clone(),
                    ))
                    .accept_compressed(CompressionEncoding::Gzip)
                    .send_compressed(CompressionEncoding::Gzip),
                )
                .add_service(
                    ApiDefinitionServiceServer::new(GrpcApiDefinitionService::new(
                        services.definition_service.clone(),
                    ))
                    .accept_compressed(CompressionEncoding::Gzip)
                    .send_compressed(CompressionEncoding::Gzip),
                )
                .serve_with_incoming(TcpListenerStream::new(listener))
                .map_err(anyhow::Error::from)
                .await
        }
        .in_current_span(),
    );

    Ok(port)
}
