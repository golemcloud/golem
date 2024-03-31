pub mod template;
pub mod worker;

use crate::worker_request_to_http_response::WorkerRequestToHttpResponse;
use async_trait::async_trait;
use golem_worker_service_base::http_api_definition::{
    HttpApiDefinition, ApiDefinitionId, ResponseMapping, Version,
};
use golem_worker_service_base::api_definition_repo::{
    ApiDefinitionRepo, InMemoryRegistry, RedisApiRegistry,
};
use golem_worker_service_base::app_config::WorkerServiceBaseConfig;
use golem_worker_service_base::auth::{CommonNamespace, EmptyAuthCtx};
use golem_worker_service_base::http_request::InputHttpRequest;
use golem_worker_service_base::oas_worker_bridge::{
    GOLEM_API_DEFINITION_ID_EXTENSION, GOLEM_API_DEFINITION_VERSION,
};
use golem_worker_service_base::service::api_definition::{
    ApiDefinitionKey, ApiDefinitionService, RegisterApiDefinitionDefault,
};
use golem_worker_service_base::service::api_definition_validator::{
    HttpApiDefinitionValidator, ApiDefinitionValidatorNoop, ApiDefinitionValidatorService,
};
use golem_worker_service_base::service::http_request_definition_lookup::{
    ApiDefinitionLookupError, HttpRequestDefinitionLookup,
};
use golem_worker_service_base::service::template::{RemoteTemplateService, TemplateServiceNoop};
use golem_worker_service_base::service::worker::{
    WorkerRequestMetadata, WorkerServiceDefault, WorkerServiceNoOp,
};
use golem_worker_service_base::worker_request_to_response::WorkerRequestToResponse;
use http::HeaderMap;
use poem::Response;
use std::sync::Arc;
use tracing::error;

#[derive(Clone)]
pub struct Services {
    pub worker_service: worker::WorkerService,
    pub template_service: template::TemplateService,
    pub definition_service:
        Arc<dyn ApiDefinitionService<EmptyAuthCtx, CommonNamespace> + Sync + Send>,
    pub definition_lookup_service: Arc<dyn HttpRequestDefinitionLookup + Sync + Send>,
    pub worker_to_http_service:
        Arc<dyn WorkerRequestToResponse<ResponseMapping, Response> + Sync + Send>,
    pub api_definition_validator_service: Arc<dyn ApiDefinitionValidatorService + Sync + Send>,
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

        let worker_to_http_service: Arc<
            dyn WorkerRequestToResponse<ResponseMapping, Response> + Sync + Send,
        > = Arc::new(WorkerRequestToHttpResponse::new(worker_service.clone()));

        let definition_repo: Arc<dyn ApiDefinitionRepo<CommonNamespace> + Sync + Send> =
            Arc::new(RedisApiRegistry::new(&config.redis).await.map_err(|e| {
                error!("RedisApiRegistry - init error: {}", e);
                format!("RedisApiRegistry - init error: {}", e)
            })?);

        let definition_lookup_service = Arc::new(CustomRequestDefinitionLookupDefault::new(
            definition_repo.clone(),
        ));

        let api_definition_validator_service: Arc<dyn ApiDefinitionValidatorService + Sync + Send> =
            Arc::new(HttpApiDefinitionValidator {});

        let definition_service: Arc<
            dyn ApiDefinitionService<EmptyAuthCtx, CommonNamespace> + Sync + Send,
        > = Arc::new(RegisterApiDefinitionDefault::new(
            template_service.clone(),
            definition_repo.clone(),
            api_definition_validator_service.clone(),
        ));

        Ok(Services {
            worker_service,
            definition_service,
            definition_lookup_service,
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

        let definition_repo: Arc<dyn ApiDefinitionRepo<CommonNamespace> + Sync + Send> =
            Arc::new(InMemoryRegistry::default());

        let definition_lookup_service: Arc<dyn HttpRequestDefinitionLookup + Sync + Send> =
            Arc::new(CustomRequestDefinitionLookupDefault::new(
                definition_repo.clone(),
            ));

        let api_definition_validator_service: Arc<dyn ApiDefinitionValidatorService + Sync + Send> =
            Arc::new(ApiDefinitionValidatorNoop {});

        let definition_service = Arc::new(RegisterApiDefinitionDefault::new(
            template_service.clone(),
            Arc::new(InMemoryRegistry::default()),
            api_definition_validator_service.clone(),
        ));

        let worker_to_http_service: Arc<
            dyn WorkerRequestToResponse<ResponseMapping, Response> + Sync + Send,
        > = Arc::new(WorkerRequestToHttpResponse::new(worker_service.clone()));

        Services {
            worker_service,
            definition_service,
            definition_lookup_service,
            worker_to_http_service,
            template_service,
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
    ) -> Result<HttpApiDefinition, ApiDefinitionLookupError> {
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
