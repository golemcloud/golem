use crate::aws_config::AwsConfig;
use crate::config::WorkerServiceCloudConfig;
use crate::service::api_certificate::{
    AwsCertificateManager, CertificateManager, CertificateService, CertificateServiceDefault,
    CertificateServiceNoop,
};

use crate::repo::api_certificate::{ApiCertificateRepo, DbApiCertificateRepo};
use crate::repo::api_domain::{ApiDomainRepo, DbApiDomainRepo};
use crate::service::api_definition::{ApiDefinitionService, ApiDefinitionServiceDefault};
use crate::service::api_domain::{
    ApiDomainService, ApiDomainServiceDefault, ApiDomainServiceNoop, AwsDomainRoute,
    RegisterDomainRoute, RegisterDomainRouteNoop,
};
use crate::service::api_domain::{AwsRegisterDomain, RegisterDomain};
use crate::service::auth::{
    AuthService, CloudAuthCtx, CloudAuthService, CloudAuthServiceNoop, CloudNamespace,
};
use crate::service::limit::{LimitService, LimitServiceDefault, LimitServiceNoop};
use crate::service::project::{ProjectService, ProjectServiceDefault, ProjectServiceNoop};
use crate::service::worker::{WorkerService, WorkerServiceDefault, WorkerServiceNoop};
use crate::worker_component_metadata_fetcher::DefaultWorkerComponentMetadataFetcher;
use crate::worker_request_to_http_response::CloudWorkerRequestToHttpResponse;
use golem_worker_service_base::api_definition::http::HttpApiDefinition;
use golem_worker_service_base::evaluator::WorkerMetadataFetcher;
use golem_worker_service_base::http::InputHttpRequest;

use golem_service_base::config::DbConfig;
use golem_service_base::db;
use golem_worker_service_base::repo::api_definition::{ApiDefinitionRepo, DbApiDefinitionRepo};
use golem_worker_service_base::repo::api_deployment::{ApiDeploymentRepo, DbApiDeploymentRepo};
use golem_worker_service_base::service::api_definition::{
    ApiDefinitionService as BaseApiDefinitionService,
    ApiDefinitionServiceDefault as BaseApiDefinitionServiceDefault, ApiDefinitionServiceNoop,
};
use golem_worker_service_base::service::api_definition_lookup::{
    ApiDefinitionsLookup, HttpApiDefinitionLookup,
};
use golem_worker_service_base::service::api_definition_validator::ApiDefinitionValidatorService;
use golem_worker_service_base::service::api_deployment::{
    ApiDeploymentService, ApiDeploymentServiceDefault, ApiDeploymentServiceNoop,
};
use golem_worker_service_base::service::component::{ComponentService, RemoteComponentService};
use golem_worker_service_base::service::http::http_api_definition_validator::{
    HttpApiDefinitionValidator, RouteValidationError,
};
use golem_worker_service_base::worker_bridge_execution::WorkerRequestExecutor;
use std::sync::Arc;
use tracing::{error, info};

pub mod api_certificate;
pub mod api_definition;
pub mod api_domain;
pub mod auth;
mod limit;
pub mod project;
pub mod worker;

#[derive(Clone)]
pub struct ApiServices {
    pub auth_service: Arc<dyn AuthService + Sync + Send>,
    pub project_service: Arc<dyn ProjectService + Sync + Send>,
    pub limit_service: Arc<dyn LimitService + Sync + Send>,
    pub definition_service: Arc<dyn ApiDefinitionService + Sync + Send>,
    pub deployment_service: Arc<dyn ApiDeploymentService<CloudNamespace> + Sync + Send>,
    pub domain_route: Arc<dyn RegisterDomainRoute + Sync + Send>,
    pub domain_service: Arc<dyn ApiDomainService + Sync + Send>,
    pub certificate_service: Arc<dyn CertificateService + Sync + Send>,
    pub component_service: Arc<dyn ComponentService<CloudAuthCtx> + Sync + Send>,
    pub worker_service: Arc<dyn WorkerService + Sync + Send>,
    // Custom request specific services
    pub worker_metadata_fetcher: Arc<dyn WorkerMetadataFetcher + Sync + Send>,
    pub worker_request_to_http_service: Arc<dyn WorkerRequestExecutor + Sync + Send>,
    pub http_request_api_definition_lookup_service:
        Arc<dyn ApiDefinitionsLookup<InputHttpRequest, HttpApiDefinition> + Sync + Send>,
}

pub async fn get_api_services(
    config: &WorkerServiceCloudConfig,
) -> Result<ApiServices, std::io::Error> {
    let project_service: Arc<dyn ProjectService + Sync + Send> =
        Arc::new(ProjectServiceDefault::new(&config.cloud_service));

    let auth_service: Arc<dyn AuthService + Sync + Send> = Arc::new(CloudAuthService::new(
        project_service.clone(),
        config.base_config.component_service.clone(),
    ));

    let (api_definition_repo, api_deployment_repo, api_certificate_repo, api_domain_repo) =
        match config.base_config.db.clone() {
            DbConfig::Postgres(c) => {
                let db_pool = db::create_postgres_pool(&c).await.map_err(|e| {
                    std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("Init error (pg pool): {e:?}"),
                    )
                })?;
                let api_definition_repo: Arc<dyn ApiDefinitionRepo + Sync + Send> =
                    Arc::new(DbApiDefinitionRepo::new(db_pool.clone().into()));
                let api_deployment_repo: Arc<dyn ApiDeploymentRepo + Sync + Send> =
                    Arc::new(DbApiDeploymentRepo::new(db_pool.clone().into()));
                let api_certificate_repo: Arc<dyn ApiCertificateRepo + Sync + Send> =
                    Arc::new(DbApiCertificateRepo::new(db_pool.clone().into()));
                let api_domain_repo: Arc<dyn ApiDomainRepo + Sync + Send> =
                    Arc::new(DbApiDomainRepo::new(db_pool.clone().into()));
                (
                    api_definition_repo,
                    api_deployment_repo,
                    api_certificate_repo,
                    api_domain_repo,
                )
            }
            DbConfig::Sqlite(c) => {
                let db_pool = db::create_sqlite_pool(&c).await.map_err(|e| {
                    std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("Init error (sqlite pool): {e:?}"),
                    )
                })?;
                let api_definition_repo: Arc<dyn ApiDefinitionRepo + Sync + Send> =
                    Arc::new(DbApiDefinitionRepo::new(db_pool.clone().into()));
                let api_deployment_repo: Arc<dyn ApiDeploymentRepo + Sync + Send> =
                    Arc::new(DbApiDeploymentRepo::new(db_pool.clone().into()));
                let api_certificate_repo: Arc<dyn ApiCertificateRepo + Sync + Send> =
                    Arc::new(DbApiCertificateRepo::new(db_pool.clone().into()));
                let api_domain_repo: Arc<dyn ApiDomainRepo + Sync + Send> =
                    Arc::new(DbApiDomainRepo::new(db_pool.clone().into()));
                (
                    api_definition_repo,
                    api_deployment_repo,
                    api_certificate_repo,
                    api_domain_repo,
                )
            }
        };

    let api_definition_validator: Arc<
        dyn ApiDefinitionValidatorService<HttpApiDefinition, RouteValidationError> + Send + Sync,
    > = Arc::new(HttpApiDefinitionValidator {});

    let component_service: Arc<dyn ComponentService<CloudAuthCtx> + Sync + Send> =
        Arc::new(RemoteComponentService::new(
            config.base_config.component_service.uri(),
            config.base_config.component_service.retries.clone(),
        ));

    let base_definition_service: Arc<
        dyn BaseApiDefinitionService<CloudAuthCtx, CloudNamespace, RouteValidationError>
            + Sync
            + Send,
    > = Arc::new(BaseApiDefinitionServiceDefault::new(
        component_service.clone(),
        api_definition_repo.clone(),
        api_definition_validator,
    ));

    let definition_service: Arc<dyn ApiDefinitionService + Send + Sync> = Arc::new(
        ApiDefinitionServiceDefault::new(auth_service.clone(), base_definition_service),
    );

    let deployment_service: Arc<dyn ApiDeploymentService<CloudNamespace> + Sync + Send> = Arc::new(
        ApiDeploymentServiceDefault::new(api_deployment_repo.clone(), api_definition_repo.clone()),
    );

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

        std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("Init error (aws domain): {e:?}"),
        )
    })?;

    info!(
        "AWS domain environment: {}, workspace: {}, region: {:?}, DNS name: {}",
        config.base_config.environment,
        config.cloud_specific_config.workspace,
        aws_config.region,
        aws_domain_route.load_balancer.dns_name
    );

    let domain_route: Arc<dyn RegisterDomainRoute + Sync + Send> = Arc::new(aws_domain_route);

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

            std::io::Error::new(std::io::ErrorKind::Other, format!("Init error (aws cert): {e:?}"))
        })?;

    info!(
        "AWS Certificate Manager environment: {}, workspace: {}, region: {:?}, DNS name: {}",
        config.base_config.environment,
        config.cloud_specific_config.workspace,
        aws_config.region,
        aws_cm.load_balancer.dns_name
    );

    let certificate_manager: Arc<dyn CertificateManager + Sync + Send> = Arc::new(aws_cm);

    let domain_register_service: Arc<dyn RegisterDomain + Sync + Send> = Arc::new(
        AwsRegisterDomain::new(&aws_config, &config.cloud_specific_config.domain_records),
    );

    let domain_service: Arc<dyn ApiDomainService + Sync + Send> =
        Arc::new(ApiDomainServiceDefault::new(
            auth_service.clone(),
            domain_register_service.clone(),
            api_domain_repo.clone(),
        ));

    let certificate_service: Arc<dyn CertificateService + Sync + Send> =
        Arc::new(CertificateServiceDefault::new(
            auth_service.clone(),
            certificate_manager.clone(),
            api_certificate_repo.clone(),
        ));

    let limit_service: Arc<dyn LimitService + Sync + Send> =
        Arc::new(LimitServiceDefault::new(&config.cloud_service));

    let routing_table_service: Arc<
        dyn golem_service_base::routing_table::RoutingTableService + Send + Sync,
    > = Arc::new(
        golem_service_base::routing_table::RoutingTableServiceDefault::new(
            config.base_config.routing_table.clone(),
        ),
    );

    let worker_executor_clients: Arc<
        dyn golem_service_base::worker_executor_clients::WorkerExecutorClients + Sync + Send,
    > = Arc::new(
        golem_service_base::worker_executor_clients::WorkerExecutorClientsDefault::new(
            config.base_config.worker_executor_client_cache.max_capacity,
            config.base_config.worker_executor_client_cache.time_to_idle,
        ),
    );

    let worker_service: Arc<dyn WorkerService + Sync + Send> = Arc::new(WorkerServiceDefault::new(
        auth_service.clone(),
        limit_service.clone(),
        Arc::new(
            golem_worker_service_base::service::worker::WorkerServiceDefault::new(
                worker_executor_clients.clone(),
                component_service.clone(),
                routing_table_service.clone(),
            ),
        ),
    ));

    let worker_metadata_fetcher: Arc<dyn WorkerMetadataFetcher + Sync + Send> =
        Arc::new(DefaultWorkerComponentMetadataFetcher::new(
            worker_service.clone(),
            config.base_config.component_service.access_token,
        ));

    let worker_request_to_http_service: Arc<dyn WorkerRequestExecutor + Sync + Send> =
        Arc::new(CloudWorkerRequestToHttpResponse::new(
            worker_service.clone(),
            config.base_config.component_service.access_token,
        ));

    let http_request_api_definition_lookup_service =
        Arc::new(HttpApiDefinitionLookup::new(deployment_service.clone()));

    Ok(ApiServices {
        auth_service,
        limit_service,
        project_service,
        definition_service,
        deployment_service,
        domain_route,
        domain_service,
        certificate_service,
        component_service,
        worker_service,
        worker_metadata_fetcher,
        worker_request_to_http_service,
        http_request_api_definition_lookup_service,
    })
}

pub fn get_api_services_local(config: &WorkerServiceCloudConfig) -> ApiServices {
    let auth_service: Arc<dyn AuthService + Sync + Send> =
        Arc::new(CloudAuthServiceNoop::default());
    let component_service: Arc<dyn ComponentService<CloudAuthCtx> + Sync + Send> =
        Arc::new(RemoteComponentService::new(
            config.base_config.component_service.uri(),
            config.base_config.component_service.retries.clone(),
        ));

    let base_definition_service: Arc<
        dyn BaseApiDefinitionService<CloudAuthCtx, CloudNamespace, RouteValidationError>
            + Sync
            + Send,
    > = Arc::new(ApiDefinitionServiceNoop::default());

    let definition_service: Arc<dyn ApiDefinitionService + Send + Sync> = Arc::new(
        ApiDefinitionServiceDefault::new(auth_service.clone(), base_definition_service),
    );

    let deployment_service: Arc<dyn ApiDeploymentService<CloudNamespace> + Sync + Send> =
        Arc::new(ApiDeploymentServiceNoop::default());
    let domain_route: Arc<dyn RegisterDomainRoute + Sync + Send> =
        Arc::new(RegisterDomainRouteNoop::new(
            &config.base_config.environment,
            "golem.cloud.local",
            &config.cloud_specific_config.domain_records,
        ));

    let project_service: Arc<dyn ProjectService + Sync + Send> =
        Arc::new(ProjectServiceNoop::default());

    let domain_service: Arc<dyn ApiDomainService + Sync + Send> =
        Arc::new(ApiDomainServiceNoop::default());

    let certificate_service: Arc<dyn CertificateService + Sync + Send> =
        Arc::new(CertificateServiceNoop::default());

    let limit_service: Arc<dyn LimitService + Sync + Send> = Arc::new(LimitServiceNoop::default());

    let worker_service: Arc<dyn WorkerService + Sync + Send> =
        Arc::new(WorkerServiceNoop::default());

    let worker_metadata_fetcher: Arc<dyn WorkerMetadataFetcher + Sync + Send> =
        Arc::new(DefaultWorkerComponentMetadataFetcher::new(
            worker_service.clone(),
            config.base_config.component_service.access_token,
        ));

    let worker_request_to_http_service: Arc<dyn WorkerRequestExecutor + Sync + Send> =
        Arc::new(CloudWorkerRequestToHttpResponse::new(
            worker_service.clone(),
            config.base_config.component_service.access_token,
        ));

    let http_request_api_definition_lookup_service =
        Arc::new(HttpApiDefinitionLookup::new(deployment_service.clone()));

    ApiServices {
        project_service,
        auth_service,
        limit_service,
        definition_service,
        deployment_service,
        domain_route,
        domain_service,
        certificate_service,
        component_service,
        worker_service,
        worker_metadata_fetcher,
        worker_request_to_http_service,
        http_request_api_definition_lookup_service,
    }
}
