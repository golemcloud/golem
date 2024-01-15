use std::net::SocketAddr;
use crate::service::Services;
use tonic::transport::{Error, Server};
use golem_common::proto;
use golem_common::proto::golem::cloudservices::templateservice::template_service_server::TemplateServiceServer;
use crate::grpcapi::template::TemplateGrpcApi;

mod template;


pub async fn start_grpc_server(addr: SocketAddr, services: &Services) -> Result<(), Error> {
    let (mut health_reporter, health_service) = tonic_health::server::health_reporter();
    health_reporter
        .set_serving::<TemplateServiceServer<TemplateGrpcApi>>()
        .await;

    let reflection_service = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(proto::FILE_DESCRIPTOR_SET)
        .build()
        .unwrap();

    Server::builder()
        .add_service(reflection_service)
        .add_service(health_service)
        .add_service(TemplateServiceServer::new(TemplateGrpcApi {
            template_service: services.template_service.clone(),
        }))
        .serve(addr)
        .await
}