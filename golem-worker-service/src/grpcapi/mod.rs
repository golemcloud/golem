use golem_api_grpc::proto;
use golem_api_grpc::proto::golem::worker::worker_service_server::WorkerServiceServer;
use std::net::SocketAddr;
use tonic::transport::{Error, Server};

use crate::grpcapi::worker::WorkerGrpcApi;
use crate::service::Services;

mod worker;

pub async fn start_grpc_server(addr: SocketAddr, services: &Services) -> Result<(), Error> {
    let (mut health_reporter, health_service) = tonic_health::server::health_reporter();

    health_reporter
        .set_serving::<WorkerServiceServer<WorkerGrpcApi>>()
        .await;

    let reflection_service = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(proto::FILE_DESCRIPTOR_SET)
        .build()
        .unwrap();

    Server::builder()
        .add_service(reflection_service)
        .add_service(health_service)
        .add_service(WorkerServiceServer::new(WorkerGrpcApi {
            template_service: services.template_service.clone(),
            worker_service: services.worker_service.clone(),
        }))
        .serve(addr)
        .await
}
