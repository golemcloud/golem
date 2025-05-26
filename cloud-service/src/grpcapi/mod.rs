use crate::grpcapi::account::AccountGrpcApi;
use crate::grpcapi::account_summary::AccountSummaryGrpcApi;
use crate::grpcapi::limits::LimitsGrpcApi;
use crate::grpcapi::login::LoginGrpcApi;
use crate::grpcapi::project::ProjectGrpcApi;
use crate::grpcapi::token::TokenGrpcApi;
use crate::service::Services;
use auth::AuthGrpcApi;
use cloud_api_grpc::proto::golem::cloud::account::v1::cloud_account_service_server::CloudAccountServiceServer;
use cloud_api_grpc::proto::golem::cloud::accountsummary::v1::cloud_account_summary_service_server::CloudAccountSummaryServiceServer;
use cloud_api_grpc::proto::golem::cloud::auth::v1::cloud_auth_service_server::CloudAuthServiceServer;
use cloud_api_grpc::proto::golem::cloud::limit::v1::cloud_limits_service_server::CloudLimitsServiceServer;
use cloud_api_grpc::proto::golem::cloud::login::v1::cloud_login_service_server::CloudLoginServiceServer;
use cloud_api_grpc::proto::golem::cloud::project::v1::cloud_project_service_server::CloudProjectServiceServer;
use cloud_api_grpc::proto::golem::cloud::token::v1::cloud_token_service_server::CloudTokenServiceServer;
use cloud_common::model::TokenSecret as ModelTokenSecret;
use std::net::SocketAddr;
use std::str::FromStr;
use tonic::codec::CompressionEncoding;
use tonic::metadata::MetadataMap;
use tonic::transport::{Error, Server};

mod account;
mod account_summary;
mod auth;
mod limits;
mod login;
mod project;
mod token;

pub fn get_authorisation_token(metadata: MetadataMap) -> Option<ModelTokenSecret> {
    let auth = metadata
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_string());

    match auth {
        Some(a) if a.to_lowercase().starts_with("bearer ") => {
            let t = &a[7..a.len()];
            ModelTokenSecret::from_str(t.trim()).ok()
        }
        _ => None,
    }
}

pub async fn start_grpc_server(addr: SocketAddr, services: &Services) -> Result<(), Error> {
    let (mut health_reporter, health_service) = tonic_health::server::health_reporter();
    health_reporter
        .set_serving::<CloudAccountServiceServer<AccountGrpcApi>>()
        .await;
    health_reporter
        .set_serving::<CloudAccountSummaryServiceServer<AccountSummaryGrpcApi>>()
        .await;
    health_reporter
        .set_serving::<CloudAuthServiceServer<AuthGrpcApi>>()
        .await;
    health_reporter
        .set_serving::<CloudLimitsServiceServer<LimitsGrpcApi>>()
        .await;
    health_reporter
        .set_serving::<CloudLoginServiceServer<LoginGrpcApi>>()
        .await;
    health_reporter
        .set_serving::<CloudProjectServiceServer<ProjectGrpcApi>>()
        .await;
    health_reporter
        .set_serving::<CloudTokenServiceServer<TokenGrpcApi>>()
        .await;

    let reflection_service = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(golem_api_grpc::proto::FILE_DESCRIPTOR_SET)
        .register_encoded_file_descriptor_set(cloud_api_grpc::proto::FILE_DESCRIPTOR_SET)
        .build_v1()
        .unwrap();

    Server::builder()
        .add_service(reflection_service)
        .add_service(health_service)
        .add_service(
            CloudAccountServiceServer::new(AccountGrpcApi {
                auth_service: services.auth_service.clone(),
                account_service: services.account_service.clone(),
            })
            .send_compressed(CompressionEncoding::Gzip)
            .accept_compressed(CompressionEncoding::Gzip),
        )
        .add_service(
            CloudAccountSummaryServiceServer::new(AccountSummaryGrpcApi {
                auth_service: services.auth_service.clone(),
                account_summary_service: services.account_summary_service.clone(),
            })
            .send_compressed(CompressionEncoding::Gzip)
            .accept_compressed(CompressionEncoding::Gzip),
        )
        .add_service(
            CloudAuthServiceServer::new(AuthGrpcApi {
                auth_service: services.auth_service.clone(),
            })
            .send_compressed(CompressionEncoding::Gzip)
            .accept_compressed(CompressionEncoding::Gzip),
        )
        .add_service(
            CloudLimitsServiceServer::new(LimitsGrpcApi {
                auth_service: services.auth_service.clone(),
                plan_limit_service: services.plan_limit_service.clone(),
            })
            .send_compressed(CompressionEncoding::Gzip)
            .accept_compressed(CompressionEncoding::Gzip),
        )
        .add_service(
            CloudLoginServiceServer::new(LoginGrpcApi {
                auth_service: services.auth_service.clone(),
                login_service: services.login_service.clone(),
                oauth2_service: services.oauth2_service.clone(),
            })
            .send_compressed(CompressionEncoding::Gzip)
            .accept_compressed(CompressionEncoding::Gzip),
        )
        .add_service(
            CloudProjectServiceServer::new(ProjectGrpcApi {
                auth_service: services.auth_service.clone(),
                project_service: services.project_service.clone(),
            })
            .send_compressed(CompressionEncoding::Gzip)
            .accept_compressed(CompressionEncoding::Gzip),
        )
        .add_service(
            CloudTokenServiceServer::new(TokenGrpcApi {
                auth_service: services.auth_service.clone(),
                token_service: services.token_service.clone(),
            })
            .send_compressed(CompressionEncoding::Gzip)
            .accept_compressed(CompressionEncoding::Gzip),
        )
        .serve(addr)
        .await
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use crate::grpcapi::get_authorisation_token;
    use cloud_common::model::TokenSecret as ModelTokenSecret;
    use tonic::metadata::MetadataMap;
    use uuid::Uuid;

    #[test]
    fn test_get_authorisation_token() {
        let mut m = MetadataMap::new();
        m.insert(
            "authorization",
            "Bearer 7E0BBC59-DB10-4A6F-B508-7673FE948315"
                .parse()
                .unwrap(),
        );
        assert_eq!(
            get_authorisation_token(m),
            Some(ModelTokenSecret::new(
                Uuid::parse_str("7E0BBC59-DB10-4A6F-B508-7673FE948315").unwrap()
            ))
        );

        let mut m = MetadataMap::new();
        m.insert(
            "authorization",
            "bearer   7E0BBC59-DB10-4A6F-B508-7673FE948315 "
                .parse()
                .unwrap(),
        );
        assert_eq!(
            get_authorisation_token(m),
            Some(ModelTokenSecret::new(
                Uuid::parse_str("7E0BBC59-DB10-4A6F-B508-7673FE948315").unwrap()
            ))
        );

        let mut m = MetadataMap::new();
        m.insert("authorization", "Bearer token".parse().unwrap());
        assert_eq!(get_authorisation_token(m), None);

        let mut m = MetadataMap::new();
        m.insert("authorization", "Bearer ".parse().unwrap());
        assert_eq!(get_authorisation_token(m), None);

        let mut m = MetadataMap::new();
        m.insert("authorization", "Bearer".parse().unwrap());
        assert_eq!(get_authorisation_token(m), None);
    }
}
