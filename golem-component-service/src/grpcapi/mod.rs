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

use crate::grpcapi::component::ComponentGrpcApi;
use crate::grpcapi::plugin::PluginGrpcApi;
use crate::service::Services;
use futures_util::TryFutureExt;
use golem_api_grpc::proto;
use golem_api_grpc::proto::golem::component::v1::component_service_server::ComponentServiceServer;
use golem_api_grpc::proto::golem::component::v1::plugin_service_server::PluginServiceServer;
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tokio::task::JoinSet;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::codec::CompressionEncoding;
use tonic::transport::Server;
use tracing::Instrument;
mod component;
mod plugin;

pub async fn start_grpc_server(
    addr: SocketAddr,
    services: Services,
    join_set: &mut JoinSet<Result<(), anyhow::Error>>,
) -> anyhow::Result<u16> {
    let (mut health_reporter, health_service) = tonic_health::server::health_reporter();

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
        async move {
            Server::builder()
                .add_service(reflection_service)
                .add_service(health_service)
                .add_service(
                    ComponentServiceServer::new(ComponentGrpcApi::new(
                        services.component_service.clone(),
                        services.plugin_service.clone(),
                    ))
                    .accept_compressed(CompressionEncoding::Gzip)
                    .send_compressed(CompressionEncoding::Gzip),
                )
                .add_service(
                    PluginServiceServer::new(PluginGrpcApi {
                        plugin_service: services.plugin_service.clone(),
                    })
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
