use std::sync::Arc;

use crate::api_definition::ApiDefinitionId;
use async_trait::async_trait;
use hyper::header::HOST;
use poem::http::StatusCode;
use poem::{Body, Endpoint, Request, Response};
use tracing::{error, info};

use crate::api_request_route_resolver::RouteResolver;
use crate::http_request::{ApiInputPath, InputHttpRequest};
use crate::register::RegisterApiDefinition;
use crate::worker_bridge_reponse::GetWorkerBridgeResponse;
use crate::worker_request::GolemWorkerRequest;
use crate::worker_request_executor::{WorkerRequestExecutor, WorkerResponse};

// Executes custom request with the help of worker_request_executor and definition_service
#[derive(Clone)]
pub struct CustomRequestEndpoint {
    worker_request_executor: Arc<dyn WorkerRequestExecutor + Sync + Send>,
    definition_service: Arc<dyn RegisterApiDefinition + Sync + Send>,
}

impl CustomRequestEndpoint {
    pub fn new(
        worker_request_executor: Arc<dyn WorkerRequestExecutor + Sync + Send>,
        definition_service: Arc<dyn RegisterApiDefinition + Sync + Send>,
    ) -> Self {
        Self {
            worker_request_executor,
            definition_service,
        }
    }

    pub async fn execute(&self, request: Request) -> Response {
        let (req_parts, body) = request.into_parts();
        let headers = req_parts.headers;
        let uri = req_parts.uri;

        let host = match headers.get(HOST).and_then(|h| h.to_str().ok()) {
            Some(host) => host.to_string(),
            None => {
                return Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .body(Body::from_string("Missing host".to_string()));
            }
        };

        info!("API request host: {}", host);

        let json_request_body: serde_json::Value = if body.is_empty() {
            serde_json::Value::Null
        } else {
            match body.into_json().await {
                Ok(json_request_body) => json_request_body,
                Err(err) => {
                    error!("API request host: {} - error: {}", host, err);
                    return Response::builder()
                        .status(StatusCode::BAD_REQUEST)
                        .body(Body::from_string("Request body parse error".to_string()));
                }
            }
        };

        let api_request = InputHttpRequest {
            input_path: ApiInputPath {
                base_path: uri.path(),
                query_path: uri.query(),
            },
            headers: &headers,
            req_method: &req_parts.method,
            req_body: json_request_body,
        };

        let api_definition_id_res = headers
            .iter()
            .find(|(key, _)| key.as_str().to_lowercase() == "x-api-definition-id")
            .map(|(_, value)| value)
            .ok_or("Missing X-API-Definition-Id header");

        let api_definition_id_header = match api_definition_id_res {
            Ok(api_definition_id) => api_definition_id,
            Err(err) => {
                return Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .body(Body::from_string(err.to_string()));
            }
        };

        let api_definition_id_string = api_definition_id_header
            .to_str()
            .map_err(|e| format!("Invalid X-API-Definition-Id header: {}", e));

        let api_definition_id = match api_definition_id_string {
            Ok(api_definition_id) => ApiDefinitionId(api_definition_id.to_string()),
            Err(err) => {
                error!("Invalid X-API-Definition-Id header: {}", err);
                return Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .body(Body::from_string(err.to_string()));
            }
        };

        // Get ApiSpec corresponding to the API Definition Id
        let api_definition = match self.definition_service.get(&api_definition_id).await {
            Ok(Some(api_definition)) => api_definition,
            Ok(None) => {
                error!("API request definition id {} not found", &api_definition_id);

                return Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .body(Body::from_string(format!(
                        "API request definition id {} not found",
                        &api_definition_id
                    )));
            }
            Err(err) => {
                error!("API request id: {} - error: {}", &api_definition_id, err);
                return Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(Body::from_string("Internal error".to_string()));
            }
        };

        match api_request.resolve(&api_definition) {
            Some(resolved_route) => {
                let golem_worker_request =
                    match GolemWorkerRequest::from_resolved_route(&resolved_route) {
                        Ok(golem_worker_request) => golem_worker_request,
                        Err(e) => {
                            error!(
                                "API request id: {} - request error: {}",
                                &api_definition_id, e
                            );
                            return Response::builder()
                                .status(StatusCode::INTERNAL_SERVER_ERROR)
                                .body(Body::from_string(
                                    format!("API request error {}", e).to_string(),
                                ));
                        }
                    };

                // Execute the request using a executor
                let worker_response: WorkerResponse = match self
                    .worker_request_executor
                    .execute(golem_worker_request.clone())
                    .await
                {
                    Ok(response) => response,
                    Err(e) => {
                        error!("API request host: {} - error: {}", host, e);
                        return Response::builder()
                            .status(StatusCode::INTERNAL_SERVER_ERROR)
                            .body(Body::from_string(
                                format!("API request error {}", e).to_string(),
                            ));
                    }
                };

                match golem_worker_request.response_mapping {
                    Some(response_mapping) => {
                        let worker_bridge_response = worker_response.to_worker_bridge_response(
                            &response_mapping,
                            &resolved_route.resolved_variables,
                        );

                        match worker_bridge_response {
                            Ok(worker_bridge_response) => worker_bridge_response.to_http_response(),
                            Err(e) => {
                                error!(
                                    "API request id: {} - request error: {}",
                                    &api_definition_id, e
                                );
                                Response::builder().status(StatusCode::BAD_REQUEST).body(
                                    Body::from_string(format!("Bad request {}", e).to_string()),
                                )
                            }
                        }
                    }
                    None => {
                        let body: Body = Body::from_json(worker_response.result).unwrap();
                        Response::builder().body(body)
                    }
                }
            }

            None => {
                error!(
                    "API request id: {} - request error: {}",
                    &api_definition_id, "Unable to find a route"
                );

                Response::builder()
                    .status(StatusCode::METHOD_NOT_ALLOWED)
                    .finish()
            }
        }
    }
}

#[async_trait]
impl Endpoint for CustomRequestEndpoint {
    type Output = Response;

    async fn call(&self, req: Request) -> poem::Result<Self::Output> {
        let result = self.execute(req).await;
        Ok(result)
    }
}
