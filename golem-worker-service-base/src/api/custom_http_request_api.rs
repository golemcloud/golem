use std::sync::Arc;

use crate::api_definition::{ApiDefinitionId, ResponseMapping, Version};
use async_trait::async_trait;
use http::HeaderMap;
use hyper::header::HOST;
use poem::http::StatusCode;
use poem::{Body, Endpoint, Request, Response};
use tracing::{error, info};

use crate::api_request_route_resolver::RouteResolver;
use crate::http_request::{ApiInputPath, InputHttpRequest};
use crate::oas_worker_bridge::{GOLEM_API_DEFINITION_ID_EXTENSION, GOLEM_API_DEFINITION_VERSION};
use crate::register::{ApiDefinitionKey, RegisterApiDefinition};
use crate::worker_request::WorkerRequest;
use crate::worker_request_to_response::WorkerRequestToResponse;

// Executes custom request with the help of worker_request_executor and definition_service
#[derive(Clone)]
pub struct CustomHttpRequestApi {
    worker_to_http_response_service:
        Arc<dyn WorkerRequestToResponse<ResponseMapping, Response> + Sync + Send>,
    api_definition_service: Arc<dyn RegisterApiDefinition + Sync + Send>,
}

impl CustomHttpRequestApi {
    pub fn new(
        worker_to_http_response_service: Arc<
            dyn WorkerRequestToResponse<ResponseMapping, Response> + Sync + Send,
        >,
        api_definition_service: Arc<dyn RegisterApiDefinition + Sync + Send>,
    ) -> Self {
        Self {
            worker_to_http_response_service,
            api_definition_service,
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

        let api_definition_id = match get_header_value(&headers, GOLEM_API_DEFINITION_ID_EXTENSION)
        {
            Ok(api_definition_id) => ApiDefinitionId(api_definition_id.to_string()),
            Err(err) => {
                error!(err);
                return Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .body(Body::from_string(err.to_string()));
            }
        };

        let version = match get_header_value(&headers, GOLEM_API_DEFINITION_VERSION) {
            Ok(version) => Version(version),
            Err(err) => {
                error!(err);
                return Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .body(Body::from_string(err.to_string()));
            }
        };

        let api_key = ApiDefinitionKey {
            id: api_definition_id.clone(),
            version,
        };

        // Get ApiSpec corresponding to the API Definition Id
        let api_definition = match self.api_definition_service.get(&api_key).await {
            Ok(Some(api_definition)) => api_definition,
            Ok(None) => {
                error!(
                    "API request definition id {} with version {} not found",
                    &api_key.id, &api_key.version
                );

                return Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .body(Body::from_string(format!(
                        "API request definition id {} with version {} not found",
                        &api_key.id, &api_key.version
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
                let resolved_worker_request =
                    match WorkerRequest::from_resolved_route(resolved_route.clone()) {
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
                self.worker_to_http_response_service
                    .execute(
                        resolved_worker_request.clone(),
                        &resolved_route.route_definition.binding.response,
                        &resolved_route.resolved_variables,
                    )
                    .await
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

#[async_trait]
impl Endpoint for CustomHttpRequestApi {
    type Output = Response;

    async fn call(&self, req: Request) -> poem::Result<Self::Output> {
        let result = self.execute(req).await;
        Ok(result)
    }
}
