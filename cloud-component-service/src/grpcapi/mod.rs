use crate::grpcapi::component::ComponentGrpcApi;
use crate::grpcapi::plugin::PluginGrpcApi;
use crate::service::Services;
use cloud_api_grpc::proto::golem::cloud::component::v1::plugin_service_server::PluginServiceServer;
use cloud_common::auth::CloudAuthCtx;
use cloud_common::clients::auth::get_authorisation_token;
use golem_api_grpc::proto;
use golem_api_grpc::proto::golem::common::{ErrorBody, ErrorsBody};
use golem_api_grpc::proto::golem::component::v1::component_service_server::ComponentServiceServer;
use golem_api_grpc::proto::golem::component::v1::{component_error, ComponentError};
use golem_common::model::ComponentId;
use std::net::SocketAddr;
use tonic::codec::CompressionEncoding;
use tonic::metadata::MetadataMap;
use tonic::transport::{Error, Server};

mod component;
mod plugin;

pub async fn start_grpc_server(addr: SocketAddr, services: &Services) -> Result<(), Error> {
    let (mut health_reporter, health_service) = tonic_health::server::health_reporter();

    health_reporter
        .set_serving::<ComponentServiceServer<ComponentGrpcApi>>()
        .await;

    let reflection_service = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(proto::FILE_DESCRIPTOR_SET)
        .build_v1()
        .unwrap();

    Server::builder()
        .add_service(reflection_service)
        .add_service(health_service)
        .add_service(
            ComponentServiceServer::new(ComponentGrpcApi::new(services.component_service.clone()))
                .send_compressed(CompressionEncoding::Gzip)
                .accept_compressed(CompressionEncoding::Gzip),
        )
        .add_service(
            PluginServiceServer::new(PluginGrpcApi::new(services.plugin_service.clone()))
                .send_compressed(CompressionEncoding::Gzip)
                .accept_compressed(CompressionEncoding::Gzip),
        )
        .serve(addr)
        .await
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

fn auth(metadata: MetadataMap) -> Result<CloudAuthCtx, ComponentError> {
    match get_authorisation_token(metadata) {
        Some(t) => Ok(CloudAuthCtx::new(t)),
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
