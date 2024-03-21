pub mod worker;

use crate::worker_request_to_http_response::WorkerRequestToHttpResponse;
use async_trait::async_trait;
use golem_service_base::service::auth::AuthService;
use golem_worker_service_base::api_definition::{
    ApiDefinition, ApiDefinitionId, ResponseMapping, Version,
};
use golem_worker_service_base::api_definition_repo::{
    ApiDefinitionRepo, InMemoryRegistry, RedisApiRegistry,
};
use golem_worker_service_base::app_config::WorkerServiceBaseConfig;
use golem_worker_service_base::auth::{AuthServiceNoop, CommonNamespace, EmptyAuthCtx};
use golem_worker_service_base::http_request::InputHttpRequest;
use golem_worker_service_base::oas_worker_bridge::{
    GOLEM_API_DEFINITION_ID_EXTENSION, GOLEM_API_DEFINITION_VERSION,
};
use golem_worker_service_base::service::api_definition::{
    ApiDefinitionKey, ApiDefinitionService, RegisterApiDefinitionDefault,
};
use golem_worker_service_base::service::api_definition_validator::{
    ApiDefinitionValidatorDefault, ApiDefinitionValidatorNoop, ApiDefinitionValidatorService,
};
use golem_worker_service_base::service::http_request_definition_lookup::{
    ApiDefinitionLookupError, HttpRequestDefinitionLookup,
};
use golem_worker_service_base::service::template::{
    TemplateService, TemplateServiceDefault, TemplateServiceNoop,
};
use golem_worker_service_base::worker_request_to_response::WorkerRequestToResponse;
use http::HeaderMap;
use poem::Response;
use std::sync::Arc;
use tracing::error;

#[derive(Clone)]
pub struct Services {
    pub worker_service: Arc<dyn worker::WorkerService + Sync + Send>,
    pub definition_service:
        Arc<dyn ApiDefinitionService<CommonNamespace, EmptyAuthCtx> + Sync + Send>,
    pub definition_lookup_service: Arc<dyn HttpRequestDefinitionLookup + Sync + Send>,
    pub worker_to_http_service:
        Arc<dyn WorkerRequestToResponse<ResponseMapping, Response> + Sync + Send>,
    pub template_service: Arc<dyn TemplateService + Sync + Send>,
    pub auth_service: Arc<dyn AuthService<EmptyAuthCtx, CommonNamespace> + Sync + Send>,
    pub api_definition_validator_service: Arc<dyn ApiDefinitionValidatorService + Sync + Send>,
}

impl Services {
    pub async fn new(config: &WorkerServiceBaseConfig) -> Result<Services, String> {
        let template_service: Arc<dyn TemplateService + Sync + Send> = {
            let config = &config.template_service;
            let uri = config.uri();
            let retries = config.retries.clone();

            Arc::new(TemplateServiceDefault::new(uri, retries))
        };

        let auth_service: Arc<dyn AuthService<EmptyAuthCtx, CommonNamespace> + Sync + Send> =
            Arc::new(AuthServiceNoop {});

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

        let worker_service: Arc<dyn worker::WorkerService + Sync + Send> =
            Arc::new(worker::WorkerServiceDefault::new(
                worker_executor_grpc_clients.clone(),
                template_service.clone(),
                routing_table_service.clone(),
            ));

        let definition_repo: Arc<dyn ApiDefinitionRepo<CommonNamespace> + Sync + Send> =
            Arc::new(RedisApiRegistry::new(&config.redis).await.map_err(|e| {
                error!("RedisApiRegistry - init error: {}", e);
                format!("RedisApiRegistry - init error: {}", e)
            })?);

        let definition_lookup_service = Arc::new(CustomRequestDefinitionLookupDefault::new(
            definition_repo.clone(),
        ));

        let api_definition_validator_service: Arc<dyn ApiDefinitionValidatorService + Sync + Send> =
            Arc::new(ApiDefinitionValidatorDefault::new(template_service.clone()));

        let definition_service: Arc<
            dyn ApiDefinitionService<CommonNamespace, EmptyAuthCtx> + Sync + Send,
        > = Arc::new(RegisterApiDefinitionDefault::new(
            Arc::new(AuthServiceNoop {}),
            definition_repo.clone(),
            api_definition_validator_service.clone(),
        ));

        let worker_to_http_service: Arc<
            dyn WorkerRequestToResponse<ResponseMapping, Response> + Sync + Send,
        > = Arc::new(WorkerRequestToHttpResponse::new(
            worker::WorkerServiceDefault::new(
                worker_executor_grpc_clients.clone(),
                template_service.clone(),
                routing_table_service.clone(),
            ),
        ));

        Ok(Services {
            worker_service,
            definition_service,
            definition_lookup_service,
            worker_to_http_service,
            template_service,
            auth_service,
            api_definition_validator_service,
        })
    }

    pub fn noop() -> Services {
        let template_service: Arc<dyn TemplateService + Sync + Send> =
            Arc::new(TemplateServiceNoop {});

        let routing_table_service: Arc<
            dyn golem_service_base::routing_table::RoutingTableService + Send + Sync,
        > = Arc::new(golem_service_base::routing_table::RoutingTableServiceNoop {});

        let worker_executor_grpc_clients: Arc<
            dyn golem_service_base::worker_executor_clients::WorkerExecutorClients + Sync + Send,
        > = Arc::new(golem_service_base::worker_executor_clients::WorkerExecutorClientsNoop {});

        let worker_service: Arc<dyn worker::WorkerService + Sync + Send> =
            Arc::new(worker::WorkerServiceNoOp {});

        let definition_repo: Arc<dyn ApiDefinitionRepo<CommonNamespace> + Sync + Send> =
            Arc::new(InMemoryRegistry::default());

        let definition_lookup_service: Arc<dyn HttpRequestDefinitionLookup + Sync + Send> =
            Arc::new(CustomRequestDefinitionLookupDefault::new(
                definition_repo.clone(),
            ));

        let definition_service = Arc::new(RegisterApiDefinitionDefault::new(
            Arc::new(AuthServiceNoop {}),
            Arc::new(InMemoryRegistry::default()),
            Arc::new(ApiDefinitionValidatorNoop {}),
        ));

        let worker_to_http_service: Arc<
            dyn WorkerRequestToResponse<ResponseMapping, Response> + Sync + Send,
        > = Arc::new(WorkerRequestToHttpResponse::new(
            worker::WorkerServiceDefault::new(
                worker_executor_grpc_clients.clone(),
                template_service.clone(),
                routing_table_service.clone(),
            ),
        ));

        let auth_service: Arc<dyn AuthService<EmptyAuthCtx, CommonNamespace> + Sync + Send> =
            Arc::new(AuthServiceNoop {});

        let api_definition_validator_service: Arc<dyn ApiDefinitionValidatorService + Sync + Send> =
            Arc::new(ApiDefinitionValidatorNoop {});

        Services {
            worker_service,
            definition_service,
            definition_lookup_service,
            worker_to_http_service,
            template_service,
            auth_service,
            api_definition_validator_service,
        }
    }
}

pub struct CustomRequestDefinitionLookupDefault {
    register_api_definition_repo: Arc<dyn ApiDefinitionRepo<CommonNamespace> + Sync + Send>,
}

impl CustomRequestDefinitionLookupDefault {
    pub fn new(
        register_api_definition_repo: Arc<dyn ApiDefinitionRepo<CommonNamespace> + Sync + Send>,
    ) -> Self {
        Self {
            register_api_definition_repo,
        }
    }
}

#[async_trait]
impl HttpRequestDefinitionLookup for CustomRequestDefinitionLookupDefault {
    async fn get(
        &self,
        input_http_request: &InputHttpRequest<'_>,
    ) -> Result<ApiDefinition, ApiDefinitionLookupError> {
        let api_definition_id = match get_header_value(
            input_http_request.headers,
            GOLEM_API_DEFINITION_ID_EXTENSION,
        ) {
            Ok(api_definition_id) => Ok(ApiDefinitionId(api_definition_id.to_string())),
            Err(err) => Err(ApiDefinitionLookupError(format!(
                "{} not found in the request headers. Error: {}",
                GOLEM_API_DEFINITION_ID_EXTENSION, err
            ))),
        }?;

        let version =
            match get_header_value(input_http_request.headers, GOLEM_API_DEFINITION_VERSION) {
                Ok(version) => Ok(Version(version)),
                Err(err) => Err(ApiDefinitionLookupError(format!(
                    "{} not found in the request headers. Error: {}",
                    GOLEM_API_DEFINITION_VERSION, err
                ))),
            }?;

        let api_key = ApiDefinitionKey {
            namespace: CommonNamespace::default(),
            id: api_definition_id.clone(),
            version: version.clone(),
        };

        let value = self
            .register_api_definition_repo
            .get(&api_key)
            .await
            .map_err(|err| {
                error!("Error getting api definition from the repo: {}", err);
                ApiDefinitionLookupError(format!(
                    "Error getting api definition from the repo: {}",
                    err
                ))
            })?;

        value.ok_or(ApiDefinitionLookupError(format!(
            "Api definition with id: {} and version: {} not found",
            &api_definition_id, &version
        )))
    }
}

fn get_header_value(headers: &HeaderMap, header_name: &str) -> Result<String, String> {
    let header_value = headers
        .iter()
        .find(|(key, _)| key.as_str().to_lowercase() == header_name)
        .map(|(_, value)| value)
        .ok_or(format!("Missing {} header", header_name))?;

    header_value
        .to_str()
        .map(|x| x.to_string())
        .map_err(|e| format!("Invalid value for the header {} error: {}", header_name, e))
}
