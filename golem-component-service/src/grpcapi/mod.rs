// Copyright 2024 Golem Cloud
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

use golem_api_grpc::proto;
use golem_api_grpc::proto::golem::component::v1::component_service_server::ComponentServiceServer;
use std::net::SocketAddr;
use tonic::codec::CompressionEncoding;
use tonic::transport::{Error, Server};
use golem_api_grpc::proto::golem::component::v1::ifs_service_server::IfsServiceServer;
use crate::grpcapi::component::ComponentGrpcApi;
use crate::grpcapi::ifs::IFSGrpcApi;
use crate::service::Services;
mod component;
mod ifs;

pub async fn start_grpc_server(addr: SocketAddr, services: &Services) -> Result<(), Error> {
    let (mut health_reporter, health_service) = tonic_health::server::health_reporter();

    health_reporter
        .set_serving::<ComponentServiceServer<ComponentGrpcApi>>()
        .await;

    let reflection_service = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(proto::FILE_DESCRIPTOR_SET)
        .build()
        .unwrap();

    Server::builder()
        .add_service(reflection_service)
        .add_service(health_service)
        .add_service(
            ComponentServiceServer::new(ComponentGrpcApi {
                component_service: services.component_service.clone(),
            })
            .accept_compressed(CompressionEncoding::Gzip)
            .send_compressed(CompressionEncoding::Gzip),
        )
        .add_service(
            IfsServiceServer::new(IFSGrpcApi {
                ifs_service: services.ifs_service.clone(),
            })
                .accept_compressed(CompressionEncoding::Gzip)
                .send_compressed(CompressionEncoding::Gzip),
        )
        .serve(addr)
        .await
}
