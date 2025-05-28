pub mod api_certificate;
pub mod api_domain;
pub mod api_security;
pub mod auth;
pub mod file_server_binding_handler;
pub mod worker;
pub mod worker_request_executor;

use crate::aws_config::AwsConfig;
use crate::config::WorkerServiceCloudConfig;
use crate::repo::api_certificate::{ApiCertificateRepo, DbApiCertificateRepo};
use crate::repo::api_domain::{ApiDomainRepo, DbApiDomainRepo};
use crate::service::api_certificate::{
    AwsCertificateManager, CertificateManager, CertificateService, CertificateServiceDefault,
    InMemoryCertificateManager,
};
use crate::service::api_domain::{
    ApiDomainService, ApiDomainServiceDefault, AwsDomainRoute, InMemoryRegisterDomain,
    InMemoryRegisterDomainRoute, RegisterDomainRoute,
};
use crate::service::api_domain::{AwsRegisterDomain, RegisterDomain};
use crate::service::api_security::{SecuritySchemeService, SecuritySchemeServiceDefault};
use crate::service::auth::{AuthService, CloudAuthService};
use crate::service::worker::{WorkerService, WorkerServiceDefault};
use crate::service::worker_request_executor::CloudGatewayWorkerRequestExecutor;
use cloud_common::auth::{CloudAuthCtx, CloudNamespace};
use cloud_common::clients::limit::{LimitService, LimitServiceDefault};
use cloud_common::clients::project::{ProjectService, ProjectServiceDefault};
use cloud_common::model::TokenSecret;
use file_server_binding_handler::CloudWorkerServiceAdapter;
use golem_api_grpc::proto::golem::workerexecutor::v1::worker_executor_client::WorkerExecutorClient;
use golem_common::client::{GrpcClientConfig, MultiTargetGrpcClient};
use golem_common::config::DbConfig;
use golem_common::model::RetryConfig;
use golem_common::redis::RedisPool;
use golem_service_base::config::BlobStorageConfig;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::service::initial_component_files::InitialComponentFilesService;
use golem_service_base::service::routing_table::{RoutingTableService, RoutingTableServiceDefault};
use golem_service_base::storage::blob::BlobStorage;
use golem_worker_service_base::app_config::GatewaySessionStorageConfig;
use golem_worker_service_base::gateway_api_definition::http::HttpApiDefinition;
use golem_worker_service_base::gateway_execution::api_definition_lookup::{
    DefaultHttpApiDefinitionLookup, HttpApiDefinitionsLookup,
};
use golem_worker_service_base::gateway_execution::file_server_binding_handler::{
    DefaultFileServerBindingHandler, FileServerBindingHandler,
};
use golem_worker_service_base::gateway_execution::gateway_session::{
    GatewaySession, RedisGatewaySession, RedisGatewaySessionExpiration, SqliteGatewaySession,
    SqliteGatewaySessionExpiration,
};
use golem_worker_service_base::gateway_execution::http_handler_binding_handler::{
    DefaultHttpHandlerBindingHandler, HttpHandlerBindingHandler,
};
use golem_worker_service_base::gateway_execution::GatewayWorkerRequestExecutor;
use golem_worker_service_base::gateway_security::DefaultIdentityProvider;
use golem_worker_service_base::repo::api_definition::{ApiDefinitionRepo, DbApiDefinitionRepo};
use golem_worker_service_base::repo::api_deployment::{ApiDeploymentRepo, DbApiDeploymentRepo};
use golem_worker_service_base::repo::security_scheme::{DbSecuritySchemeRepo, SecuritySchemeRepo};
use golem_worker_service_base::service::component::{ComponentService, RemoteComponentService};
use golem_worker_service_base::service::gateway::api_definition::{
    ApiDefinitionService, ApiDefinitionServiceConfig, ApiDefinitionServiceDefault,
};
use golem_worker_service_base::service::gateway::api_definition_validator::ApiDefinitionValidatorService;
use golem_worker_service_base::service::gateway::api_deployment::{
    ApiDeploymentService, ApiDeploymentServiceDefault,
};
use golem_worker_service_base::service::gateway::http_api_definition_validator::HttpApiDefinitionValidator;
use golem_worker_service_base::service::gateway::security_scheme::DefaultSecuritySchemeService as BaseDefaultSecuritySchemeService;
use std::sync::Arc;
use std::time::Duration;
use tonic::codec::CompressionEncoding;
use tracing::{error, info};

#[derive(Clone)]
pub struct ApiServices {
    pub worker_auth_service: Arc<dyn AuthService + Send + Sync>,
    pub project_service: Arc<dyn ProjectService + Send + Sync>,
    pub limit_service: Arc<dyn LimitService + Send + Sync>,
    pub definition_service:
        Arc<dyn ApiDefinitionService<CloudAuthCtx, CloudNamespace> + Send + Sync>,
    pub deployment_service:
        Arc<dyn ApiDeploymentService<CloudAuthCtx, CloudNamespace> + Send + Sync>,
    pub domain_route: Arc<dyn RegisterDomainRoute + Send + Sync>,
    pub domain_service: Arc<dyn ApiDomainService + Send + Sync>,
    pub certificate_service: Arc<dyn CertificateService + Send + Sync>,
    pub component_service: Arc<dyn ComponentService<CloudNamespace, CloudAuthCtx>>,
    pub worker_service: Arc<dyn WorkerService + Send + Sync>,
    pub worker_request_to_http_service:
        Arc<dyn GatewayWorkerRequestExecutor<CloudNamespace> + Send + Sync>,
    pub http_request_api_definition_lookup_service:
        Arc<dyn HttpApiDefinitionsLookup<CloudNamespace> + Sync + Send>,
    pub file_server_binding_handler:
        Arc<dyn FileServerBindingHandler<CloudNamespace> + Send + Sync>,
    pub http_handler_binding_handler:
        Arc<dyn HttpHandlerBindingHandler<CloudNamespace> + Sync + Send>,
    pub security_scheme_service: Arc<dyn SecuritySchemeService + Send + Sync>,
    pub gateway_session_store: Arc<dyn GatewaySession + Send + Sync>,
}

impl ApiServices {
    pub async fn new(config: &WorkerServiceCloudConfig) -> Result<Self, String> {
        let project_service: Arc<dyn ProjectService + Send + Sync> = Arc::new(
            ProjectServiceDefault::new(&config.cloud_specific_config.cloud_service),
        );

        let auth_service: Arc<dyn AuthService + Send + Sync> = Arc::new(CloudAuthService::new(
            cloud_common::clients::auth::CloudAuthService::new(
                &config.cloud_specific_config.cloud_service,
            ),
            config.base_config.component_service.clone(),
        ));

        let (
            api_definition_repo,
            api_deployment_repo,
            api_certificate_repo,
            api_domain_repo,
            security_scheme_repo,
        ) = match config.base_config.db.clone() {
            DbConfig::Postgres(config) => {
                let db_pool = PostgresPool::configured(&config)
                    .await
                    .map_err(|e| format!("Init error (postgres pool): {e:?}"))?;
                let api_definition_repo: Arc<dyn ApiDefinitionRepo + Send + Sync> =
                    Arc::new(DbApiDefinitionRepo::new(db_pool.clone()));
                let api_deployment_repo: Arc<dyn ApiDeploymentRepo + Send + Sync> =
                    Arc::new(DbApiDeploymentRepo::new(db_pool.clone()));
                let api_certificate_repo: Arc<dyn ApiCertificateRepo + Send + Sync> =
                    Arc::new(DbApiCertificateRepo::new(db_pool.clone()));
                let api_domain_repo: Arc<dyn ApiDomainRepo + Send + Sync> =
                    Arc::new(DbApiDomainRepo::new(db_pool.clone()));
                let security_scheme_repo: Arc<dyn SecuritySchemeRepo + Sync + Send> =
                    Arc::new(DbSecuritySchemeRepo::new(db_pool.clone()));
                (
                    api_definition_repo,
                    api_deployment_repo,
                    api_certificate_repo,
                    api_domain_repo,
                    security_scheme_repo,
                )
            }
            DbConfig::Sqlite(config) => {
                let db_pool = SqlitePool::configured(&config)
                    .await
                    .map_err(|e| format!("Init error (sqlite pool): {e:?}"))?;
                let api_definition_repo: Arc<dyn ApiDefinitionRepo + Send + Sync> =
                    Arc::new(DbApiDefinitionRepo::new(db_pool.clone()));
                let api_deployment_repo: Arc<dyn ApiDeploymentRepo + Send + Sync> =
                    Arc::new(DbApiDeploymentRepo::new(db_pool.clone()));
                let api_certificate_repo: Arc<dyn ApiCertificateRepo + Send + Sync> =
                    Arc::new(DbApiCertificateRepo::new(db_pool.clone()));
                let api_domain_repo: Arc<dyn ApiDomainRepo + Send + Sync> =
                    Arc::new(DbApiDomainRepo::new(db_pool.clone()));
                let security_scheme_repo: Arc<dyn SecuritySchemeRepo + Sync + Send> =
                    Arc::new(DbSecuritySchemeRepo::new(db_pool.clone()));

                (
                    api_definition_repo,
                    api_deployment_repo,
                    api_certificate_repo,
                    api_domain_repo,
                    security_scheme_repo,
                )
            }
        };

        let gateway_session_store: Arc<dyn GatewaySession + Sync + Send> =
            match &config.base_config.gateway_session_storage {
                GatewaySessionStorageConfig::Redis(redis_config) => {
                    let redis = RedisPool::configured(redis_config)
                        .await
                        .map_err(|e| e.to_string())?;

                    let gateway_session_with_redis =
                        RedisGatewaySession::new(redis, RedisGatewaySessionExpiration::default());

                    Arc::new(gateway_session_with_redis)
                }

                GatewaySessionStorageConfig::Sqlite(sqlite_config) => {
                    let pool = SqlitePool::configured(sqlite_config)
                        .await
                        .map_err(|e| e.to_string())?;

                    let gateway_session_with_sqlite =
                        SqliteGatewaySession::new(pool, SqliteGatewaySessionExpiration::default())
                            .await?;

                    Arc::new(gateway_session_with_sqlite)
                }
            };

        let blob_storage: Arc<dyn BlobStorage + Send + Sync> = match &config
            .base_config
            .blob_storage
        {
            BlobStorageConfig::S3(config) => Arc::new(
                golem_service_base::storage::blob::s3::S3BlobStorage::new(config.clone()).await,
            ),
            BlobStorageConfig::LocalFileSystem(config) => Arc::new(
                golem_service_base::storage::blob::fs::FileSystemBlobStorage::new(&config.root)
                    .await?,
            ),
            BlobStorageConfig::Sqlite(sqlite) => {
                let pool = SqlitePool::configured(sqlite)
                    .await
                    .map_err(|e| format!("Failed to create sqlite pool: {}", e))?;
                Arc::new(
                    golem_service_base::storage::blob::sqlite::SqliteBlobStorage::new(pool.clone())
                        .await?,
                )
            }
            BlobStorageConfig::InMemory(_) => {
                Arc::new(golem_service_base::storage::blob::memory::InMemoryBlobStorage::new())
            }
            _ => {
                return Err("Unsupported blob storage configuration".to_string());
            }
        };

        let initial_component_files_service: Arc<InitialComponentFilesService> =
            Arc::new(InitialComponentFilesService::new(blob_storage.clone()));

        let api_definition_validator: Arc<
            dyn ApiDefinitionValidatorService<HttpApiDefinition> + Send + Sync,
        > = Arc::new(HttpApiDefinitionValidator {});

        let component_service: Arc<dyn ComponentService<CloudNamespace, CloudAuthCtx>> =
            Arc::new(RemoteComponentService::new(
                config.base_config.component_service.uri(),
                config.base_config.component_service.retries.clone(),
                config.base_config.component_service.connect_timeout,
            ));

        let identity_provider = Arc::new(DefaultIdentityProvider);

        let base_security_scheme_service = Arc::new(BaseDefaultSecuritySchemeService::new(
            security_scheme_repo,
            identity_provider,
        ));

        let security_scheme_service = Arc::new(SecuritySchemeServiceDefault::new(
            auth_service.clone(),
            base_security_scheme_service.clone(),
        ));

        let definition_service: Arc<
            dyn ApiDefinitionService<CloudAuthCtx, CloudNamespace> + Sync + Send,
        > = Arc::new(ApiDefinitionServiceDefault::new(
            component_service.clone(),
            api_definition_repo.clone(),
            api_deployment_repo.clone(),
            base_security_scheme_service.clone(),
            api_definition_validator,
            ApiDefinitionServiceConfig::default(),
        ));

        let deployment_service: Arc<
            dyn ApiDeploymentService<CloudAuthCtx, CloudNamespace> + Send + Sync,
        > = Arc::new(ApiDeploymentServiceDefault::new(
            api_deployment_repo.clone(),
            api_definition_repo.clone(),
            component_service.clone(),
        ));

        let (domain_route, domain_register_service, certificate_manager) = if config.is_local_env()
        {
            let domain_route: Arc<dyn RegisterDomainRoute + Send + Sync> =
                Arc::new(InMemoryRegisterDomainRoute::new(
                    &config.base_config.environment,
                    "golem.cloud.local",
                    &config.cloud_specific_config.domain_records,
                ));

            let certificate_manager: Arc<dyn CertificateManager + Send + Sync> =
                Arc::new(InMemoryCertificateManager::default());

            let domain_register_service: Arc<dyn RegisterDomain + Send + Sync> =
                Arc::new(InMemoryRegisterDomain::default());

            (domain_route, domain_register_service, certificate_manager)
        } else {
            let aws_config = AwsConfig::from_k8s_env();

            let aws_domain_route = AwsDomainRoute::new(
                &config.base_config.environment,
                &config.cloud_specific_config.workspace,
                &aws_config,
                &config.cloud_specific_config.domain_records,
            )
            .await
            .map_err(|e| {
                error!(
                    "AWS domain for environment: {}, workspace: {}, region: {:?} - init error: {}",
                    config.base_config.environment,
                    config.cloud_specific_config.workspace,
                    aws_config.region,
                    e
                );

                format!("Init error (aws domain): {e:?}")
            })?;

            info!(
                "AWS domain environment: {}, workspace: {}, region: {:?}, DNS name: {}",
                config.base_config.environment,
                config.cloud_specific_config.workspace,
                aws_config.region,
                aws_domain_route.load_balancer.dns_name
            );

            let domain_route: Arc<dyn RegisterDomainRoute + Send + Sync> =
                Arc::new(aws_domain_route);

            let aws_cm = AwsCertificateManager::new(
                &config.base_config.environment,
                &config.cloud_specific_config.workspace,
                &aws_config,
                &config.cloud_specific_config.domain_records,
            )
                .await
                .map_err(|e| {
                    error!(
                    "AWS Certificate Manager for environment: {}, workspace: {}, region: {:?} - init error: {}",
                    config.base_config.environment, config.cloud_specific_config.workspace, aws_config.region, e
                );

                    format!("Init error (aws cert): {e:?}")
                })?;

            info!(
                "AWS Certificate Manager environment: {}, workspace: {}, region: {:?}, DNS name: {}",
                config.base_config.environment,
                config.cloud_specific_config.workspace,
                aws_config.region,
                aws_cm.load_balancer.dns_name
            );

            let certificate_manager: Arc<dyn CertificateManager + Send + Sync> = Arc::new(aws_cm);

            let domain_register_service: Arc<dyn RegisterDomain + Send + Sync> = Arc::new(
                AwsRegisterDomain::new(&aws_config, &config.cloud_specific_config.domain_records),
            );

            (domain_route, domain_register_service, certificate_manager)
        };

        let domain_service: Arc<dyn ApiDomainService + Send + Sync> =
            Arc::new(ApiDomainServiceDefault::new(
                auth_service.clone(),
                domain_register_service.clone(),
                api_domain_repo.clone(),
            ));

        let certificate_service: Arc<dyn CertificateService + Send + Sync> =
            Arc::new(CertificateServiceDefault::new(
                auth_service.clone(),
                certificate_manager.clone(),
                api_certificate_repo.clone(),
            ));

        let limit_service: Arc<dyn LimitService + Send + Sync> = Arc::new(
            LimitServiceDefault::new(&config.cloud_specific_config.cloud_service),
        );

        let routing_table_service: Arc<dyn RoutingTableService + Send + Sync> = Arc::new(
            RoutingTableServiceDefault::new(config.base_config.routing_table.clone()),
        );

        let worker_executor_clients = MultiTargetGrpcClient::new(
            "worker_executor",
            |channel| {
                WorkerExecutorClient::new(channel)
                    .send_compressed(CompressionEncoding::Gzip)
                    .accept_compressed(CompressionEncoding::Gzip)
            },
            GrpcClientConfig {
                // TODO
                retries_on_unavailable: RetryConfig {
                    max_attempts: 0, // we want to invalidate the routing table asap
                    min_delay: Duration::from_millis(100),
                    max_delay: Duration::from_secs(2),
                    multiplier: 2.0,
                    max_jitter_factor: Some(0.15),
                },
                connect_timeout: Duration::from_secs(10),
            },
        );

        let worker_service: Arc<dyn WorkerService + Send + Sync> =
            Arc::new(WorkerServiceDefault::new(
                limit_service.clone(),
                Arc::new(
                    golem_worker_service_base::service::worker::WorkerServiceDefault::new(
                        worker_executor_clients.clone(),
                        config.base_config.worker_executor_retries.clone(),
                        routing_table_service.clone(),
                    ),
                ),
            ));

        let worker_request_to_http_service: Arc<
            dyn GatewayWorkerRequestExecutor<CloudNamespace> + Send + Sync,
        > = Arc::new(CloudGatewayWorkerRequestExecutor::new(
            worker_service.clone(),
        ));

        let http_request_api_definition_lookup_service = Arc::new(
            DefaultHttpApiDefinitionLookup::new(deployment_service.clone()),
        );

        let file_server_binding_handler: Arc<
            dyn FileServerBindingHandler<CloudNamespace> + Send + Sync,
        > = Arc::new(DefaultFileServerBindingHandler::new(
            component_service.clone(),
            initial_component_files_service.clone(),
            Arc::new(CloudWorkerServiceAdapter::new(worker_service.clone())),
            CloudAuthCtx::new(TokenSecret::new(
                config.base_config.component_service.access_token,
            )),
        ));

        let http_handler_binding_handler: Arc<
            dyn HttpHandlerBindingHandler<CloudNamespace> + Send + Sync,
        > = Arc::new(DefaultHttpHandlerBindingHandler::new(
            worker_request_to_http_service.clone(),
        ));

        Ok(Self {
            worker_auth_service: auth_service,
            limit_service,
            project_service,
            definition_service,
            deployment_service,
            domain_route,
            domain_service,
            certificate_service,
            component_service,
            worker_service,
            worker_request_to_http_service,
            http_request_api_definition_lookup_service,
            file_server_binding_handler,
            http_handler_binding_handler,
            security_scheme_service,
            gateway_session_store,
        })
    }
}
