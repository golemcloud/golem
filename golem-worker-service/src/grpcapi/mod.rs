use golem_api_grpc::proto;
use golem_api_grpc::proto::golem::worker::v1::worker_service_server::WorkerServiceServer;
use std::net::SocketAddr;
use tonic::codec::CompressionEncoding;
use tonic::transport::{Error, Server};

use crate::grpcapi::worker::WorkerGrpcApi;
use crate::service::ApiServices;

mod worker;

pub async fn start_grpc_server(addr: SocketAddr, services: ApiServices) -> Result<(), Error> {
    let (mut health_reporter, health_service) = tonic_health::server::health_reporter();

    health_reporter
        .set_serving::<WorkerServiceServer<WorkerGrpcApi>>()
        .await;

    let reflection_service = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(proto::FILE_DESCRIPTOR_SET)
        .build_v1()
        .unwrap();

    Server::builder()
        .add_service(reflection_service)
        .add_service(health_service)
        .add_service(
            WorkerServiceServer::new(WorkerGrpcApi::new(
                services.component_service.clone(),
                services.worker_service.clone(),
                services.worker_auth_service.clone(),
            ))
            .send_compressed(CompressionEncoding::Gzip)
            .accept_compressed(CompressionEncoding::Gzip),
        )
        .serve(addr)
        .await
}
