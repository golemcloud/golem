pub mod template;
pub mod worker;
pub mod api_definition_lookup_impl;

use crate::worker_bridge_request_executor::WorkerRequestToHttpResponse;
use async_trait::async_trait;
use golem_worker_service_base::api_definition::http::HttpApiDefinition;
use golem_worker_service_base::api_definition::{ApiDefinitionId, ApiVersion};
use golem_worker_service_base::app_config::WorkerServiceBaseConfig;
use golem_worker_service_base::auth::{CommonNamespace, EmptyAuthCtx};
use golem_worker_service_base::http::InputHttpRequest;
use golem_worker_service_base::repo::api_definition_repo::{
    ApiDefinitionRepo, InMemoryRegistry, RedisApiRegistry,
};
use golem_worker_service_base::service::api_definition::{
    ApiDefinitionKey, ApiDefinitionService, ApiDefinitionServiceDefault,
};
use golem_worker_service_base::service::api_definition_lookup::{
    ApiDefinitionLookup, ApiDefinitionLookupError,
};
use golem_worker_service_base::service::api_definition_validator::ApiDefinitionValidatorNoop;
use golem_worker_service_base::service::api_definition_validator::ApiDefinitionValidatorService;
use golem_worker_service_base::service::http::http_api_definition_validator::{
    HttpApiDefinitionValidator, RouteValidationError,
};
use golem_worker_service_base::service::template::{RemoteTemplateService, TemplateServiceNoop};
use golem_worker_service_base::service::worker::{
    WorkerRequestMetadata, WorkerServiceDefault, WorkerServiceNoOp,
};
use golem_worker_service_base::worker_bridge_execution::WorkerRequestExecutor;
use http::HeaderMap;
use poem::Response;
use std::sync::Arc;
use tracing::error;
use crate::service::api_definition_lookup_impl::CustomRequestDefinitionLookupDefault;

#[derive(Clone)]
pub struct Services {
    pub worker_service: worker::WorkerService,
    pub template_service: template::TemplateService,
    pub definition_service: Arc<
        dyn ApiDefinitionService<
                EmptyAuthCtx,
                CommonNamespace,
                HttpApiDefinition,
                RouteValidationError,
            > + Sync
            + Send,
    >,
    pub http_definition_lookup_service:
        Arc<dyn ApiDefinitionLookup<InputHttpRequest, HttpApiDefinition> + Sync + Send>,
    pub worker_to_http_service: Arc<dyn WorkerRequestExecutor<Response> + Sync + Send>,
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

        let worker_executor_grpc_clients: Arc<
            dyn golem_service_base::worker_executor_clients::WorkerExecutorClients + Sync + Send,
        > = Arc::new(
            golem_service_base::worker_executor_clients::WorkerExecutorClientsDefault::new(
                config.worker_executor_client_cache.max_capacity,
                config.worker_executor_client_cache.time_to_idle,
            ),
        );

        let template_service: template::TemplateService = {
            let config = &config.template_service;
            let uri = config.uri();
            let retry_config = config.retries.clone();

            Arc::new(RemoteTemplateService::new(uri, retry_config))
        };

        let worker_service: worker::WorkerService = Arc::new(WorkerServiceDefault::new(
            worker_executor_grpc_clients.clone(),
            template_service.clone(),
            routing_table_service.clone(),
        ));

        let worker_to_http_service: Arc<dyn WorkerRequestExecutor<Response> + Sync + Send> =
            Arc::new(WorkerRequestToHttpResponse::new(worker_service.clone()));

        let definition_repo: Arc<
            dyn ApiDefinitionRepo<CommonNamespace, HttpApiDefinition> + Sync + Send,
        > = Arc::new(RedisApiRegistry::new(&config.redis).await.map_err(|e| {
            error!("RedisApiRegistry - init error: {}", e);
            format!("RedisApiRegistry - init error: {}", e)
        })?);

        let definition_lookup_service = Arc::new(CustomRequestDefinitionLookupDefault::new(
            definition_repo.clone(),
        ));

        let api_definition_validator_service = Arc::new(HttpApiDefinitionValidator {});

        let definition_service: Arc<
            dyn ApiDefinitionService<
                    EmptyAuthCtx,
                    CommonNamespace,
                    HttpApiDefinition,
                    RouteValidationError,
                > + Sync
                + Send,
        > = Arc::new(ApiDefinitionServiceDefault::new(
            template_service.clone(),
            definition_repo.clone(),
            api_definition_validator_service.clone(),
        ));

        Ok(Services {
            worker_service,
            definition_service,
            http_definition_lookup_service: definition_lookup_service,
            worker_to_http_service,
            template_service,
            api_definition_validator_service,
        })
    }

    pub fn noop() -> Services {
        let template_service: template::TemplateService = Arc::new(TemplateServiceNoop {});

        let worker_service: worker::WorkerService = Arc::new(WorkerServiceNoOp {
            metadata: WorkerRequestMetadata {
                account_id: None,
                limits: None,
            },
        });

        let definition_repo: Arc<
            dyn ApiDefinitionRepo<CommonNamespace, HttpApiDefinition> + Sync + Send,
        > = Arc::new(InMemoryRegistry::default());

        let definition_lookup_service: Arc<
            dyn ApiDefinitionLookup<InputHttpRequest, HttpApiDefinition> + Sync + Send,
        > = Arc::new(CustomRequestDefinitionLookupDefault::new(
            definition_repo.clone(),
        ));

        let api_definition_validator_service: Arc<
            dyn ApiDefinitionValidatorService<HttpApiDefinition, RouteValidationError>
                + Sync
                + Send,
        > = Arc::new(ApiDefinitionValidatorNoop {});

        let definition_service = Arc::new(ApiDefinitionServiceDefault::new(
            template_service.clone(),
            Arc::new(InMemoryRegistry::default()),
            api_definition_validator_service.clone(),
        ));

        let worker_to_http_service: Arc<dyn WorkerRequestExecutor<Response> + Sync + Send> =
            Arc::new(WorkerRequestToHttpResponse::new(worker_service.clone()));

        Services {
            worker_service,
            definition_service,
            http_definition_lookup_service: definition_lookup_service,
            worker_to_http_service,
            template_service,
            api_definition_validator_service,
        }
    }
}
