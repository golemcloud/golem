use golem_api_grpc::proto;
use golem_api_grpc::proto::golem::component::v1::component_service_server::ComponentServiceServer;
use std::net::SocketAddr;
use tonic::transport::{Error, Server};

use crate::grpcapi::component::ComponentGrpcApi;
use crate::service::Services;

mod component;

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
        .add_service(ComponentServiceServer::new(ComponentGrpcApi::new(
            services.component_service.clone(),
        )))
        .serve(addr)
        .await
}
