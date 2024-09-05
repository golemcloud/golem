pub mod component;
pub mod worker;

use crate::worker_bridge_request_executor::UnauthorisedWorkerRequestExecutor;

use golem_worker_service_base::api_definition::http::{
    CompiledHttpApiDefinition, HttpApiDefinition,
};

use golem_service_base::auth::{DefaultNamespace, EmptyAuthCtx};
use golem_worker_service_base::app_config::WorkerServiceBaseConfig;
use golem_worker_service_base::http::InputHttpRequest;

use golem_worker_service_base::repo::api_definition;
use golem_worker_service_base::repo::api_deployment;
use golem_worker_service_base::service::api_definition::{
    ApiDefinitionService, ApiDefinitionServiceDefault, ApiDefinitionServiceNoop,
};
use golem_worker_service_base::service::api_definition_lookup::{
    ApiDefinitionsLookup, HttpApiDefinitionLookup,
};
use golem_worker_service_base::service::api_definition_validator::ApiDefinitionValidatorNoop;
use golem_worker_service_base::service::api_definition_validator::ApiDefinitionValidatorService;
use golem_worker_service_base::service::component::{ComponentServiceNoop, RemoteComponentService};
use golem_worker_service_base::service::http::http_api_definition_validator::{
    HttpApiDefinitionValidator, RouteValidationError,
};
use golem_worker_service_base::service::worker::{
    WorkerRequestMetadata, WorkerServiceDefault, WorkerServiceNoOp,
};
use golem_worker_service_base::worker_bridge_execution::WorkerRequestExecutor;

use golem_api_grpc::proto::golem::workerexecutor::v1::worker_executor_client::WorkerExecutorClient;
use golem_common::client::{GrpcClientConfig, MultiTargetGrpcClient};
use golem_common::config::RetryConfig;

use golem_worker_service_base::service::api_deployment::{
    ApiDeploymentService, ApiDeploymentServiceDefault, ApiDeploymentServiceNoop,
};
use std::sync::Arc;
use std::time::Duration;

use golem_common::config::DbConfig;
use golem_service_base::db;

#[derive(Clone)]
pub struct Services {
    pub worker_service: worker::WorkerService,
    pub component_service: component::ComponentService,
    pub definition_service: Arc<
        dyn ApiDefinitionService<EmptyAuthCtx, DefaultNamespace, RouteValidationError>
            + Sync
            + Send,
    >,
    pub deployment_service: Arc<dyn ApiDeploymentService<DefaultNamespace> + Sync + Send>,
    pub http_definition_lookup_service:
        Arc<dyn ApiDefinitionsLookup<InputHttpRequest, CompiledHttpApiDefinition> + Sync + Send>,
    pub worker_to_http_service: Arc<dyn WorkerRequestExecutor + Sync + Send>,
    pub api_definition_validator_service: Arc<
        dyn ApiDefinitionValidatorService<HttpApiDefinition, RouteValidationError> + Sync + Send,
    >,
}

impl Services {
    pub async fn new(config: &WorkerServiceBaseConfig) -> Result<Services, String> {
        let routing_table_service: Arc<
            dyn golem_service_base::routing_table::RoutingTableService + Send + Sync,
        > = Arc::new(
            golem_service_base::routing_table::RoutingTableServiceDefault::new(
                config.routing_table.clone(),
            ),
        );

        let worker_executor_grpc_clients = MultiTargetGrpcClient::new(
            WorkerExecutorClient::new,
            GrpcClientConfig {
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

        let component_service: component::ComponentService = {
            let config = &config.component_service;
            let uri = config.uri();
            let retry_config = config.retries.clone();

            Arc::new(RemoteComponentService::new(uri, retry_config))
        };

        let worker_service: worker::WorkerService = Arc::new(WorkerServiceDefault::new(
            worker_executor_grpc_clients.clone(),
            config.worker_executor_retries.clone(),
            component_service.clone(),
            routing_table_service.clone(),
        ));

        let worker_to_http_service: Arc<dyn WorkerRequestExecutor + Sync + Send> = Arc::new(
            UnauthorisedWorkerRequestExecutor::new(worker_service.clone()),
        );

        let (api_definition_repo, api_deployment_repo) = match config.db.clone() {
            DbConfig::Postgres(c) => {
                let db_pool = db::create_postgres_pool(&c)
                    .await
                    .map_err(|e| e.to_string())?;
                let api_definition_repo: Arc<dyn api_definition::ApiDefinitionRepo + Sync + Send> =
                    Arc::new(api_definition::DbApiDefinitionRepo::new(
                        db_pool.clone().into(),
                    ));
                let api_deployment_repo: Arc<dyn api_deployment::ApiDeploymentRepo + Sync + Send> =
                    Arc::new(api_deployment::DbApiDeploymentRepo::new(
                        db_pool.clone().into(),
                    ));
                (api_definition_repo, api_deployment_repo)
            }
            DbConfig::Sqlite(c) => {
                let db_pool = db::create_sqlite_pool(&c)
                    .await
                    .map_err(|e| e.to_string())?;
                let api_definition_repo: Arc<dyn api_definition::ApiDefinitionRepo + Sync + Send> =
                    Arc::new(api_definition::DbApiDefinitionRepo::new(
                        db_pool.clone().into(),
                    ));
                let api_deployment_repo: Arc<dyn api_deployment::ApiDeploymentRepo + Sync + Send> =
                    Arc::new(api_deployment::DbApiDeploymentRepo::new(
                        db_pool.clone().into(),
                    ));
                (api_definition_repo, api_deployment_repo)
            }
        };

        let api_definition_validator_service = Arc::new(HttpApiDefinitionValidator {});

        let definition_service: Arc<
            dyn ApiDefinitionService<EmptyAuthCtx, DefaultNamespace, RouteValidationError>
                + Sync
                + Send,
        > = Arc::new(ApiDefinitionServiceDefault::new(
            component_service.clone(),
            api_definition_repo.clone(),
            api_deployment_repo.clone(),
            api_definition_validator_service.clone(),
        ));

        let deployment_service: Arc<dyn ApiDeploymentService<DefaultNamespace> + Sync + Send> =
            Arc::new(ApiDeploymentServiceDefault::new(
                api_deployment_repo.clone(),
                api_definition_repo.clone(),
            ));

        let http_definition_lookup_service =
            Arc::new(HttpApiDefinitionLookup::new(deployment_service.clone()));

        Ok(Services {
            worker_service,
            definition_service,
            deployment_service,
            http_definition_lookup_service,
            worker_to_http_service,
            component_service,
            api_definition_validator_service,
        })
    }

    pub fn noop() -> Services {
        let component_service: component::ComponentService =
            Arc::new(ComponentServiceNoop::default());

        let worker_service: worker::WorkerService = Arc::new(WorkerServiceNoOp {
            metadata: WorkerRequestMetadata {
                account_id: None,
                limits: None,
            },
        });

        let api_definition_validator_service: Arc<
            dyn ApiDefinitionValidatorService<HttpApiDefinition, RouteValidationError>
                + Sync
                + Send,
        > = Arc::new(ApiDefinitionValidatorNoop::default());

        let worker_to_http_service: Arc<dyn WorkerRequestExecutor + Sync + Send> = Arc::new(
            UnauthorisedWorkerRequestExecutor::new(worker_service.clone()),
        );

        let definition_service: Arc<
            dyn ApiDefinitionService<EmptyAuthCtx, DefaultNamespace, RouteValidationError>
                + Sync
                + Send,
        > = Arc::new(ApiDefinitionServiceNoop::default());

        let deployment_service: Arc<dyn ApiDeploymentService<DefaultNamespace> + Sync + Send> =
            Arc::new(ApiDeploymentServiceNoop::default());

        let http_definition_lookup_service =
            Arc::new(HttpApiDefinitionLookup::new(deployment_service.clone()));

        Services {
            worker_service,
            definition_service,
            deployment_service,
            http_definition_lookup_service,
            worker_to_http_service,
            component_service,
            api_definition_validator_service,
        }
    }
}
